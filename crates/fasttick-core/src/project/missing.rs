//! Find the files that have the trees and the node models that a project
//! uses but does not have.
//!
//! A tree file often uses SubTrees that are in other files of the same
//! folder, without an `<include>`. The executor loads all these files.
//! [`Project::find_missing_files`] looks at the XML files in the folders of
//! the project files and selects the files that have the missing trees
//! (as tree files) and the missing node models (as model files, only files
//! without trees). The search continues with the trees that the selected
//! files use, until nothing changes.

use std::collections::{BTreeSet, HashSet};
use std::path::{Path, PathBuf};

use crate::bt::BtFile;
use crate::models::{ModelSource, parse_models_document};
use crate::validate::{ProblemCode, validate_project};

use super::{FileRole, Project, ProjectError, identity};

/// The maximum number of XML files that the search reads in one folder.
pub const MAX_FOLDER_FILES: usize = 500;

/// The search does not read XML files that are larger than this.
pub const MAX_FOLDER_FILE_SIZE: u64 = 16 * 1024 * 1024;

/// The trees and the node types that a project uses but does not have.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct MissingNames {
    /// The IDs of the missing trees (from `<SubTree ID="...">`).
    pub trees: BTreeSet<String>,
    /// The node types without a model.
    pub node_types: BTreeSet<String>,
}

impl MissingNames {
    /// True if nothing is missing.
    pub fn is_empty(&self) -> bool {
        self.trees.is_empty() && self.node_types.is_empty()
    }

    /// The number of missing names.
    pub fn len(&self) -> usize {
        self.trees.len() + self.node_types.len()
    }
}

/// The result of [`Project::find_missing_files`] and
/// [`Project::add_missing_files`].
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct MissingFiles {
    /// The names that were missing before the search.
    pub missing: MissingNames,
    /// The files to add as tree files, in the order to add them.
    pub tree_files: Vec<PathBuf>,
    /// The files to add as model files, in the order to add them.
    pub model_files: Vec<PathBuf>,
    /// The names that are still missing after the files are added.
    pub still_missing: MissingNames,
}

impl MissingFiles {
    /// True if the search found no file.
    pub fn is_empty(&self) -> bool {
        self.tree_files.is_empty() && self.model_files.is_empty()
    }

    /// All files: the tree files, then the model files.
    pub fn files(&self) -> impl Iterator<Item = &PathBuf> {
        self.tree_files.iter().chain(&self.model_files)
    }
}

/// An XML file of a folder and the names that it has.
#[derive(Debug, Clone)]
struct Candidate {
    path: PathBuf,
    trees: Vec<String>,
    models: Vec<String>,
}

impl Project {
    /// The trees and the node types that the project uses but does not
    /// have. These are the targets of the `MissingSubTree` problems and
    /// the types of the `UnknownNodeType` problems of the validation.
    pub fn missing_names(&self) -> MissingNames {
        let mut out = MissingNames::default();
        for problem in validate_project(self) {
            let (Some(file), Some(node)) = (self.file(problem.file), problem.node) else {
                continue;
            };
            let Some(view) = file.node(node) else {
                continue;
            };
            match problem.code {
                ProblemCode::MissingSubTree => {
                    if let Some(id) = view.attribute("ID").filter(|id| !id.is_empty()) {
                        out.trees.insert(id.to_owned());
                    }
                }
                ProblemCode::UnknownNodeType => {
                    // A tree that is used as a node type is not missing.
                    let tag = view.tag();
                    if self.find_tree(tag).is_none() {
                        out.node_types.insert(tag.to_owned());
                    }
                }
                _ => {}
            }
        }
        out
    }

    /// Looks in the folders of the project files for the XML files that
    /// have the missing trees and node models. The project does not change.
    /// See the [module documentation](self).
    pub fn find_missing_files(&self) -> MissingFiles {
        let missing = self.missing_names();
        let mut out = MissingFiles {
            missing: missing.clone(),
            still_missing: missing.clone(),
            ..MissingFiles::default()
        };
        if missing.is_empty() {
            return out;
        }
        let mut candidates = self.folder_candidates();
        let mut trial = self.clone();
        let mut now = missing;
        while !now.is_empty() {
            let mut added = false;
            let mut index = 0;
            while index < candidates.len() {
                let candidate = &candidates[index];
                let role = if candidate.trees.iter().any(|t| now.trees.contains(t)) {
                    Some(FileRole::Tree)
                } else if candidate.trees.is_empty()
                    && candidate.models.iter().any(|m| now.node_types.contains(m))
                {
                    Some(FileRole::Models)
                } else {
                    None
                };
                let Some(role) = role else {
                    index += 1;
                    continue;
                };
                let candidate = candidates.remove(index);
                let result = match role {
                    FileRole::Models => trial.add_model_file(&candidate.path),
                    _ => trial.add_file(&candidate.path),
                };
                if result.is_ok() {
                    added = true;
                    match role {
                        FileRole::Models => out.model_files.push(candidate.path),
                        _ => out.tree_files.push(candidate.path),
                    }
                }
            }
            if !added {
                break;
            }
            now = trial.missing_names();
        }
        out.still_missing = now;
        out
    }

    /// Adds the files of [`Project::find_missing_files`] to the project
    /// and returns them. If a file cannot be loaded, the function stops
    /// with the error; the files before it stay in the project.
    pub fn add_missing_files(&mut self) -> Result<MissingFiles, ProjectError> {
        let found = self.find_missing_files();
        for path in &found.tree_files {
            self.add_file(path)?;
        }
        for path in &found.model_files {
            self.add_model_file(path)?;
        }
        Ok(found)
    }

    /// The XML files in the folders of the project files (and of the
    /// project file) that are not in the project, sorted by path.
    fn folder_candidates(&self) -> Vec<Candidate> {
        let mut folders: Vec<PathBuf> = Vec::new();
        let mut add_folder = |path: &Path| {
            if let Some(dir) = path.parent() {
                let dir = if dir.as_os_str().is_empty() {
                    PathBuf::from(".")
                } else {
                    dir.to_path_buf()
                };
                if !folders.contains(&dir) {
                    folders.push(dir);
                }
            }
        };
        if let Some(path) = self.path() {
            add_folder(path);
        }
        for (_, role, file) in self.files() {
            if role != FileRole::Included
                && let Some(path) = file.path()
            {
                add_folder(path);
            }
        }
        let known: HashSet<PathBuf> = self
            .files()
            .filter_map(|(_, _, file)| file.path().map(identity))
            .collect();
        let mut seen = HashSet::new();
        let mut out = Vec::new();
        for folder in folders {
            for path in xml_files(&folder) {
                let id = identity(&path);
                if known.contains(&id) || !seen.insert(id) {
                    continue;
                }
                if let Some(candidate) = read_candidate(path) {
                    out.push(candidate);
                }
            }
        }
        out.sort_by(|a, b| a.path.cmp(&b.path));
        out
    }
}

/// The `*.xml` files of a folder (not of its subfolders), sorted, at most
/// [`MAX_FOLDER_FILES`].
fn xml_files(folder: &Path) -> Vec<PathBuf> {
    let Ok(entries) = std::fs::read_dir(folder) else {
        return Vec::new();
    };
    let mut out: Vec<PathBuf> = entries
        .filter_map(Result::ok)
        .filter(|entry| entry.file_type().is_ok_and(|t| t.is_file()))
        .map(|entry| entry.path())
        .filter(|path| {
            path.extension()
                .is_some_and(|e| e.eq_ignore_ascii_case("xml"))
        })
        .collect();
    out.sort();
    out.truncate(MAX_FOLDER_FILES);
    out
}

/// Reads the tree IDs and the model IDs of a file. `None` if the file is
/// too large or is not a BT.CPP XML file.
fn read_candidate(path: PathBuf) -> Option<Candidate> {
    let size = std::fs::metadata(&path).ok()?.len();
    if size > MAX_FOLDER_FILE_SIZE {
        return None;
    }
    let file = BtFile::load(&path).ok()?;
    let doc = file.doc();
    if doc.document_element().and_then(|r| doc.name(r)) != Some("root") {
        return None;
    }
    let trees = file.trees().iter().map(|t| t.id.clone()).collect();
    let models = parse_models_document(doc, ModelSource::Unattached)
        .models
        .into_iter()
        .map(|m| m.model.id)
        .collect();
    Some(Candidate {
        path,
        trees,
        models,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn write(dir: &Path, name: &str, text: &str) -> PathBuf {
        let path = dir.join(name);
        std::fs::write(&path, text).unwrap();
        path
    }

    fn setup() -> tempfile::TempDir {
        let dir = tempfile::tempdir().unwrap();
        write(
            dir.path(),
            "main.xml",
            r#"<root BTCPP_format="4" main_tree_to_execute="Main">
                 <BehaviorTree ID="Main"><Sequence>
                   <SubTree ID="Get"/><Grab/><SubTree ID="Gone"/>
                 </Sequence></BehaviorTree>
               </root>"#,
        );
        write(
            dir.path(),
            "subtrees.xml",
            r#"<root BTCPP_format="4">
                 <BehaviorTree ID="Get"><SubTree ID="Reset"/></BehaviorTree>
               </root>"#,
        );
        write(
            dir.path(),
            "automation.xml",
            r#"<root BTCPP_format="4">
                 <BehaviorTree ID="Reset"><AlwaysSuccess/></BehaviorTree>
               </root>"#,
        );
        write(
            dir.path(),
            "models.xml",
            r#"<root BTCPP_format="4"><TreeNodesModel>
                 <Action ID="Grab"/></TreeNodesModel></root>"#,
        );
        write(
            dir.path(),
            "other.xml",
            r#"<root BTCPP_format="4">
                 <BehaviorTree ID="Unused"><AlwaysSuccess/></BehaviorTree>
               </root>"#,
        );
        write(dir.path(), "broken.xml", "<root><BehaviorTree");
        write(dir.path(), "notes.txt", "Get Reset Grab");
        dir
    }

    fn names(list: &[&str]) -> BTreeSet<String> {
        list.iter().map(|s| (*s).to_owned()).collect()
    }

    fn file_names(paths: &[PathBuf]) -> Vec<String> {
        paths
            .iter()
            .map(|p| p.file_name().unwrap().to_string_lossy().into_owned())
            .collect()
    }

    #[test]
    fn missing_names_come_from_the_validation() {
        let dir = setup();
        let project = Project::open_xml(dir.path().join("main.xml")).unwrap();
        let missing = project.missing_names();
        assert_eq!(missing.trees, names(&["Get", "Gone"]));
        assert_eq!(missing.node_types, names(&["Grab"]));
        assert_eq!(missing.len(), 3);
    }

    #[test]
    fn the_search_follows_the_subtrees_of_the_found_files() {
        let dir = setup();
        let project = Project::open_xml(dir.path().join("main.xml")).unwrap();
        let found = project.find_missing_files();
        assert_eq!(
            file_names(&found.tree_files),
            ["subtrees.xml", "automation.xml"]
        );
        assert_eq!(file_names(&found.model_files), ["models.xml"]);
        assert_eq!(found.still_missing.trees, names(&["Gone"]));
        assert!(found.still_missing.node_types.is_empty());
        // The search does not change the project.
        assert_eq!(project.files().count(), 1);
    }

    #[test]
    fn adding_the_files_removes_the_errors() {
        let dir = setup();
        let mut project = Project::open_xml(dir.path().join("main.xml")).unwrap();
        let found = project.add_missing_files().unwrap();
        assert_eq!(found.files().count(), 3);
        assert_eq!(project.missing_names().trees, names(&["Gone"]));
        assert!(project.missing_names().node_types.is_empty());
        // A second search finds nothing new.
        assert!(project.find_missing_files().is_empty());
    }

    #[test]
    fn nothing_missing_reads_no_folder() {
        let dir = setup();
        let project = Project::open_xml(dir.path().join("automation.xml")).unwrap();
        let found = project.find_missing_files();
        assert!(found.is_empty());
        assert!(found.missing.is_empty());
    }
}
