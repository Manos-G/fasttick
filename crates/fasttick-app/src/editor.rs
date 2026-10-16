//! The state of one edit-mode document: a project, its undo history and
//! the validation problems.
//!
//! All changes go through [`EditorDoc::edit`], which runs a session of the
//! core edit module (`fasttick_core::edit`), records it for undo, validates
//! the project again and increments [`EditorDoc::revision`]. Views compare
//! the revision with the one that they show and build their canvas again.
//! The GUI contains no XML logic: it calls the core commands.

use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use fasttick_core::bt::BtFile;
use fasttick_core::edit::{Coalesce, CoalesceKey, CommandError, Editor, History};
use fasttick_core::project::{FileId, FileRole, MissingFiles, Project, TreeRef};
use fasttick_core::validate::{Problem, ProblemCounts, validate_project};
use fasttick_core::xml::NodeId;

use crate::canvas::{CanvasTree, Expansion};

/// The time window in which edits with the same key merge into one undo
/// step (for example the keys typed into one text field).
pub const COALESCE_WINDOW: Duration = Duration::from_millis(1500);

/// The result of [`EditorDoc::save`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SaveOutcome {
    /// All files were written.
    Saved,
    /// The main file has no path yet: the caller must ask for one and call
    /// [`EditorDoc::save_as`].
    NeedsPath,
}

/// One edit-mode document. See the [module documentation](self).
#[derive(Debug)]
pub struct EditorDoc {
    project: Project,
    history: History,
    file: FileId,
    problems: Vec<Problem>,
    revision: u64,
}

impl EditorDoc {
    /// Makes an editor for a project and its main file, and validates it.
    pub fn new(project: Project, file: FileId) -> EditorDoc {
        let mut editor = EditorDoc {
            project,
            history: History::new(),
            file,
            problems: Vec::new(),
            revision: 1,
        };
        editor.validate();
        editor
    }

    /// Opens a tree XML file (with its includes) or a project file
    /// (`.ftproj`).
    pub fn open(path: &Path) -> Result<EditorDoc, String> {
        let is_project = path
            .extension()
            .is_some_and(|ext| ext.eq_ignore_ascii_case("ftproj"));
        let project = if is_project {
            Project::open(path)
        } else {
            Project::open_xml(path)
        }
        .map_err(|err| err.to_string())?;
        let file = if is_project {
            project
                .files()
                .find(|(_, role, _)| *role == FileRole::Tree)
                .map(|(id, _, _)| id)
        } else {
            project
                .file_id_by_path(path)
                .or_else(|| project.files().next().map(|(id, _, _)| id))
        }
        .ok_or_else(|| "the project has no tree file".to_owned())?;
        Ok(EditorDoc::new(project, file))
    }

    /// A new, unsaved file with one empty tree, `MainTree`, that is the
    /// main tree.
    pub fn untitled() -> EditorDoc {
        let (mut project, file) = Project::with_file("Untitled", BtFile::new_empty());
        // The first tree is part of the new file, not an undo step.
        let _ = History::new().run(&mut project, |ed| {
            ed.add_tree(file, "MainTree")?;
            ed.set_main_tree(file, Some("MainTree"))
        });
        EditorDoc::new(project, file)
    }

    /// A document from the text of a tree file that is not saved (for
    /// example a copy of unsaved changes after a crash). `path` is the file
    /// to save to (`None` for a new file); it does not need to exist. The
    /// document has unsaved changes.
    pub fn from_text(text: &str, path: Option<PathBuf>) -> Result<EditorDoc, String> {
        let name = path
            .as_deref()
            .and_then(Path::file_stem)
            .map(|s| s.to_string_lossy().into_owned())
            .unwrap_or_else(|| "Untitled".to_owned());
        let file = BtFile::parse(text, path).map_err(|err| err.to_string())?;
        let (mut project, id) = Project::with_file(name, file);
        if let Some(file) = project.file_mut(id) {
            file.bump_generation();
        }
        Ok(EditorDoc::new(project, id))
    }

    /// The paths and texts of the XML files with unsaved changes (the path
    /// is `None` for a new file). [`EditorDoc::restore_files`] puts them
    /// back.
    pub fn unsaved_texts(&self) -> Vec<(Option<PathBuf>, String)> {
        self.project
            .dirty_files()
            .into_iter()
            .filter_map(|id| {
                let file = self.project.file(id)?;
                Some((file.path().map(Path::to_path_buf), file.to_xml_string()))
            })
            .collect()
    }

    /// Puts texts that are not saved into the files of the document (see
    /// [`EditorDoc::unsaved_texts`]). A text without a path goes into the
    /// main file. A file that the project does not have yet is added. The
    /// undo history is cleared: its steps are for the old texts.
    ///
    /// Returns the messages of the texts that FastTick could not use.
    pub fn restore_files(&mut self, files: &[(Option<PathBuf>, String)]) -> Vec<String> {
        let mut pending: Vec<&(Option<PathBuf>, String)> = files.iter().collect();
        let mut errors = Vec::new();
        // A restored file can include a file that has a restored text too:
        // that file is in the project only after the first file.
        loop {
            let before = pending.len();
            pending.retain(|(path, text)| {
                let id = match path {
                    Some(path) => self.project.file_id_by_path(path),
                    None => Some(self.file),
                };
                let Some(id) = id else {
                    return true;
                };
                if let Err(err) = self.project.restore_file_text(id, text) {
                    errors.push(err.to_string());
                }
                false
            });
            if pending.is_empty() || pending.len() == before {
                break;
            }
        }
        for (path, text) in pending {
            match BtFile::parse(text, path.clone()) {
                Ok(mut file) => {
                    file.bump_generation();
                    self.project.add_bt_file(file, FileRole::Tree);
                    self.project.refresh();
                }
                Err(err) => errors.push(err.to_string()),
            }
        }
        self.history = History::new();
        self.changed();
        errors
    }

    /// A tree ID that no tree of the project has: `base` (or `MainTree` if
    /// `base` is not a valid ID), with a number after it if needed.
    pub fn unique_tree_id(&self, base: &str) -> String {
        let base = if fasttick_core::bt::names::validate_model_name(base).is_ok() {
            base
        } else {
            "MainTree"
        };
        let taken = |id: &str| self.project.find_tree(id).is_some();
        if !taken(base) {
            return base.to_owned();
        }
        (2..)
            .map(|n| format!("{base}{n}"))
            .find(|id| !taken(id))
            .unwrap_or_else(|| base.to_owned())
    }

    /// The project.
    pub fn project(&self) -> &Project {
        &self.project
    }

    /// The main file of the document.
    pub fn file_id(&self) -> FileId {
        self.file
    }

    /// The main file, if it is still in the project.
    pub fn file(&self) -> Option<&BtFile> {
        self.project.file(self.file)
    }

    /// The path of the main file, if it has one.
    pub fn path(&self) -> Option<PathBuf> {
        self.file().and_then(|f| f.path().map(Path::to_path_buf))
    }

    /// All trees of all files of the project, in project order. The
    /// position of a tree in this list is its *tree index*: views and the
    /// tree list use it.
    pub fn tree_refs(&self) -> Vec<TreeRef> {
        self.project.trees()
    }

    /// The trees of the project: tree index and ID.
    pub fn trees(&self) -> Vec<(usize, String)> {
        self.tree_refs()
            .into_iter()
            .enumerate()
            .filter_map(|(index, r)| Some((index, self.project.tree(r)?.id.clone())))
            .collect()
    }

    /// The file and the position in the file of a tree index.
    pub fn tree_ref(&self, index: usize) -> Option<TreeRef> {
        self.tree_refs().get(index).copied()
    }

    /// The file of a tree index.
    pub fn tree_file(&self, index: usize) -> Option<FileId> {
        self.tree_ref(index).map(|r| r.file)
    }

    /// The tree index of a reference.
    fn index_of(&self, tree: TreeRef) -> Option<usize> {
        self.tree_refs().iter().position(|r| *r == tree)
    }

    /// The index of the main tree: the tree that the main file runs
    /// (`main_tree_to_execute`, see [`Project::main_tree`]), else the first
    /// tree of the main file, else the first tree.
    pub fn main_tree_index(&self) -> usize {
        self.project
            .main_tree(self.file)
            .and_then(|r| self.index_of(r))
            .or_else(|| {
                self.index_of(TreeRef {
                    file: self.file,
                    index: 0,
                })
            })
            .unwrap_or(0)
    }

    /// True if the tree is the `main_tree_to_execute` of its file.
    pub fn is_main_tree(&self, index: usize) -> bool {
        let Some(r) = self.tree_ref(index) else {
            return false;
        };
        let Some(file) = self.project.file(r.file) else {
            return false;
        };
        let main = file.index().main_tree_to_execute.as_deref();
        main.is_some() && file.trees().get(r.index).map(|t| t.id.as_str()) == main
    }

    /// The tree index of a tree ID (the first tree with this ID in project
    /// order).
    pub fn tree_index(&self, id: &str) -> Option<usize> {
        self.project.find_tree(id).and_then(|r| self.index_of(r))
    }

    /// The tree index of the tree `id` of `file`; else of the first tree
    /// with this ID. Several files can have trees with the same ID (for
    /// example a `MainTree` in each task file).
    pub fn tree_index_in(&self, file: FileId, id: &str) -> Option<usize> {
        self.tree_refs()
            .iter()
            .position(|r| r.file == file && self.project.tree(*r).is_some_and(|t| t.id == id))
            .or_else(|| self.tree_index(id))
    }

    /// The tree index of the tree that a `<SubTree ID="id">` node of `file`
    /// shows (see `Project::find_tree_from`).
    pub fn subtree_target_index(&self, file: FileId, id: &str) -> Option<usize> {
        self.project
            .find_tree_from(file, id)
            .and_then(|r| self.index_of(r))
    }

    /// True if more than one file of the project has a tree with this ID.
    pub fn tree_id_repeats(&self, id: &str) -> bool {
        let mut files = self
            .tree_refs()
            .into_iter()
            .filter(|r| self.project.tree(*r).is_some_and(|t| t.id == id))
            .map(|r| r.file);
        files.next().is_some_and(|first| files.any(|f| f != first))
    }

    /// The tree ID of a tree index.
    pub fn tree_id(&self, index: usize) -> Option<String> {
        self.tree_ref(index)
            .and_then(|r| self.project.tree(r))
            .map(|t| t.id.clone())
    }

    /// The files of the project that have trees or can get trees (not the
    /// model files), in project order.
    pub fn tree_files(&self) -> Vec<FileId> {
        self.project
            .files()
            .filter(|(_, role, _)| *role != FileRole::Models)
            .map(|(id, _, _)| id)
            .collect()
    }

    /// True if the elements of a file can be edited in this document.
    pub fn is_tree_file(&self, file: FileId) -> bool {
        self.project
            .role(file)
            .is_some_and(|role| role != FileRole::Models)
    }

    /// A short name of a file: its file name, or "Untitled".
    pub fn file_name(&self, file: FileId) -> String {
        self.project
            .file(file)
            .and_then(|f| f.path())
            .and_then(|p| p.file_name())
            .map(|n| n.to_string_lossy().into_owned())
            .unwrap_or_else(|| "Untitled".to_owned())
    }

    /// The tree index of the tree that holds `node` (an element of
    /// `file`): the tree of its `<BehaviorTree>` ancestor, or the tree of an
    /// unconnected node.
    pub fn tree_of_node(&self, file: FileId, node: NodeId) -> Option<usize> {
        let bt = self.project.file(file)?;
        let doc = bt.doc();
        let mut top = node;
        while let Some(parent) = doc.parent(top) {
            if let Some(index) = bt.index().trees.iter().position(|t| t.element == parent) {
                return self.index_of(TreeRef { file, index });
            }
            top = parent;
        }
        let (_, tree, _) = self
            .project
            .all_unconnected()
            .find(|(f, _, u)| *f == file && u.node == top)?;
        let index = bt.index().tree_index(tree)?;
        self.index_of(TreeRef { file, index })
    }

    /// Adds an XML file to the project (with its includes). This change
    /// has no undo; remove the file from the project file to undo it.
    pub fn add_tree_file(&mut self, path: &Path) -> Result<FileId, String> {
        let id = self.project.add_file(path).map_err(|err| err.to_string())?;
        self.changed();
        Ok(id)
    }

    /// Adds a node model file (for example the output of
    /// `BT::writeTreeNodesModelXML`) to the project. Its models are visible
    /// in all files. This change has no undo.
    pub fn add_model_file(&mut self, path: &Path) -> Result<FileId, String> {
        let id = self
            .project
            .add_model_file(path)
            .map_err(|err| err.to_string())?;
        self.changed();
        Ok(id)
    }

    /// Adds the XML files of the folder that have the missing trees and
    /// node models (see [`Project::add_missing_files`]). This change has no
    /// undo.
    pub fn add_missing_files(&mut self) -> Result<MissingFiles, String> {
        let result = self.project.add_missing_files();
        // Some files can be in the project also after an error.
        self.changed();
        result.map_err(|err| err.to_string())
    }

    /// Makes a new XML file with one empty tree (its ID comes from the file
    /// name), writes it to `path` and adds it to the project.
    pub fn new_tree_file(&mut self, path: &Path) -> Result<FileId, String> {
        if path.exists() {
            return Err(format!("{} exists already.", path.display()));
        }
        let stem = path
            .file_stem()
            .map(|s| s.to_string_lossy().into_owned())
            .unwrap_or_default();
        let tree_id = self.unique_tree_id(&stem);
        let id = self
            .project
            .add_bt_file(BtFile::new_empty(), FileRole::Tree);
        // The first tree is part of the new file, not an undo step.
        History::new()
            .run(&mut self.project, |ed| {
                ed.add_tree(id, &tree_id)?;
                ed.set_main_tree(id, Some(&tree_id))
            })
            .map_err(|err| err.to_string())?;
        self.project
            .save_file_as(id, path)
            .map_err(|err| err.to_string())?;
        self.project.refresh();
        self.changed();
        Ok(id)
    }

    /// Writes the project file (`.ftproj`) to a new path. The document
    /// becomes a project document. Changed XML files are saved too.
    pub fn save_project_as(&mut self, path: &Path) -> Result<(), String> {
        if self.file().is_some_and(|f| f.path().is_none()) {
            return Err("Save the tree file first: a project lists files by path.".into());
        }
        self.project
            .save_dirty_files()
            .map_err(|err| err.to_string())?;
        self.project.save_as(path).map_err(|err| err.to_string())?;
        self.changed();
        Ok(())
    }

    /// The path of the project file, if the document is a project.
    pub fn project_path(&self) -> Option<&Path> {
        self.project.path()
    }

    /// The revision. It increments after each change.
    pub fn revision(&self) -> u64 {
        self.revision
    }

    /// The problems of the project, from the last validation.
    pub fn problems(&self) -> &[Problem] {
        &self.problems
    }

    /// The number of errors and warnings.
    pub fn problem_counts(&self) -> ProblemCounts {
        ProblemCounts::of(&self.problems)
    }

    /// Validates the project again.
    pub fn validate(&mut self) {
        self.problems = validate_project(&self.project);
    }

    fn changed(&mut self) {
        self.validate();
        self.revision += 1;
    }

    /// Runs one edit session (one undo step). On an error nothing changes.
    pub fn edit<T>(
        &mut self,
        f: impl FnOnce(&mut Editor<'_>) -> Result<T, CommandError>,
    ) -> Result<T, String> {
        let result = self.history.run(&mut self.project, f);
        match result {
            Ok(value) => {
                self.changed();
                Ok(value)
            }
            Err(err) => Err(err.to_string()),
        }
    }

    /// Like [`EditorDoc::edit`], but merges with the previous undo step if
    /// it has the same key and came less than [`COALESCE_WINDOW`] ago.
    pub fn edit_coalesced<T>(
        &mut self,
        key: CoalesceKey,
        f: impl FnOnce(&mut Editor<'_>) -> Result<T, CommandError>,
    ) -> Result<T, String> {
        let coalesce = Coalesce {
            key,
            now: Instant::now(),
            window: COALESCE_WINDOW,
        };
        let result = self.history.run_coalesced(&mut self.project, coalesce, f);
        match result {
            Ok(value) => {
                self.changed();
                Ok(value)
            }
            Err(err) => Err(err.to_string()),
        }
    }

    /// Undoes the last step. Returns its label.
    pub fn undo(&mut self) -> Result<Option<String>, String> {
        let result = self
            .history
            .undo(&mut self.project)
            .map_err(|err| err.to_string())?;
        if result.is_some() {
            self.changed();
        }
        Ok(result)
    }

    /// Redoes the last undone step. Returns its label.
    pub fn redo(&mut self) -> Result<Option<String>, String> {
        let result = self
            .history
            .redo(&mut self.project)
            .map_err(|err| err.to_string())?;
        if result.is_some() {
            self.changed();
        }
        Ok(result)
    }

    /// True if there is a step to undo.
    pub fn can_undo(&self) -> bool {
        self.history.can_undo()
    }

    /// True if there is a step to redo.
    pub fn can_redo(&self) -> bool {
        self.history.can_redo()
    }

    /// The label of the step that undo would revert.
    pub fn undo_label(&self) -> Option<&str> {
        self.history.undo_label()
    }

    /// The label of the step that redo would apply again.
    pub fn redo_label(&self) -> Option<&str> {
        self.history.redo_label()
    }

    /// True if an XML file has unsaved changes, or the project file (if
    /// the document has one) has unsaved state. A document that is a plain
    /// XML file has no project file, so only its XML counts.
    pub fn is_dirty(&self) -> bool {
        !self.project.dirty_files().is_empty()
            || (self.project.path().is_some() && self.project.is_project_dirty())
    }

    /// Sets the folder with the copies of the files of another computer
    /// (see [`fasttick_core::project::MirrorRoot`]); the includes and the
    /// problems are computed again.
    pub fn set_mirror_root(&mut self, mirror: Option<fasttick_core::project::MirrorRoot>) {
        self.project.set_mirror_root(mirror);
        self.changed();
    }

    /// The files that [`EditorDoc::save`] writes: the XML files with
    /// changes, and the project file if it has changes.
    pub fn unsaved_paths(&self) -> Vec<PathBuf> {
        let mut paths: Vec<PathBuf> = self
            .project
            .dirty_files()
            .into_iter()
            .filter_map(|id| self.project.file(id)?.path().map(Path::to_path_buf))
            .collect();
        if self.project.is_project_dirty()
            && let Some(path) = self.project.path()
        {
            paths.push(path.to_path_buf());
        }
        paths
    }

    /// The paths of all files of the project, and of the project file.
    pub fn all_paths(&self) -> Vec<PathBuf> {
        let mut paths: Vec<PathBuf> = self
            .project
            .files()
            .filter_map(|(_, _, file)| file.path().map(Path::to_path_buf))
            .collect();
        if let Some(path) = self.project.path() {
            paths.push(path.to_path_buf());
        }
        paths
    }

    /// Saves all changed XML files, and the project file if the project
    /// has one. See [`SaveOutcome`].
    pub fn save(&mut self) -> Result<SaveOutcome, String> {
        if self.file().is_some_and(|f| f.path().is_none()) {
            return Ok(SaveOutcome::NeedsPath);
        }
        self.project
            .save_dirty_files()
            .map_err(|err| err.to_string())?;
        if self.project.path().is_some() && self.project.is_project_dirty() {
            self.project.save().map_err(|err| err.to_string())?;
        }
        self.revision += 1;
        Ok(SaveOutcome::Saved)
    }

    /// Saves the main file under a new path, then the other changed files.
    pub fn save_as(&mut self, path: &Path) -> Result<(), String> {
        self.project
            .save_file_as(self.file, path)
            .map_err(|err| err.to_string())?;
        self.project
            .save_dirty_files()
            .map_err(|err| err.to_string())?;
        if self.project.path().is_some() && self.project.is_project_dirty() {
            self.project.save().map_err(|err| err.to_string())?;
        }
        self.changed();
        Ok(())
    }

    /// Builds the canvas tree of a tree (a tree index, see
    /// [`EditorDoc::tree_refs`]), with the problem markers.
    pub fn build_canvas(&self, tree_index: usize, expansion: &Expansion) -> CanvasTree {
        let r = self.tree_ref(tree_index).unwrap_or(TreeRef {
            file: self.file,
            index: 0,
        });
        let mut tree = CanvasTree::from_project(&self.project, r.file, r.index, expansion);
        // A problem in a folded part marks the folded node.
        tree.apply_problems_with(&self.problems, |file, node| {
            self.project.file(file)?.doc().parent(node)
        });
        tree
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::canvas::NodeSource;

    fn editor_from(xml: &str) -> EditorDoc {
        let (project, file) = Project::with_file("t", BtFile::parse(xml, None).unwrap());
        EditorDoc::new(project, file)
    }

    #[test]
    fn edit_undo_redo_and_revision() {
        let mut editor = editor_from(
            r#"<root BTCPP_format="4"><BehaviorTree ID="T"><Sequence name="s"/></BehaviorTree></root>"#,
        );
        assert_eq!(editor.trees(), vec![(0, "T".to_owned())]);
        let start = editor.revision();
        // An empty Sequence is an error (a control needs a child).
        assert!(editor.problem_counts().errors > 0);
        let tree = editor.build_canvas(0, &Expansion::none());
        let NodeSource::Xml(file, seq) = tree.nodes()[0].source else {
            panic!("an XML node");
        };
        editor
            .edit(|ed| ed.add_node(file, seq, 0, "AlwaysSuccess", None))
            .unwrap();
        assert!(editor.revision() > start);
        assert_eq!(editor.problem_counts().errors, 0);
        assert!(editor.is_dirty());
        assert_eq!(editor.build_canvas(0, &Expansion::none()).len(), 2);
        assert!(editor.can_undo());
        editor.undo().unwrap();
        assert_eq!(editor.build_canvas(0, &Expansion::none()).len(), 1);
        assert!(editor.problem_counts().errors > 0);
        editor.redo().unwrap();
        assert_eq!(editor.build_canvas(0, &Expansion::none()).len(), 2);
        // A failed command changes nothing: the <BehaviorTree> element is
        // not a tree node, so it cannot be deleted as one.
        let before = editor.revision();
        let tree_element = editor.file().unwrap().index().trees[0].element;
        assert!(
            editor
                .edit(|ed| ed.delete_nodes(file, &[tree_element]))
                .is_err()
        );
        assert_eq!(editor.revision(), before);
        assert_eq!(editor.build_canvas(0, &Expansion::none()).len(), 2);
    }

    #[test]
    fn save_needs_a_path_for_a_new_file() {
        let mut editor = EditorDoc::untitled();
        assert_eq!(editor.save().unwrap(), SaveOutcome::NeedsPath);
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("new.xml");
        editor.save_as(&path).unwrap();
        assert!(!editor.is_dirty());
        assert_eq!(editor.path().as_deref(), Some(path.as_path()));
        assert!(
            std::fs::read_to_string(&path)
                .unwrap()
                .contains("BTCPP_format")
        );
        assert_eq!(editor.save().unwrap(), SaveOutcome::Saved);
    }

    #[test]
    fn open_xml_and_main_tree() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("two.xml");
        std::fs::write(
            &path,
            r#"<root BTCPP_format="4" main_tree_to_execute="B">
                <BehaviorTree ID="A"><AlwaysSuccess/></BehaviorTree>
                <BehaviorTree ID="B"><AlwaysFailure/></BehaviorTree>
            </root>"#,
        )
        .unwrap();
        let editor = EditorDoc::open(&path).unwrap();
        assert_eq!(editor.main_tree_index(), 1);
        assert_eq!(editor.tree_id(1).as_deref(), Some("B"));
        assert!(EditorDoc::open(&dir.path().join("missing.xml")).is_err());
    }
}
