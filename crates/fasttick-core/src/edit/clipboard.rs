//! Copy, paste and duplicate.

use std::collections::BTreeMap;

use crate::project::{FileId, Project, UnconnectedNode};
use crate::xml::{Document, NodeId};

use super::editor::{Editor, NodePlace, element_index, node_place, top_most};
use super::error::CommandError;
use super::nodes::STRUCTURE_TAGS;
use super::transaction::Edit;

/// The distance between a duplicated or pasted unconnected node and the
/// node before it, in canvas units.
pub const DUPLICATE_OFFSET: [f32; 2] = [24.0, 24.0];

fn offset(position: [f32; 2], steps: usize) -> [f32; 2] {
    let n = steps as f32;
    [
        position[0] + DUPLICATE_OFFSET[0] * n,
        position[1] + DUPLICATE_OFFSET[1] * n,
    ]
}

/// Writes tree nodes as an XML fragment for the clipboard.
///
/// The fragment has one top-level element for each selected node, in
/// document order, on separate lines. A node whose ancestor is also
/// selected is written only once, inside its ancestor. The white space
/// inside each node is indented as if the node had no indentation, with
/// the indentation unit and the line end of the file. The attributes keep
/// their text.
pub fn copy(project: &Project, file: FileId, nodes: &[NodeId]) -> Result<String, CommandError> {
    let bt = project.file(file).ok_or(CommandError::UnknownFile(file))?;
    if nodes.is_empty() {
        return Err(CommandError::EmptySelection);
    }
    for &node in nodes {
        node_place(project, file, node)?;
    }
    let source = bt.doc();
    let mut out = Document::new();
    out.set_line_ending(source.line_ending());
    out.set_indent_unit(source.indent_unit());
    for (i, node) in top_most(source, nodes).into_iter().enumerate() {
        let copy = out.import_subtree(source, node)?;
        let root = out.root();
        // At the top level, the indentation is "": the white space in the
        // copy is indented from column 1.
        out.insert_element_pretty(root, i, copy)?;
        if i > 0 {
            let line = out.create_text(source.line_ending().as_str())?;
            let at = out.children(root).len() - 1;
            out.insert_child(root, at, line)?;
        }
    }
    Ok(out.to_string())
}

impl Editor<'_> {
    /// Parses a clipboard text into detached elements of a file.
    fn parse_clipboard(&mut self, file: FileId, text: &str) -> Result<Vec<NodeId>, CommandError> {
        self.alloc(file, |doc| {
            let items = doc.parse_fragment(text).map_err(CommandError::Clipboard)?;
            let elements: Vec<NodeId> = items.into_iter().filter(|&n| doc.is_element(n)).collect();
            if elements.is_empty() {
                return Err(CommandError::NothingToPaste);
            }
            for &e in &elements {
                let name = doc.name(e).unwrap_or_default();
                if STRUCTURE_TAGS.contains(&name) {
                    return Err(CommandError::NotPastable(name.to_owned()));
                }
            }
            Ok(elements)
        })
    }

    /// Pastes the elements of a clipboard text (see [`copy`]) as element
    /// children `index`, `index + 1`, ... of `parent`, and returns the new
    /// nodes.
    ///
    /// Comments between the elements are dropped. The text cannot contain
    /// `<root>`, `<BehaviorTree>`, `<TreeNodesModel>` or `<include>`
    /// elements. The new nodes get the indentation of their siblings.
    pub fn paste(
        &mut self,
        file: FileId,
        text: &str,
        parent: NodeId,
        index: usize,
    ) -> Result<Vec<NodeId>, CommandError> {
        self.atomic("Paste", |ed| {
            let (_, count) = ed.insertion_point(file, parent, index)?;
            let elements = ed.parse_clipboard(file, text)?;
            ed.check_capacity(file, parent, count, elements.len())?;
            for (i, &element) in elements.iter().enumerate() {
                ed.xml(file, |doc| {
                    Ok(((), doc.insert_element_pretty(parent, index + i, element)?))
                })?;
            }
            Ok(elements)
        })
    }

    /// Pastes the elements of a clipboard text as unconnected nodes of a
    /// tree. The first node goes to `position`; each next node goes
    /// [`DUPLICATE_OFFSET`] further.
    pub fn paste_unconnected(
        &mut self,
        file: FileId,
        tree: &str,
        text: &str,
        position: [f32; 2],
    ) -> Result<Vec<NodeId>, CommandError> {
        self.atomic("Paste", |ed| {
            ed.check_tree_exists(file, tree)?;
            let elements = ed.parse_clipboard(file, text)?;
            for (i, &node) in elements.iter().enumerate() {
                ed.push_unconnected(file, tree, node, offset(position, i))?;
            }
            Ok(elements)
        })
    }

    /// Duplicates tree nodes with their subtrees and returns the copies, in
    /// document order.
    ///
    /// Each copy goes directly after its original. A copy of an unconnected
    /// node becomes an unconnected node [`DUPLICATE_OFFSET`] away from the
    /// original. A node whose ancestor is also selected is copied only with
    /// the ancestor. The copies keep the text of the originals.
    pub fn duplicate(
        &mut self,
        file: FileId,
        nodes: &[NodeId],
    ) -> Result<Vec<NodeId>, CommandError> {
        self.atomic("Duplicate", |ed| {
            if nodes.is_empty() {
                return Err(CommandError::EmptySelection);
            }
            for &node in nodes {
                node_place(ed.project(), file, node)?;
            }
            let tops = top_most(ed.doc(file)?, nodes);
            // The capacity check: the number of copies for each parent.
            let mut per_parent: BTreeMap<NodeId, usize> = BTreeMap::new();
            for &node in &tops {
                if let Some(parent) = ed.doc(file)?.parent(node) {
                    *per_parent.entry(parent).or_default() += 1;
                }
            }
            for (parent, adding) in per_parent {
                let count = ed.element_count(file, parent);
                ed.check_capacity(file, parent, count, adding)?;
            }
            let mut copies = Vec::with_capacity(tops.len());
            for node in tops {
                let copy = ed.alloc(file, |doc| Ok(doc.clone_subtree(node)?))?;
                match node_place(ed.project(), file, node)? {
                    NodePlace::Unconnected { root, tree_id } if root == node => {
                        let position = ed
                            .project()
                            .unconnected(file, &tree_id)
                            .iter()
                            .find(|n| n.node == node)
                            .map_or([0.0, 0.0], |n| n.position);
                        ed.push_unconnected(file, &tree_id, copy, offset(position, 1))?;
                    }
                    _ => {
                        let doc = ed.doc(file)?;
                        let parent = doc.parent(node).ok_or(CommandError::NotATreeNode(node))?;
                        let index = element_index(doc, node).unwrap_or(0) + 1;
                        ed.xml(file, |doc| {
                            Ok(((), doc.insert_element_pretty(parent, index, copy)?))
                        })?;
                    }
                }
                copies.push(copy);
            }
            Ok(copies)
        })
    }

    /// Checks that the file has a tree with this ID.
    pub(crate) fn check_tree_exists(&self, file: FileId, tree: &str) -> Result<(), CommandError> {
        let bt = self
            .project()
            .file(file)
            .ok_or(CommandError::UnknownFile(file))?;
        if bt.tree(tree).is_some() {
            Ok(())
        } else {
            Err(CommandError::TreeNotFound(tree.to_owned()))
        }
    }

    /// Adds a detached element as the last unconnected node of a tree.
    pub(crate) fn push_unconnected(
        &mut self,
        file: FileId,
        tree: &str,
        node: NodeId,
        position: [f32; 2],
    ) -> Result<(), CommandError> {
        check_position(position)?;
        let index = self.project().unconnected(file, tree).len();
        self.apply(Edit::InsertUnconnected {
            file,
            tree: tree.to_owned(),
            index,
            node: UnconnectedNode { node, position },
        })
    }
}

/// Checks that a canvas position is finite (JSON cannot hold other values).
pub(crate) fn check_position(position: [f32; 2]) -> Result<(), CommandError> {
    if position.iter().all(|v| v.is_finite()) {
        Ok(())
    } else {
        Err(CommandError::InvalidPosition(position))
    }
}
