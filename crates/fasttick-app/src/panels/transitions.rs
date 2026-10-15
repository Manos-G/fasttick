//! The transitions panel.
//!
//! - Replay mode: all transitions of the log in a virtual table (only the
//!   visible rows are drawn, so 100,000 and more rows stay fast), with
//!   include and exclude patterns, status check boxes and a time range
//!   (`fasttick_log::filter`). A click on a row moves the replay to that
//!   transition and selects the node. The filtered rows can be exported as
//!   JSON or CSV (`fasttick_log::export`).
//! - Live mode: the transitions of the current recording.

use egui::{Align, Id, RichText, Sense, Ui};
use egui_extras::{Column, TableBuilder};
use fasttick_core::status::NodeStatus;

use super::{PanelContext, empty, status_cell};
use crate::actions::AppAction;
use crate::app::Mode;
use crate::jobs::{DialogPurpose, ExportFormat};
use crate::replay::{ReplayModel, format_seconds};
use crate::theme::CanvasPalette;

/// The row height of the tables.
pub const ROW_HEIGHT: f32 = 20.0;

/// Draws the transitions panel.
pub fn show(ui: &mut Ui, cx: &mut PanelContext<'_>) {
    match cx.mode {
        Mode::Replay => replay(ui, cx),
        Mode::Live => live(ui, cx),
        Mode::Edit => empty(ui, "Transitions show in live and replay mode."),
    }
}

fn replay(ui: &mut Ui, cx: &mut PanelContext<'_>) {
    let Some(doc) = cx.workspace.active_replay_doc() else {
        empty(ui, "Open a log to see its transitions.");
        return;
    };
    let palette = cx.palette.clone();
    let Some(model) = cx.workspace.replay_mut(doc) else {
        return;
    };
    let mut export = None;
    filter_bar(ui, model, &palette);
    let total = model.index().len();
    let shown = model.rows().len();
    let follow_id = Id::new("fasttick_transitions_follow");
    let mut follow: bool = ui.data(|d| d.get_temp(follow_id)).unwrap_or(true);
    ui.horizontal(|ui| {
        time_range_fields(ui, model);
        ui.separator();
        ui.label(RichText::new(format!("{shown} of {total} transitions")).small());
        if let Some(error) = model.filter_error() {
            ui.label(
                RichText::new(error)
                    .small()
                    .color(ui.visuals().error_fg_color),
            );
        }
        ui.with_layout(egui::Layout::right_to_left(Align::Center), |ui| {
            if ui
                .add_enabled(shown > 0, egui::Button::new("CSV\u{2026}"))
                .on_hover_text("Export the rows of the table as CSV.")
                .clicked()
            {
                export = Some(ExportFormat::Csv);
            }
            if ui
                .add_enabled(shown > 0, egui::Button::new("JSON\u{2026}"))
                .on_hover_text("Export the rows of the table as JSON.")
                .clicked()
            {
                export = Some(ExportFormat::Json);
            }
            ui.label(RichText::new("Export").small().weak());
            ui.separator();
            if ui
                .checkbox(&mut follow, "Follow")
                .on_hover_text("Scroll to the current transition during playback.")
                .changed()
            {
                ui.data_mut(|d| d.insert_temp(follow_id, follow));
            }
        });
    });

    // The time fields of the second row can have changed.
    model.rows();
    let model: &ReplayModel = model;
    let rows = model.current_rows();
    let current = model.cursor().current_index();
    // The table row of the current transition (the rows are sorted).
    let current_row = current.and_then(|c| {
        let row = rows.partition_point(|r| (*r as usize) < c);
        (rows.get(row).is_some_and(|r| *r as usize == c)).then_some(row)
    });
    let mut clicked = None;
    let mut table = TableBuilder::new(ui)
        .id_salt("replay_transitions")
        .striped(true)
        .resizable(true)
        .sense(Sense::click())
        .cell_layout(egui::Layout::left_to_right(Align::Center))
        .column(Column::initial(96.0).at_least(60.0))
        .column(Column::initial(150.0).at_least(60.0).clip(true))
        .column(Column::initial(240.0).at_least(60.0).clip(true))
        .column(Column::initial(130.0).at_least(50.0).clip(true))
        .column(Column::initial(100.0).at_least(70.0))
        .column(Column::remainder().at_least(70.0))
        .min_scrolled_height(0.0);
    if follow
        && model.cursor().is_playing()
        && let Some(row) = current_row
    {
        table = table.scroll_to_row(row, Some(Align::Center));
    }
    table
        .header(ROW_HEIGHT + 2.0, |mut header| {
            for title in ["Time (s)", "Node", "Path", "Type", "Previous", "New status"] {
                header.col(|ui| {
                    ui.strong(title);
                });
            }
        })
        .body(|body| {
            body.rows(ROW_HEIGHT, rows.len(), |mut row| {
                let position = rows[row.index()] as usize;
                let Some((transition, previous)) = model.transition(position) else {
                    return;
                };
                row.set_selected(Some(position) == current || Some(position) == model.selected_transition);
                let node = model.node(transition.uid);
                row.col(|ui| {
                    ui.label(RichText::new(format_seconds(transition.offset_us)).monospace());
                });
                row.col(|ui| {
                    match node {
                        Some(node) if !node.name.is_empty() => ui.label(&node.name),
                        Some(_) => ui.label(RichText::new("(no name)").weak()),
                        None => ui.label(
                            RichText::new(format!("UID {}", transition.uid.0)).weak(),
                        ),
                    };
                });
                row.col(|ui| {
                    ui.label(RichText::new(node.map_or("", |n| n.path.as_str())).weak());
                });
                row.col(|ui| {
                    ui.label(node.map_or("", |n| n.type_id.as_str()));
                });
                row.col(|ui| status_cell(ui, previous, &palette));
                row.col(|ui| status_cell(ui, transition.status, &palette));
                if row.response().clicked() {
                    clicked = Some((position, transition.uid));
                }
            });
        });

    if let Some(format) = export {
        cx.actions.push(AppAction::PickFile(DialogPurpose::ExportTransitions(format)));
    }
    if let Some((position, uid)) = clicked {
        if let Some(model) = cx.workspace.replay_mut(doc) {
            model.seek_to_transition(position);
            model.selected_transition = Some(position);
        }
        cx.actions.push(AppAction::RevealNode(doc, uid));
    }
}

/// The first filter row: the patterns and the statuses.
fn filter_bar(ui: &mut Ui, model: &mut ReplayModel, palette: &CanvasPalette) {
    ui.horizontal_wrapped(|ui| {
        ui.label("Include");
        ui.add(
            egui::TextEdit::singleline(&mut model.filter.include)
                .hint_text("name, path or type")
                .desired_width(140.0),
        )
        .on_hover_text(
            "Show only nodes that match. Separate patterns with commas. * matches any \
             text, ? one character. Case is ignored.",
        );
        ui.label("Exclude");
        ui.add(
            egui::TextEdit::singleline(&mut model.filter.exclude)
                .hint_text("patterns")
                .desired_width(120.0),
        )
        .on_hover_text("Hide nodes that match. Exclude wins over include.");
        ui.separator();
        for status in NodeStatus::ALL {
            let index = usize::from(status.as_u8());
            let color = palette
                .status(status)
                .unwrap_or(ui.visuals().weak_text_color());
            let text = RichText::new(status.as_str()).small().color(color);
            ui.checkbox(&mut model.filter.statuses[index], text)
                .on_hover_text(format!("Show the transitions to {status}."));
        }
    });
}

/// The time range fields of the second filter row.
fn time_range_fields(ui: &mut Ui, model: &mut ReplayModel) {
    ui.label("From");
    ui.add(
        egui::TextEdit::singleline(&mut model.filter.from_s)
            .hint_text("start")
            .desired_width(64.0),
    );
    ui.label("to");
    ui.add(
        egui::TextEdit::singleline(&mut model.filter.to_s)
            .hint_text("end")
            .desired_width(64.0),
    );
    ui.label("s");
    let range = model.cursor().range();
    if ui
        .add_enabled(range.is_some(), egui::Button::new("Timeline range"))
        .on_hover_text("Use the time range of the timeline zoom.")
        .on_disabled_hover_text("Zoom into a time range on the timeline first.")
        .clicked()
    {
        model.filter.set_time_range(range);
    }
    if ui
        .button("Clear filter")
        .on_hover_text("Show all transitions.")
        .clicked()
    {
        model.filter = crate::replay::FilterForm::default();
    }
}

fn live(ui: &mut Ui, cx: &mut PanelContext<'_>) {
    let recording = cx.live.recording();
    if !recording.has_data() {
        empty(
            ui,
            "Start a recording in the live bar to see the transitions of the running tree.",
        );
        return;
    }
    let transitions = recording.transitions();
    let count = transitions.len();
    ui.label(
        RichText::new(if recording.is_active() {
            format!("Recording: {count} transitions")
        } else {
            format!("Recorded {count} transitions. Save the recording to replay it.")
        })
        .small(),
    );
    let palette = cx.palette.clone();
    let live = &*cx.live;
    TableBuilder::new(ui)
        .id_salt("live_transitions")
        .striped(true)
        .resizable(true)
        .stick_to_bottom(true)
        .cell_layout(egui::Layout::left_to_right(Align::Center))
        .column(Column::initial(96.0).at_least(60.0))
        .column(Column::initial(150.0).at_least(60.0).clip(true))
        .column(Column::initial(260.0).at_least(60.0).clip(true))
        .column(Column::remainder().at_least(70.0))
        .min_scrolled_height(0.0)
        .header(ROW_HEIGHT + 2.0, |mut header| {
            for title in ["Time (s)", "Node", "Path", "New status"] {
                header.col(|ui| {
                    ui.strong(title);
                });
            }
        })
        .body(|body| {
            body.rows(ROW_HEIGHT, count, |mut row| {
                let transition = live.recording().transitions()[row.index()];
                let (name, path) = live
                    .node_text(transition.uid)
                    .unwrap_or_else(|| (format!("UID {}", transition.uid.0), String::new()));
                row.col(|ui| {
                    ui.label(RichText::new(format_seconds(transition.timestamp_us)).monospace());
                });
                row.col(|ui| {
                    ui.label(name);
                });
                row.col(|ui| {
                    ui.label(RichText::new(path).weak());
                });
                row.col(|ui| status_cell(ui, transition.status, &palette));
            });
        });
}
