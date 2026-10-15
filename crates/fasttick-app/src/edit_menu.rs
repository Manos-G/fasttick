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
    /// The node types that the file can use, grouped by kind and sorted.
    pub groups: Vec<(NodeKind, Vec<String>)>,
    /// The trees of the file: index and ID.
    pub trees: Vec<(usize, String)>,
    /// True if the clipboard has nodes.
    pub has_clipboard: bool,
}

impl EditMenuContext {
    /// The menu data of a view of an editor document.
    pub fn new(view: ViewId, editor: &EditorDoc, has_clipboard: bool) -> Self {
        let project = editor.project();
        let groups = project
            .registry()
            .all_models(editor.file_id())
            .into_iter()
            .map(|(kind, models)| (kind, models.iter().map(|m| m.id.clone()).collect()))
            .collect();
        EditMenuContext {
            view: Some(view),
            groups,
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

/// A submenu that lists the node types by kind. Returns the clicked type.
fn type_menu(ui: &mut Ui, title: &str, context: &EditMenuContext) -> Option<String> {
    let mut picked = None;
    ui.menu_button(title, |ui| {
        for (kind, ids) in &context.groups {
            ui.menu_button(kind_group_name(*kind), |ui| {
                egui::ScrollArea::vertical()
                    .max_height(360.0)
                    .show(ui, |ui| {
                        for id in ids {
                            if ui.button(id).clicked() {
                                picked = Some(id.clone());
                            }
                        }
                    });
            });
        }
    });
    picked
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
    let mut edit = |op: EditOp| actions.push(AppAction::Edit(view, op));
    let Some(node) = node else {
        // An empty place: new unconnected nodes.
        if let Some(type_id) = type_menu(ui, "Add node", context) {
            edit(EditOp::AddUnconnected { type_id });
            ui.close();
        }
        if ui
            .add_enabled(context.has_clipboard, egui::Button::new("Paste"))
            .on_hover_text("Paste the copied nodes as unconnected nodes.")
            .clicked()
        {
            edit(EditOp::PasteUnconnected);
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
    if container && let Some(type_id) = type_menu(ui, "Add child", context) {
        edit(EditOp::AddNode {
            type_id,
            target: Some((xml_node, Placement::Inside)),
        });
        ui.close();
    }
    if node.parent.is_some() {
        if let Some(type_id) = type_menu(ui, "Add sibling before", context) {
            edit(EditOp::AddNode {
                type_id,
                target: Some((xml_node, Placement::Before)),
            });
            ui.close();
        }
        if let Some(type_id) = type_menu(ui, "Add sibling after", context) {
            edit(EditOp::AddNode {
                type_id,
                target: Some((xml_node, Placement::After)),
            });
            ui.close();
        }
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
        ("Delete", EditOp::Delete),
    ] {
        if ui.button(label).clicked() {
            edit(op);
            ui.close();
        }
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
