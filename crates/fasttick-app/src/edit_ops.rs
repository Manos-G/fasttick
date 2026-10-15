//! Edit operations of the GUI.
//!
//! Panels, menus and canvas gestures describe a change as an [`EditOp`]
//! and push it as [`crate::actions::AppAction::Edit`]. The app runs it with
//! [`run`], which calls the commands of `fasttick_core::edit` through the
//! [`EditorDoc`](crate::editor::EditorDoc) of the view. The result says
//! which nodes to select. All XML logic stays in the core crate.

use fasttick_core::bt::NodeView;
use fasttick_core::edit::{CoalesceKey, Condition, copy};
use fasttick_core::models::NodeModel;
use fasttick_core::project::FileId;
use fasttick_core::runtime::InstanceIndex;
use fasttick_core::xml::NodeId;

use crate::canvas::NodeSource;
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
    /// Deletes the selected nodes (with their subtrees).
    Delete,
    /// Copies the selected nodes to the clipboard.
    Copy,
    /// Copies the selected nodes, then deletes them.
    Cut,
    /// Pastes the clipboard as the last children of the selected node, or
    /// as unconnected nodes.
    Paste,
    /// Duplicates the selected nodes next to themselves.
    Duplicate,
    /// Moves the selected nodes to the unconnected area.
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
    /// Moves nodes to the unconnected area (a drop on an empty place).
    DisconnectNodes {
        /// The nodes.
        nodes: Vec<NodeId>,
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
    },
    /// Deletes a tree.
    DeleteTree {
        /// The tree ID.
        id: String,
    },
    /// Sets `main_tree_to_execute`.
    SetMainTree {
        /// The tree ID.
        id: String,
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
    /// The nodes to select (in the main file).
    pub select: Vec<NodeId>,
    /// The tree to show in the view.
    pub show_tree: Option<InstanceIndex>,
    /// A short message for the status line or a notice.
    pub message: Option<String>,
}

/// The selected, editable nodes of an edit view: XML nodes of the main
/// file that are not inline nodes of an expanded SubTree. Top-most first in
/// tree order.
pub fn selected_nodes(workspace: &Workspace, view: ViewId) -> Vec<NodeId> {
    let Some(view) = workspace.view(view) else {
        return Vec::new();
    };
    let Some(editor) = workspace.doc(view.doc).and_then(|d| d.editor.as_deref()) else {
        return Vec::new();
    };
    let main = editor.file_id();
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
    let view_ref = workspace.view(view)?;
    let editor = workspace.doc(view_ref.doc)?.editor.as_deref()?;
    let tree = view_ref.canvas.tree();
    let node = view_ref
        .canvas
        .primary()
        .filter(|id| tree.is_selected(*id))
        .or_else(|| tree.selected().next())
        .and_then(|id| tree.get(id))?;
    match node.source {
        NodeSource::Xml(file, id) if file == editor.file_id() && !node.inline => Some(id),
        _ => None,
    }
}

/// The parent element and the element index of `node`.
fn position_of(editor: &EditorDoc, node: NodeId) -> Option<(NodeId, usize)> {
    let doc = editor.file()?.doc();
    let parent = doc.parent(node)?;
    let index = doc.element_children(parent).position(|c| c == node)?;
    Some((parent, index))
}

/// True if `node` is the root node of a tree (its parent is the
/// `<BehaviorTree>` element).
fn is_tree_root(editor: &EditorDoc, node: NodeId) -> bool {
    let Some(file) = editor.file() else {
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
    target: NodeId,
    placement: Placement,
) -> Result<(NodeId, usize), String> {
    let doc = editor.file().ok_or("the file is closed")?.doc();
    match placement {
        Placement::Inside => Ok((target, doc.element_children(target).count())),
        Placement::Before | Placement::After => {
            if is_tree_root(editor, target) {
                return Err("A tree has one root node. Drop on the node to add a child.".into());
            }
            let (parent, index) = position_of(editor, target)
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
    let tree_id = view_tree_id(workspace, view);
    let doc_id = workspace
        .view(view)
        .map(|v| v.doc)
        .ok_or("the view is closed")?;
    let editor = workspace
        .editor_mut(doc_id)
        .ok_or("this document cannot be edited")?;
    let file = editor.file_id();
    let mut outcome = EditOutcome::default();
    match op {
        EditOp::AddNode { type_id, target } => {
            let model = editor
                .project()
                .lookup_model(file, &type_id)
                .map(|(model, _)| model.clone());
            let target = target.or(primary.map(|node| (node, Placement::Inside)));
            let new = match target {
                Some((target, placement)) => {
                    let (parent, index) = place(editor, target, placement)?;
                    editor.edit(|ed| ed.add_node(file, parent, index, &type_id, model.as_ref()))?
                }
                None => {
                    let tree_id = tree_id.ok_or("the view shows no tree")?;
                    let tree_element = editor
                        .file()
                        .and_then(|f| f.index().tree(&tree_id).map(|t| (t.element, t.root)));
                    match tree_element {
                        Some((element, None)) => editor
                            .edit(|ed| ed.add_node(file, element, 0, &type_id, model.as_ref()))?,
                        _ => editor.edit(|ed| {
                            ed.add_unconnected_node(
                                file,
                                &tree_id,
                                &type_id,
                                model.as_ref(),
                                [0.0, 0.0],
                            )
                        })?,
                    }
                }
            };
            outcome.select = vec![new];
        }
        EditOp::Delete => {
            if selected.is_empty() {
                return Err("Select the nodes to delete.".into());
            }
            editor.edit(|ed| ed.delete_nodes(file, &selected))?;
        }
        EditOp::Copy | EditOp::Cut => {
            if selected.is_empty() {
                return Err("Select the nodes to copy.".into());
            }
            let text = copy(editor.project(), file, &selected).map_err(|err| err.to_string())?;
            *clipboard = Some(text);
            if matches!(op, EditOp::Cut) {
                editor.edit(|ed| ed.delete_nodes(file, &selected))?;
            }
        }
        EditOp::Paste => {
            let text = clipboard.clone().ok_or("The clipboard is empty.")?;
            let new = match primary {
                Some(parent) => {
                    let index = editor
                        .file()
                        .map(|f| f.doc().element_children(parent).count())
                        .unwrap_or(0);
                    editor.edit(|ed| ed.paste(file, &text, parent, index))?
                }
                None => {
                    let tree_id = tree_id.ok_or("the view shows no tree")?;
                    editor.edit(|ed| ed.paste_unconnected(file, &tree_id, &text, [0.0, 0.0]))?
                }
            };
            outcome.select = new;
        }
        EditOp::Duplicate => {
            if selected.is_empty() {
                return Err("Select the nodes to duplicate.".into());
            }
            outcome.select = editor.edit(|ed| ed.duplicate(file, &selected))?;
        }
        EditOp::Disconnect => {
            if selected.is_empty() {
                return Err("Select the nodes to disconnect.".into());
            }
            editor.edit(|ed| {
                for node in &selected {
                    ed.disconnect(file, *node, [0.0, 0.0])?;
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
            editor.edit(|ed| {
                for node in &nodes {
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
                Ok(())
            })?;
            outcome.select = nodes;
        }
        EditOp::DisconnectNodes { nodes } => {
            editor.edit(|ed| {
                for node in &nodes {
                    ed.disconnect(file, *node, [0.0, 0.0])?;
                }
                Ok(())
            })?;
            outcome.select = nodes;
        }
        EditOp::MoveSibling(delta) => {
            let node = primary.ok_or("Select a node to move.")?;
            let (parent, index) = position_of(editor, node).ok_or("This node has no parent.")?;
            let count = editor
                .file()
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
            let created =
                editor.edit(|ed| ed.create_subtree_from_selection(file, &selected, &new_id))?;
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
            outcome.show_tree = editor.tree_index(&id).map(InstanceIndex);
        }
        EditOp::RenameTree { old, new } => {
            let count = editor.edit(|ed| ed.rename_tree(file, &old, &new))?;
            outcome.message = Some(format!(
                "Renamed {old} to {new} ({count} references changed)."
            ));
        }
        EditOp::DeleteTree { id } => {
            editor.edit(|ed| ed.delete_tree(file, &id))?;
        }
        EditOp::SetMainTree { id } => {
            editor.edit(|ed| ed.set_main_tree(file, Some(&id)))?;
        }
        EditOp::AddModel(model) => {
            editor.edit(|ed| ed.add_model(file, &model))?;
        }
        EditOp::ReplaceModel { old_id, model } => {
            editor.edit(|ed| ed.replace_model(file, &old_id, &model))?;
        }
        EditOp::RemoveModel(id) => {
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
