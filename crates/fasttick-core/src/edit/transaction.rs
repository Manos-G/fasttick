//! Invertible edits and transactions.

use crate::bt::BtIndex;
use crate::project::{FileId, LayoutState, Project, UnconnectedNode};
use crate::xml::{Change, EditError, NodeId};

use super::error::CommandError;

/// One invertible operation on a [`Project`].
///
/// Most edits change the XML document of one file ([`Edit::Xml`]). The
/// other edits change the project state that the XML cannot hold: the
/// layout state of a tree and the unconnected nodes (decision D5 in
/// `docs/PLAN.md`).
///
/// [`Transaction::apply`] applies edits and returns the edits that undo
/// them.
#[derive(Debug, Clone, PartialEq)]
pub enum Edit {
    /// A primitive change of the document of a file.
    Xml {
        /// The file.
        file: FileId,
        /// The change.
        change: Change,
    },
    /// Sets (`Some`) or removes (`None`) the layout state of a tree. The
    /// inverse puts the old state back.
    Layout {
        /// The file.
        file: FileId,
        /// The tree ID.
        tree: String,
        /// The new state.
        state: Option<Box<LayoutState>>,
    },
    /// Inserts an unconnected node at `index` in the list of a tree. The
    /// node must be a detached element of the file. The inverse is
    /// [`Edit::RemoveUnconnected`].
    InsertUnconnected {
        /// The file.
        file: FileId,
        /// The tree ID.
        tree: String,
        /// The index in the list of the tree.
        index: usize,
        /// The node and its position.
        node: UnconnectedNode,
    },
    /// Removes the unconnected node at `index` in the list of a tree. The
    /// inverse is [`Edit::InsertUnconnected`].
    RemoveUnconnected {
        /// The file.
        file: FileId,
        /// The tree ID.
        tree: String,
        /// The index in the list of the tree.
        index: usize,
    },
    /// Moves an unconnected node on the canvas. The inverse puts the old
    /// position back.
    UnconnectedPosition {
        /// The file.
        file: FileId,
        /// The unconnected node.
        node: NodeId,
        /// The new position.
        position: [f32; 2],
    },
}

impl Edit {
    /// The file of the edit.
    pub fn file(&self) -> FileId {
        match self {
            Edit::Xml { file, .. }
            | Edit::Layout { file, .. }
            | Edit::InsertUnconnected { file, .. }
            | Edit::RemoveUnconnected { file, .. }
            | Edit::UnconnectedPosition { file, .. } => *file,
        }
    }

    /// Applies the edit and returns the edit that undoes it. The function
    /// does not build the index of the file again; [`Transaction::apply`]
    /// and the [`Editor`](super::Editor) do that.
    pub(crate) fn apply_raw(self, project: &mut Project) -> Result<Edit, CommandError> {
        match self {
            Edit::Xml { file, change } => {
                let bt = project
                    .file_mut(file)
                    .ok_or(CommandError::UnknownFile(file))?;
                let inverse = bt.doc_mut().apply(change)?;
                Ok(Edit::Xml {
                    file,
                    change: inverse,
                })
            }
            Edit::Layout { file, tree, state } => {
                check_file(project, file)?;
                let old = match state {
                    Some(state) => {
                        let old = project.layout(file, &tree).cloned();
                        project.set_layout(file, &tree, *state);
                        old
                    }
                    None => project.remove_layout(file, &tree),
                };
                Ok(Edit::Layout {
                    file,
                    tree,
                    state: old.map(Box::new),
                })
            }
            Edit::InsertUnconnected {
                file,
                tree,
                index,
                node,
            } => {
                insert_unconnected(project, file, &tree, index, node)?;
                Ok(Edit::RemoveUnconnected { file, tree, index })
            }
            Edit::RemoveUnconnected { file, tree, index } => {
                check_file(project, file)?;
                let item = project
                    .unconnected(file, &tree)
                    .get(index)
                    .copied()
                    .ok_or_else(|| CommandError::UnconnectedMismatch {
                        file,
                        tree: tree.clone(),
                    })?;
                project.remove_unconnected(file, item.node);
                Ok(Edit::InsertUnconnected {
                    file,
                    tree,
                    index,
                    node: item,
                })
            }
            Edit::UnconnectedPosition {
                file,
                node,
                position,
            } => {
                check_file(project, file)?;
                let (tree, old) = project
                    .all_unconnected()
                    .find(|(f, _, n)| *f == file && n.node == node)
                    .map(|(_, tree, n)| (tree.to_owned(), n.position))
                    .ok_or(CommandError::NotUnconnected(node))?;
                if !project.set_unconnected_position(file, node, position) {
                    return Err(CommandError::UnconnectedMismatch { file, tree });
                }
                Ok(Edit::UnconnectedPosition {
                    file,
                    node,
                    position: old,
                })
            }
        }
    }
}

fn check_file(project: &Project, file: FileId) -> Result<(), CommandError> {
    project
        .file(file)
        .map(|_| ())
        .ok_or(CommandError::UnknownFile(file))
}

/// Inserts an unconnected node at `index` in the list of a tree.
///
/// The project has only an "append" function, so the function removes the
/// nodes after `index`, appends the new node and appends the removed nodes
/// again. The lists are short.
fn insert_unconnected(
    project: &mut Project,
    file: FileId,
    tree: &str,
    index: usize,
    node: UnconnectedNode,
) -> Result<(), CommandError> {
    let bt = project.file(file).ok_or(CommandError::UnknownFile(file))?;
    let doc = bt.doc();
    let mismatch = || CommandError::UnconnectedMismatch {
        file,
        tree: tree.to_owned(),
    };
    if !doc.is_element(node.node) {
        return Err(EditError::NotAnElement(node.node).into());
    }
    if doc.parent(node.node).is_some() {
        return Err(EditError::AlreadyAttached(node.node).into());
    }
    if project
        .all_unconnected()
        .any(|(f, _, n)| f == file && n.node == node.node)
    {
        return Err(CommandError::AlreadyUnconnected(node.node));
    }
    let list = project.unconnected(file, tree);
    if index > list.len() {
        return Err(mismatch());
    }
    let tail: Vec<UnconnectedNode> = list[index..].to_vec();
    for item in &tail {
        project.remove_unconnected(file, item.node);
    }
    project.add_unconnected(file, tree, node.node, node.position);
    for item in tail {
        project.add_unconnected(file, tree, item.node, item.position);
    }
    Ok(())
}

/// A list of edits with a label, for example "Add node".
///
/// [`Transaction::apply`] applies all edits or none. The edits of one
/// transaction can change several files of a project.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct Transaction {
    /// The label for the user, for example in the "Undo" menu item.
    pub label: String,
    /// The edits, in the order in which to apply them.
    pub edits: Vec<Edit>,
}

/// The generations of one file before and after a transaction (see
/// [`crate::bt::BtFile::generation`]).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct FileGeneration {
    /// The file.
    pub file: FileId,
    /// The generation before the transaction.
    pub before: u64,
    /// The generation after the transaction.
    pub after: u64,
}

impl Transaction {
    /// Makes an empty transaction.
    pub fn new(label: impl Into<String>) -> Transaction {
        Transaction {
            label: label.into(),
            edits: Vec::new(),
        }
    }

    /// Makes a transaction from XML changes.
    pub fn from_changes(
        label: impl Into<String>,
        changes: impl IntoIterator<Item = (FileId, Change)>,
    ) -> Transaction {
        Transaction {
            label: label.into(),
            edits: changes
                .into_iter()
                .map(|(file, change)| Edit::Xml { file, change })
                .collect(),
        }
    }

    /// True if the transaction has no edits.
    pub fn is_empty(&self) -> bool {
        self.edits.is_empty()
    }

    /// The XML changes of the transaction, with their files, in order.
    pub fn changes(&self) -> impl Iterator<Item = (FileId, &Change)> {
        self.edits.iter().filter_map(|e| match e {
            Edit::Xml { file, change } => Some((*file, change)),
            _ => None,
        })
    }

    /// The files that the XML edits change, each once, in order.
    pub fn xml_files(&self) -> Vec<FileId> {
        let mut out: Vec<FileId> = Vec::new();
        for edit in &self.edits {
            if let Edit::Xml { file, .. } = edit
                && !out.contains(file)
            {
                out.push(*file);
            }
        }
        out
    }

    /// Applies the edits in order and returns the transaction that undoes
    /// them (with the same label).
    ///
    /// After the edits, the function builds the index of each changed file
    /// again, gives each changed file a new generation (so the file becomes
    /// dirty), and calls [`Project::refresh`] if the trees, the includes or
    /// the models of a file changed.
    ///
    /// If an edit fails, the function undoes the edits that it applied and
    /// returns the error. The project is then as before the call, and the
    /// generations do not change.
    pub fn apply(self, project: &mut Project) -> Result<Transaction, CommandError> {
        self.apply_tracked(project).map(|(inverse, _)| inverse)
    }

    /// [`Transaction::apply`], and also the generations of the changed
    /// files.
    pub(crate) fn apply_tracked(
        self,
        project: &mut Project,
    ) -> Result<(Transaction, Vec<FileGeneration>), CommandError> {
        let mut tracker = FileTracker::default();
        let mut inverses = Vec::with_capacity(self.edits.len());
        for edit in self.edits {
            let result = match &edit {
                Edit::Xml { file, .. } => tracker.touch(project, *file),
                _ => Ok(()),
            }
            .and_then(|()| edit.apply_raw(project));
            match result {
                Ok(inverse) => inverses.push(inverse),
                Err(error) => {
                    for inverse in inverses.into_iter().rev() {
                        // The inverse of an edit that just succeeded is valid.
                        let _ = inverse.apply_raw(project);
                    }
                    tracker.finish(project, |_| false);
                    return Err(error);
                }
            }
        }
        let generations = tracker.finish(project, |_| true);
        inverses.reverse();
        Ok((
            Transaction {
                label: self.label,
                edits: inverses,
            },
            generations,
        ))
    }
}

/// The files that a transaction changes, with their state before the
/// first change.
#[derive(Debug, Default)]
pub(crate) struct FileTracker {
    files: Vec<Touched>,
}

#[derive(Debug)]
struct Touched {
    file: FileId,
    generation: u64,
    /// The index at the first change or at the last refresh of the project.
    index: BtIndex,
}

impl FileTracker {
    /// Records a file before its first change.
    pub(crate) fn touch(&mut self, project: &Project, file: FileId) -> Result<(), CommandError> {
        if self.files.iter().any(|t| t.file == file) {
            return Ok(());
        }
        let bt = project.file(file).ok_or(CommandError::UnknownFile(file))?;
        self.files.push(Touched {
            file,
            generation: bt.generation(),
            index: bt.index().clone(),
        });
        Ok(())
    }

    /// True if the index of a changed file is not the index at the last
    /// refresh: the project must call [`Project::refresh`] before it uses
    /// the model registry or the include links.
    pub(crate) fn needs_refresh(&self, project: &Project) -> bool {
        self.files.iter().any(|t| {
            project
                .file(t.file)
                .is_some_and(|bt| *bt.index() != t.index)
        })
    }

    /// Calls [`Project::refresh`] if necessary.
    pub(crate) fn refresh_if_needed(&mut self, project: &mut Project) {
        if self.needs_refresh(project) {
            project.refresh();
            for t in &mut self.files {
                if let Some(bt) = project.file(t.file) {
                    t.index = bt.index().clone();
                }
            }
        }
    }

    /// Builds the index of each changed file again, gives a new generation
    /// to each file for which `bump` is true, and refreshes the project if
    /// necessary. Returns the generations of the files that got a new
    /// generation.
    pub(crate) fn finish(
        mut self,
        project: &mut Project,
        bump: impl Fn(FileId) -> bool,
    ) -> Vec<FileGeneration> {
        let mut out = Vec::with_capacity(self.files.len());
        for t in &self.files {
            if let Some(bt) = project.file_mut(t.file) {
                bt.reindex();
                if bump(t.file) {
                    out.push(FileGeneration {
                        file: t.file,
                        before: t.generation,
                        after: bt.bump_generation(),
                    });
                }
            }
        }
        self.refresh_if_needed(project);
        out
    }

    /// Builds the index of each changed file again (after a partial
    /// rollback).
    pub(crate) fn reindex(&self, project: &mut Project) {
        for t in &self.files {
            if let Some(bt) = project.file_mut(t.file) {
                bt.reindex();
            }
        }
    }
}
