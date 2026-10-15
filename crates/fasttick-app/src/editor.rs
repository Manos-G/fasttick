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
use fasttick_core::project::{FileId, FileRole, Project};
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

    /// A new, unsaved file with one empty tree.
    pub fn untitled() -> EditorDoc {
        let (project, file) = Project::with_file("Untitled", BtFile::new_empty());
        EditorDoc::new(project, file)
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

    /// The trees of the main file: index and ID.
    pub fn trees(&self) -> Vec<(usize, String)> {
        self.file()
            .map(|file| {
                file.index()
                    .trees
                    .iter()
                    .enumerate()
                    .map(|(index, tree)| (index, tree.id.clone()))
                    .collect()
            })
            .unwrap_or_default()
    }

    /// The index of the main tree: the tree that `main_tree_to_execute`
    /// names, else the first tree.
    pub fn main_tree_index(&self) -> usize {
        let Some(file) = self.file() else {
            return 0;
        };
        let index = file.index();
        index
            .main_tree_to_execute
            .as_deref()
            .and_then(|id| index.tree_index(id))
            .unwrap_or(0)
    }

    /// The tree index of a tree ID in the main file.
    pub fn tree_index(&self, id: &str) -> Option<usize> {
        self.file().and_then(|f| f.index().tree_index(id))
    }

    /// The tree ID of a tree index in the main file.
    pub fn tree_id(&self, index: usize) -> Option<String> {
        self.file()
            .and_then(|f| f.index().trees.get(index).map(|t| t.id.clone()))
    }

    /// The index of the tree that holds `node` (an element of the main
    /// file): the tree of its `<BehaviorTree>` ancestor, or the tree of an
    /// unconnected node.
    pub fn tree_of_node(&self, node: NodeId) -> Option<usize> {
        let file = self.file()?;
        let doc = file.doc();
        let mut top = node;
        while let Some(parent) = doc.parent(top) {
            if let Some(index) = file.index().trees.iter().position(|t| t.element == parent) {
                return Some(index);
            }
            top = parent;
        }
        let (_, tree, _) = self
            .project
            .all_unconnected()
            .find(|(f, _, u)| *f == self.file && u.node == top)?;
        self.tree_index(tree)
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

    /// Builds the canvas tree of a tree of the main file, with the problem
    /// markers.
    pub fn build_canvas(&self, tree_index: usize, expansion: &Expansion) -> CanvasTree {
        let mut tree = CanvasTree::from_project(&self.project, self.file, tree_index, expansion);
        tree.apply_problems(&self.problems);
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
