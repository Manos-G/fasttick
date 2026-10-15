//! Unconnected nodes: disconnect, connect, add and move.
//!
//! BT.CPP XML cannot hold a node without a parent. FastTick keeps such
//! nodes as detached elements in the document of their file and stores
//! them in the project file (decision D5 in `docs/PLAN.md`). Each
//! unconnected node belongs to the unconnected area of one tree.

use crate::models::NodeModel;
use crate::project::FileId;
use crate::xml::NodeId;

use super::clipboard::check_position;
use super::editor::{Editor, NodePlace, node_place};
use super::error::CommandError;
use super::transaction::Edit;

impl Editor<'_> {
    /// Detaches a tree node with its subtree and makes it an unconnected
    /// node of the same tree, at `position` on the canvas. The node and its
    /// descendants keep their node IDs.
    ///
    /// A node below an unconnected node can also be disconnected; it
    /// becomes a separate unconnected node.
    pub fn disconnect(
        &mut self,
        file: FileId,
        node: NodeId,
        position: [f32; 2],
    ) -> Result<(), CommandError> {
        self.atomic("Disconnect", |ed| {
            let place = node_place(ed.project(), file, node)?;
            if let NodePlace::Unconnected { root, .. } = &place
                && *root == node
            {
                return Err(CommandError::AlreadyUnconnected(node));
            }
            ed.xml(file, |doc| Ok(((), doc.remove_element_pretty(node)?)))?;
            ed.push_unconnected(file, place.tree_id(), node, position)
        })
    }

    /// Connects an unconnected node: inserts it as element child number
    /// `index` of `parent` (a tree node, another unconnected node or a
    /// `<BehaviorTree>` element), with the indentation of its new siblings.
    ///
    /// The command refuses a parent inside the node itself, and a parent
    /// that cannot take one more child (see
    /// [`StructurePolicy`](super::StructurePolicy)).
    pub fn connect(
        &mut self,
        file: FileId,
        node: NodeId,
        parent: NodeId,
        index: usize,
    ) -> Result<(), CommandError> {
        self.atomic("Connect", |ed| {
            let tree = match node_place(ed.project(), file, node)? {
                NodePlace::Unconnected { root, tree_id } if root == node => tree_id,
                _ => return Err(CommandError::NotUnconnected(node)),
            };
            let (_, count) = ed.insertion_point(file, parent, index)?;
            let doc = ed.doc(file)?;
            if parent == node || doc.ancestors(parent).any(|a| a == node) {
                return Err(CommandError::IntoOwnSubtree(node));
            }
            ed.check_capacity(file, parent, count, 1)?;
            let list_index = ed.unconnected_index(file, &tree, node)?;
            ed.apply(Edit::RemoveUnconnected {
                file,
                tree,
                index: list_index,
            })?;
            ed.xml(file, |doc| {
                Ok(((), doc.insert_element_pretty(parent, index, node)?))
            })
        })
    }

    /// Adds a new node of the type `type_id` as an unconnected node of a
    /// tree, at `position` on the canvas (for example a node that the user
    /// drops from the palette on an empty place). See [`Editor::add_node`]
    /// for the form of the node.
    pub fn add_unconnected_node(
        &mut self,
        file: FileId,
        tree: &str,
        type_id: &str,
        model: Option<&NodeModel>,
        position: [f32; 2],
    ) -> Result<NodeId, CommandError> {
        self.atomic("Add node", |ed| {
            ed.check_tree_exists(file, tree)?;
            let node = ed.new_node(file, type_id, model)?;
            ed.push_unconnected(file, tree, node, position)?;
            Ok(node)
        })
    }

    /// Moves an unconnected node on the canvas.
    pub fn set_unconnected_position(
        &mut self,
        file: FileId,
        node: NodeId,
        position: [f32; 2],
    ) -> Result<(), CommandError> {
        self.atomic("Move node", |ed| {
            check_position(position)?;
            let old = ed
                .project()
                .all_unconnected()
                .find(|(f, _, n)| *f == file && n.node == node)
                .map(|(_, _, n)| n.position)
                .ok_or(CommandError::NotUnconnected(node))?;
            if old == position {
                return Ok(());
            }
            ed.apply(Edit::UnconnectedPosition {
                file,
                node,
                position,
            })
        })
    }
}
