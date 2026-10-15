//! The dialog that creates or changes a node model (an entry of the
//! `<TreeNodesModel>` of the open file).

use egui::{Align2, RichText};
use fasttick_core::bt::names::{validate_model_name, validate_port_name};
use fasttick_core::models::{ModelSource, NodeKind, NodeModel, PortDirection, PortModel};

use crate::dialogs::PromptChoice;
use crate::documents::ViewId;
use crate::edit_ops::EditOp;

/// One port row of the form.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PortForm {
    /// The port name.
    pub name: String,
    /// The direction.
    pub direction: PortDirection,
    /// The type name (empty: no type).
    pub type_name: String,
    /// True if the port has a default value.
    pub has_default: bool,
    /// The default value.
    pub default: String,
    /// The description (empty: none).
    pub description: String,
}

impl PortForm {
    fn new() -> Self {
        PortForm {
            name: String::new(),
            direction: PortDirection::Input,
            type_name: String::new(),
            has_default: false,
            default: String::new(),
            description: String::new(),
        }
    }
}

/// The state of the model dialog.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ModelForm {
    /// The edit view.
    pub view: ViewId,
    /// The ID of the model that the form changes (`None` for a new model).
    pub old_id: Option<String>,
    /// The model ID.
    pub id: String,
    /// The kind.
    pub kind: NodeKind,
    /// The ports.
    pub ports: Vec<PortForm>,
    /// The metadata (kept as it is).
    pub metadata: Vec<(String, String)>,
    /// The IDs of the other models that the file can see.
    pub taken: Vec<String>,
}

impl ModelForm {
    /// A form for a new Action model.
    pub fn new_model(view: ViewId, taken: Vec<String>) -> Self {
        ModelForm {
            view,
            old_id: None,
            id: String::new(),
            kind: NodeKind::Action,
            ports: Vec::new(),
            metadata: Vec::new(),
            taken,
        }
    }

    /// A form that changes a model.
    pub fn edit_model(view: ViewId, model: &NodeModel, taken: Vec<String>) -> Self {
        ModelForm {
            view,
            old_id: Some(model.id.clone()),
            id: model.id.clone(),
            kind: model.kind,
            ports: model
                .ports
                .iter()
                .map(|p| PortForm {
                    name: p.name.clone(),
                    direction: p.direction,
                    type_name: p.type_name.clone().unwrap_or_default(),
                    has_default: p.default.is_some(),
                    default: p.default.clone().unwrap_or_default(),
                    description: p.description.clone().unwrap_or_default(),
                })
                .collect(),
            metadata: model.metadata.clone(),
            taken: taken.into_iter().filter(|t| t != &model.id).collect(),
        }
    }

    /// The model of the form.
    pub fn to_model(&self) -> NodeModel {
        let non_empty = |s: &str| (!s.trim().is_empty()).then(|| s.trim().to_owned());
        NodeModel {
            id: self.id.trim().to_owned(),
            kind: self.kind,
            ports: self
                .ports
                .iter()
                .map(|p| PortModel {
                    name: p.name.trim().to_owned(),
                    direction: p.direction,
                    type_name: non_empty(&p.type_name),
                    default: p.has_default.then(|| p.default.clone()),
                    description: non_empty(&p.description),
                })
                .collect(),
            metadata: self.metadata.clone(),
            source: ModelSource::Unattached,
        }
    }

    /// The first problem of the form, if any.
    pub fn problem(&self) -> Option<String> {
        let id = self.id.trim();
        if let Err(err) = validate_model_name(id) {
            return Some(format!("Not a valid ID: {err}."));
        }
        if self.taken.iter().any(|t| t == id) {
            return Some(format!("A model with the ID {id} exists."));
        }
        let mut seen = Vec::new();
        for port in &self.ports {
            let name = port.name.trim();
            if let Err(err) = validate_port_name(name) {
                return Some(format!("Port \"{name}\": {err}."));
            }
            if seen.contains(&name) {
                return Some(format!("Two ports have the name {name}."));
            }
            seen.push(name);
        }
        None
    }

    /// The edit operation that applies the form.
    pub fn op(&self) -> EditOp {
        let model = self.to_model();
        match &self.old_id {
            None => EditOp::AddModel(model),
            Some(old_id) => EditOp::ReplaceModel {
                old_id: old_id.clone(),
                model,
            },
        }
    }
}

fn direction_label(direction: PortDirection) -> &'static str {
    match direction {
        PortDirection::Input => "input",
        PortDirection::Output => "output",
        PortDirection::InOut => "in/out",
    }
}

/// Draws the model dialog.
pub fn show(ctx: &egui::Context, form: &mut ModelForm) -> PromptChoice {
    let mut choice = PromptChoice::Open;
    let mut open = true;
    let problem = form.problem();
    let title = match &form.old_id {
        None => "New node model".to_owned(),
        Some(id) => format!("Node model {id}"),
    };
    egui::Window::new(title)
        .collapsible(false)
        .resizable(true)
        .default_width(640.0)
        .anchor(Align2::CENTER_CENTER, [0.0, -40.0])
        .open(&mut open)
        .show(ctx, |ui| {
            egui::Grid::new("model_form")
                .num_columns(2)
                .spacing([10.0, 6.0])
                .show(ui, |ui| {
                    let label = ui.label("ID").id;
                    ui.add(
                        egui::TextEdit::singleline(&mut form.id)
                            .hint_text("MyAction")
                            .desired_width(240.0),
                    )
                    .labelled_by(label);
                    ui.end_row();
                    ui.label("Kind");
                    egui::ComboBox::from_id_salt("model_kind")
                        .selected_text(form.kind.tag())
                        .show_ui(ui, |ui| {
                            for kind in NodeKind::ALL {
                                ui.selectable_value(&mut form.kind, kind, kind.tag());
                            }
                        });
                    ui.end_row();
                });
            ui.add_space(6.0);
            ui.label(RichText::new("Ports").strong());
            let mut remove = None;
            egui::Grid::new("model_ports")
                .num_columns(7)
                .spacing([6.0, 4.0])
                .show(ui, |ui| {
                    for title in [
                        "Name",
                        "Direction",
                        "Type",
                        "Default",
                        "",
                        "Description",
                        "",
                    ] {
                        ui.label(RichText::new(title).weak());
                    }
                    ui.end_row();
                    for (index, port) in form.ports.iter_mut().enumerate() {
                        ui.add(
                            egui::TextEdit::singleline(&mut port.name)
                                .id_salt(("port_name", index))
                                .hint_text("port name")
                                .desired_width(110.0),
                        );
                        egui::ComboBox::from_id_salt(("port_dir", index))
                            .selected_text(direction_label(port.direction))
                            .width(80.0)
                            .show_ui(ui, |ui| {
                                for d in [
                                    PortDirection::Input,
                                    PortDirection::Output,
                                    PortDirection::InOut,
                                ] {
                                    ui.selectable_value(&mut port.direction, d, direction_label(d));
                                }
                            });
                        ui.add(
                            egui::TextEdit::singleline(&mut port.type_name)
                                .id_salt(("port_type", index))
                                .hint_text("std::string")
                                .desired_width(100.0),
                        );
                        ui.checkbox(&mut port.has_default, "")
                            .on_hover_text("The port has a default value");
                        ui.add_enabled(
                            port.has_default,
                            egui::TextEdit::singleline(&mut port.default)
                                .id_salt(("port_default", index))
                                .desired_width(80.0),
                        );
                        ui.add(
                            egui::TextEdit::singleline(&mut port.description)
                                .id_salt(("port_desc", index))
                                .desired_width(150.0),
                        );
                        if ui
                            .small_button("×")
                            .on_hover_text("Remove the port")
                            .clicked()
                        {
                            remove = Some(index);
                        }
                        ui.end_row();
                    }
                });
            if let Some(index) = remove {
                form.ports.remove(index);
            }
            if ui.button("Add port").clicked() {
                form.ports.push(PortForm::new());
            }
            if form.old_id.is_some() {
                ui.label(
                    RichText::new("A new ID also changes the nodes of this type in the open file.")
                        .small()
                        .weak(),
                );
            }
            if let Some(problem) = &problem {
                ui.colored_label(ui.visuals().warn_fg_color, problem);
            }
            ui.add_space(8.0);
            ui.horizontal(|ui| {
                if ui
                    .add_enabled(
                        problem.is_none(),
                        egui::Button::new(RichText::new("OK").strong()),
                    )
                    .clicked()
                {
                    choice = PromptChoice::Accept;
                }
                if ui.button("Cancel").clicked() {
                    choice = PromptChoice::Cancel;
                }
            });
        });
    if !open || ctx.input(|i| i.key_pressed(egui::Key::Escape)) {
        choice = PromptChoice::Cancel;
    }
    choice
}

#[cfg(test)]
mod tests {
    use super::*;

    fn view() -> ViewId {
        ViewId(1)
    }

    #[test]
    fn new_model_needs_a_valid_unique_id() {
        let mut form = ModelForm::new_model(view(), vec!["Sequence".into()]);
        assert!(form.problem().is_some());
        form.id = "Sequence".into();
        assert!(form.problem().unwrap().contains("exists"));
        form.id = "MoveBase".into();
        assert_eq!(form.problem(), None);
        form.ports.push(PortForm::new());
        assert!(form.problem().is_some(), "an empty port name");
        form.ports[0].name = "goal".into();
        form.ports.push(form.ports[0].clone());
        assert!(form.problem().unwrap().contains("Two ports"));
        form.ports.pop();
        assert_eq!(form.op(), EditOp::AddModel(form.to_model()));
    }

    #[test]
    fn edit_model_round_trip() {
        let mut model = NodeModel::new("MoveBase", NodeKind::Action);
        let mut port = PortModel::new("goal", PortDirection::Input);
        port.type_name = Some("Pose2D".into());
        port.default = Some(String::new());
        model.ports.push(port);
        model.metadata.push(("description".into(), "Moves".into()));
        let form = ModelForm::edit_model(view(), &model, vec!["MoveBase".into()]);
        assert_eq!(form.problem(), None, "its own ID is not taken");
        let back = form.to_model();
        assert_eq!(back.ports, model.ports);
        assert_eq!(back.metadata, model.metadata);
        assert!(matches!(form.op(), EditOp::ReplaceModel { old_id, .. } if old_id == "MoveBase"));
    }
}
