//! The edit entries of the canvas context menu (edit mode).
//!
//! The canvas draws while the workspace is borrowed, so the app collects
//! what the menu needs in an [`EditMenuContext`] first. The menu pushes
//! [`AppAction`]s; the app runs them after the frame.

use egui::{RichText, Ui};
use fasttick_core::models::NodeKind;
use fasttick_core::runtime::InstanceIndex;

use crate::actions::{AppAction, NamePurpose};
use crate::canvas::drop::takes_children;
use crate::canvas::{CanvasNode, NodeSource};
use crate::documents::ViewId;
use crate::edit_ops::{EditOp, Placement};
use crate::editor::EditorDoc;

/// The data of the edit menu of one view.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct EditMenuContext {
    /// The view.
    pub view: Option<ViewId>,
    /// The trees of the file: index and ID.
    pub trees: Vec<(usize, String)>,
    /// True if the clipboard has nodes.
    pub has_clipboard: bool,
}

impl EditMenuContext {
    /// The menu data of a view of an editor document.
    pub fn new(view: ViewId, editor: &EditorDoc, has_clipboard: bool) -> Self {
        EditMenuContext {
            view: Some(view),
            trees: editor.trees(),
            has_clipboard,
        }
    }

    fn tree_index(&self, id: &str) -> Option<usize> {
        self.trees.iter().find(|(_, t)| t == id).map(|(i, _)| *i)
    }
}

/// The plural name of a kind, for menu groups.
pub fn kind_group_name(kind: NodeKind) -> &'static str {
    match kind {
        NodeKind::Action => "Actions",
        NodeKind::Condition => "Conditions",
        NodeKind::Control => "Controls",
        NodeKind::Decorator => "Decorators",
        NodeKind::SubTree => "SubTrees",
    }
}

/// A menu button that opens the search popup that adds a node (see
/// [`crate::add_popup`]) at the place of the menu.
fn add_button(
    ui: &mut Ui,
    label: &str,
    view: ViewId,
    target: Option<(fasttick_core::xml::NodeId, Placement)>,
    title: String,
    actions: &mut Vec<AppAction>,
) {
    let response = ui.button(label);
    if response.clicked() {
        actions.push(AppAction::OpenAddPopup {
            view,
            target,
            title,
            pos: response.rect.left_top(),
        });
        ui.close();
    }
}

/// Draws the edit entries for the node under the pointer (or for an empty
/// place). Returns true if it added entries.
pub fn edit_menu(
    ui: &mut Ui,
    context: &EditMenuContext,
    node: Option<&CanvasNode>,
    actions: &mut Vec<AppAction>,
) -> bool {
    let Some(view) = context.view else {
        return false;
    };
    let Some(node) = node else {
        // An empty place: new unconnected nodes.
        add_button(
            ui,
            "Add node…",
            view,
            None,
            "Add an unconnected node".to_owned(),
            actions,
        );
        if ui
            .add_enabled(context.has_clipboard, egui::Button::new("Paste"))
            .on_hover_text("Paste the copied nodes as unconnected nodes.")
            .clicked()
        {
            actions.push(AppAction::Edit(view, EditOp::PasteUnconnected));
            ui.close();
        }
        return true;
    };
    if node.inline {
        ui.label(
            RichText::new("This node is part of another tree. Open that tree to edit it.").weak(),
        );
        return true;
    }
    let container = takes_children(node.category) && node.subtree.is_none();
    let NodeSource::Xml(_, xml_node) = node.source else {
        return false;
    };
    if container {
        add_button(
            ui,
            "Add child…",
            view,
            Some((xml_node, Placement::Inside)),
            format!("Add a child of {}", node.label),
            actions,
        );
    }
    if node.parent.is_some() {
        add_button(
            ui,
            "Add sibling before…",
            view,
            Some((xml_node, Placement::Before)),
            format!("Add a node before {}", node.label),
            actions,
        );
        add_button(
            ui,
            "Add sibling after…",
            view,
            Some((xml_node, Placement::After)),
            format!("Add a node after {}", node.label),
            actions,
        );
    }
    if let Some(info) = &node.subtree {
        let index = context.tree_index(&info.tree_id);
        if ui
            .add_enabled(
                index.is_some(),
                egui::Button::new(format!("Open tree {}", info.tree_id)),
            )
            .on_disabled_hover_text("The project has no tree with this ID.")
            .clicked()
            && let Some(index) = index
        {
            actions.push(AppAction::SetViewRoot(view, InstanceIndex(index)));
            ui.close();
        }
    }
    ui.separator();
    let mut edit = |op: EditOp| actions.push(AppAction::Edit(view, op));
    for (label, op) in [
        ("Cut", EditOp::Cut),
        ("Copy", EditOp::Copy),
        ("Duplicate", EditOp::Duplicate),
        ("Delete", EditOp::DeleteKeepChildren),
    ] {
        if ui.button(label).clicked() {
            edit(op);
            ui.close();
        }
    }
    if container
        && ui
            .button("Delete with children")
            .on_hover_text(
                "Delete the node and all nodes below it. (Delete keeps the children as \
                 unconnected nodes.)",
            )
            .clicked()
    {
        edit(EditOp::Delete);
        ui.close();
    }
    if container
        && ui
            .add_enabled(context.has_clipboard, egui::Button::new("Paste as child"))
            .clicked()
    {
        edit(EditOp::Paste);
        ui.close();
    }
    ui.separator();
    if node.parent.is_some() {
        if ui.button("Move up").clicked() {
            edit(EditOp::MoveSibling(-1));
            ui.close();
        }
        if ui.button("Move down").clicked() {
            edit(EditOp::MoveSibling(1));
            ui.close();
        }
        if ui
            .button("Disconnect")
            .on_hover_text("Move the node to the unconnected nodes of this tree.")
            .clicked()
        {
            edit(EditOp::Disconnect);
            ui.close();
        }
    }
    ui.separator();
    if ui
        .button("New group")
        .on_hover_text(
            "Put a box with a title and a description around the selected nodes (Ctrl+G).",
        )
        .clicked()
    {
        edit(EditOp::NewGroup);
        ui.close();
    }
    if node.group.is_some()
        && ui
            .button("Remove from group")
            .on_hover_text("Take the selected nodes out of their group.")
            .clicked()
    {
        edit(EditOp::RemoveFromGroup);
        ui.close();
    }
    if ui
        .button("Create SubTree from selection…")
        .on_hover_text("Move the selected sibling nodes to a new tree.")
        .clicked()
    {
        actions.push(AppAction::PromptName {
            view,
            purpose: NamePurpose::CreateSubTree,
        });
        ui.close();
    }
    true
}
