//! Edit operations of the GUI.
//!
//! Panels, menus and canvas gestures describe a change as an [`EditOp`]
//! and push it as [`crate::actions::AppAction::Edit`]. The app runs it with
//! [`run`], which calls the commands of `fasttick_core::edit` through the
//! [`EditorDoc`](crate::editor::EditorDoc) of the view. The result says
//! which nodes to select. All XML logic stays in the core crate.

use std::collections::HashSet;

use fasttick_core::bt::NodeView;
use fasttick_core::bt::annotations::{DEFAULT_NOTE_SIZE, NOTE_ATTRIBUTE, TreeAnnotation};
use fasttick_core::edit::{CoalesceKey, Condition, copy};
use fasttick_core::models::{ModelSource, NodeModel};
use fasttick_core::project::FileId;
use fasttick_core::runtime::InstanceIndex;
use fasttick_core::xml::NodeId;

use crate::canvas::{EditTarget, NodeSource};
use crate::documents::{ViewId, Workspace};
use crate::editor::EditorDoc;

/// Where a new or moved node goes, relative to a target node.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Placement {
    /// The last child of the target.
    Inside,
    /// The sibling before the target.
    Before,
    /// The sibling after the target.
    After,
}

/// One change that the user asked for.
#[derive(Debug, Clone, PartialEq)]
pub enum EditOp {
    /// Adds a node of a type. With a target: at the placement; else as the
    /// last child of the selected node, as the root of an empty tree, or as
    /// an unconnected node.
    AddNode {
        /// The type ID.
        type_id: String,
        /// The target node and the placement.
        target: Option<(NodeId, Placement)>,
    },
    /// Adds a node of a type as an unconnected node, or as the root of an
    /// empty tree (a drop on an empty place).
    AddUnconnected {
        /// The type ID.
        type_id: String,
        /// The top-left corner of the new card (world units), or `None` for
        /// an automatic place.
        position: Option<[f32; 2]>,
    },
    /// Deletes the selected nodes (with their subtrees).
    Delete,
    /// Deletes the selected nodes, but keeps their children: the children
    /// become unconnected nodes at the places where the canvas shows them,
    /// and are selected.
    DeleteKeepChildren,
    /// Copies the selected nodes to the clipboard.
    Copy,
    /// Copies the selected nodes, then deletes them.
    Cut,
    /// Pastes the clipboard as the last children of the selected node. If
    /// the selected node takes no more children, the copies go after it.
    /// Without a selection, they are unconnected nodes.
    Paste,
    /// Pastes the clipboard as unconnected nodes.
    PasteUnconnected,
    /// Duplicates the selected nodes next to themselves.
    Duplicate,
    /// Moves the selected nodes to the unconnected area. They stay at the
    /// places where the canvas shows them.
    Disconnect,
    /// Moves nodes (drag and drop). Unconnected nodes connect.
    MoveNodes {
        /// The nodes, in order.
        nodes: Vec<NodeId>,
        /// The target node.
        target: NodeId,
        /// The placement.
        placement: Placement,
    },
    /// Moves nodes to the unconnected area (a drop on an empty place). An
    /// unconnected node only moves to its new place.
    DisconnectNodes {
        /// The nodes.
        nodes: Vec<NodeId>,
        /// The new top-left corner of the card of each node (world units).
        positions: Vec<[f32; 2]>,
    },
    /// Moves the selected node one place up (-1) or down (+1) among its
    /// siblings.
    MoveSibling(i32),
    /// Makes a new tree from the selected sibling nodes and puts a
    /// `<SubTree>` node at their place.
    CreateSubTree {
        /// The ID of the new tree.
        tree_id: String,
    },
    /// Sets or removes the instance name.
    SetName {
        /// The node.
        node: NodeId,
        /// The name, or `None` to remove the attribute.
        name: Option<String>,
    },
    /// Sets or removes a port value (or a SubTree remapping).
    SetPort {
        /// The node.
        node: NodeId,
        /// The port name.
        port: String,
        /// The value, or `None` to remove the attribute.
        value: Option<String>,
    },
    /// Sets or removes a pre- or post-condition script.
    SetCondition {
        /// The node.
        node: NodeId,
        /// The condition.
        condition: Condition,
        /// The script, or `None` to remove it.
        script: Option<String>,
    },
    /// Sets (`Some`) or removes (`None`) `_autoremap` of a SubTree node.
    SetAutoremap {
        /// The node.
        node: NodeId,
        /// The value.
        value: Option<bool>,
    },
    /// Sets the note of a node (the `_note` attribute); `None` or a blank
    /// text removes it.
    SetNote {
        /// The node.
        node: NodeId,
        /// The note.
        note: Option<String>,
    },
    /// Makes a new group of the selected nodes. Its title opens for
    /// editing.
    NewGroup,
    /// Removes the selected nodes from their groups.
    RemoveFromGroup,
    /// Sets the members of a group of the tree of the view.
    SetGroupMembers {
        /// The group ID.
        group: u32,
        /// The new members.
        nodes: Vec<NodeId>,
    },
    /// Sets the title of a group.
    SetGroupTitle {
        /// The group ID.
        group: u32,
        /// The title.
        title: String,
    },
    /// Sets the description of a group.
    SetGroupDescription {
        /// The group ID.
        group: u32,
        /// The description.
        description: String,
    },
    /// Sets the color of a group (`None`: the default).
    SetGroupColor {
        /// The group ID.
        group: u32,
        /// The color name.
        color: Option<String>,
    },
    /// Deletes a group (the nodes stay).
    DeleteGroup(u32),
    /// Adds an empty canvas note with its top-left corner at `at` (world
    /// units). Its text opens for editing.
    AddCanvasNote {
        /// The top-left corner.
        at: [f32; 2],
    },
    /// Sets the text of a canvas note.
    SetCanvasNoteText {
        /// The note ID.
        note: u32,
        /// The text.
        text: String,
    },
    /// Moves or resizes a canvas note.
    SetCanvasNoteBox {
        /// The note ID.
        note: u32,
        /// Left, top, width, height (world units).
        rect: [f32; 4],
    },
    /// Sets the color of a canvas note (`None`: the default).
    SetCanvasNoteColor {
        /// The note ID.
        note: u32,
        /// The color name.
        color: Option<String>,
    },
    /// Deletes a canvas note.
    DeleteCanvasNote(u32),
    /// Changes the type of a node and keeps its attributes.
    ChangeType {
        /// The node.
        node: NodeId,
        /// The new type ID.
        type_id: String,
    },
    /// Adds an empty tree and shows it.
    AddTree {
        /// The tree ID.
        id: String,
    },
    /// Renames a tree and the SubTree nodes that refer to it.
    RenameTree {
        /// The old ID.
        old: String,
        /// The new ID.
        new: String,
        /// The file of the tree (`None`: the first file with the ID). Tree
        /// IDs can repeat in several files.
        file: Option<FileId>,
    },
    /// Deletes a tree.
    DeleteTree {
        /// The tree ID.
        id: String,
        /// The file of the tree (`None`: the first file with the ID).
        file: Option<FileId>,
    },
    /// Sets `main_tree_to_execute`.
    SetMainTree {
        /// The tree ID.
        id: String,
        /// The file of the tree (`None`: the first file with the ID).
        file: Option<FileId>,
    },
    /// Adds a node model to the `<TreeNodesModel>` of the main file.
    AddModel(NodeModel),
    /// Replaces a node model (its ID can change).
    ReplaceModel {
        /// The ID of the model to replace.
        old_id: String,
        /// The new model.
        model: NodeModel,
    },
    /// Removes the node model with this ID.
    RemoveModel(String),
    /// Undoes the last change.
    Undo,
    /// Redoes the last undone change.
    Redo,
}

impl EditOp {
    /// True for operations that change the document (all except copy).
    pub fn changes_document(&self) -> bool {
        !matches!(self, EditOp::Copy)
    }
}

/// What the app does after an operation.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct EditOutcome {
    /// The nodes to select (in the file of the view).
    pub select: Vec<NodeId>,
    /// The tree to show in the view.
    pub show_tree: Option<InstanceIndex>,
    /// A short message for the status line or a notice.
    pub message: Option<String>,
    /// A text to open in the in-place editor of the canvas (a new group
    /// or note).
    pub edit_text: Option<EditTarget>,
}

/// The file of the tree that an edit view shows.
pub fn view_file(workspace: &Workspace, view: ViewId) -> Option<FileId> {
    let view = workspace.view(view)?;
    let editor = workspace.doc(view.doc)?.editor.as_deref()?;
    Some(
        editor
            .tree_file(view.root_instance.0)
            .unwrap_or(editor.file_id()),
    )
}

/// The selected, editable nodes of an edit view: XML nodes of the file of
/// the view that are not inline nodes of an expanded SubTree. Top-most
/// first in tree order.
pub fn selected_nodes(workspace: &Workspace, view: ViewId) -> Vec<NodeId> {
    let Some(main) = view_file(workspace, view) else {
        return Vec::new();
    };
    let Some(view) = workspace.view(view) else {
        return Vec::new();
    };
    let tree = view.canvas.tree();
    tree.selected()
        .filter_map(|id| tree.get(id))
        .filter(|node| !node.inline)
        .filter_map(|node| match node.source {
            NodeSource::Xml(file, node) if file == main => Some(node),
            _ => None,
        })
        .collect()
}

/// The primary selected, editable node of an edit view.
pub fn primary_node(workspace: &Workspace, view: ViewId) -> Option<NodeId> {
    let main = view_file(workspace, view)?;
    let view_ref = workspace.view(view)?;
    let tree = view_ref.canvas.tree();
    let node = view_ref
        .canvas
        .primary()
        .filter(|id| tree.is_selected(*id))
        .or_else(|| tree.selected().next())
        .and_then(|id| tree.get(id))?;
    match node.source {
        NodeSource::Xml(file, id) if file == main && !node.inline => Some(id),
        _ => None,
    }
}

/// True if the primary selected node can take one more child (see
/// [`crate::canvas::connect::has_dot`]), or if no node is selected.
fn primary_takes_children(workspace: &Workspace, view: ViewId) -> bool {
    let Some(view_ref) = workspace.view(view) else {
        return true;
    };
    let tree = view_ref.canvas.tree();
    view_ref
        .canvas
        .primary()
        .filter(|id| tree.is_selected(*id))
        .or_else(|| tree.selected().next())
        .and_then(|id| tree.get(id))
        .is_none_or(crate::canvas::connect::has_dot)
}

/// The parent element and the element index of `node`.
fn position_of(editor: &EditorDoc, file: FileId, node: NodeId) -> Option<(NodeId, usize)> {
    let doc = editor.project().file(file)?.doc();
    let parent = doc.parent(node)?;
    let index = doc.element_children(parent).position(|c| c == node)?;
    Some((parent, index))
}

/// True if `node` is the root node of a tree (its parent is the
/// `<BehaviorTree>` element).
fn is_tree_root(editor: &EditorDoc, file: FileId, node: NodeId) -> bool {
    let Some(file) = editor.project().file(file) else {
        return false;
    };
    let Some(parent) = file.doc().parent(node) else {
        return false;
    };
    file.index().trees.iter().any(|t| t.element == parent)
}

/// The parent and the index for a placement relative to `target`.
fn place(
    editor: &EditorDoc,
    file: FileId,
    target: NodeId,
    placement: Placement,
) -> Result<(NodeId, usize), String> {
    let doc = editor
        .project()
        .file(file)
        .ok_or("the file is closed")?
        .doc();
    match placement {
        Placement::Inside => Ok((target, doc.element_children(target).count())),
        Placement::Before | Placement::After => {
            if is_tree_root(editor, file, target) {
                return Err("A tree has one root node. Drop on the node to add a child.".into());
            }
            let (parent, index) = position_of(editor, file, target)
                .ok_or("This node has no parent. Drop on the node to add a child.")?;
            Ok((
                parent,
                if placement == Placement::After {
                    index + 1
                } else {
                    index
                },
            ))
        }
    }
}

/// Adds a node at a target place; without a target, as the root of an
/// empty tree or as an unconnected node of the tree `tree_id` at
/// `position` (see [`AUTO_POSITION`]).
fn add_node(
    editor: &mut EditorDoc,
    file: FileId,
    tree_id: Option<String>,
    type_id: &str,
    target: Option<(NodeId, Placement)>,
    position: [f32; 2],
) -> Result<NodeId, String> {
    let model = editor
        .project()
        .lookup_model(file, type_id)
        .map(|(model, _)| model.clone());
    match target {
        Some((target, placement)) => {
            let (parent, index) = place(editor, file, target, placement)?;
            editor.edit(|ed| ed.add_node(file, parent, index, type_id, model.as_ref()))
        }
        None => {
            let tree_id = tree_id.ok_or("the view shows no tree")?;
            let tree_element = editor
                .project()
                .file(file)
                .and_then(|f| f.index().tree(&tree_id).map(|t| (t.element, t.root)));
            match tree_element {
                Some((element, None)) => {
                    editor.edit(|ed| ed.add_node(file, element, 0, type_id, model.as_ref()))
                }
                _ => editor.edit(|ed| {
                    ed.add_unconnected_node(file, &tree_id, type_id, model.as_ref(), position)
                }),
            }
        }
    }
}

/// Pastes clipboard text as the last children of `parent`, or as
/// unconnected nodes of the tree `tree_id`.
fn paste(
    editor: &mut EditorDoc,
    file: FileId,
    tree_id: Option<String>,
    text: &str,
    parent: Option<NodeId>,
) -> Result<Vec<NodeId>, String> {
    match parent {
        Some(parent) => {
            let index = editor
                .project()
                .file(file)
                .map(|f| f.doc().element_children(parent).count())
                .unwrap_or(0);
            editor.edit(|ed| {
                let pasted = ed.paste(file, text, parent, index)?;
                ed.keep_groups_in_place(file, &pasted)?;
                Ok(pasted)
            })
        }
        None => {
            let tree_id = tree_id.ok_or("the view shows no tree")?;
            editor.edit(|ed| {
                let pasted = ed.paste_unconnected(file, &tree_id, text, [0.0, 0.0])?;
                ed.keep_groups_in_place(file, &pasted)?;
                Ok(pasted)
            })
        }
    }
}

/// The place of an unconnected node that the canvas chooses itself: the
/// layout puts such nodes in a row next to the tree. Any other position
/// is the top-left corner of the card (world units, see
/// [`crate::canvas::layout`]).
pub const AUTO_POSITION: [f32; 2] = [0.0, 0.0];

/// The top-left corner of the card of each XML node that the view shows
/// (world units).
fn card_positions(
    workspace: &Workspace,
    view: ViewId,
) -> std::collections::HashMap<NodeId, [f32; 2]> {
    let Some(view) = workspace.view(view) else {
        return Default::default();
    };
    let rects = &view.canvas.layout().rects;
    view.canvas
        .tree()
        .nodes()
        .iter()
        .zip(rects)
        .filter(|(node, _)| !node.inline)
        .filter_map(|(node, rect)| match node.source {
            NodeSource::Xml(_, xml) => Some((xml, [rect.min.x, rect.min.y])),
            _ => None,
        })
        .collect()
}

/// The nodes of a drop on an empty place whose card centre lands in the
/// box of their own group: they stay in the group.
fn dropped_in_their_group(
    workspace: &Workspace,
    view: ViewId,
    nodes: &[NodeId],
    positions: &[[f32; 2]],
) -> HashSet<NodeId> {
    let (Some(file), Some(view)) = (view_file(workspace, view), workspace.view(view)) else {
        return HashSet::new();
    };
    let canvas = &view.canvas;
    nodes
        .iter()
        .zip(positions)
        .filter(|(node, position)| {
            let Some(id) = canvas.tree().find_source(NodeSource::Xml(file, **node)) else {
                return false;
            };
            let (Some(canvas_node), Some(rect)) = (canvas.tree().get(id), canvas.layout().rect(id))
            else {
                return false;
            };
            let Some(group) = canvas_node.group else {
                return false;
            };
            let centre = egui::pos2(position[0], position[1]) + rect.size() * 0.5;
            canvas
                .group_boxes()
                .iter()
                .any(|g| g.id == group && g.rect.contains(centre))
        })
        .map(|(node, _)| *node)
        .collect()
}

/// The `<BehaviorTree>` element of a tree.
fn tree_element(editor: &EditorDoc, file: FileId, tree: &str) -> Option<NodeId> {
    editor.project().file(file)?.tree(tree).map(|t| t.element)
}

/// The tree ID that a view shows.
fn view_tree_id(workspace: &Workspace, view: ViewId) -> Option<String> {
    let view_ref = workspace.view(view)?;
    let editor = workspace.doc(view_ref.doc)?.editor.as_deref()?;
    editor.tree_id(view_ref.root_instance.0)
}

/// Runs an operation on the edit document of a view. `clipboard` is the
/// app's clipboard text (copy writes it, paste reads it).
pub fn run(
    workspace: &mut Workspace,
    view: ViewId,
    op: EditOp,
    clipboard: &mut Option<String>,
) -> Result<EditOutcome, String> {
    let selected = selected_nodes(workspace, view);
    let primary = primary_node(workspace, view);
    // Nodes that become unconnected stay where the canvas shows them.
    let positions = match op {
        EditOp::Disconnect | EditOp::DeleteKeepChildren => card_positions(workspace, view),
        _ => Default::default(),
    };
    let place_of = |node: &NodeId| positions.get(node).copied().unwrap_or(AUTO_POSITION);
    // Unconnected nodes that the user drops inside their group stay in it.
    let stay_in_group = match &op {
        EditOp::DisconnectNodes { nodes, positions } => {
            dropped_in_their_group(workspace, view, nodes, positions)
        }
        _ => HashSet::new(),
    };
    let takes_children = primary_takes_children(workspace, view);
    let tree_id = view_tree_id(workspace, view);
    let file = view_file(workspace, view).ok_or("the view is closed")?;
    let doc_id = workspace
        .view(view)
        .map(|v| v.doc)
        .ok_or("the view is closed")?;
    let editor = workspace
        .editor_mut(doc_id)
        .ok_or("this document cannot be edited")?;
    // Tree and model commands work in the file that holds the tree or the
    // model.
    let file_of_tree =
        |editor: &EditorDoc, id: &str| editor.project().find_tree(id).map_or(file, |r| r.file);
    let file_of_model = |editor: &EditorDoc, id: &str| match editor.project().lookup_model(file, id)
    {
        Some((_, ModelSource::File(f))) => f,
        _ => file,
    };
    let mut outcome = EditOutcome::default();
    match op {
        EditOp::AddNode { type_id, target } => {
            let target = target.or(primary.map(|node| (node, Placement::Inside)));
            outcome.select = vec![add_node(
                editor,
                file,
                tree_id,
                &type_id,
                target,
                AUTO_POSITION,
            )?];
        }
        EditOp::AddUnconnected { type_id, position } => {
            outcome.select = vec![add_node(
                editor,
                file,
                tree_id,
                &type_id,
                None,
                position.unwrap_or(AUTO_POSITION),
            )?];
        }
        EditOp::Delete => {
            if selected.is_empty() {
                return Err("Select the nodes to delete.".into());
            }
            editor.edit(|ed| {
                ed.delete_nodes(file, &selected)?;
                ed.remove_empty_groups(file)
            })?;
        }
        EditOp::DeleteKeepChildren => {
            if selected.is_empty() {
                return Err("Select the nodes to delete.".into());
            }
            outcome.select = editor.edit(|ed| {
                let kept = ed.delete_keep_children(file, &selected)?;
                for node in &kept {
                    ed.set_unconnected_position(file, *node, place_of(node))?;
                }
                ed.remove_empty_groups(file)?;
                Ok(kept)
            })?;
        }
        EditOp::Copy | EditOp::Cut => {
            if selected.is_empty() {
                return Err("Select the nodes to copy.".into());
            }
            let text = copy(editor.project(), file, &selected).map_err(|err| err.to_string())?;
            *clipboard = Some(text);
            if matches!(op, EditOp::Cut) {
                editor.edit(|ed| {
                    ed.delete_nodes(file, &selected)?;
                    ed.remove_empty_groups(file)
                })?;
            }
        }
        EditOp::Paste => {
            let text = clipboard.clone().ok_or("The clipboard is empty.")?;
            outcome.select = match primary {
                // A leaf (or a full decorator): the copies go after it. A
                // root or an unconnected node has no parent node: the
                // copies are unconnected.
                Some(node) if !takes_children => {
                    match position_of(editor, file, node)
                        .filter(|_| !is_tree_root(editor, file, node))
                    {
                        Some((parent, index)) => editor.edit(|ed| {
                            let pasted = ed.paste(file, &text, parent, index + 1)?;
                            ed.keep_groups_in_place(file, &pasted)?;
                            Ok(pasted)
                        })?,
                        None => paste(editor, file, tree_id, &text, None)?,
                    }
                }
                _ => paste(editor, file, tree_id, &text, primary)?,
            };
        }
        EditOp::PasteUnconnected => {
            let text = clipboard.clone().ok_or("The clipboard is empty.")?;
            outcome.select = paste(editor, file, tree_id, &text, None)?;
        }
        EditOp::Duplicate => {
            if selected.is_empty() {
                return Err("Select the nodes to duplicate.".into());
            }
            outcome.select = editor.edit(|ed| {
                let copies = ed.duplicate(file, &selected)?;
                ed.keep_groups_in_place(file, &copies)?;
                Ok(copies)
            })?;
        }
        EditOp::Disconnect => {
            if selected.is_empty() {
                return Err("Select the nodes to disconnect.".into());
            }
            editor.edit(|ed| {
                for node in &selected {
                    ed.disconnect(file, *node, place_of(node))?;
                }
                Ok(())
            })?;
            outcome.select = selected;
        }
        EditOp::MoveNodes {
            nodes,
            target,
            placement,
        } => {
            if nodes.contains(&target) {
                return Ok(outcome);
            }
            // Each node goes next to the target, so the nodes keep their
            // order if they go after it in reverse order.
            let ordered: Vec<NodeId> = if placement == Placement::After {
                nodes.iter().rev().copied().collect()
            } else {
                nodes.clone()
            };
            editor.edit(|ed| {
                for node in &ordered {
                    let (parent, mut index) = place_in(ed.project(), file, target, placement)?;
                    // The index counts the children after the node leaves its
                    // old place.
                    let doc = ed.project().file(file).map(|f| f.doc());
                    if let Some(doc) = doc
                        && doc.parent(*node) == Some(parent)
                        && let Some(old) = doc.element_children(parent).position(|c| c == *node)
                        && old < index
                    {
                        index -= 1;
                    }
                    ed.move_node(file, *node, parent, index)?;
                }
                ed.keep_groups_in_place(file, &nodes)
            })?;
            outcome.select = nodes;
        }
        EditOp::DisconnectNodes { nodes, positions } => {
            editor.edit(|ed| {
                for (index, node) in nodes.iter().enumerate() {
                    let position = positions.get(index).copied().unwrap_or(AUTO_POSITION);
                    let unconnected = matches!(
                        fasttick_core::edit::node_place(ed.project(), file, *node),
                        Ok(fasttick_core::edit::NodePlace::Unconnected { root, .. }) if root == *node
                    );
                    if unconnected {
                        ed.set_unconnected_position(file, *node, position)?;
                    } else {
                        ed.disconnect(file, *node, position)?;
                    }
                }
                let leave: Vec<NodeId> = nodes
                    .iter()
                    .copied()
                    .filter(|n| !stay_in_group.contains(n))
                    .collect();
                ed.keep_groups_in_place(file, &leave)
            })?;
            outcome.select = nodes;
        }
        EditOp::MoveSibling(delta) => {
            let node = primary.ok_or("Select a node to move.")?;
            let (parent, index) =
                position_of(editor, file, node).ok_or("This node has no parent.")?;
            let count = editor
                .project()
                .file(file)
                .map(|f| f.doc().element_children(parent).count())
                .unwrap_or(0);
            let new_index = index as i64 + i64::from(delta);
            if new_index < 0 || new_index >= count as i64 {
                return Ok(outcome);
            }
            editor.edit(|ed| ed.move_node(file, node, parent, new_index as usize))?;
            outcome.select = vec![node];
        }
        EditOp::CreateSubTree { tree_id: new_id } => {
            if selected.is_empty() {
                return Err("Select the nodes for the new tree.".into());
            }
            let created = editor.edit(|ed| {
                let created = ed.create_subtree_from_selection(file, &selected, &new_id)?;
                // The moved nodes leave the groups of the old tree.
                let root = ed
                    .project()
                    .file(file)
                    .and_then(|f| f.tree(&new_id))
                    .and_then(|t| t.root);
                if let Some(root) = root {
                    ed.keep_groups_in_place(file, &[root])?;
                }
                ed.remove_empty_groups(file)?;
                Ok(created)
            })?;
            outcome.select = vec![created.subtree_node];
            outcome.message = Some(format!("Created the tree {new_id}."));
        }
        EditOp::SetName { node, name } => {
            let key = CoalesceKey::attribute(file, node, "name");
            editor.edit_coalesced(key, |ed| ed.set_instance_name(file, node, name.as_deref()))?;
        }
        EditOp::SetPort { node, port, value } => {
            let key = CoalesceKey::attribute(file, node, &port);
            editor.edit_coalesced(key, |ed| {
                ed.set_port_value(file, node, &port, value.as_deref())
            })?;
        }
        EditOp::SetCondition {
            node,
            condition,
            script,
        } => {
            let key = CoalesceKey::attribute(file, node, condition.attribute_name());
            editor.edit_coalesced(key, |ed| {
                ed.set_condition(file, node, condition, script.as_deref())
            })?;
        }
        EditOp::SetAutoremap { node, value } => {
            editor.edit(|ed| match value {
                Some(value) => ed.set_attribute(
                    file,
                    node,
                    "_autoremap",
                    if value { "true" } else { "false" },
                ),
                None => ed.remove_attribute(file, node, "_autoremap"),
            })?;
        }
        EditOp::SetNote { node, note } => {
            let key = CoalesceKey::attribute(file, node, NOTE_ATTRIBUTE);
            editor.edit_coalesced(key, |ed| ed.set_node_note(file, node, note.as_deref()))?;
        }
        EditOp::NewGroup => {
            if selected.is_empty() {
                return Err("Select the nodes for the group.".into());
            }
            let tree = tree_id.ok_or("the view shows no tree")?;
            let id = editor.edit(|ed| ed.create_group(file, &tree, &selected, "Group"))?;
            outcome.edit_text = Some(EditTarget::GroupTitle(id));
        }
        EditOp::RemoveFromGroup => {
            if selected.is_empty() {
                return Err("Select the nodes to remove from their group.".into());
            }
            editor.edit(|ed| ed.remove_from_group(file, &selected))?;
            outcome.select = selected;
        }
        EditOp::SetGroupMembers { group, nodes } => {
            let tree = tree_id.ok_or("the view shows no tree")?;
            editor.edit(|ed| ed.set_group_members(file, &tree, group, &nodes))?;
        }
        EditOp::SetGroupTitle { group, title } => {
            let tree = tree_id.ok_or("the view shows no tree")?;
            let element = tree_element(editor, file, &tree).ok_or("the tree is gone")?;
            let key =
                CoalesceKey::attribute(file, element, &TreeAnnotation::GroupTitle(group).name());
            editor.edit_coalesced(key, |ed| ed.set_group_title(file, &tree, group, &title))?;
        }
        EditOp::SetGroupDescription { group, description } => {
            let tree = tree_id.ok_or("the view shows no tree")?;
            let element = tree_element(editor, file, &tree).ok_or("the tree is gone")?;
            let key = CoalesceKey::attribute(
                file,
                element,
                &TreeAnnotation::GroupDescription(group).name(),
            );
            editor.edit_coalesced(key, |ed| {
                ed.set_group_description(file, &tree, group, &description)
            })?;
        }
        EditOp::SetGroupColor { group, color } => {
            let tree = tree_id.ok_or("the view shows no tree")?;
            editor.edit(|ed| ed.set_group_color(file, &tree, group, color.as_deref()))?;
        }
        EditOp::DeleteGroup(group) => {
            let tree = tree_id.ok_or("the view shows no tree")?;
            editor.edit(|ed| ed.delete_group(file, &tree, group))?;
        }
        EditOp::AddCanvasNote { at } => {
            let tree = tree_id.ok_or("the view shows no tree")?;
            let rect = [at[0], at[1], DEFAULT_NOTE_SIZE[0], DEFAULT_NOTE_SIZE[1]];
            let id = editor.edit(|ed| ed.add_canvas_note(file, &tree, rect, ""))?;
            outcome.edit_text = Some(EditTarget::Note(id));
        }
        EditOp::SetCanvasNoteText { note, text } => {
            let tree = tree_id.ok_or("the view shows no tree")?;
            let element = tree_element(editor, file, &tree).ok_or("the tree is gone")?;
            let key = CoalesceKey::attribute(file, element, &TreeAnnotation::NoteText(note).name());
            editor.edit_coalesced(key, |ed| ed.set_canvas_note_text(file, &tree, note, &text))?;
        }
        EditOp::SetCanvasNoteBox { note, rect } => {
            let tree = tree_id.ok_or("the view shows no tree")?;
            editor.edit(|ed| ed.set_canvas_note_box(file, &tree, note, rect))?;
        }
        EditOp::SetCanvasNoteColor { note, color } => {
            let tree = tree_id.ok_or("the view shows no tree")?;
            editor.edit(|ed| ed.set_canvas_note_color(file, &tree, note, color.as_deref()))?;
        }
        EditOp::DeleteCanvasNote(note) => {
            let tree = tree_id.ok_or("the view shows no tree")?;
            editor.edit(|ed| ed.delete_canvas_note(file, &tree, note))?;
        }
        EditOp::ChangeType { node, type_id } => {
            let model = editor
                .project()
                .lookup_model(file, &type_id)
                .map(|(model, _)| model.clone());
            editor.edit(|ed| ed.change_node_type(file, node, &type_id, model.as_ref()))?;
            outcome.select = vec![node];
        }
        EditOp::AddTree { id } => {
            editor.edit(|ed| ed.add_tree(file, &id))?;
            outcome.show_tree = editor.tree_index_in(file, &id).map(InstanceIndex);
        }
        EditOp::RenameTree {
            old,
            new,
            file: tree_file,
        } => {
            let file = tree_file.unwrap_or_else(|| file_of_tree(editor, &old));
            let count = editor.edit(|ed| ed.rename_tree(file, &old, &new))?;
            outcome.message = Some(format!(
                "Renamed {old} to {new} ({count} references changed)."
            ));
        }
        EditOp::DeleteTree {
            id,
            file: tree_file,
        } => {
            let file = tree_file.unwrap_or_else(|| file_of_tree(editor, &id));
            editor.edit(|ed| ed.delete_tree(file, &id))?;
        }
        EditOp::SetMainTree {
            id,
            file: tree_file,
        } => {
            let file = tree_file.unwrap_or_else(|| file_of_tree(editor, &id));
            editor.edit(|ed| ed.set_main_tree(file, Some(&id)))?;
        }
        EditOp::AddModel(model) => {
            editor.edit(|ed| ed.add_model(file, &model))?;
        }
        EditOp::ReplaceModel { old_id, model } => {
            let file = file_of_model(editor, &old_id);
            editor.edit(|ed| ed.replace_model(file, &old_id, &model))?;
        }
        EditOp::RemoveModel(id) => {
            let file = file_of_model(editor, &id);
            editor.edit(|ed| ed.remove_model(file, &id))?;
        }
        EditOp::Undo => {
            outcome.message = editor.undo()?.map(|label| format!("Undo: {label}"));
        }
        EditOp::Redo => {
            outcome.message = editor.redo()?.map(|label| format!("Redo: {label}"));
        }
    }
    Ok(outcome)
}

/// Like [`place`], inside an edit session (the project can differ from
/// the one before the session started).
fn place_in(
    project: &fasttick_core::project::Project,
    file: FileId,
    target: NodeId,
    placement: Placement,
) -> Result<(NodeId, usize), fasttick_core::edit::CommandError> {
    use fasttick_core::edit::CommandError;
    let bt = project.file(file).ok_or(CommandError::UnknownFile(file))?;
    let doc = bt.doc();
    if !doc.is_element(target) {
        return Err(CommandError::UnknownNode { file, node: target });
    }
    match placement {
        Placement::Inside => Ok((target, doc.element_children(target).count())),
        Placement::Before | Placement::After => {
            let parent = doc
                .parent(target)
                .filter(|p| {
                    NodeView::new(doc, *p).is_some()
                        && !bt.index().trees.iter().any(|t| t.element == *p)
                })
                .ok_or(CommandError::NotATreeNode(target))?;
            let index = doc
                .element_children(parent)
                .position(|c| c == target)
                .ok_or(CommandError::NotATreeNode(target))?;
            Ok((
                parent,
                if placement == Placement::After {
                    index + 1
                } else {
                    index
                },
            ))
        }
    }
}
