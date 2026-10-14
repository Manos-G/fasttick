//! The FastTick project: XML files, model files, include resolution, node
//! models, layout state and unconnected nodes.
//!
//! A [`Project`] holds [`BtFile`]s. Each file has a [`FileId`] that stays
//! valid while the project exists (also after other files are removed).
//! Each file has a [`FileRole`]:
//!
//! - [`FileRole::Tree`]: a tree file that the project file lists.
//! - [`FileRole::Models`]: a model file that the project file lists.
//! - [`FileRole::Included`]: a file that a project file includes (directly
//!   or through other files) and that the project file does not list.
//!
//! The project loads the included files of all its files, transitively, and
//! detects include cycles ([`Project::refresh`]). The model registry
//! ([`Project::registry`]) uses the include graph for the model look-up
//! order.
//!
//! The project file (`*.ftproj`) is JSON; see [`ProjectFile`]. Paths in it
//! are relative to the project file. A user can also open one XML file
//! without a project file ([`Project::open_xml`]).
//!
//! # Changes
//!
//! Change a file with [`Project::file_mut`] and the functions of
//! [`BtFile`], then call [`Project::refresh`] if the change can change the
//! trees, the includes or the models of the file. [`Project::edit_file`]
//! does both.

mod file;
mod includes;

use std::collections::{BTreeMap, HashSet};
use std::ffi::OsString;
use std::fmt;
use std::path::{Path, PathBuf};

use crate::bt::{BtError, BtFile, StructureHash, TreeEntry, format_index_path, parse_index_path};
use crate::fsio::{normalize, relative_path, write_atomic};
use crate::models::{ModelRegistry, ModelSource, NodeModel, RegistryFile};
use crate::xml::{Node, NodeId};

pub use file::{
    LayoutDirection, LayoutEntry, PROJECT_FILE_EXTENSION, PROJECT_FILE_VERSION, ProjectFile,
    ProjectFileError, RecentProjects, UnconnectedEntry,
};
pub use includes::{
    AMENT_PREFIX_PATH, IncludeLink, IncludeProblem, ament_prefix_path_from_env, find_package_share,
    resolve_include_path,
};

/// The identifier of a file in a [`Project`].
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct FileId(pub usize);

impl fmt::Display for FileId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "file {}", self.0)
    }
}

/// Why a file is in the project.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum FileRole {
    /// A tree file that the project file lists.
    Tree,
    /// A model file that the project file lists.
    Models,
    /// A file that another file includes and that the project file does
    /// not list.
    Included,
}

/// A reference to one tree: the file and the position in
/// [`crate::bt::BtIndex::trees`].
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct TreeRef {
    /// The file.
    pub file: FileId,
    /// The index of the tree in the file.
    pub index: usize,
}

/// The saved layout state of one tree.
#[derive(Debug, Clone, PartialEq)]
pub struct LayoutState {
    /// The layout direction.
    pub direction: LayoutDirection,
    /// The structure hash of the tree when the positions were made.
    pub structure_hash: StructureHash,
    /// Node positions by child-index path text (`"0"`, `"0/1"`; see
    /// [`crate::bt::format_index_path`]).
    pub positions: BTreeMap<String, [f32; 2]>,
}

impl LayoutState {
    /// Makes a state without positions.
    pub fn new(direction: LayoutDirection, structure_hash: StructureHash) -> LayoutState {
        LayoutState {
            direction,
            structure_hash,
            positions: BTreeMap::new(),
        }
    }

    /// The position of the node at a child-index path.
    pub fn position(&self, index_path: &[usize]) -> Option<[f32; 2]> {
        self.positions.get(&format_index_path(index_path)).copied()
    }

    /// Sets the position of the node at a child-index path. A position with
    /// a value that is not finite is not stored (JSON cannot hold it).
    pub fn set_position(&mut self, index_path: &[usize], position: [f32; 2]) {
        if position.iter().all(|v| v.is_finite()) && !index_path.is_empty() {
            self.positions
                .insert(format_index_path(index_path), position);
        }
    }
}

/// A node without a parent, kept in the project file (decision D5).
///
/// The node is a detached element in the document of its file. BT.CPP XML
/// cannot hold it, so it is not written to the XML file.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct UnconnectedNode {
    /// The detached element.
    pub node: NodeId,
    /// The position on the canvas.
    pub position: [f32; 2],
}

/// A problem that did not stop the loading of a project.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ProjectWarning {
    /// A file that the project file lists cannot be loaded. The project
    /// keeps the path and writes it again on save.
    FileNotLoaded {
        /// The path.
        path: PathBuf,
        /// The error message.
        message: String,
    },
    /// A layout entry of the project file was not used.
    LayoutIgnored {
        /// The key of the entry.
        key: String,
        /// The reason.
        reason: String,
    },
    /// An unconnected node of the project file was not loaded.
    UnconnectedIgnored {
        /// The file of the entry.
        file: String,
        /// The tree of the entry.
        tree: String,
        /// The reason.
        reason: String,
    },
}

impl fmt::Display for ProjectWarning {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            ProjectWarning::FileNotLoaded { path, message } => {
                write!(f, "cannot load {}: {message}", path.display())
            }
            ProjectWarning::LayoutIgnored { key, reason } => {
                write!(f, "the layout of \"{key}\" is not used: {reason}")
            }
            ProjectWarning::UnconnectedIgnored { file, tree, reason } => write!(
                f,
                "an unconnected node of the tree \"{tree}\" in {file} is not loaded: {reason}"
            ),
        }
    }
}

/// An error from a project operation.
#[derive(Debug, thiserror::Error)]
pub enum ProjectError {
    /// The project file cannot be read.
    #[error("cannot read the project file {}: {source}", .path.display())]
    Read {
        /// The path.
        path: PathBuf,
        /// The I/O error.
        #[source]
        source: std::io::Error,
    },
    /// The project file cannot be written.
    #[error("cannot write the project file {}: {source}", .path.display())]
    Write {
        /// The path.
        path: PathBuf,
        /// The I/O error.
        #[source]
        source: std::io::Error,
    },
    /// The content of the project file is not valid.
    #[error("{}: {source}", .path.display())]
    Format {
        /// The path.
        path: PathBuf,
        /// The format error.
        #[source]
        source: ProjectFileError,
    },
    /// An XML file cannot be loaded or saved.
    #[error(transparent)]
    File(#[from] BtError),
    /// The project has no file path yet.
    #[error("the project has no file path; save it with a file name first")]
    NoPath,
    /// A file of the project has no path, so the project file cannot refer
    /// to it.
    #[error("the file \"{0}\" has no path; save it first")]
    UnsavedFile(String),
    /// The file ID is not in the project.
    #[error("{0} is not in the project")]
    UnknownFile(FileId),
}

#[derive(Debug, Clone)]
struct Entry {
    file: BtFile,
    role: FileRole,
    /// The path that identifies the file: canonical if possible, else
    /// normalized.
    identity: Option<PathBuf>,
    includes: Vec<IncludeLink>,
}

/// A FastTick project. See the [module documentation](self).
#[derive(Debug, Clone)]
pub struct Project {
    path: Option<PathBuf>,
    name: String,
    entries: Vec<Option<Entry>>,
    missing: Vec<(PathBuf, FileRole)>,
    registry: ModelRegistry,
    layouts: BTreeMap<(FileId, String), LayoutState>,
    unconnected: BTreeMap<(FileId, String), Vec<UnconnectedNode>>,
    warnings: Vec<ProjectWarning>,
    ament_prefix_path: Option<OsString>,
    dirty: bool,
}

/// The path that identifies a file: the canonical path if the file exists,
/// else the normalized path.
fn identity(path: &Path) -> PathBuf {
    std::fs::canonicalize(path).unwrap_or_else(|_| normalize(path))
}

/// The message of a load error, without the path.
fn load_message(error: &BtError) -> String {
    match error {
        BtError::Read { source, .. } | BtError::Write { source, .. } => source.to_string(),
        BtError::Parse { error, .. } => error.to_string(),
        BtError::NoPath => error.to_string(),
    }
}

impl Project {
    /// Makes an empty project without a path. `AMENT_PREFIX_PATH` comes
    /// from the environment.
    pub fn new(name: impl Into<String>) -> Project {
        Project {
            path: None,
            name: name.into(),
            entries: Vec::new(),
            missing: Vec::new(),
            registry: ModelRegistry::new(),
            layouts: BTreeMap::new(),
            unconnected: BTreeMap::new(),
            warnings: Vec::new(),
            ament_prefix_path: ament_prefix_path_from_env(),
            dirty: false,
        }
    }

    /// Opens a project file (`*.ftproj`).
    ///
    /// Files that cannot be loaded, unusable layout entries and unusable
    /// unconnected nodes do not stop the load; see [`Project::warnings`].
    /// Include problems are in [`Project::includes`].
    pub fn open(path: impl AsRef<Path>) -> Result<Project, ProjectError> {
        let path = normalize(path.as_ref());
        let text = std::fs::read_to_string(&path).map_err(|source| ProjectError::Read {
            path: path.clone(),
            source,
        })?;
        let project_file =
            ProjectFile::from_json(&text).map_err(|source| ProjectError::Format {
                path: path.clone(),
                source,
            })?;
        let mut project = Project::new(project_file.name.clone());
        if project.name.is_empty() {
            project.name = stem(&path);
        }
        project.path = Some(path);
        project.load_project_file(&project_file);
        Ok(project)
    }

    /// Opens one XML file as an in-memory project (without a project file).
    /// The project also loads the files that it includes. The project name
    /// is the file name without the extension.
    pub fn open_xml(path: impl AsRef<Path>) -> Result<Project, ProjectError> {
        let path = normalize(path.as_ref());
        let mut project = Project::new(stem(&path));
        project.load_entry(&path, FileRole::Tree)?;
        project.refresh();
        project.dirty = false;
        Ok(project)
    }

    /// Makes an in-memory project with one file (for example a new file).
    pub fn with_file(name: impl Into<String>, file: BtFile) -> (Project, FileId) {
        let mut project = Project::new(name);
        let id = project.add_bt_file(file, FileRole::Tree);
        project.dirty = false;
        (project, id)
    }

    fn base_dir(&self) -> PathBuf {
        match self.path.as_deref().and_then(Path::parent) {
            Some(dir) => dir.to_path_buf(),
            None => normalize(Path::new(".")),
        }
    }

    fn load_project_file(&mut self, project_file: &ProjectFile) {
        let base = self.base_dir();
        for (list, role) in [
            (&project_file.files, FileRole::Tree),
            (&project_file.model_files, FileRole::Models),
        ] {
            for rel in list {
                let path = normalize(&base.join(rel));
                if let Err(error) = self.load_entry(&path, role) {
                    self.warnings.push(ProjectWarning::FileNotLoaded {
                        path: path.clone(),
                        message: load_message(&error),
                    });
                    self.missing.push((path, role));
                }
            }
        }
        self.refresh();

        for (key, entry) in &project_file.layout {
            match self.layout_from_entry(&base, key, entry) {
                Ok((file, tree, state)) => {
                    self.layouts.insert((file, tree), state);
                }
                Err(reason) => self.warnings.push(ProjectWarning::LayoutIgnored {
                    key: key.clone(),
                    reason,
                }),
            }
        }

        for entry in &project_file.unconnected {
            if let Err(reason) = self.unconnected_from_entry(&base, entry) {
                self.warnings.push(ProjectWarning::UnconnectedIgnored {
                    file: entry.file.clone(),
                    tree: entry.tree.clone(),
                    reason,
                });
            }
        }
        self.dirty = false;
    }

    fn layout_from_entry(
        &mut self,
        base: &Path,
        key: &str,
        entry: &LayoutEntry,
    ) -> Result<(FileId, String, LayoutState), String> {
        // The key is "<file>#<tree>". Both parts can contain '#', so try
        // each '#' until the file part names a loaded file.
        let (file, tree) = key
            .match_indices('#')
            .find_map(|(i, _)| {
                self.file_id_by_path(base.join(&key[..i]))
                    .map(|file| (file, key[i + 1..].to_owned()))
            })
            .ok_or_else(|| "no loaded file matches the key".to_owned())?;
        let mut state = LayoutState::new(entry.direction, StructureHash(0));
        match entry.structure_hash.parse::<StructureHash>() {
            Ok(hash) => state.structure_hash = hash,
            Err(_) => {
                // Keep the direction; the positions cannot be checked.
                self.warnings.push(ProjectWarning::LayoutIgnored {
                    key: key.to_owned(),
                    reason: format!(
                        "the structure hash \"{}\" is not valid; the positions are not used",
                        entry.structure_hash
                    ),
                });
                return Ok((file, tree, state));
            }
        }
        for (path, position) in &entry.positions {
            match parse_index_path(path) {
                Some(index_path) => state.set_position(&index_path, *position),
                None => self.warnings.push(ProjectWarning::LayoutIgnored {
                    key: key.to_owned(),
                    reason: format!("the position key \"{path}\" is not a child-index path"),
                }),
            }
        }
        Ok((file, tree, state))
    }

    fn unconnected_from_entry(
        &mut self,
        base: &Path,
        entry: &UnconnectedEntry,
    ) -> Result<(), String> {
        let file = self
            .file_id_by_path(base.join(&entry.file))
            .ok_or_else(|| "the file is not loaded".to_owned())?;
        if !entry.position.iter().all(|v| v.is_finite()) {
            return Err("the position is not valid".to_owned());
        }
        let bt = self
            .file_mut(file)
            .ok_or_else(|| "the file is not loaded".to_owned())?;
        let nodes = bt
            .doc_mut()
            .parse_fragment(&entry.xml)
            .map_err(|e| format!("the XML is not valid: {e}"))?;
        let elements: Vec<NodeId> = nodes
            .into_iter()
            .filter(|&n| matches!(bt.doc().node(n), Some(Node::Element(_))))
            .collect();
        let list = self
            .unconnected
            .entry((file, entry.tree.clone()))
            .or_default();
        for node in elements {
            list.push(UnconnectedNode {
                node,
                position: entry.position,
            });
        }
        Ok(())
    }

    /// Loads a file (or finds it if it is loaded) with a role. A file that
    /// is only included becomes a listed file.
    fn load_entry(&mut self, path: &Path, role: FileRole) -> Result<FileId, BtError> {
        if let Some(id) = self.file_id_by_path(path) {
            if let Some(entry) = self.entries[id.0].as_mut()
                && entry.role == FileRole::Included
            {
                entry.role = role;
            }
            return Ok(id);
        }
        let file = BtFile::load(normalize(path))?;
        Ok(self.add_bt_file(file, role))
    }

    /// Adds a file that is in memory (for example a new file without a
    /// path) and returns its ID. Call [`Project::refresh`] after it to load
    /// its includes.
    pub fn add_bt_file(&mut self, file: BtFile, role: FileRole) -> FileId {
        let identity = file.path().map(identity);
        let id = FileId(self.entries.len());
        self.entries.push(Some(Entry {
            file,
            role,
            identity,
            includes: Vec::new(),
        }));
        // An included file does not change the project file.
        if role != FileRole::Included {
            self.dirty = true;
        }
        id
    }

    /// Loads a tree file and adds it to the project, with its includes.
    /// If the file is loaded already, the function returns its ID (an
    /// included file becomes a listed file).
    pub fn add_file(&mut self, path: impl AsRef<Path>) -> Result<FileId, ProjectError> {
        let id = self.load_entry(path.as_ref(), FileRole::Tree)?;
        self.dirty = true;
        self.refresh();
        Ok(id)
    }

    /// Loads a model file and adds it to the project.
    pub fn add_model_file(&mut self, path: impl AsRef<Path>) -> Result<FileId, ProjectError> {
        let id = self.load_entry(path.as_ref(), FileRole::Models)?;
        self.dirty = true;
        self.refresh();
        Ok(id)
    }

    /// Removes a file from the project and returns it. The layout states and
    /// unconnected nodes of the file are removed too. If another file still
    /// includes it, the next [`Project::refresh`] loads it again as an
    /// included file.
    pub fn remove_file(&mut self, id: FileId) -> Option<BtFile> {
        let entry = self.entries.get_mut(id.0)?.take()?;
        self.forget_file_state(id);
        self.dirty = true;
        self.refresh();
        Some(entry.file)
    }

    fn forget_file_state(&mut self, id: FileId) {
        self.layouts.retain(|(f, _), _| *f != id);
        self.unconnected.retain(|(f, _), _| *f != id);
    }

    /// Loads a file again from its path. The unconnected nodes of the file
    /// stay (they are copied into the new document); the layout states stay
    /// (their structure hash decides if the positions are still used).
    ///
    /// On error, the project does not change.
    pub fn reload_file(&mut self, id: FileId) -> Result<(), ProjectError> {
        let entry = self
            .entries
            .get(id.0)
            .and_then(Option::as_ref)
            .ok_or(ProjectError::UnknownFile(id))?;
        let path = entry.file.path().ok_or(BtError::NoPath)?.to_path_buf();
        let mut new_file = BtFile::load(&path)?;
        let old_doc = entry.file.doc();
        let mut moved = Vec::new();
        for ((file, tree), list) in &self.unconnected {
            if *file != id {
                continue;
            }
            let mut new_list = Vec::new();
            for item in list {
                let xml = old_doc.write_node(item.node);
                if let Ok(nodes) = new_file.doc_mut().parse_fragment(&xml)
                    && let Some(&node) = nodes.first()
                {
                    new_list.push(UnconnectedNode {
                        node,
                        position: item.position,
                    });
                }
            }
            moved.push((tree.clone(), new_list));
        }
        for (tree, list) in moved {
            self.unconnected.insert((id, tree), list);
        }
        if let Some(entry) = self.entries[id.0].as_mut() {
            entry.identity = new_file.path().map(identity);
            entry.file = new_file;
        }
        self.refresh();
        Ok(())
    }

    /// Resolves the includes of all files again, loads included files that
    /// are not loaded yet, removes included files that no file needs any
    /// more (if they have no unsaved changes), and builds the model registry
    /// again.
    ///
    /// Call it after a change that can change the trees, the includes or the
    /// models of a file, and after a change of the file paths.
    pub fn refresh(&mut self) {
        self.resolve_includes();
        self.prune_included();
        self.rebuild_registry();
    }

    fn live_ids(&self) -> Vec<FileId> {
        self.entries
            .iter()
            .enumerate()
            .filter(|(_, e)| e.is_some())
            .map(|(i, _)| FileId(i))
            .collect()
    }

    fn resolve_includes(&mut self) {
        let mut queue = self.live_ids();
        let mut next = 0;
        while next < queue.len() {
            let id = queue[next];
            next += 1;
            let Some(entry) = self.entries[id.0].as_ref() else {
                continue;
            };
            let base_dir = match entry.file.path().and_then(Path::parent) {
                Some(dir) => normalize(dir),
                // BT.CPP uses the current directory for text that is not
                // from a file (`loadFromText`).
                None => normalize(Path::new(".")),
            };
            let includes = entry.file.index().includes.clone();
            let mut links = Vec::with_capacity(includes.len());
            for include in &includes {
                let mut link = IncludeLink {
                    element: include.element,
                    path: None,
                    target: None,
                    problem: None,
                };
                match resolve_include_path(&base_dir, include, self.ament_prefix_path.as_deref()) {
                    Err(problem) => link.problem = Some(problem),
                    Ok(path) => {
                        match self.file_id_by_path(&path) {
                            Some(target) => link.target = Some(target),
                            None => match BtFile::load(&path) {
                                Ok(file) => {
                                    let target = self.add_bt_file(file, FileRole::Included);
                                    queue.push(target);
                                    link.target = Some(target);
                                }
                                Err(error) => {
                                    link.problem = Some(IncludeProblem::Load {
                                        path: path.clone(),
                                        message: load_message(&error),
                                    });
                                }
                            },
                        }
                        link.path = Some(path);
                    }
                }
                links.push(link);
            }
            if let Some(entry) = self.entries[id.0].as_mut() {
                entry.includes = links;
            }
        }
        self.mark_cycles();
    }

    /// Marks the include links that close a cycle. The search starts at the
    /// listed files, in project order, so the link that closes the cycle is
    /// the one that is farthest from a listed file.
    fn mark_cycles(&mut self) {
        let n = self.entries.len();
        let mut state = vec![0u8; n]; // 0: new, 1: on the path, 2: done
        let mut roots: Vec<FileId> = Vec::new();
        for role in [FileRole::Tree, FileRole::Models, FileRole::Included] {
            roots.extend(
                self.live_ids()
                    .into_iter()
                    .filter(|id| self.role(*id) == Some(role)),
            );
        }
        let mut cycles: Vec<(FileId, usize)> = Vec::new();
        for root in roots {
            if state[root.0] != 0 {
                continue;
            }
            state[root.0] = 1;
            let mut stack: Vec<(FileId, usize)> = vec![(root, 0)];
            while let Some(top) = stack.last_mut() {
                let (node, i) = (top.0, top.1);
                let links = self.includes(node);
                if i < links.len() {
                    top.1 += 1;
                    if let Some(target) = links[i].target {
                        match state[target.0] {
                            0 => {
                                state[target.0] = 1;
                                stack.push((target, 0));
                            }
                            1 => cycles.push((node, i)),
                            _ => {}
                        }
                    }
                } else {
                    state[node.0] = 2;
                    stack.pop();
                }
            }
        }
        for (file, i) in cycles {
            if let Some(link) = self.entries[file.0]
                .as_mut()
                .and_then(|e| e.includes.get_mut(i))
            {
                link.problem = Some(IncludeProblem::Cycle {
                    path: link.path.clone().unwrap_or_default(),
                });
            }
        }
    }

    fn prune_included(&mut self) {
        let mut reachable: HashSet<FileId> = HashSet::new();
        let mut stack: Vec<FileId> = self
            .live_ids()
            .into_iter()
            .filter(|id| self.role(*id) != Some(FileRole::Included))
            .collect();
        while let Some(id) = stack.pop() {
            if !reachable.insert(id) {
                continue;
            }
            stack.extend(self.includes(id).iter().filter_map(|l| l.target));
        }
        for id in self.live_ids() {
            let remove = matches!(
                &self.entries[id.0],
                Some(e) if e.role == FileRole::Included && !reachable.contains(&id) && !e.file.is_dirty()
            );
            if remove {
                self.entries[id.0] = None;
                self.forget_file_state(id);
            }
        }
    }

    fn rebuild_registry(&mut self) {
        let ids = self.live_ids();
        let closures: Vec<(FileId, Vec<FileId>)> = ids
            .iter()
            .map(|&id| (id, self.include_closure(id)))
            .collect();
        let entries = &self.entries;
        self.registry
            .rebuild(closures.iter().filter_map(|(id, closure)| {
                entries[id.0].as_ref().map(|e| RegistryFile {
                    id: *id,
                    is_model_file: e.role == FileRole::Models,
                    file: &e.file,
                    closure,
                })
            }));
    }

    /// The file and the files that it includes, transitively: depth first,
    /// in the order of the `<include>` elements, each file once. The first
    /// item is the file itself. Empty if the file is not in the project.
    pub fn include_closure(&self, id: FileId) -> Vec<FileId> {
        if self.file(id).is_none() {
            return Vec::new();
        }
        let mut out = Vec::new();
        let mut seen = HashSet::new();
        let mut stack = vec![id];
        while let Some(f) = stack.pop() {
            if !seen.insert(f) {
                continue;
            }
            out.push(f);
            let targets: Vec<FileId> = self.includes(f).iter().filter_map(|l| l.target).collect();
            stack.extend(targets.into_iter().rev());
        }
        out
    }

    /// The include links of a file, in the order of its `<include>`
    /// elements.
    pub fn includes(&self, id: FileId) -> &[IncludeLink] {
        self.entries
            .get(id.0)
            .and_then(Option::as_ref)
            .map_or(&[], |e| e.includes.as_slice())
    }

    /// The project file path, or `None` for an in-memory project.
    pub fn path(&self) -> Option<&Path> {
        self.path.as_deref()
    }

    /// The project name.
    pub fn name(&self) -> &str {
        &self.name
    }

    /// Sets the project name.
    pub fn set_name(&mut self, name: impl Into<String>) {
        self.name = name.into();
        self.dirty = true;
    }

    /// The problems of the last load that did not stop it.
    pub fn warnings(&self) -> &[ProjectWarning] {
        &self.warnings
    }

    /// Sets the value of `AMENT_PREFIX_PATH` for `ros_pkg` includes (the
    /// default comes from the environment), then calls
    /// [`Project::refresh`].
    pub fn set_ament_prefix_path(&mut self, value: Option<OsString>) {
        self.ament_prefix_path = value;
        self.refresh();
    }

    /// The file with this ID.
    pub fn file(&self, id: FileId) -> Option<&BtFile> {
        self.entries.get(id.0)?.as_ref().map(|e| &e.file)
    }

    /// The file with this ID, for changes. Call [`Project::refresh`] after a
    /// change of the trees, the includes, the models or the path.
    pub fn file_mut(&mut self, id: FileId) -> Option<&mut BtFile> {
        self.entries.get_mut(id.0)?.as_mut().map(|e| &mut e.file)
    }

    /// Runs a function on a file and then calls [`Project::refresh`].
    pub fn edit_file<R>(
        &mut self,
        id: FileId,
        f: impl FnOnce(&mut BtFile) -> R,
    ) -> Result<R, ProjectError> {
        let file = self.file_mut(id).ok_or(ProjectError::UnknownFile(id))?;
        let result = f(file);
        if let Some(entry) = self.entries[id.0].as_mut() {
            entry.identity = entry.file.path().map(identity);
        }
        self.refresh();
        Ok(result)
    }

    /// The role of a file.
    pub fn role(&self, id: FileId) -> Option<FileRole> {
        self.entries.get(id.0)?.as_ref().map(|e| e.role)
    }

    /// The IDs of all files, in project order.
    pub fn file_ids(&self) -> Vec<FileId> {
        self.live_ids()
    }

    /// All files with their IDs and roles, in project order.
    pub fn files(&self) -> impl Iterator<Item = (FileId, FileRole, &BtFile)> {
        self.entries
            .iter()
            .enumerate()
            .filter_map(|(i, e)| e.as_ref().map(|e| (FileId(i), e.role, &e.file)))
    }

    /// The file with this path. Paths are compared after they are made
    /// canonical (symbolic links resolved) or, for files that do not exist,
    /// normalized.
    pub fn file_id_by_path(&self, path: impl AsRef<Path>) -> Option<FileId> {
        let wanted = identity(path.as_ref());
        self.entries.iter().enumerate().find_map(|(i, e)| {
            e.as_ref()
                .filter(|e| e.identity.as_deref() == Some(wanted.as_path()))
                .map(|_| FileId(i))
        })
    }

    /// The files that the project file lists but that could not be loaded.
    pub fn missing_files(&self) -> &[(PathBuf, FileRole)] {
        &self.missing
    }

    /// All trees of all files, in project order.
    pub fn trees(&self) -> Vec<TreeRef> {
        self.files()
            .flat_map(|(file, _, bt)| {
                (0..bt.trees().len()).map(move |index| TreeRef { file, index })
            })
            .collect()
    }

    /// The tree entry of a reference.
    pub fn tree(&self, tree: TreeRef) -> Option<&TreeEntry> {
        self.file(tree.file)?.trees().get(tree.index)
    }

    /// Finds a tree by ID in all files, in project order. In one file, the
    /// last tree with the ID wins (as in BT.CPP).
    pub fn find_tree(&self, id: &str) -> Option<TreeRef> {
        self.files().find_map(|(file, _, bt)| {
            bt.index()
                .tree_index(id)
                .map(|index| TreeRef { file, index })
        })
    }

    /// All trees with this ID, in project order (more than one is the
    /// validation problem `DuplicateTreeId`).
    pub fn find_trees(&self, id: &str) -> Vec<TreeRef> {
        self.files()
            .flat_map(|(file, _, bt)| {
                bt.trees()
                    .iter()
                    .enumerate()
                    .filter(|(_, t)| t.id == id)
                    .map(move |(index, _)| TreeRef { file, index })
            })
            .collect()
    }

    /// Finds the tree that a SubTree in `file` refers to: first in the file
    /// and the files that it includes (in that order), then in all files.
    pub fn find_tree_from(&self, file: FileId, id: &str) -> Option<TreeRef> {
        self.find_tree_in_closure(file, id)
            .or_else(|| self.find_tree(id))
    }

    fn find_tree_in_closure(&self, file: FileId, id: &str) -> Option<TreeRef> {
        self.include_closure(file).into_iter().find_map(|f| {
            self.file(f)?
                .index()
                .tree_index(id)
                .map(|index| TreeRef { file: f, index })
        })
    }

    /// The tree that BT.CPP runs when it loads this file and no tree ID is
    /// given (`XMLParser::instantiateTree`): the tree named by
    /// `main_tree_to_execute`, searched in the file and its includes; else
    /// the only tree ID of the file and its includes.
    pub fn main_tree(&self, file: FileId) -> Option<TreeRef> {
        let bt = self.file(file)?;
        if let Some(id) = &bt.index().main_tree_to_execute {
            return self.find_tree_in_closure(file, id);
        }
        let mut ids: Vec<&str> = Vec::new();
        for f in self.include_closure(file) {
            if let Some(b) = self.file(f) {
                ids.extend(b.trees().iter().map(|t| t.id.as_str()));
            }
        }
        ids.sort_unstable();
        ids.dedup();
        match ids.as_slice() {
            [only] => self.find_tree_in_closure(file, only),
            _ => None,
        }
    }

    /// The model registry.
    pub fn registry(&self) -> &ModelRegistry {
        &self.registry
    }

    /// The model registry, for settings such as
    /// [`ModelRegistry::set_v3_aliases`].
    pub fn registry_mut(&mut self) -> &mut ModelRegistry {
        &mut self.registry
    }

    /// Finds the model of a type ID for a node in `file` (see
    /// [`ModelRegistry::lookup`]).
    pub fn lookup_model(&self, file: FileId, type_id: &str) -> Option<(&NodeModel, ModelSource)> {
        self.registry.lookup(file, type_id)
    }

    /// The layout state of a tree.
    pub fn layout(&self, file: FileId, tree: &str) -> Option<&LayoutState> {
        self.layouts.get(&(file, tree.to_owned()))
    }

    /// Sets the layout state of a tree.
    pub fn set_layout(&mut self, file: FileId, tree: &str, state: LayoutState) {
        self.layouts.insert((file, tree.to_owned()), state);
        self.dirty = true;
    }

    /// Removes the layout state of a tree.
    pub fn remove_layout(&mut self, file: FileId, tree: &str) -> Option<LayoutState> {
        let old = self.layouts.remove(&(file, tree.to_owned()));
        self.dirty |= old.is_some();
        old
    }

    /// The stored node positions of a tree, but only if the structure hash
    /// of the tree is still the stored hash. Else the file changed outside
    /// FastTick, and the canvas uses the automatic layout.
    pub fn valid_positions(&self, file: FileId, tree: &str) -> Option<&BTreeMap<String, [f32; 2]>> {
        let state = self.layout(file, tree)?;
        let bt = self.file(file)?;
        let index = bt.index().tree_index(tree)?;
        (bt.structure_hash(index)? == state.structure_hash).then_some(&state.positions)
    }

    /// Moves the layout state and the unconnected nodes of a tree to a new
    /// tree ID (after the tree is renamed).
    pub fn rename_tree_state(&mut self, file: FileId, old: &str, new: &str) {
        if let Some(state) = self.layouts.remove(&(file, old.to_owned())) {
            self.layouts.insert((file, new.to_owned()), state);
            self.dirty = true;
        }
        if let Some(list) = self.unconnected.remove(&(file, old.to_owned())) {
            self.unconnected
                .entry((file, new.to_owned()))
                .or_default()
                .extend(list);
            self.dirty = true;
        }
    }

    /// The unconnected nodes of a tree.
    pub fn unconnected(&self, file: FileId, tree: &str) -> &[UnconnectedNode] {
        self.unconnected
            .get(&(file, tree.to_owned()))
            .map_or(&[], Vec::as_slice)
    }

    /// All unconnected nodes: file, tree ID and node.
    pub fn all_unconnected(&self) -> impl Iterator<Item = (FileId, &str, &UnconnectedNode)> {
        self.unconnected
            .iter()
            .flat_map(|((file, tree), list)| list.iter().map(move |n| (*file, tree.as_str(), n)))
    }

    /// Adds an unconnected node to a tree. `node` must be a detached element
    /// of the file.
    pub fn add_unconnected(&mut self, file: FileId, tree: &str, node: NodeId, position: [f32; 2]) {
        self.unconnected
            .entry((file, tree.to_owned()))
            .or_default()
            .push(UnconnectedNode { node, position });
        self.dirty = true;
    }

    /// Removes an unconnected node. Returns its tree ID and its data.
    pub fn remove_unconnected(
        &mut self,
        file: FileId,
        node: NodeId,
    ) -> Option<(String, UnconnectedNode)> {
        let mut found = None;
        for ((f, tree), list) in self.unconnected.iter_mut() {
            if *f != file {
                continue;
            }
            if let Some(i) = list.iter().position(|n| n.node == node) {
                found = Some((tree.clone(), list.remove(i)));
                break;
            }
        }
        if found.is_some() {
            self.unconnected.retain(|_, list| !list.is_empty());
            self.dirty = true;
        }
        found
    }

    /// Moves an unconnected node. Returns false if it is not in the list.
    pub fn set_unconnected_position(
        &mut self,
        file: FileId,
        node: NodeId,
        position: [f32; 2],
    ) -> bool {
        for ((f, _), list) in self.unconnected.iter_mut() {
            if *f != file {
                continue;
            }
            if let Some(item) = list.iter_mut().find(|n| n.node == node) {
                item.position = position;
                self.dirty = true;
                return true;
            }
        }
        false
    }

    /// True if the project state (file list, layouts, unconnected nodes,
    /// name) changed after the last save or load. The XML files have their
    /// own dirty state ([`BtFile::is_dirty`]).
    pub fn is_project_dirty(&self) -> bool {
        self.dirty
    }

    /// The files with unsaved changes.
    pub fn dirty_files(&self) -> Vec<FileId> {
        self.files()
            .filter(|(_, _, f)| f.is_dirty())
            .map(|(id, _, _)| id)
            .collect()
    }

    /// True if the project state or a file has unsaved changes.
    pub fn is_dirty(&self) -> bool {
        self.dirty || self.files().any(|(_, _, f)| f.is_dirty())
    }

    /// Saves one XML file to its path.
    pub fn save_file(&mut self, id: FileId) -> Result<(), ProjectError> {
        let file = self.file_mut(id).ok_or(ProjectError::UnknownFile(id))?;
        file.save_in_place()?;
        Ok(())
    }

    /// Saves one XML file to a new path, then resolves the includes again
    /// (relative includes now start at the new directory).
    pub fn save_file_as(&mut self, id: FileId, path: impl AsRef<Path>) -> Result<(), ProjectError> {
        let path = normalize(path.as_ref());
        let file = self.file_mut(id).ok_or(ProjectError::UnknownFile(id))?;
        file.save(&path)?;
        if let Some(entry) = self.entries[id.0].as_mut() {
            entry.identity = Some(identity(&path));
        }
        self.dirty = true;
        self.refresh();
        Ok(())
    }

    /// Saves all XML files with unsaved changes. Stops at the first error.
    pub fn save_dirty_files(&mut self) -> Result<(), ProjectError> {
        for id in self.dirty_files() {
            self.save_file(id)?;
        }
        Ok(())
    }

    /// Makes the project file data, with paths relative to `base_dir`.
    pub fn to_project_file(&self, base_dir: &Path) -> Result<ProjectFile, ProjectError> {
        let mut out = ProjectFile {
            name: self.name.clone(),
            ..ProjectFile::default()
        };
        let rel = |id: FileId| -> Result<String, ProjectError> {
            let bt = self.file(id).ok_or(ProjectError::UnknownFile(id))?;
            let path = bt
                .path()
                .ok_or_else(|| ProjectError::UnsavedFile(bt.display_name()))?;
            Ok(relative_path(base_dir, path))
        };
        for (id, role, _) in self.files() {
            match role {
                FileRole::Tree => out.files.push(rel(id)?),
                FileRole::Models => out.model_files.push(rel(id)?),
                FileRole::Included => {}
            }
        }
        for (path, role) in &self.missing {
            let text = relative_path(base_dir, path);
            match role {
                FileRole::Models => out.model_files.push(text),
                _ => out.files.push(text),
            }
        }
        for ((file, tree), state) in &self.layouts {
            out.layout.insert(
                format!("{}#{tree}", rel(*file)?),
                LayoutEntry {
                    direction: state.direction,
                    structure_hash: state.structure_hash.to_string(),
                    positions: state.positions.clone(),
                },
            );
        }
        for ((file, tree), list) in &self.unconnected {
            let Some(bt) = self.file(*file) else { continue };
            for item in list {
                // A node that was connected again is in the XML; skip it.
                if bt.doc().parent(item.node).is_some() || !bt.doc().is_element(item.node) {
                    continue;
                }
                out.unconnected.push(UnconnectedEntry {
                    file: rel(*file)?,
                    tree: tree.clone(),
                    xml: bt.doc().write_node(item.node),
                    position: item.position,
                });
            }
        }
        Ok(out)
    }

    /// Writes the project file to its path. The XML files are not saved;
    /// see [`Project::save_dirty_files`].
    pub fn save(&mut self) -> Result<(), ProjectError> {
        let path = self.path.clone().ok_or(ProjectError::NoPath)?;
        self.save_as(path)
    }

    /// Writes the project file to a new path and makes it the project path.
    /// The paths in the file are relative to its directory.
    pub fn save_as(&mut self, path: impl AsRef<Path>) -> Result<(), ProjectError> {
        let path = normalize(path.as_ref());
        let base = path
            .parent()
            .map(Path::to_path_buf)
            .unwrap_or_else(|| normalize(Path::new(".")));
        let data = self.to_project_file(&base)?;
        write_atomic(&path, data.to_json().as_bytes()).map_err(|source| ProjectError::Write {
            path: path.clone(),
            source,
        })?;
        if self.name.is_empty() {
            self.name = stem(&path);
        }
        self.path = Some(path);
        self.dirty = false;
        Ok(())
    }
}

/// The file name without the extension.
fn stem(path: &Path) -> String {
    path.file_stem()
        .map(|s| s.to_string_lossy().into_owned())
        .unwrap_or_default()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn layout_state_positions() {
        let mut state = LayoutState::new(LayoutDirection::LeftRight, StructureHash(7));
        state.set_position(&[0, 1], [1.0, 2.0]);
        state.set_position(&[0], [f32::NAN, 0.0]);
        state.set_position(&[], [1.0, 1.0]);
        assert_eq!(state.position(&[0, 1]), Some([1.0, 2.0]));
        assert_eq!(state.position(&[0]), None);
        assert_eq!(state.positions.len(), 1);
    }

    #[test]
    fn in_memory_project() {
        let file = BtFile::parse(
            r#"<root BTCPP_format="4"><BehaviorTree ID="A"><SubTree ID="B"/></BehaviorTree>
            <BehaviorTree ID="B"><AlwaysSuccess/></BehaviorTree></root>"#,
            None,
        )
        .unwrap();
        let (mut project, id) = Project::with_file("mem", file);
        project.refresh();
        assert!(!project.is_dirty());
        assert_eq!(project.name(), "mem");
        assert_eq!(project.trees().len(), 2);
        assert_eq!(project.find_tree("B"), Some(TreeRef { file: id, index: 1 }));
        assert_eq!(project.main_tree(id), None);
        assert_eq!(
            project.lookup_model(id, "B").unwrap().1,
            ModelSource::InferredSubTree(id)
        );
        assert_eq!(project.include_closure(id), vec![id]);
        assert!(matches!(project.save(), Err(ProjectError::NoPath)));
        let dir = tempfile::tempdir().unwrap();
        assert!(matches!(
            project.save_as(dir.path().join("p.ftproj")),
            Err(ProjectError::UnsavedFile(_))
        ));
        assert_eq!(project.remove_file(FileId(99)).map(|_| ()), None);
        assert!(project.file(FileId(99)).is_none());
        assert!(project.includes(FileId(99)).is_empty());
        assert!(project.include_closure(FileId(99)).is_empty());
    }
}
