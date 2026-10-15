//! The inspector. In an edit view it edits the selected node (see
//! [`super::node_editor`]); in live and replay views it shows the node
//! read-only.

use egui::{RichText, Ui};
use fasttick_core::status::NodeStatus;

use super::{PanelContext, node_editor};
use crate::canvas::NodeSource;

/// Draws the inspector.
pub fn show(ui: &mut Ui, cx: &mut PanelContext<'_>) {
    let Some(view) = cx.workspace.active() else {
        empty(ui, "No document is open.");
        return;
    };
    if cx
        .workspace
        .doc(view.doc)
        .is_none_or(|d| d.mode() != cx.mode)
    {
        empty(ui, "No document in this mode.");
        return;
    }
    let tree = view.canvas.tree();
    let count = tree.selected_count();
    let Some(id) = view
        .canvas
        .primary()
        .filter(|id| tree.is_selected(*id))
        .or_else(|| tree.selected().next())
    else {
        empty(ui, "Select a node to see its properties.");
        return;
    };
    let Some(node) = tree.get(id) else {
        return;
    };
    let view_id = view.id;
    if let Some(editor) = cx.workspace.doc(view.doc).and_then(|d| d.editor.as_deref()) {
        let source = node.source;
        let inline = node.inline;
        egui::ScrollArea::vertical()
            .auto_shrink([false, false])
            .show(ui, |ui| match source {
                NodeSource::Xml(file, xml) if !inline && editor.is_tree_file(file) => {
                    node_editor::show(ui, view_id, editor, file, xml, count, cx.actions);
                }
                _ => {
                    ui.label(RichText::new(&node.label).strong().size(15.0));
                    ui.label(RichText::new(&node.type_id).weak());
                    ui.add_space(6.0);
                    ui.label(
                        RichText::new(
                            "This node is part of another tree. Open that tree to edit it.",
                        )
                        .weak(),
                    );
                }
            });
        return;
    }
    egui::ScrollArea::vertical()
        .auto_shrink([false, false])
        .show(ui, |ui| {
            ui.label(RichText::new(&node.label).strong().size(15.0));
            ui.label(
                RichText::new(format!("{}  ·  {}", node.type_id, node.category.name())).weak(),
            );
            if count > 1 {
                ui.label(
                    RichText::new(format!("{count} nodes are selected."))
                        .small()
                        .weak(),
                );
            }
            ui.add_space(6.0);
            egui::Grid::new("inspector_grid")
                .num_columns(2)
                .spacing([12.0, 4.0])
                .striped(true)
                .show(ui, |ui| {
                    if !node.path.is_empty() {
                        ui.label(RichText::new("Path").weak());
                        ui.label(&node.path);
                        ui.end_row();
                    }
                    if let Some(uid) = node.uid {
                        ui.label(RichText::new("UID").weak());
                        ui.label(uid.to_string());
                        ui.end_row();
                    }
                    ui.label(RichText::new("Status").weak());
                    ui.label(match (node.deco.status, node.deco.previous_status) {
                        (NodeStatus::Idle, Some(previous)) => format!("IDLE (before: {previous})"),
                        (status, _) => status.to_string(),
                    });
                    ui.end_row();
                    if let Some(info) = &node.subtree {
                        ui.label(RichText::new("Tree").weak());
                        ui.label(&info.tree_id);
                        ui.end_row();
                    }
                });
            if !node.ports.is_empty() {
                ui.add_space(8.0);
                ui.label(RichText::new("Ports").strong());
                egui::Grid::new("inspector_ports")
                    .num_columns(2)
                    .spacing([12.0, 4.0])
                    .striped(true)
                    .show(ui, |ui| {
                        for port in &node.ports {
                            ui.label(RichText::new(&port.name).weak());
                            ui.label(RichText::new(&port.value).monospace());
                            ui.end_row();
                        }
                    });
            }
            if !node.conditions.is_empty() {
                ui.add_space(8.0);
                ui.label(RichText::new("Pre- and post-conditions").strong());
                egui::Grid::new("inspector_conditions")
                    .num_columns(2)
                    .spacing([12.0, 4.0])
                    .striped(true)
                    .show(ui, |ui| {
                        for line in &node.conditions {
                            ui.label(RichText::new(&line.name).color(cx.palette.condition))
                                .on_hover_text(super::node_editor::condition_help(&line.name));
                            ui.label(RichText::new(&line.value).monospace());
                            ui.end_row();
                        }
                    });
            }
            ui.add_space(10.0);
            ui.label(
                RichText::new("Read-only. Open the tree file in edit mode to change it.")
                    .small()
                    .weak(),
            );
        });
}

fn empty(ui: &mut Ui, text: &str) {
    ui.add_space(8.0);
    ui.label(RichText::new(text).weak());
}
