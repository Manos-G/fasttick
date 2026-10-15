//! The palette: the node types that the open file can use, grouped by
//! kind, with a search field. Drag a type onto the canvas to add a node;
//! double-click it to add it to the selected node.

use egui::{Id, RichText, Sense, Ui};
use fasttick_core::models::{ModelSource, NodeKind, NodeModel};

use super::PanelContext;
use crate::actions::AppAction;
use crate::canvas::{NewNodePayload, category_of_kind};
use crate::documents::ViewId;
use crate::edit_menu::kind_group_name;
use crate::edit_ops::EditOp;
use crate::theme::CanvasPalette;

/// True if a model matches a search text (case-insensitive, in the ID or
/// in a port name).
pub fn matches(model: &NodeModel, query: &str) -> bool {
    let query = query.trim().to_lowercase();
    query.is_empty()
        || model.id.to_lowercase().contains(&query)
        || model
            .ports
            .iter()
            .any(|p| p.name.to_lowercase().contains(&query))
}

/// True if the user can change or remove a model in FastTick: it is in a
/// `<TreeNodesModel>` of a project file.
pub fn is_editable(source: ModelSource) -> bool {
    matches!(source, ModelSource::File(_))
}

/// Draws the palette.
pub fn show(ui: &mut Ui, cx: &mut PanelContext<'_>) {
    let Some((view, editor, file)) = super::active_edit(cx.workspace, cx.mode) else {
        super::empty(ui, "Open a tree file to see its node types.");
        return;
    };
    let search_id = ui.id().with("palette_search");
    let mut query: String = ui.data_mut(|d| d.get_temp(search_id).unwrap_or_default());
    ui.horizontal(|ui| {
        ui.add(
            egui::TextEdit::singleline(&mut query)
                .hint_text("Search node types")
                .desired_width(ui.available_width() - 70.0),
        );
        if ui
            .button("New…")
            .on_hover_text("Add a node model to this file")
            .clicked()
        {
            cx.actions.push(AppAction::EditModel { view, model: None });
        }
    });
    ui.data_mut(|d| d.insert_temp(search_id, query.clone()));
    ui.label(
        RichText::new(
            "Drag a type onto the canvas, or double-click it to add it to the selected node.",
        )
        .small()
        .weak(),
    );
    ui.add_space(4.0);

    let project = editor.project();
    let all = project.registry().all_models(file);
    egui::ScrollArea::vertical()
        .auto_shrink([false, false])
        .show(ui, |ui| {
            let mut any = false;
            for (kind, models) in &all {
                let shown: Vec<&&NodeModel> =
                    models.iter().filter(|m| matches(m, &query)).collect();
                if shown.is_empty() {
                    continue;
                }
                any = true;
                egui::CollapsingHeader::new(format!(
                    "{} ({})",
                    kind_group_name(*kind),
                    shown.len()
                ))
                .id_salt(("palette_group", kind.tag()))
                .default_open(true)
                .open((!query.trim().is_empty()).then_some(true))
                .show(ui, |ui| {
                    for model in shown {
                        let source = project
                            .lookup_model(file, &model.id)
                            .map(|(_, s)| s)
                            .unwrap_or(ModelSource::Unattached);
                        item(ui, view, model, source, cx.palette, cx.actions, |model| {
                            if model.kind == NodeKind::SubTree {
                                project.tree_keys(file, &model.id)
                            } else {
                                Vec::new()
                            }
                        });
                    }
                });
            }
            if !any {
                ui.label(RichText::new("No node type matches the search.").weak());
            }
        });
}

/// One palette entry: a drag source with a tooltip and a context menu.
fn item(
    ui: &mut Ui,
    view: ViewId,
    model: &NodeModel,
    source: ModelSource,
    palette: &CanvasPalette,
    actions: &mut Vec<AppAction>,
    tree_keys: impl FnOnce(&NodeModel) -> Vec<String>,
) {
    let payload = NewNodePayload {
        type_id: model.id.clone(),
    };
    let color = palette.category(category_of_kind(model.kind));
    // The content senses no clicks: a widget that senses clicks would stay
    // on top of the drop target while it follows the pointer.
    let response = ui
        .dnd_drag_source(Id::new(("palette_item", &model.id)), payload, |ui| {
            ui.horizontal(|ui| {
                let (rect, _) = ui.allocate_exact_size(egui::vec2(10.0, 10.0), Sense::hover());
                ui.painter().rect_filled(rect, 2.0, color);
                ui.add(egui::Label::new(&model.id).selectable(false));
            });
        })
        .response;
    // egui counts a click soon after a click somewhere else as a triple
    // click, so both counts add the node.
    let double_click = response.hovered()
        && ui.input(|i| {
            i.pointer
                .button_double_clicked(egui::PointerButton::Primary)
                || i.pointer
                    .button_triple_clicked(egui::PointerButton::Primary)
        });
    if double_click {
        actions.push(AppAction::Edit(
            view,
            EditOp::AddNode {
                type_id: model.id.clone(),
                target: None,
            },
        ));
    }
    let response = response.on_hover_ui(|ui| model_tooltip(ui, model, source, tree_keys(model)));
    response.context_menu(|ui| {
        if ui.button("Add to the selected node").clicked() {
            actions.push(AppAction::Edit(
                view,
                EditOp::AddNode {
                    type_id: model.id.clone(),
                    target: None,
                },
            ));
            ui.close();
        }
        if ui.button("Add as unconnected node").clicked() {
            actions.push(AppAction::Edit(
                view,
                EditOp::AddUnconnected {
                    type_id: model.id.clone(),
                },
            ));
            ui.close();
        }
        ui.separator();
        let editable = is_editable(source);
        if ui
            .add_enabled(editable, egui::Button::new("Edit model…"))
            .on_disabled_hover_text("Built-in models and models of other files are read-only.")
            .clicked()
        {
            actions.push(AppAction::EditModel {
                view,
                model: Some(model.id.clone()),
            });
            ui.close();
        }
        if ui
            .add_enabled(editable, egui::Button::new("Delete model"))
            .clicked()
        {
            actions.push(AppAction::Edit(view, EditOp::RemoveModel(model.id.clone())));
            ui.close();
        }
    });
}

/// The tooltip of a palette entry: the kind, the source, the description,
/// all ports and, for a tree, the blackboard keys that it uses.
fn model_tooltip(ui: &mut Ui, model: &NodeModel, source: ModelSource, tree_keys: Vec<String>) {
    ui.set_max_width(420.0);
    ui.label(RichText::new(&model.id).strong());
    ui.label(
        RichText::new(format!(
            "{}  ·  {}",
            model.kind.tag(),
            match source {
                ModelSource::Builtin => "built-in",
                ModelSource::File(_) => "model in this project",
                ModelSource::ProjectModels(_) => "project model file",
                ModelSource::InferredSubTree(_) => "tree of this project",
                ModelSource::Unattached => "model",
            }
        ))
        .weak(),
    );
    if let Some(description) = model.description() {
        ui.label(description);
    }
    let keys: Vec<&String> = tree_keys
        .iter()
        .filter(|key| model.port(key).is_none())
        .collect();
    if model.ports.is_empty() && keys.is_empty() {
        ui.label(RichText::new("No ports").weak());
        return;
    }
    if !model.ports.is_empty() {
        ui.separator();
        egui::Grid::new(("palette_tip", &model.id))
            .num_columns(3)
            .spacing([8.0, 2.0])
            .show(ui, |ui| {
                for port in &model.ports {
                    ui.label(RichText::new(port.direction.tag()).weak());
                    let mut name = RichText::new(&port.name).monospace();
                    if model.is_port_required(port) {
                        name = name.strong();
                    }
                    ui.label(name);
                    let mut details = port
                        .type_name
                        .as_deref()
                        .map(crate::panels::node_editor::short_type)
                        .unwrap_or_default();
                    if let Some(default) = port.default.as_deref().filter(|d| !d.is_empty()) {
                        if !details.is_empty() {
                            details.push_str(", ");
                        }
                        details.push_str(&format!("default {default}"));
                    }
                    ui.label(RichText::new(details).weak());
                    ui.end_row();
                    if let Some(description) = &port.description {
                        ui.label("");
                        ui.label(RichText::new(description).small());
                        ui.label("");
                        ui.end_row();
                    }
                }
            });
    }
    if !keys.is_empty() {
        ui.separator();
        ui.label(
            RichText::new("Blackboard keys of the tree (connect them on the SubTree node):")
                .small()
                .weak(),
        );
        ui.label(
            RichText::new(
                keys.iter()
                    .map(|k| k.as_str())
                    .collect::<Vec<_>>()
                    .join(", "),
            )
            .monospace(),
        );
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use fasttick_core::models::{NodeKind, PortDirection, PortModel};

    #[test]
    fn search_matches_ids_and_ports() {
        let mut model = NodeModel::new("MoveBase", NodeKind::Action);
        model
            .ports
            .push(PortModel::new("goal", PortDirection::Input));
        assert!(matches(&model, ""));
        assert!(matches(&model, "move"));
        assert!(matches(&model, " BASE "));
        assert!(matches(&model, "goa"));
        assert!(!matches(&model, "sequence"));
    }
}
