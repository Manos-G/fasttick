//! The inspector of an edit view: it edits the selected node.
//!
//! Each change goes to the app as an [`EditOp`]. Text fields send one
//! operation for each keystroke; the editor joins the changes of one field
//! into one undo step (see [`crate::editor::COALESCE_WINDOW`]).

use egui::{Color32, RichText, Ui};
use fasttick_core::bt::{
    AttributeClass, NodeView, PostCondition, PreCondition, blackboard_pointer,
};
use fasttick_core::edit::Condition;
use fasttick_core::models::{ModelSource, NodeKind, NodeModel, PortDirection};
use fasttick_core::project::FileId;
use fasttick_core::runtime::InstanceIndex;
use fasttick_core::validate::{Problem, Severity};
use fasttick_core::xml::NodeId;

use crate::actions::AppAction;
use crate::canvas::condition_frame;
use crate::documents::ViewId;
use crate::edit_ops::EditOp;
use crate::editor::EditorDoc;
use crate::theme::CanvasPalette;

/// Draws the editor of an element (file and node) of `editor`. `selected`
/// is the number of selected nodes.
pub fn show(
    ui: &mut Ui,
    view: ViewId,
    editor: &EditorDoc,
    (file_id, node): (FileId, NodeId),
    selected: usize,
    palette: &CanvasPalette,
    actions: &mut Vec<AppAction>,
) {
    let Some(file) = editor.project().file(file_id) else {
        return;
    };
    let Some(element) = NodeView::new(file.doc(), node) else {
        ui.label(RichText::new("The node is not in the file any more.").weak());
        return;
    };
    let type_id = element.type_id().unwrap_or("").to_owned();
    let model = editor.project().lookup_model(file_id, &type_id);
    let node_problems: Vec<&Problem> = editor
        .problems()
        .iter()
        .filter(|p| p.file == file_id && p.node == Some(node))
        .collect();
    let mut edit = |op: EditOp| actions.push(AppAction::Edit(view, op));

    // Header.
    let title = element.instance_name().unwrap_or(&type_id).to_owned();
    ui.label(RichText::new(&title).strong().size(15.0));
    let kind = model
        .map(|(m, _)| m.kind.tag())
        .or_else(|| element.category_tag().map(NodeKind::tag))
        .unwrap_or("unknown kind");
    ui.label(RichText::new(format!("{type_id}  ·  {kind}")).weak());
    let description = match element.subtree_id() {
        Some(tree_id) => editor
            .project()
            .lookup_model(file_id, tree_id)
            .and_then(|(m, _)| m.description())
            .or_else(|| fasttick_core::builtin::description("SubTree")),
        None => model.and_then(|(m, _)| m.description()),
    };
    if let Some(description) = description {
        ui.label(RichText::new(description).small());
    }
    if selected > 1 {
        ui.label(
            RichText::new(format!(
                "{selected} nodes are selected. The inspector edits the primary node."
            ))
            .small()
            .weak(),
        );
    }
    match model {
        Some((_, source)) => {
            ui.label(RichText::new(source_text(source)).small().weak());
        }
        None => {
            ui.label(
                RichText::new("No model is known for this type. Add one in the palette.")
                    .small()
                    .color(ui.visuals().warn_fg_color),
            );
        }
    }
    ui.add_space(6.0);

    egui::Grid::new(("node_editor_main", node))
        .num_columns(2)
        .spacing([10.0, 6.0])
        .show(ui, |ui| {
            let label = ui.label("Name").id;
            let current = element.name_attribute().unwrap_or("");
            if let Some(text) = text_field(ui, label, ("name", node), current, &type_id) {
                edit(EditOp::SetName {
                    node,
                    name: (!text.is_empty()).then_some(text),
                });
            }
            ui.end_row();
            inline_problems(ui, &node_problems, "name");

            ui.label("Type");
            if let Some(new_type) =
                type_combo(ui, editor, file_id, node, &type_id, model.map(|(m, _)| m))
            {
                edit(EditOp::ChangeType {
                    node,
                    type_id: new_type,
                });
            }
            ui.end_row();

            if element.is_subtree() {
                ui.label("Remapping");
                let mut auto = element.autoremap().and_then(Result::ok).unwrap_or(false);
                if ui
                    .checkbox(&mut auto, "Automatic (_autoremap)")
                    .on_hover_text("Connect the ports of the tree to the keys with the same names.")
                    .changed()
                {
                    edit(EditOp::SetAutoremap {
                        node,
                        value: auto.then_some(true),
                    });
                }
                ui.end_row();
            }
        });

    if let Some(tree_id) = element.subtree_id() {
        let index = editor.tree_index(tree_id);
        if ui
            .add_enabled(
                index.is_some(),
                egui::Button::new(format!("Open tree {tree_id}")),
            )
            .on_disabled_hover_text("The project has no tree with this ID.")
            .clicked()
            && let Some(index) = index
        {
            actions.push(AppAction::SetViewRoot(view, InstanceIndex(index)));
        }
    }
    let mut edit = |op: EditOp| actions.push(AppAction::Edit(view, op));

    // Ports.
    ui.add_space(8.0);
    ui.label(
        RichText::new(if element.is_subtree() {
            "Ports and remappings"
        } else {
            "Ports"
        })
        .strong(),
    );
    let tree_keys = element
        .subtree_id()
        .map(|tree_id| editor.project().tree_keys(file_id, tree_id))
        .unwrap_or_default();
    ports(
        ui,
        &element,
        model,
        &tree_keys,
        node,
        &node_problems,
        &mut edit,
    );

    // Conditions.
    ui.add_space(8.0);
    let condition_count = element.pre_conditions().count() + element.post_conditions().count();
    let heading = match condition_count {
        0 => "Pre- and post-conditions".to_owned(),
        1 => "Pre- and post-conditions (1 set)".to_owned(),
        n => format!("Pre- and post-conditions ({n} set)"),
    };
    egui::CollapsingHeader::new(RichText::new(heading).strong().color(palette.condition))
        .id_salt(("conditions", node))
        .default_open(condition_count > 0)
        .show(ui, |ui| {
            condition_frame(palette).show(ui, |ui| {
                let pre = PreCondition::ALL
                    .into_iter()
                    .map(|c| (Condition::Pre(c), element.pre_condition(c), pre_help(c)));
                let post = PostCondition::ALL
                    .into_iter()
                    .map(|c| (Condition::Post(c), element.post_condition(c), post_help(c)));
                let label_width = CONDITION_LABEL_WIDTH;
                for (heading, rows) in [
                    (
                        "Pre-conditions: checked before each tick",
                        pre.collect::<Vec<_>>(),
                    ),
                    (
                        "Post-conditions: run after the node has a result",
                        post.collect::<Vec<_>>(),
                    ),
                ] {
                    ui.label(RichText::new(heading).small().weak());
                    for (condition, script, help) in rows {
                        let name = condition.attribute_name();
                        ui.horizontal_top(|ui| {
                            let mut text = RichText::new(name).monospace().color(palette.condition);
                            if script.is_some() {
                                text = text.strong();
                            }
                            let label = ui
                                .add_sized([label_width, FIELD_ROW_HEIGHT], egui::Label::new(text))
                                .on_hover_text(help)
                                .id;
                            if let Some(text) = value_field(
                                ui,
                                label,
                                ("cond", node, name),
                                script.unwrap_or(""),
                                "script",
                            ) {
                                edit(EditOp::SetCondition {
                                    node,
                                    condition,
                                    script: (!text.is_empty()).then_some(text),
                                });
                            }
                        });
                        inline_problems(ui, &node_problems, name);
                    }
                    ui.add_space(4.0);
                }
            })
        });

    // The other problems of the node (the problems of a field show below
    // the field).
    let problems: Vec<&Problem> = node_problems
        .iter()
        .copied()
        .filter(|p| {
            p.attribute
                .as_deref()
                .is_none_or(|a| !has_field(&element, model.map(|(m, _)| m), a))
        })
        .collect();
    if !problems.is_empty() {
        ui.add_space(8.0);
        ui.label(RichText::new("Problems").strong());
        for problem in problems {
            let color = severity_color(ui, problem.severity);
            ui.horizontal_wrapped(|ui| {
                severity_badge(ui, problem.severity, color);
                ui.label(&problem.message);
            });
        }
    }
}

/// True if the inspector has a field for this attribute: the name, a
/// condition, a port of the model or a port attribute of the element.
fn has_field(element: &NodeView<'_>, model: Option<&NodeModel>, attribute: &str) -> bool {
    match AttributeClass::of(attribute) {
        AttributeClass::Name
        | AttributeClass::PreCondition(_)
        | AttributeClass::PostCondition(_) => true,
        AttributeClass::Port => {
            element.attribute(attribute).is_some()
                || model.is_some_and(|m| m.port(attribute).is_some())
        }
        _ => false,
    }
}

/// The problems of an attribute, below its field.
fn inline_problems(ui: &mut Ui, problems: &[&Problem], attribute: &str) {
    for problem in problems
        .iter()
        .filter(|p| p.attribute.as_deref() == Some(attribute))
    {
        let color = severity_color(ui, problem.severity);
        ui.horizontal_wrapped(|ui| {
            severity_badge(ui, problem.severity, color);
            ui.label(RichText::new(&problem.message).small().color(color));
        });
    }
}

/// A single-line text field. Returns the new text if the user changed it.
fn text_field(
    ui: &mut Ui,
    label: egui::Id,
    id_salt: impl std::hash::Hash + std::fmt::Debug,
    current: &str,
    hint: &str,
) -> Option<String> {
    let mut text = current.to_owned();
    let response = ui
        .add(
            egui::TextEdit::singleline(&mut text)
                .id_salt(id_salt)
                .hint_text(hint)
                .desired_width(f32::INFINITY),
        )
        .labelled_by(label);
    (response.changed() && text != current).then_some(text)
}

/// The width of the condition names.
const CONDITION_LABEL_WIDTH: f32 = 82.0;

/// The height of one line of a value field.
const FIELD_ROW_HEIGHT: f32 = 24.0;

/// A wide, monospace field for a port value or a script. A long value
/// continues on the next lines; the field grows to show all of it. Enter
/// ends the edit (it does not add a new line). Returns the new text if the
/// user changed it.
fn value_field(
    ui: &mut Ui,
    label: egui::Id,
    id_salt: impl std::hash::Hash + std::fmt::Debug,
    current: &str,
    hint: &str,
) -> Option<String> {
    let mut text = current.to_owned();
    let response = ui
        .add(
            egui::TextEdit::multiline(&mut text)
                .id_salt(id_salt)
                .hint_text(hint)
                .font(egui::TextStyle::Monospace)
                .desired_rows(1)
                .return_key(None)
                .margin(egui::vec2(6.0, 5.0))
                .desired_width(ui.available_width()),
        )
        .labelled_by(label);
    if response.has_focus() && ui.input(|i| i.key_pressed(egui::Key::Enter)) {
        response.surrender_focus();
    }
    (response.changed() && text != current).then_some(text)
}

/// The type selector: the types of the same kind. Returns the new type.
fn type_combo(
    ui: &mut Ui,
    editor: &EditorDoc,
    file: FileId,
    node: NodeId,
    type_id: &str,
    model: Option<&NodeModel>,
) -> Option<String> {
    let mut picked = None;
    egui::ComboBox::from_id_salt(("type", node))
        .selected_text(type_id)
        .height(320.0)
        .show_ui(ui, |ui| {
            let all = editor.project().registry().all_models(file);
            let kinds: Vec<NodeKind> = match model {
                Some(model) => vec![model.kind],
                None => NodeKind::ALL.to_vec(),
            };
            for kind in kinds {
                let Some(models) = all.get(&kind) else {
                    continue;
                };
                if model.is_none() {
                    ui.label(RichText::new(crate::edit_menu::kind_group_name(kind)).weak());
                }
                for m in models {
                    if ui.selectable_label(m.id == type_id, &m.id).clicked() && m.id != type_id {
                        picked = Some(m.id.clone());
                    }
                }
            }
        });
    picked
}

/// The rows of the ports: the ports of the model first (in model order),
/// then the other port attributes of the element.
fn ports(
    ui: &mut Ui,
    element: &NodeView<'_>,
    model: Option<(&NodeModel, ModelSource)>,
    tree_keys: &[String],
    node: NodeId,
    problems: &[&Problem],
    edit: &mut dyn FnMut(EditOp),
) {
    // A tree without a declared model accepts all remappings.
    let declared =
        model.is_some_and(|(_, source)| !matches!(source, ModelSource::InferredSubTree(_)));
    let model = model.map(|(m, _)| m);
    let model_ports = model.map(|m| m.ports.as_slice()).unwrap_or_default();
    let extra: Vec<(String, String)> = element
        .attributes()
        .filter(|a| a.class == AttributeClass::Port)
        .filter(|a| !model_ports.iter().any(|p| p.name == a.name))
        .map(|a| (a.name.to_owned(), a.value.to_owned()))
        .collect();
    if model_ports.is_empty() && extra.is_empty() {
        ui.label(RichText::new("This node has no ports.").weak());
    }
    for port in model_ports {
        let required = model.is_some_and(|m| m.is_port_required(port));
        let mut label = RichText::new(&port.name).monospace();
        if required {
            label = label.strong();
        }
        let mut tip = format!("{} port", direction_text(port.direction));
        if let Some(type_name) = &port.type_name {
            tip.push_str(&format!(" of type {type_name}"));
        }
        if required {
            tip.push_str(". Required: it has no default value");
        }
        if let Some(default) = &port.default {
            tip.push_str(&format!(". Default: \"{default}\""));
        }
        if let Some(description) = &port.description {
            tip.push_str(&format!(".\n{description}"));
        }
        let value = element.attribute(&port.name);
        let label_id = port_header(ui, node, &port.name, value, edit, |ui| {
            ui.label(RichText::new(direction_tag(port.direction)).small().weak())
                .on_hover_text(direction_text(port.direction));
            let id = ui.label(label).on_hover_text(&tip).id;
            if required {
                ui.label(RichText::new("*").color(ui.visuals().warn_fg_color))
                    .on_hover_text("Required");
            }
            if let Some(type_name) = &port.type_name {
                ui.label(RichText::new(short_type(type_name)).small().weak())
                    .on_hover_text(type_name);
            }
            id
        });
        if let Some(description) = &port.description {
            ui.label(RichText::new(description).small().weak());
        }
        let hint = match &port.default {
            Some(default) if default.is_empty() => "(empty default)".to_owned(),
            Some(default) => default.clone(),
            None if port.direction == PortDirection::Output => "{key}".to_owned(),
            None => "required".to_owned(),
        };
        port_field(ui, label_id, node, &port.name, value, &hint, edit);
        inline_problems(ui, problems, &port.name);
        ui.add_space(6.0);
    }
    for (name, value) in &extra {
        let label_id = port_header(ui, node, name, Some(value), edit, |ui| {
            let id = ui.label(RichText::new(name).monospace()).id;
            if tree_keys.contains(name) {
                ui.label(RichText::new("key of the tree").small().weak());
            } else if declared {
                ui.label(RichText::new("?").color(ui.visuals().warn_fg_color))
                    .on_hover_text("The model of this type has no port with this name.");
            }
            id
        });
        port_field(ui, label_id, node, name, Some(value), "", edit);
        inline_problems(ui, problems, name);
        ui.add_space(6.0);
    }
    // The keys of the tree of a SubTree that the node does not connect.
    let autoremap = element.autoremap().and_then(Result::ok).unwrap_or(false);
    for key in tree_keys {
        if element.attribute(key).is_some() || model.is_some_and(|m| m.port(key).is_some()) {
            continue;
        }
        let label_id = port_header(ui, node, key, None, edit, |ui| {
            let id = ui.label(RichText::new(key).monospace()).id;
            ui.label(RichText::new("key of the tree").small().weak())
                .on_hover_text(
                    "The tree uses this blackboard key. Type a value or {key} of this \
                     tree to connect it.",
                );
            id
        });
        let hint = if autoremap {
            format!("{{{key}}} (automatic)")
        } else {
            "not connected".to_owned()
        };
        port_field(ui, label_id, node, key, None, &hint, edit);
        ui.add_space(6.0);
    }
    add_port_row(ui, node, edit);
}

/// The line above a port field: the name (from `name_ui`, which returns
/// the ID of the name label), then at the right the reference toggle and
/// the remove button. Returns the ID of the name label.
fn port_header(
    ui: &mut Ui,
    node: NodeId,
    port: &str,
    value: Option<&str>,
    edit: &mut dyn FnMut(EditOp),
    name_ui: impl FnOnce(&mut Ui) -> egui::Id,
) -> egui::Id {
    ui.horizontal(|ui| {
        ui.spacing_mut().item_spacing.x = 4.0;
        let id = name_ui(ui);
        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
            if ui
                .add_enabled(value.is_some(), egui::Button::new("×").small())
                .on_hover_text("Remove the attribute (use the default value)")
                .clicked()
            {
                edit(EditOp::SetPort {
                    node,
                    port: port.to_owned(),
                    value: None,
                });
            }
            let current = value.unwrap_or("");
            let is_reference = blackboard_pointer(current).is_some();
            let toggle = ui
                .selectable_label(is_reference, RichText::new("{ }").monospace())
                .on_hover_text(if is_reference {
                    "The value is a blackboard key. Click to use a literal value."
                } else {
                    "Click to use a blackboard key."
                });
            if toggle.clicked() {
                edit(EditOp::SetPort {
                    node,
                    port: port.to_owned(),
                    value: Some(toggle_reference(current, port)),
                });
            }
        });
        id
    })
    .inner
}

/// The value field of a port, the full width of the panel.
fn port_field(
    ui: &mut Ui,
    label: egui::Id,
    node: NodeId,
    port: &str,
    value: Option<&str>,
    hint: &str,
    edit: &mut dyn FnMut(EditOp),
) {
    if let Some(text) = value_field(ui, label, ("port", node, port), value.unwrap_or(""), hint) {
        edit(EditOp::SetPort {
            node,
            port: port.to_owned(),
            value: Some(text),
        });
    }
}

/// A short form of a C++ type name: `std::vector<int, std::allocator<int> >`
/// becomes `vector<int>`, `std::string` becomes `string`.
pub fn short_type(type_name: &str) -> String {
    let without_std = type_name.replace("std::", "");
    // Remove the allocator argument of the standard containers.
    let mut out = String::with_capacity(without_std.len());
    let mut rest = without_std.as_str();
    while let Some(start) = rest.find(", allocator<") {
        out.push_str(&rest[..start]);
        let tail = &rest[start + 2..];
        // Skip to the end of the allocator argument.
        let mut depth = 0;
        let mut end = tail.len();
        for (i, c) in tail.char_indices() {
            match c {
                '<' => depth += 1,
                '>' => {
                    depth -= 1;
                    if depth == 0 {
                        end = i + 1;
                        break;
                    }
                }
                _ => {}
            }
        }
        rest = &tail[end..];
    }
    out.push_str(rest);
    out.replace(" >", ">")
}

/// Changes a literal value to a blackboard reference and back. An empty
/// value becomes a reference to a key with the port name.
pub fn toggle_reference(value: &str, port: &str) -> String {
    match blackboard_pointer(value) {
        Some(key) => key.to_owned(),
        None if value.is_empty() => format!("{{{port}}}"),
        None => format!("{{{value}}}"),
    }
}

/// A row to add a port attribute that is not in the model.
fn add_port_row(ui: &mut Ui, node: NodeId, edit: &mut dyn FnMut(EditOp)) {
    let id = ui.id().with(("new_port", node));
    let mut name: String = ui.data_mut(|d| d.get_temp(id).unwrap_or_default());
    ui.horizontal(|ui| {
        ui.add(
            egui::TextEdit::singleline(&mut name)
                .id_salt(("new_port_name", node))
                .hint_text("new port")
                .desired_width(110.0),
        );
        let valid = fasttick_core::bt::names::is_allowed_port_name(name.trim())
            && AttributeClass::of(name.trim()) == AttributeClass::Port;
        if ui
            .add_enabled(valid, egui::Button::new("Add port"))
            .on_disabled_hover_text("Type a port name that BT.CPP accepts.")
            .clicked()
        {
            edit(EditOp::SetPort {
                node,
                port: name.trim().to_owned(),
                value: Some(String::new()),
            });
            name.clear();
        }
    });
    ui.data_mut(|d| d.insert_temp(id, name));
}

fn source_text(source: ModelSource) -> &'static str {
    match source {
        ModelSource::Builtin => "Built-in model of BT.CPP",
        ModelSource::File(_) => "Model from a <TreeNodesModel> of the project",
        ModelSource::ProjectModels(_) => "Model from a project model file",
        ModelSource::InferredSubTree(_) => "Tree of the project (no declared ports)",
        ModelSource::Unattached => "Model",
    }
}

fn direction_text(direction: PortDirection) -> &'static str {
    match direction {
        PortDirection::Input => "Input",
        PortDirection::Output => "Output",
        PortDirection::InOut => "In/out",
    }
}

/// A short tag of a port direction. (Roboto has few symbols, so the UI
/// uses words; see `crate::fonts`.)
fn direction_tag(direction: PortDirection) -> &'static str {
    match direction {
        PortDirection::Input => "in",
        PortDirection::Output => "out",
        PortDirection::InOut => "in/out",
    }
}

/// The help text of a condition attribute name (for example `_skipIf`).
pub fn condition_help(name: &str) -> &'static str {
    if let Some(pre) = PreCondition::from_attribute_name(name) {
        pre_help(pre)
    } else if let Some(post) = PostCondition::from_attribute_name(name) {
        post_help(post)
    } else {
        ""
    }
}

fn pre_help(condition: PreCondition) -> &'static str {
    match condition {
        PreCondition::FailureIf => {
            "If the script is true, the node returns FAILURE without a tick."
        }
        PreCondition::SuccessIf => {
            "If the script is true, the node returns SUCCESS without a tick."
        }
        PreCondition::SkipIf => "If the script is true, the node is skipped.",
        PreCondition::While => {
            "If the script is false, the node is skipped (or halted if it is running)."
        }
    }
}

fn post_help(condition: PostCondition) -> &'static str {
    match condition {
        PostCondition::OnHalted => "Runs after the node is halted.",
        PostCondition::OnFailure => "Runs after the node returns FAILURE.",
        PostCondition::OnSuccess => "Runs after the node returns SUCCESS.",
        PostCondition::Post => "Runs after each completion (SUCCESS, FAILURE or halt).",
    }
}

/// Paints the badge of a severity: a filled circle for an error, a
/// filled triangle for a warning and a ring for a note. The painter draws
/// it, because the bundled font has no symbol glyphs (see `crate::fonts`).
pub fn severity_badge(ui: &mut Ui, severity: Severity, color: Color32) -> egui::Response {
    let size = ui.text_style_height(&egui::TextStyle::Body).min(14.0);
    let (rect, response) = ui.allocate_exact_size(egui::vec2(size, size), egui::Sense::hover());
    let c = rect.center();
    let r = size * 0.38;
    let painter = ui.painter();
    match severity {
        Severity::Error => {
            painter.circle_filled(c, r, color);
        }
        Severity::Warning => {
            let h = r * 1.15;
            painter.add(egui::Shape::convex_polygon(
                vec![
                    egui::pos2(c.x, c.y - h),
                    egui::pos2(c.x + h, c.y + h * 0.8),
                    egui::pos2(c.x - h, c.y + h * 0.8),
                ],
                color,
                egui::Stroke::NONE,
            ));
        }
        Severity::Info => {
            painter.circle_stroke(c, r * 0.9, egui::Stroke::new(1.5, color));
        }
    }
    response.on_hover_text(match severity {
        Severity::Error => "Error",
        Severity::Warning => "Warning",
        Severity::Info => "Note",
    })
}

/// The colour of a severity.
pub fn severity_color(ui: &Ui, severity: Severity) -> Color32 {
    match severity {
        Severity::Error => ui.visuals().error_fg_color,
        Severity::Warning => ui.visuals().warn_fg_color,
        Severity::Info => ui.visuals().weak_text_color(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn short_type_names() {
        assert_eq!(
            short_type("std::vector<int, std::allocator<int> >"),
            "vector<int>"
        );
        assert_eq!(short_type("std::string"), "string");
        assert_eq!(short_type("double"), "double");
        assert_eq!(
            short_type("std::map<std::string, std::vector<int, std::allocator<int> > >"),
            "map<string, vector<int>>"
        );
    }

    #[test]
    fn reference_toggle() {
        assert_eq!(toggle_reference("goal", "target"), "{goal}");
        assert_eq!(toggle_reference("{goal}", "target"), "goal");
        assert_eq!(toggle_reference("", "target"), "{target}");
        assert_eq!(toggle_reference("{=}", "target"), "=");
    }
}
