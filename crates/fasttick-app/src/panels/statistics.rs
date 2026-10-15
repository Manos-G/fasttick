//! The statistics panel (replay mode): for each node, the time in RUNNING,
//! the number of status changes and the SUCCESS, FAILURE and SKIPPED
//! counts in the time range of the timeline (`fasttick_log::stats`).
//!
//! A click on a column title sorts by that column; a second click changes
//! the direction. A click on a row selects the node on the canvas.

use egui::{Align, RichText, Sense, Stroke, Ui, pos2, vec2};
use egui_extras::{Column, TableBuilder};

use super::transitions::ROW_HEIGHT;
use super::{PanelContext, empty};
use crate::actions::AppAction;
use crate::app::Mode;
use crate::replay::{StatsColumn, StatsSort, format_clock, format_duration};

/// Draws the statistics panel.
pub fn show(ui: &mut Ui, cx: &mut PanelContext<'_>) {
    if cx.mode != Mode::Replay {
        empty(
            ui,
            "Statistics are available in replay mode. Save a recording and open it there.",
        );
        return;
    }
    let Some(doc) = cx.workspace.active_replay_doc() else {
        empty(ui, "Open a log to see the statistics of its nodes.");
        return;
    };
    let Some(model) = cx.workspace.replay_mut(doc) else {
        return;
    };
    ui.horizontal(|ui| {
        let range = match model.cursor().range() {
            Some((start, end)) => format!(
                "Time range {} \u{2013} {} (the timeline zoom)",
                format_clock(start),
                format_clock(end)
            ),
            None => "The whole log. Zoom into a time range on the timeline to limit it.".into(),
        };
        ui.label(RichText::new(range).small());
        ui.with_layout(egui::Layout::right_to_left(Align::Center), |ui| {
            ui.checkbox(&mut model.hide_unchanged, "Hide nodes without transitions");
        });
    });

    let sort = model.stats_sort;
    let mut sort_by = None;
    let mut clicked = None;
    let (stats, order) = model.stats();
    let stats = stats.to_vec();
    let order = order.to_vec();
    let model = &*model;
    // Selectable text would take the clicks of the rows.
    ui.style_mut().interaction.selectable_labels = false;
    TableBuilder::new(ui)
        .id_salt("replay_statistics")
        .striped(true)
        .resizable(true)
        .sense(Sense::click())
        .cell_layout(egui::Layout::left_to_right(Align::Center))
        .column(Column::initial(150.0).at_least(60.0).clip(true))
        .column(Column::initial(220.0).at_least(60.0).clip(true))
        .column(Column::initial(100.0).at_least(70.0))
        .columns(Column::initial(76.0).at_least(56.0), 3)
        .column(Column::remainder().at_least(56.0))
        .min_scrolled_height(0.0)
        .header(ROW_HEIGHT + 4.0, |mut header| {
            for column in StatsColumn::ALL {
                header.col(|ui| {
                    if sort_header(ui, column, sort) {
                        sort_by = Some(column);
                    }
                });
            }
        })
        .body(|body| {
            body.rows(ROW_HEIGHT, order.len(), |mut row| {
                let node = &stats[order[row.index()]];
                let info = model.node(node.uid);
                row.col(|ui| {
                    match info {
                        Some(info) if !info.name.is_empty() => ui.label(&info.name),
                        _ => ui.label(RichText::new(format!("UID {}", node.uid.0)).weak()),
                    };
                });
                row.col(|ui| {
                    ui.label(RichText::new(info.map_or("", |i| i.path.as_str())).weak());
                });
                row.col(|ui| {
                    ui.label(RichText::new(format_duration(node.running_time_us)).monospace());
                });
                for value in [
                    node.status_changes,
                    node.success_count,
                    node.failure_count,
                    node.skipped_count,
                ] {
                    row.col(|ui| {
                        let text = RichText::new(value.to_string()).monospace();
                        ui.label(if value == 0 { text.weak() } else { text });
                    });
                }
                if row.response().clicked() {
                    clicked = Some(node.uid);
                }
            });
        });
    if let Some(column) = sort_by
        && let Some(model) = cx.workspace.replay_mut(doc)
    {
        model.sort_stats_by(column);
    }
    if let Some(uid) = clicked {
        cx.actions.push(AppAction::RevealNode(doc, uid));
    }
}

/// A column title that sorts on click. The sort direction is a small
/// painted triangle (the bundled font has no arrow glyphs).
fn sort_header(ui: &mut Ui, column: StatsColumn, sort: StatsSort) -> bool {
    let active = sort.column == column;
    let mut text = RichText::new(column.title()).strong();
    if active {
        text = text.color(ui.visuals().hyperlink_color);
    }
    let response = ui
        .add(egui::Button::new(text).frame(false))
        .on_hover_text(format!("Sort by {}", column.title().to_lowercase()));
    if active {
        let (rect, _) = ui.allocate_exact_size(vec2(10.0, 10.0), Sense::hover());
        let c = rect.center();
        let color = ui.visuals().hyperlink_color;
        let points = if sort.ascending {
            vec![pos2(c.x - 4.0, c.y + 2.5), pos2(c.x + 4.0, c.y + 2.5), pos2(c.x, c.y - 3.0)]
        } else {
            vec![pos2(c.x - 4.0, c.y - 2.5), pos2(c.x + 4.0, c.y - 2.5), pos2(c.x, c.y + 3.0)]
        };
        ui.painter()
            .add(egui::Shape::convex_polygon(points, color, Stroke::NONE));
    }
    response.clicked()
}
