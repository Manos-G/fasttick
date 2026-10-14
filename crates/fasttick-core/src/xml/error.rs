//! Error types of the XML module.

use super::node::NodeId;

/// A parse error, with its position in the input.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
#[error("line {line}, column {column}: {message}")]
pub struct XmlError {
    /// The line number. The first line is 1.
    pub line: usize,
    /// The column number, in characters. The first column is 1. A UTF-8
    /// byte order mark does not count.
    pub column: usize,
    /// The byte offset in the input.
    pub offset: usize,
    /// A description of the problem.
    pub message: String,
}

/// An error from an editing function of [`Document`](super::Document).
///
/// An editing function that returns an error does not change the document.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum EditError {
    /// The node identifier is not in this document.
    #[error("node {0} does not exist in this document")]
    UnknownNode(NodeId),
    /// The operation needs an element.
    #[error("node {0} is not an element")]
    NotAnElement(NodeId),
    /// The operation needs a node that can have children (an element or
    /// the document node).
    #[error("node {0} cannot have children")]
    NotAContainer(NodeId),
    /// The operation is not possible for this kind of node.
    #[error("node {0} does not support this operation")]
    WrongKind(NodeId),
    /// The node must be detached before it can be inserted.
    #[error("node {0} already has a parent")]
    AlreadyAttached(NodeId),
    /// The node has no parent.
    #[error("node {0} has no parent")]
    NotAttached(NodeId),
    /// The document node cannot be moved, inserted or removed.
    #[error("the document node cannot be moved, inserted or removed")]
    DocumentNode,
    /// The operation would put a node inside itself.
    #[error("node {0} cannot be put inside itself or one of its descendants")]
    Cycle(NodeId),
    /// A child index is out of range.
    #[error("index {index} is out of range (the node has {len} children)")]
    IndexOutOfRange {
        /// The index that was given.
        index: usize,
        /// The number of children (or attributes).
        len: usize,
    },
    /// An element or attribute name is not valid.
    #[error("{0:?} is not a valid XML name")]
    InvalidName(String),
    /// An attribute with this name exists already.
    #[error("the element already has an attribute {0:?}")]
    DuplicateAttribute(String),
    /// The arena cannot hold more nodes (the limit is `u32::MAX`).
    #[error("the document has too many nodes")]
    TooManyNodes,
    /// A value cannot be written as XML (for example, it contains U+0000,
    /// or a comment contains `--`).
    #[error("invalid content: {0}")]
    InvalidContent(String),
}
