//! The document arena, queries and the writer.

use std::fmt;
use std::sync::Arc;

use super::node::{Attribute, Element, Node, NodeId, NodeKind, Slot};

/// The line end that a document uses.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default)]
pub enum LineEnding {
    /// LF (`\n`).
    #[default]
    Lf,
    /// CR LF (`\r\n`).
    CrLf,
}

impl LineEnding {
    /// Returns the line end text.
    pub fn as_str(self) -> &'static str {
        match self {
            LineEnding::Lf => "\n",
            LineEnding::CrLf => "\r\n",
        }
    }

    /// Finds the line end of the first line of `text`. The default is LF.
    pub(crate) fn detect(text: &str) -> LineEnding {
        match text.find('\n') {
            Some(i) if i > 0 && text.as_bytes()[i - 1] == b'\r' => LineEnding::CrLf,
            _ => LineEnding::Lf,
        }
    }
}

/// The default indentation unit: four spaces.
pub(crate) const DEFAULT_INDENT: &str = "    ";

/// A lossless XML document.
///
/// The document keeps all nodes in an arena. A [`NodeId`] identifies a
/// node. The node [`Document::root`] is the document node; its children are
/// the top-level items (XML declaration, comments, processing instructions,
/// DOCTYPE, white space and the document element).
///
/// When nothing changed, [`Document::to_string`](ToString::to_string) gives
/// the same bytes as the input of [`Document::parse`]. After a change, the
/// writer makes new text only for the changed parts.
///
/// Removed nodes stay in the arena as detached nodes. Their identifiers stay
/// valid, so an undo function can insert them again.
#[derive(Debug, Clone)]
pub struct Document {
    pub(crate) slots: Vec<Slot>,
    pub(crate) bom: bool,
    pub(crate) source: Arc<str>,
    pub(crate) line_starts: Arc<[usize]>,
    pub(crate) line_ending: LineEnding,
    pub(crate) indent_unit: String,
}

impl Default for Document {
    fn default() -> Self {
        Document::new()
    }
}

impl Document {
    /// Makes an empty document. It has only the document node.
    pub fn new() -> Document {
        Document::with_source("", false)
    }

    /// Makes a document with only the document node, for the given source.
    pub(crate) fn with_source(source: &str, bom: bool) -> Document {
        let mut line_starts = vec![0];
        line_starts.extend(
            source
                .bytes()
                .enumerate()
                .filter(|&(_, b)| b == b'\n')
                .map(|(i, _)| i + 1),
        );
        Document {
            slots: vec![Slot::new(Node::Document, None)],
            bom,
            source: Arc::from(source),
            line_starts: Arc::from(line_starts),
            line_ending: LineEnding::detect(source),
            indent_unit: DEFAULT_INDENT.to_owned(),
        }
    }

    pub(crate) fn slot(&self, id: NodeId) -> Option<&Slot> {
        self.slots.get(id.index())
    }

    pub(crate) fn slot_mut(&mut self, id: NodeId) -> Option<&mut Slot> {
        self.slots.get_mut(id.index())
    }

    /// Returns the document node. It is the parent of the top-level items.
    pub fn root(&self) -> NodeId {
        NodeId(0)
    }

    /// Returns the number of nodes in the arena, with detached nodes.
    pub fn node_count(&self) -> usize {
        self.slots.len()
    }

    /// Returns true if `id` is a node of the arena.
    pub fn contains(&self, id: NodeId) -> bool {
        id.index() < self.slots.len()
    }

    /// Returns the data of a node.
    pub fn node(&self, id: NodeId) -> Option<&Node> {
        self.slot(id).map(|s| &s.node)
    }

    /// Returns the kind of a node.
    pub fn kind(&self, id: NodeId) -> Option<NodeKind> {
        self.node(id).map(Node::kind)
    }

    /// Returns the element data of a node, if it is an element.
    pub fn element(&self, id: NodeId) -> Option<&Element> {
        self.node(id).and_then(Node::as_element)
    }

    /// Returns true if the node is an element.
    pub fn is_element(&self, id: NodeId) -> bool {
        self.element(id).is_some()
    }

    /// Returns true if the source started with a UTF-8 byte order mark.
    /// The writer then writes it again.
    pub fn has_bom(&self) -> bool {
        self.bom
    }

    /// Sets whether the writer writes a UTF-8 byte order mark.
    pub fn set_bom(&mut self, bom: bool) {
        self.bom = bom;
    }

    /// Returns the line end of the document: the line end of the first line
    /// of the source, or LF.
    pub fn line_ending(&self) -> LineEnding {
        self.line_ending
    }

    /// Sets the line end that new white space uses.
    pub fn set_line_ending(&mut self, line_ending: LineEnding) {
        self.line_ending = line_ending;
    }

    /// Returns the indentation unit that new white space uses. The parser
    /// finds it from the source; the default is four spaces.
    pub fn indent_unit(&self) -> &str {
        &self.indent_unit
    }

    /// Sets the indentation unit that new white space uses.
    pub fn set_indent_unit(&mut self, unit: impl Into<String>) {
        self.indent_unit = unit.into();
    }

    /// Returns the source text that the document was parsed from.
    pub fn source(&self) -> &str {
        &self.source
    }

    /// Returns the first element child of the document node.
    pub fn document_element(&self) -> Option<NodeId> {
        self.element_children(self.root()).next()
    }

    /// Returns the parent of a node. The document node and detached nodes
    /// have no parent.
    pub fn parent(&self, id: NodeId) -> Option<NodeId> {
        self.slot(id).and_then(|s| s.parent)
    }

    /// Returns the children of a node, in document order.
    pub fn children(&self, id: NodeId) -> &[NodeId] {
        self.slot(id).map(|s| s.children.as_slice()).unwrap_or(&[])
    }

    /// Returns the element children of a node, in document order.
    pub fn element_children(&self, id: NodeId) -> impl Iterator<Item = NodeId> + '_ {
        self.children(id)
            .iter()
            .copied()
            .filter(move |&c| self.is_element(c))
    }

    /// Returns the position of a node in the children of its parent.
    pub fn index_in_parent(&self, id: NodeId) -> Option<usize> {
        let parent = self.parent(id)?;
        self.children(parent).iter().position(|&c| c == id)
    }

    /// Returns the sibling after a node.
    pub fn next_sibling(&self, id: NodeId) -> Option<NodeId> {
        let parent = self.parent(id)?;
        let index = self.index_in_parent(id)?;
        self.children(parent).get(index + 1).copied()
    }

    /// Returns the sibling before a node.
    pub fn previous_sibling(&self, id: NodeId) -> Option<NodeId> {
        let parent = self.parent(id)?;
        let index = self.index_in_parent(id)?;
        index
            .checked_sub(1)
            .and_then(|i| self.children(parent).get(i).copied())
    }

    /// Returns the ancestors of a node, from its parent to the document node
    /// (or to the top of a detached subtree).
    pub fn ancestors(&self, id: NodeId) -> impl Iterator<Item = NodeId> + '_ {
        std::iter::successors(self.parent(id), move |&p| self.parent(p))
    }

    /// Returns true if the node is in the tree of the document node (it is
    /// not detached, and no ancestor is detached).
    pub fn is_attached(&self, id: NodeId) -> bool {
        id == self.root() || self.ancestors(id).any(|a| a == self.root())
    }

    /// Returns the element name, or `None` if the node is not an element.
    pub fn name(&self, id: NodeId) -> Option<&str> {
        self.element(id).map(Element::name)
    }

    /// Returns the decoded value of an attribute.
    pub fn attribute(&self, id: NodeId, name: &str) -> Option<&str> {
        self.element(id).and_then(|e| e.attribute(name))
    }

    /// Returns the attributes of an element (empty for other nodes).
    pub fn attributes(&self, id: NodeId) -> &[Attribute] {
        self.element(id).map(Element::attributes).unwrap_or(&[])
    }

    /// Returns the node and all its descendants, in pre-order (document
    /// order). The first item is `id` itself.
    pub fn descendants(&self, id: NodeId) -> Descendants<'_> {
        Descendants {
            doc: self,
            stack: if self.contains(id) { vec![id] } else { Vec::new() },
        }
    }

    /// Returns the decoded text of all text and CDATA nodes in the subtree,
    /// in document order.
    pub fn text_content(&self, id: NodeId) -> String {
        let mut out = String::new();
        for d in self.descendants(id) {
            match self.node(d) {
                Some(Node::Text(t)) => out.push_str(&t.decoded()),
                Some(Node::CData(c)) => out.push_str(&c.decoded()),
                _ => {}
            }
        }
        out
    }

    /// Returns the byte offset of the node in the source, or `None` for a
    /// node that a program made (also a copy or an imported node).
    pub fn source_offset(&self, id: NodeId) -> Option<usize> {
        self.slot(id).and_then(|s| s.offset)
    }

    /// Returns the line and the column of a byte offset in the source. Both
    /// start at 1. The column counts characters; a UTF-8 byte order mark
    /// does not count.
    pub fn line_col(&self, offset: usize) -> (usize, usize) {
        line_col_in(&self.source, &self.line_starts, offset)
    }

    /// Returns the line and the column of a node in the source.
    pub fn node_line_col(&self, id: NodeId) -> Option<(usize, usize)> {
        self.source_offset(id).map(|o| self.line_col(o))
    }

    /// Writes the document. When nothing changed, the result is equal to
    /// the parsed input, byte for byte.
    pub fn write<W: fmt::Write>(&self, out: &mut W) -> fmt::Result {
        let mut s = String::with_capacity(self.source.len() + 64);
        if self.bom {
            s.push('\u{feff}');
        }
        self.write_subtree(self.root(), false, &mut s);
        out.write_str(&s)
    }

    /// Writes one node and its subtree, for example for the clipboard.
    /// The result is empty for an unknown node. For the document node, the
    /// result is the full document.
    pub fn write_node(&self, id: NodeId) -> String {
        if id == self.root() {
            return self.to_string();
        }
        let mut s = String::new();
        self.write_subtree(id, false, &mut s);
        s
    }

    /// Writes the document, but makes each start tag again from its parts
    /// (name, attributes and white space) instead of the source text.
    ///
    /// The result is the same as [`Document::write`]. Tests use this function
    /// to make sure that the parts of each start tag hold all of its text.
    pub fn write_regenerated(&self) -> String {
        let mut s = String::new();
        if self.bom {
            s.push('\u{feff}');
        }
        self.write_subtree(self.root(), true, &mut s);
        s
    }

    /// Writes a subtree without recursion, so that deep trees cannot
    /// overflow the stack.
    fn write_subtree(&self, id: NodeId, regenerate: bool, out: &mut String) {
        enum Step {
            Enter(NodeId),
            Leave(NodeId),
        }
        let mut stack = vec![Step::Enter(id)];
        while let Some(step) = stack.pop() {
            match step {
                Step::Enter(n) => {
                    let Some(slot) = self.slot(n) else { continue };
                    match &slot.node {
                        Node::Document => {
                            stack.extend(slot.children.iter().rev().map(|&c| Step::Enter(c)));
                        }
                        Node::Element(e) => {
                            let has_children = !slot.children.is_empty();
                            if regenerate {
                                e.generate_start(e.self_closing && !has_children, out);
                            } else {
                                e.write_start(has_children, out);
                            }
                            if !(e.self_closing && !has_children) {
                                stack.push(Step::Leave(n));
                                stack.extend(slot.children.iter().rev().map(|&c| Step::Enter(c)));
                            }
                        }
                        other => out.push_str(other.raw().unwrap_or("")),
                    }
                }
                Step::Leave(n) => {
                    if let Some(e) = self.element(n) {
                        e.write_end(out);
                    }
                }
            }
        }
    }
}

impl fmt::Display for Document {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        self.write(f)
    }
}

/// Computes the line and the column of `offset` in `source`.
pub(crate) fn line_col_in(source: &str, line_starts: &[usize], offset: usize) -> (usize, usize) {
    let mut offset = offset.min(source.len());
    while !source.is_char_boundary(offset) {
        offset -= 1;
    }
    let line = match line_starts.binary_search(&offset) {
        Ok(i) => i,
        Err(i) => i.saturating_sub(1),
    };
    let start = line_starts.get(line).copied().unwrap_or(0).min(offset);
    let mut column = source[start..offset].chars().count() + 1;
    if start == 0 && source.starts_with('\u{feff}') && offset > 0 {
        column -= 1;
    }
    (line + 1, column)
}

/// Iterator over a subtree in pre-order. See [`Document::descendants`].
#[derive(Debug, Clone)]
pub struct Descendants<'a> {
    doc: &'a Document,
    stack: Vec<NodeId>,
}

impl Iterator for Descendants<'_> {
    type Item = NodeId;

    fn next(&mut self) -> Option<NodeId> {
        let id = self.stack.pop()?;
        self.stack
            .extend(self.doc.children(id).iter().rev().copied());
        Some(id)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn line_col_basic() {
        let src = "ab\ncd\r\nef";
        let doc = Document::with_source(src, false);
        assert_eq!(doc.line_col(0), (1, 1));
        assert_eq!(doc.line_col(1), (1, 2));
        assert_eq!(doc.line_col(3), (2, 1));
        assert_eq!(doc.line_col(7), (3, 1));
        assert_eq!(doc.line_col(100), (3, 3));
    }

    #[test]
    fn line_col_counts_characters_and_skips_bom() {
        let src = "\u{feff}ä<a/>";
        let doc = Document::with_source(src, true);
        assert_eq!(doc.line_col(3), (1, 1));
        assert_eq!(doc.line_col(5), (1, 2));
        // Inside a multi-byte character.
        assert_eq!(doc.line_col(4), (1, 1));
    }

    #[test]
    fn line_ending_detection() {
        assert_eq!(LineEnding::detect("a\r\nb"), LineEnding::CrLf);
        assert_eq!(LineEnding::detect("a\nb\r\n"), LineEnding::Lf);
        assert_eq!(LineEnding::detect("ab"), LineEnding::Lf);
        assert_eq!(LineEnding::detect("\nb"), LineEnding::Lf);
    }

    #[test]
    fn empty_document() {
        let doc = Document::new();
        assert_eq!(doc.to_string(), "");
        assert_eq!(doc.document_element(), None);
        assert_eq!(doc.descendants(doc.root()).count(), 1);
        assert!(doc.children(NodeId(99)).is_empty());
        assert_eq!(doc.descendants(NodeId(99)).count(), 0);
    }
}
