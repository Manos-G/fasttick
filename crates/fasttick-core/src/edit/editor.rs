//! The editor: one transaction that is being built.

use crate::bt::NodeView;
use crate::builtin::{self, ChildRule};
use crate::project::{FileId, Project};
use crate::xml::{Change, Document, NodeId};

use super::error::CommandError;
use super::transaction::{Edit, FileGeneration, FileTracker, Transaction};

/// How the commands treat the child rules of BT.CPP
/// ([`crate::builtin::ChildRule`]).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default)]
pub enum StructurePolicy {
    /// Refuse a command that gives a parent more children than its rule
    /// allows (for example a second child for a decorator, or a child for
    /// an action). This is the default.
    #[default]
    Enforce,
    /// Allow such commands. The validator then reports the problem
    /// (`ChildCount`). The GUI uses this policy when the user wants to
    /// change the structure in several steps.
    AllowInvalid,
}

/// The result of a successful editor session: the transaction that undoes
/// it and the generations of the changed files.
#[derive(Debug, Clone, PartialEq)]
pub struct Applied {
    /// The transaction that undoes the session. Its label is the label of
    /// the session.
    pub undo: Transaction,
    /// The generations of the XML files that the session changed.
    pub generations: Vec<FileGeneration>,
}

impl Applied {
    /// True if the session changed nothing.
    pub fn is_empty(&self) -> bool {
        self.undo.is_empty()
    }

    /// The label of the session.
    pub fn label(&self) -> &str {
        &self.undo.label
    }
}

/// An editor session: it applies commands to a project and records the
/// edits that undo them.
///
/// Get an editor from [`run`] or from
/// [`History::run`](super::History::run). The commands (for example
/// [`Editor::add_node`]) change the project at once, so a later command in
/// the same session sees the result of an earlier one. If the session
/// fails, all its changes are undone; if it succeeds, it becomes one undo
/// step.
#[derive(Debug)]
pub struct Editor<'p> {
    project: &'p mut Project,
    label: Option<String>,
    /// The undo edits, in the reverse of the order in which to apply them.
    undo: Vec<Edit>,
    tracker: FileTracker,
    policy: StructurePolicy,
}

/// Runs an editor session on a project.
///
/// The function calls `f` with an [`Editor`]. If `f` returns `Ok`, the
/// changes stay, and the function returns the value of `f` and the
/// [`Applied`] data (the undo transaction). If `f` returns an error, the
/// function undoes all changes of the session and returns the error.
///
/// Use [`History::run`](super::History::run) to record the session for
/// undo and redo.
pub fn run<T>(
    project: &mut Project,
    policy: StructurePolicy,
    f: impl FnOnce(&mut Editor<'_>) -> Result<T, CommandError>,
) -> Result<(T, Applied), CommandError> {
    let mut editor = Editor::new(project, policy);
    match f(&mut editor) {
        Ok(value) => Ok((value, editor.commit())),
        Err(error) => {
            editor.rollback();
            Err(error)
        }
    }
}

impl<'p> Editor<'p> {
    pub(crate) fn new(project: &'p mut Project, policy: StructurePolicy) -> Editor<'p> {
        Editor {
            project,
            label: None,
            undo: Vec::new(),
            tracker: FileTracker::default(),
            policy,
        }
    }

    /// The project, with all changes of the session so far.
    pub fn project(&self) -> &Project {
        self.project
    }

    /// The label of the session. The first command sets it if the caller
    /// did not set it.
    pub fn label(&self) -> Option<&str> {
        self.label.as_deref()
    }

    /// Sets the label of the session, for example "Paste".
    pub fn set_label(&mut self, label: impl Into<String>) {
        self.label = Some(label.into());
    }

    /// Sets the label if no label is set.
    pub(crate) fn default_label(&mut self, label: &str) {
        if self.label.is_none() {
            self.label = Some(label.to_owned());
        }
    }

    /// The structure policy of the session.
    pub fn structure_policy(&self) -> StructurePolicy {
        self.policy
    }

    /// Sets the structure policy for the next commands of the session.
    pub fn set_structure_policy(&mut self, policy: StructurePolicy) {
        self.policy = policy;
    }

    /// True if the session changed nothing so far.
    pub fn is_empty(&self) -> bool {
        self.undo.is_empty()
    }

    /// Applies one edit and records its inverse.
    pub fn apply(&mut self, edit: Edit) -> Result<(), CommandError> {
        if let Edit::Xml { file, .. } = &edit {
            self.tracker.touch(self.project, *file)?;
        }
        let xml_file = match &edit {
            Edit::Xml { file, .. } => Some(*file),
            _ => None,
        };
        let inverse = edit.apply_raw(self.project)?;
        if let Some(file) = xml_file
            && let Some(bt) = self.project.file_mut(file)
        {
            bt.reindex();
        }
        self.undo.push(inverse);
        Ok(())
    }

    /// Applies one primitive XML change and records its inverse.
    pub fn apply_change(&mut self, file: FileId, change: Change) -> Result<(), CommandError> {
        self.apply(Edit::Xml { file, change })
    }

    /// Runs a function that changes the document of a file and returns the
    /// changes that undo it (in the order in which to apply them, as
    /// [`Document::apply_all`] takes them). The function must leave the
    /// document unchanged when it returns an error.
    pub(crate) fn xml<R>(
        &mut self,
        file: FileId,
        f: impl FnOnce(&mut Document) -> Result<(R, Vec<Change>), CommandError>,
    ) -> Result<R, CommandError> {
        self.tracker.touch(self.project, file)?;
        let bt = self
            .project
            .file_mut(file)
            .ok_or(CommandError::UnknownFile(file))?;
        let (value, undo) = f(bt.doc_mut())?;
        bt.reindex();
        self.undo.extend(
            undo.into_iter()
                .rev()
                .map(|change| Edit::Xml { file, change }),
        );
        Ok(value)
    }

    /// Refreshes the project if a change of the session changed the trees,
    /// the includes or the models of a file. Call it before a look-up in
    /// the model registry.
    pub(crate) fn sync(&mut self) {
        self.tracker.refresh_if_needed(self.project);
    }

    /// The project for a look-up in the model registry: refreshed first if
    /// necessary.
    pub(crate) fn synced(&mut self) -> &Project {
        self.sync();
        self.project
    }

    /// Ends the session and keeps the changes.
    pub(crate) fn commit(self) -> Applied {
        let Editor {
            project,
            label,
            mut undo,
            tracker,
            ..
        } = self;
        // Only the files with XML edits get a new generation.
        let generations = tracker.finish(project, |file| {
            undo.iter()
                .any(|e| matches!(e, Edit::Xml { file: f, .. } if *f == file))
        });
        undo.reverse();
        Applied {
            undo: Transaction {
                label: label.unwrap_or_default(),
                edits: undo,
            },
            generations,
        }
    }

    /// Ends the session and undoes all its changes.
    pub(crate) fn rollback(self) {
        let Editor {
            project,
            undo,
            tracker,
            ..
        } = self;
        for edit in undo.into_iter().rev() {
            // The inverse of an edit that just succeeded is valid.
            let _ = edit.apply_raw(project);
        }
        tracker.finish(project, |_| false);
    }

    /// Runs a command atomically: if it fails, the function undoes the
    /// changes that the command made, and the label stays as before.
    pub(crate) fn atomic<T>(
        &mut self,
        label: &str,
        f: impl FnOnce(&mut Self) -> Result<T, CommandError>,
    ) -> Result<T, CommandError> {
        let mark = self.undo.len();
        let old_label = self.label.clone();
        self.default_label(label);
        match f(self) {
            Ok(value) => Ok(value),
            Err(error) => {
                while self.undo.len() > mark {
                    if let Some(edit) = self.undo.pop() {
                        // The inverse of an edit that just succeeded is valid.
                        let _ = edit.apply_raw(self.project);
                    }
                }
                self.tracker.reindex(self.project);
                self.label = old_label;
                Err(error)
            }
        }
    }

    /// Runs a function that changes only detached nodes of a file (for
    /// example a new element and its attributes). Such changes are not
    /// recorded: undo removes the new node from the tree, and redo inserts
    /// the same node again.
    pub(crate) fn alloc<R>(
        &mut self,
        file: FileId,
        f: impl FnOnce(&mut Document) -> Result<R, CommandError>,
    ) -> Result<R, CommandError> {
        let bt = self
            .project
            .file_mut(file)
            .ok_or(CommandError::UnknownFile(file))?;
        f(bt.doc_mut())
    }

    // ----- Checks ---------------------------------------------------------

    /// The view of an element of a file.
    pub(crate) fn view(&self, file: FileId, node: NodeId) -> Result<NodeView<'_>, CommandError> {
        element_view(self.project, file, node)
    }

    /// Checks that `parent` can take `adding` more children (see
    /// [`StructurePolicy`]). `current` is the number of element children
    /// that count.
    pub(crate) fn check_capacity(
        &mut self,
        file: FileId,
        parent: NodeId,
        current: usize,
        adding: usize,
    ) -> Result<(), CommandError> {
        if self.policy == StructurePolicy::AllowInvalid || adding == 0 {
            return Ok(());
        }
        let project = self.synced();
        let (label, rule) = parent_rule(project, file, parent)?;
        if let Some(rule) = rule
            && let Some(max) = rule.max()
            && current + adding > max
        {
            return Err(CommandError::ChildLimit {
                parent: label,
                rule,
            });
        }
        Ok(())
    }
}

/// The view of an element of a file of a project.
pub(crate) fn element_view(
    project: &Project,
    file: FileId,
    node: NodeId,
) -> Result<NodeView<'_>, CommandError> {
    let bt = project.file(file).ok_or(CommandError::UnknownFile(file))?;
    bt.node(node)
        .ok_or(CommandError::UnknownNode { file, node })
}

/// Where a tree node is.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub enum NodePlace {
    /// The node is below the `<BehaviorTree>` element of a tree.
    Tree {
        /// The index of the tree in [`crate::bt::BtIndex::trees`].
        tree: usize,
        /// The tree ID.
        tree_id: String,
    },
    /// The node is an unconnected node, or a node below one.
    Unconnected {
        /// The unconnected node (the top of the detached subtree).
        root: NodeId,
        /// The tree of the unconnected node.
        tree_id: String,
    },
}

impl NodePlace {
    /// The tree ID.
    pub fn tree_id(&self) -> &str {
        match self {
            NodePlace::Tree { tree_id, .. } | NodePlace::Unconnected { tree_id, .. } => tree_id,
        }
    }
}

/// Finds where a tree node is: in a tree or in the unconnected area.
///
/// Returns [`CommandError::NotATreeNode`] for other elements: the root
/// element, a `<BehaviorTree>` element, model elements, and detached
/// elements that are not unconnected nodes.
pub fn node_place(
    project: &Project,
    file: FileId,
    node: NodeId,
) -> Result<NodePlace, CommandError> {
    let bt = project.file(file).ok_or(CommandError::UnknownFile(file))?;
    let doc = bt.doc();
    if !doc.is_element(node) {
        return Err(CommandError::UnknownNode { file, node });
    }
    let index = bt.index();
    let mut current = node;
    loop {
        let Some(parent) = doc.parent(current) else {
            // A detached subtree: its top must be an unconnected node.
            return project
                .all_unconnected()
                .find(|(f, _, n)| *f == file && n.node == current)
                .map(|(_, tree, _)| NodePlace::Unconnected {
                    root: current,
                    tree_id: tree.to_owned(),
                })
                .ok_or(CommandError::NotATreeNode(node));
        };
        if Some(parent) == index.root {
            // `current` is a child of the root element.
            return match index.tree_index_of_element(current) {
                Some(tree) if current != node => Ok(NodePlace::Tree {
                    tree,
                    tree_id: index.trees[tree].id.clone(),
                }),
                _ => Err(CommandError::NotATreeNode(node)),
            };
        }
        if parent == doc.root() {
            return Err(CommandError::NotATreeNode(node));
        }
        current = parent;
    }
}

/// Where a new child can go.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum ParentPlace {
    /// The `<BehaviorTree>` element of a tree: the child becomes the root
    /// node.
    TreeElement,
    /// A tree node.
    Node(NodePlace),
}

/// Checks that `parent` can be the parent of tree nodes: a `<BehaviorTree>`
/// element of the file or a tree node.
pub(crate) fn parent_place(
    project: &Project,
    file: FileId,
    parent: NodeId,
) -> Result<ParentPlace, CommandError> {
    let bt = project.file(file).ok_or(CommandError::UnknownFile(file))?;
    if !bt.doc().is_element(parent) {
        return Err(CommandError::UnknownNode { file, node: parent });
    }
    if bt.index().tree_index_of_element(parent).is_some() && bt.doc().is_attached(parent) {
        return Ok(ParentPlace::TreeElement);
    }
    match node_place(project, file, parent) {
        Ok(place) => Ok(ParentPlace::Node(place)),
        Err(CommandError::NotATreeNode(_)) => Err(CommandError::NotAParent(parent)),
        Err(e) => Err(e),
    }
}

/// The label and the child rule of a parent: `BehaviorTree` and exactly one
/// child for a `<BehaviorTree>` element (`VerifyXML` in
/// `src/xml_parsing.cpp`), else the type ID and the rule of the node type
/// ([`builtin::child_rule`]). The rule is `None` if the kind of the node is
/// not known.
///
/// The GUI can use it to show whether a drop target takes one more child
/// ([`ChildRule::accepts_more`]) before it runs a command.
pub fn parent_rule(
    project: &Project,
    file: FileId,
    parent: NodeId,
) -> Result<(String, Option<ChildRule>), CommandError> {
    let bt = project.file(file).ok_or(CommandError::UnknownFile(file))?;
    if bt.index().tree_index_of_element(parent).is_some() {
        return Ok(("BehaviorTree".to_owned(), Some(ChildRule::Exactly(1))));
    }
    let view = element_view(project, file, parent)?;
    let kind = project.registry().node_kind(file, &view);
    // A SubTree uses the rule of the registration ID "SubTree", not of
    // the tree ID.
    let id = view.registration_id().unwrap_or(view.tag());
    Ok((id.to_owned(), builtin::child_rule(id, kind)))
}

/// Removes duplicates and the nodes that have a selected ancestor, and
/// sorts the nodes in document order (attached nodes first, then each
/// detached subtree).
pub fn top_most(doc: &Document, nodes: &[NodeId]) -> Vec<NodeId> {
    /// The sort key: detached subtrees after the document, then the top
    /// node of the subtree, then the child positions from the top.
    type OrderKey = (bool, NodeId, Vec<usize>);
    let mut keys: Vec<(OrderKey, NodeId)> = Vec::new();
    for &node in nodes {
        if !doc.contains(node) || keys.iter().any(|(_, n)| *n == node) {
            continue;
        }
        if doc.ancestors(node).any(|a| nodes.contains(&a)) {
            continue;
        }
        let mut path = Vec::new();
        let mut current = node;
        while let Some(parent) = doc.parent(current) {
            path.push(doc.index_in_parent(current).unwrap_or(0));
            current = parent;
        }
        path.reverse();
        keys.push(((current != doc.root(), current, path), node));
    }
    keys.sort();
    keys.into_iter().map(|(_, n)| n).collect()
}

/// The position of an element among the element children of its parent.
pub(crate) fn element_index(doc: &Document, node: NodeId) -> Option<usize> {
    let parent = doc.parent(node)?;
    doc.element_children(parent).position(|c| c == node)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn top_most_sorts_and_filters() {
        let mut doc = Document::parse("<r><a><b/><c/></a><d/></r>").unwrap();
        let find = |doc: &Document, name: &str| {
            doc.descendants(doc.root())
                .find(|&n| doc.name(n) == Some(name))
                .unwrap()
        };
        let (a, b, c, d) = (
            find(&doc, "a"),
            find(&doc, "b"),
            find(&doc, "c"),
            find(&doc, "d"),
        );
        assert_eq!(top_most(&doc, &[d, c, b, c]), vec![b, c, d]);
        assert_eq!(top_most(&doc, &[c, a, d]), vec![a, d]);
        // Detached subtrees come after the document, by their top node.
        let x = doc.create_element("x").unwrap();
        let y = doc.create_element("y").unwrap();
        let z = doc.create_element("z").unwrap();
        doc.append_child(y, z).unwrap();
        assert_eq!(top_most(&doc, &[z, x, d]), vec![d, x, z]);
        assert_eq!(top_most(&doc, &[z, y]), vec![y]);
        assert_eq!(top_most(&doc, &[NodeId(999)]), vec![]);
        assert_eq!(element_index(&doc, c), Some(1));
        assert_eq!(element_index(&doc, x), None);
    }
}
