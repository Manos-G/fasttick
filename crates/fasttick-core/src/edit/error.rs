//! The error type of the edit module.

use std::fmt;

use crate::bt::names::NameError;
use crate::builtin::ChildRule;
use crate::models::ModelWriteError;
use crate::project::FileId;
use crate::xml::{self, NodeId, XmlError};

/// The kind of a name that a command checks.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum NameKind {
    /// A `<BehaviorTree>` ID.
    TreeId,
    /// A node type ID (a model ID).
    TypeId,
    /// An instance name (the `name` attribute).
    InstanceName,
    /// A port name.
    PortName,
}

impl fmt::Display for NameKind {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(match self {
            NameKind::TreeId => "tree ID",
            NameKind::TypeId => "node type ID",
            NameKind::InstanceName => "instance name",
            NameKind::PortName => "port name",
        })
    }
}

/// An error from an edit command or from a transaction.
///
/// A command that returns an error does not change the project: the
/// command checks its input before it changes anything, and a change that
/// fails later is rolled back.
#[derive(Debug, Clone, PartialEq, thiserror::Error)]
pub enum CommandError {
    /// The file is not in the project.
    #[error("{0} is not in the project")]
    UnknownFile(FileId),
    /// The node is not an element of the file.
    #[error("node {node} is not an element of {file}")]
    UnknownNode {
        /// The file.
        file: FileId,
        /// The node.
        node: NodeId,
    },
    /// The node is not a node of a tree (below a `<BehaviorTree>`) and not
    /// a node of the unconnected area.
    #[error("node {0} is not a node of a tree or of the unconnected area")]
    NotATreeNode(NodeId),
    /// The node cannot be the parent of tree nodes.
    #[error("node {0} cannot be the parent of tree nodes")]
    NotAParent(NodeId),
    /// The parent cannot take one more child (see
    /// [`StructurePolicy`](super::StructurePolicy)).
    #[error("\"{parent}\" cannot take more children: it allows {rule}")]
    ChildLimit {
        /// The type ID of the parent (or `BehaviorTree`).
        parent: String,
        /// The child rule of the parent.
        rule: ChildRule,
    },
    /// The new type of a node does not allow its current children.
    #[error("the type \"{type_id}\" allows {rule}, but the node has {count}")]
    TooManyChildren {
        /// The new type ID.
        type_id: String,
        /// The child rule of the new type.
        rule: ChildRule,
        /// The number of children of the node.
        count: usize,
    },
    /// A node cannot go into its own subtree.
    #[error("node {0} cannot be put into itself or into one of its descendants")]
    IntoOwnSubtree(NodeId),
    /// The selection is empty.
    #[error("no node is selected")]
    EmptySelection,
    /// The node has no parent (it is an unconnected node).
    #[error("node {0} has no parent; connect it first")]
    NoParent(NodeId),
    /// The selected nodes do not have the same parent.
    #[error("the selected nodes must have the same parent")]
    NotSiblings,
    /// The selected nodes are not next to each other.
    #[error("the selected nodes must be next to each other")]
    NotContiguous,
    /// A name is not valid in BT.CPP.
    #[error("the {kind} \"{name}\" is not valid: {error}")]
    InvalidName {
        /// The kind of name.
        kind: NameKind,
        /// The name.
        name: String,
        /// The reason.
        error: NameError,
    },
    /// An attribute name is not a valid XML name.
    #[error("\"{0}\" is not a valid XML attribute name")]
    InvalidAttributeName(String),
    /// The attribute name is not a port name that BT.CPP reads.
    #[error(
        "\"{0}\" is not a port name: BT.CPP reads only names that start with a letter and are not reserved"
    )]
    NotAPortName(String),
    /// A tree with this ID exists in the project.
    #[error("a tree with the ID \"{0}\" exists already")]
    TreeExists(String),
    /// No tree with this ID is visible.
    #[error("no tree with the ID \"{0}\" is visible from this file")]
    TreeNotFound(String),
    /// A node model with this ID exists.
    #[error("a node model with the ID \"{0}\" exists already")]
    ModelExists(String),
    /// The file has no root element.
    #[error("the file has no root element")]
    NoRootElement,
    /// The file is a model file (its root element is `<TreeNodesModel>`).
    #[error("the file is a model file; it cannot hold trees")]
    NotATreeFile,
    /// The node is not an unconnected node.
    #[error("node {0} is not an unconnected node")]
    NotUnconnected(NodeId),
    /// The node is an unconnected node already.
    #[error("node {0} is an unconnected node already")]
    AlreadyUnconnected(NodeId),
    /// A `<SubTree>` node needs a target tree ID.
    #[error("a SubTree node needs the ID of a tree; add the tree from the tree list")]
    SubTreeNeedsTarget,
    /// The type ID cannot be an element name, and the kind is not known,
    /// so the explicit form cannot be used.
    #[error("the type ID \"{0}\" cannot be an XML element name, and its node kind is not known")]
    TypeNeedsKind(String),
    /// The clipboard text is not an XML fragment with elements.
    #[error("the clipboard text cannot be pasted: {0}")]
    Clipboard(XmlError),
    /// The clipboard text has no element.
    #[error("the clipboard text contains no tree node")]
    NothingToPaste,
    /// The clipboard text has an element that is not a tree node.
    #[error("<{0}> is not a tree node and cannot be pasted into a tree")]
    NotPastable(String),
    /// Blackboard keys that a SubTree cannot remap.
    #[error(
        "these blackboard keys cannot be remapped to a SubTree, because they are not valid port names: {}",
        .0.join(", ")
    )]
    KeysNotRemappable(Vec<String>),
    /// A canvas position is not a finite number.
    #[error("the position [{}, {}] is not valid", .0[0], .0[1])]
    InvalidPosition([f32; 2]),
    /// A model has two ports with the same name.
    #[error("the model has two ports with the name \"{0}\"")]
    DuplicatePort(String),
    /// The file changed outside the edit history (for example, it was
    /// loaded again from the disk), so an undo or a redo step does not fit
    /// it any more.
    #[error("{0} changed outside the edit history")]
    FileChanged(FileId),
    /// An unconnected-node edit does not match the project state.
    #[error("the unconnected nodes of tree \"{tree}\" in {file} do not match the edit")]
    UnconnectedMismatch {
        /// The file.
        file: FileId,
        /// The tree ID.
        tree: String,
    },
    /// The node is not a node of this tree.
    #[error("node {node} is not a node of the tree \"{tree}\"")]
    NotInTree {
        /// The node.
        node: NodeId,
        /// The tree ID.
        tree: String,
    },
    /// The tree has no group with this ID.
    #[error("the tree \"{tree}\" has no group {id}")]
    UnknownGroup {
        /// The tree ID.
        tree: String,
        /// The group ID.
        id: u32,
    },
    /// The tree has no canvas note with this ID.
    #[error("the tree \"{tree}\" has no note {id}")]
    UnknownNote {
        /// The tree ID.
        tree: String,
        /// The note ID.
        id: u32,
    },
    /// A group needs at least one node.
    #[error("a group needs at least one node")]
    EmptyGroup,
    /// A change of the model section failed.
    #[error(transparent)]
    Model(#[from] ModelWriteError),
    /// A change of the XML document failed.
    #[error(transparent)]
    Xml(#[from] xml::EditError),
}

impl CommandError {
    /// Makes an [`CommandError::InvalidName`] error.
    pub(crate) fn name(kind: NameKind, name: &str, error: NameError) -> CommandError {
        CommandError::InvalidName {
            kind,
            name: name.to_owned(),
            error,
        }
    }
}
