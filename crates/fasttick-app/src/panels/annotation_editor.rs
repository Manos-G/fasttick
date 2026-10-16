//! The inspector of a selected group or canvas note (edit mode).

use egui::{RichText, Ui};
use fasttick_core::bt::annotations::ANNOTATION_COLORS;

use crate::actions::AppAction;
use crate::canvas::CanvasState;
use crate::documents::ViewId;
use crate::edit_ops::EditOp;

/// A color selector. Returns the picked color (`Some(None)` for the
/// default).
fn color_combo(
    ui: &mut Ui,
    id: impl std::hash::Hash + std::fmt::Debug,
    current: Option<&str>,
    default: &str,
) -> Option<Option<String>> {
    let title = |name: &str| {
        let mut label = name.to_owned();
        label[..1].make_ascii_uppercase();
        label
    };
    let mut picked = None;
    egui::ComboBox::from_id_salt(id)
        .selected_text(current.map_or_else(|| default.to_owned(), title))
        .show_ui(ui, |ui| {
            if ui.selectable_label(current.is_none(), default).clicked() {
                picked = Some(None);
            }
            for name in ANNOTATION_COLORS {
                if ui
                    .selectable_label(current == Some(name), title(name))
                    .clicked()
                {
                    picked = Some(Some(name.to_owned()));
                }
            }
        });
    picked.filter(|p| p.as_deref() != current)
}

/// Shows the editor of a group. `editable` is false in a read-only view.
pub fn show_group(
    ui: &mut Ui,
    view: ViewId,
    canvas: &CanvasState,
    group: u32,
    editable: bool,
    actions: &mut Vec<AppAction>,
) {
    let Some(info) = canvas.tree().groups.iter().find(|g| g.id == group) else {
        ui.label(RichText::new("The group is not in the tree any more.").weak());
        return;
    };
    let members = canvas
        .group_boxes()
        .iter()
        .find(|g| g.id == group)
        .map_or(0, |g| g.members.len());
    let mut edit = |op: EditOp| actions.push(AppAction::Edit(view, op));
    ui.label(RichText::new(&info.title).strong().size(15.0));
    ui.label(
        RichText::new(match members {
            1 => "Group  ·  1 node".to_owned(),
            n => format!("Group  ·  {n} nodes"),
        })
        .weak(),
    );
    ui.label(
        RichText::new(
            "Drag a side or a corner of the box on the canvas to change the nodes in the group.",
        )
        .small(),
    );
    ui.add_space(6.0);
    ui.add_enabled_ui(editable, |ui| {
        let title_label = ui.label(RichText::new("Title").strong()).id;
        let mut title = info.title.clone();
        let response = ui
            .add(
                egui::TextEdit::singleline(&mut title)
                    .id_salt(("group_title", group))
                    .desired_width(f32::INFINITY),
            )
            .labelled_by(title_label);
        if response.changed() && title != info.title {
            edit(EditOp::SetGroupTitle { group, title });
        }
        ui.add_space(4.0);
        let description_label = ui.label(RichText::new("Description").strong()).id;
        let mut description = info.description.clone();
        let response = ui
            .add(
                egui::TextEdit::multiline(&mut description)
                    .id_salt(("group_description", group))
                    .hint_text("What the nodes in this group do.")
                    .desired_rows(3)
                    .desired_width(f32::INFINITY),
            )
            .labelled_by(description_label);
        if response.changed() && description != info.description {
            edit(EditOp::SetGroupDescription { group, description });
        }
        ui.add_space(4.0);
        ui.horizontal(|ui| {
            ui.label(RichText::new("Color").strong());
            if let Some(color) =
                color_combo(ui, ("group_color", group), info.color.as_deref(), "Default")
            {
                edit(EditOp::SetGroupColor { group, color });
            }
        });
        ui.add_space(8.0);
        if ui
            .button("Delete group")
            .on_hover_text("Delete the group box. The nodes stay.")
            .clicked()
        {
            edit(EditOp::DeleteGroup(group));
        }
    });
    ui.add_space(6.0);
    ui.label(
        RichText::new(format!(
            "Saved in the XML file: _group=\"{group}\" on each node, and _group_{group} on the \
             <BehaviorTree> element. BT.CPP ignores these attributes."
        ))
        .small()
        .weak(),
    );
}

/// Shows the editor of a canvas note.
pub fn show_note(
    ui: &mut Ui,
    view: ViewId,
    canvas: &CanvasState,
    note: u32,
    editable: bool,
    actions: &mut Vec<AppAction>,
) {
    let Some(info) = canvas.tree().notes.iter().find(|n| n.id == note) else {
        ui.label(RichText::new("The note is not in the tree any more.").weak());
        return;
    };
    let mut edit = |op: EditOp| actions.push(AppAction::Edit(view, op));
    ui.label(RichText::new("Note").strong().size(15.0));
    ui.label(
        RichText::new("Drag the note on the canvas to move it; drag a side to resize it.").small(),
    );
    ui.add_space(6.0);
    ui.add_enabled_ui(editable, |ui| {
        let label = ui.label(RichText::new("Text").strong()).id;
        let mut text = info.text.clone();
        let response = ui
            .add(
                egui::TextEdit::multiline(&mut text)
                    .id_salt(("canvas_note", note))
                    .desired_rows(5)
                    .desired_width(f32::INFINITY),
            )
            .labelled_by(label);
        if response.changed() && text != info.text {
            edit(EditOp::SetCanvasNoteText { note, text });
        }
        ui.add_space(4.0);
        ui.horizontal(|ui| {
            ui.label(RichText::new("Color").strong());
            if let Some(color) = color_combo(
                ui,
                ("note_color", note),
                info.color.as_deref(),
                "Yellow (default)",
            ) {
                edit(EditOp::SetCanvasNoteColor { note, color });
            }
        });
        ui.add_space(8.0);
        if ui.button("Delete note").clicked() {
            edit(EditOp::DeleteCanvasNote(note));
        }
    });
    ui.add_space(6.0);
    ui.label(
        RichText::new(format!(
            "Saved in the XML file as _note_{note} on the <BehaviorTree> element. BT.CPP \
             ignores it."
        ))
        .small()
        .weak(),
    );
}
