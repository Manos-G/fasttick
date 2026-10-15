//! The timeline panel (replay mode): playback controls, the speed, the
//! current time, and the time bar.
//!
//! The time bar shows the transition density of the shown time range and
//! the playback position:
//!
//! - click or drag on the bar: move the position (scrub);
//! - drag on the ruler at the top (or Shift + drag on the bar): select a
//!   time range and zoom into it;
//! - "Show the whole log" (or a double-click on the ruler): zoom out.

use egui::{
    Align2, Color32, CornerRadius, FontId, Id, Pos2, Rect, RichText, Sense, Stroke, StrokeKind,
    Ui, WidgetInfo, pos2, vec2,
};
use fasttick_log::replay::PlaybackSpeed;

use super::{PanelContext, empty};
use crate::app::Mode;
use crate::replay::{ReplayModel, format_clock, format_seconds, format_wall_time};
use crate::shortcuts::Command;
use crate::theme::CanvasPalette;
use crate::widgets::{self, Icon};

/// The height of the ruler at the top of the time bar.
const RULER_HEIGHT: f32 = 16.0;
/// The height of the density lane.
const LANE_HEIGHT: f32 = 34.0;
/// A drag on the ruler shorter than this (points) does not select a range.
const MIN_SELECTION: f32 = 4.0;

/// What a drag on the time bar does.
#[derive(Debug, Clone, Copy, PartialEq)]
enum DragKind {
    /// Moves the position.
    Scrub,
    /// Selects a range from this time.
    Select(u64),
}

/// Draws the timeline panel.
pub fn show(ui: &mut Ui, cx: &mut PanelContext<'_>) {
    if cx.mode != Mode::Replay {
        empty(ui, "The timeline is available in replay mode.");
        return;
    }
    let Some(doc) = cx.workspace.active_replay_doc() else {
        empty(ui, "Open a log to replay it.");
        return;
    };
    let palette = cx.palette.clone();
    let keymap = cx.keymap;
    let Some(model) = cx.workspace.replay_mut(doc) else {
        return;
    };
    transport(ui, model, keymap, &palette);
    ui.add_space(4.0);
    time_bar(ui, model, &palette);
}

/// A transport button. The label is also the accessibility label; the
/// shortcut is in a second tooltip line.
fn transport_button(
    ui: &mut Ui,
    keymap: &crate::shortcuts::KeyMap,
    command: Command,
    icon: Icon,
    label: &str,
    enabled: bool,
    color: Option<Color32>,
) -> bool {
    let response = widgets::icon_button_colored(ui, icon, label, enabled, color);
    let response = match keymap.shortcut_text(ui.ctx(), command) {
        Some(shortcut) => response.on_hover_text(RichText::new(shortcut).small().weak()),
        None => response,
    };
    response.clicked()
}

fn transport(
    ui: &mut Ui,
    model: &mut ReplayModel,
    keymap: &crate::shortcuts::KeyMap,
    palette: &CanvasPalette,
) {
    ui.horizontal_wrapped(|ui| {
        let applied = model.cursor().applied();
        let total = model.index().len();
        let (low, high) = {
            let index = model.index();
            let (start, end) = model.cursor().bounds(index);
            (index.count_before_time(start), index.count_at_time(end))
        };
        let back = applied > low;
        let forward = applied < high;
        if transport_button(ui, keymap, Command::GoToStart, Icon::First, "Go to the start", back, None) {
            model.go_to_start();
        }
        if transport_button(ui, keymap, Command::StepBack, Icon::Previous, "One transition back", back, None) {
            model.step_back();
        }
        let playing = model.cursor().is_playing();
        let (icon, text) = if playing {
            (Icon::Pause, "Pause")
        } else {
            (Icon::Play, "Play")
        };
        if transport_button(ui, keymap, Command::PlayPause, icon, text, total > 0, Some(palette.accent)) {
            model.toggle_play();
        }
        if transport_button(ui, keymap, Command::StepForward, Icon::Next, "One transition forward", forward, None) {
            model.step_forward();
        }
        if transport_button(ui, keymap, Command::GoToEnd, Icon::Last, "Go to the end", forward, None) {
            model.go_to_end();
        }
        ui.separator();

        let speed = model.cursor().speed().factor();
        ui.label(RichText::new("Speed").small().weak());
        egui::ComboBox::from_id_salt("replay_speed")
            .width(70.0)
            .selected_text(speed_text(speed))
            .show_ui(ui, |ui| {
                for step in PlaybackSpeed::STEPS {
                    if ui
                        .selectable_label((step - speed).abs() < 1e-9, speed_text(step))
                        .clicked()
                    {
                        model.set_speed(PlaybackSpeed::new(step));
                    }
                }
            })
            .response
            .on_hover_text("Playback speed");
        ui.separator();

        let time = model.cursor().time_us();
        ui.label(
            RichText::new(format!(
                "{} / {}",
                format_clock(time),
                format_clock(model.end_us())
            ))
            .monospace()
            .strong(),
        )
        .on_hover_text(format!("{} s from the log start", format_seconds(time)));
        let wall = model.start_time_us().saturating_add(time as i64);
        ui.label(RichText::new(format_wall_time(wall)).small().weak())
            .on_hover_text("The absolute time of the position (UTC)");
        ui.separator();
        ui.label(RichText::new(format!("transition {applied} of {total}")).small());

        if let Some((start, end)) = model.cursor().range() {
            ui.separator();
            ui.label(
                RichText::new(format!(
                    "range {} \u{2013} {}",
                    format_clock(start),
                    format_clock(end)
                ))
                .small()
                .color(palette.accent),
            );
            let label = "Show the whole log";
            if widgets::icon_button(ui, Icon::ZoomOutRange, label, true).clicked() {
                model.set_range(None);
            }
        }
    });
}

fn speed_text(factor: f64) -> String {
    if factor < 1.0 {
        format!("{factor}\u{00D7}")
    } else {
        format!("{factor:.0}\u{00D7}")
    }
}

/// A "nice" tick step (1, 2 or 5 times a power of ten) for at least
/// `min_step` microseconds.
pub fn tick_step(min_step: f64) -> u64 {
    let min_step = min_step.max(1.0);
    let power = 10f64.powf(min_step.log10().floor());
    for factor in [1.0, 2.0, 5.0, 10.0] {
        let step = factor * power;
        if step >= min_step {
            return step.round().max(1.0) as u64;
        }
    }
    (10.0 * power) as u64
}

fn time_bar(ui: &mut Ui, model: &mut ReplayModel, palette: &CanvasPalette) {
    let width = ui.available_width().max(120.0);
    let (rect, response) = ui.allocate_exact_size(
        vec2(width, RULER_HEIGHT + LANE_HEIGHT),
        Sense::click_and_drag(),
    );
    let (start, end) = model.cursor().bounds(model.index());
    let span = end.saturating_sub(start).max(1);
    let fraction = (model.cursor().time_us().saturating_sub(start)) as f64 / span as f64;
    response.widget_info(|| {
        WidgetInfo::slider(true, fraction.clamp(0.0, 1.0), "Timeline position")
    });
    let response = response.on_hover_text(
        "Click or drag to move. Drag on the ruler (or Shift + drag) to zoom into a time range.",
    );

    let to_x = |t: u64| -> f32 {
        rect.left() + (t.saturating_sub(start) as f64 / span as f64) as f32 * rect.width()
    };
    let to_time = |x: f32| -> u64 {
        let f = ((x - rect.left()) / rect.width()).clamp(0.0, 1.0) as f64;
        start + (f * span as f64).round() as u64
    };
    let ruler = Rect::from_min_size(rect.min, vec2(rect.width(), RULER_HEIGHT));
    let lane = Rect::from_min_max(pos2(rect.left(), ruler.bottom()), rect.max);

    // Interaction.
    let drag_id = Id::new("fasttick_timeline_drag");
    let shift = ui.input(|i| i.modifiers.shift);
    // A drag starts after the pointer moved a little: use the press point.
    let press = ui.input(|i| i.pointer.press_origin());
    if response.drag_started()
        && let Some(pointer) = press.or(response.interact_pointer_pos())
    {
        let kind = if ruler.contains(pointer) || shift {
            DragKind::Select(to_time(pointer.x))
        } else {
            model.pause();
            DragKind::Scrub
        };
        ui.data_mut(|d| d.insert_temp(drag_id, kind));
    }
    let drag: Option<DragKind> = ui.data(|d| d.get_temp(drag_id));
    let pointer = response.interact_pointer_pos();
    if response.dragged()
        && let (Some(DragKind::Scrub), Some(pointer)) = (drag, pointer)
    {
        model.seek(to_time(pointer.x));
    }
    let mut selection = None;
    if let (Some(DragKind::Select(from)), Some(pointer)) = (drag, pointer) {
        selection = Some((from, to_time(pointer.x)));
    }
    if response.drag_stopped() {
        if let Some((from, to)) = selection
            && (to_x(from) - to_x(to)).abs() >= MIN_SELECTION
        {
            model.set_range(Some((from.min(to), from.max(to))));
        }
        ui.data_mut(|d| d.remove::<DragKind>(drag_id));
        selection = None;
    }
    if response.clicked()
        && let Some(pointer) = response.interact_pointer_pos()
        && lane.contains(pointer)
    {
        model.pause();
        model.seek(to_time(pointer.x));
    }
    if response.double_clicked()
        && response
            .interact_pointer_pos()
            .is_some_and(|p| ruler.contains(p))
    {
        model.set_range(None);
    }

    if !ui.is_rect_visible(rect) {
        return;
    }
    // Drawing.
    let visuals = ui.visuals();
    let painter = ui.painter_at(rect.expand(1.0));
    let text_color = visuals.weak_text_color();
    let lane_fill = palette.background;
    painter.rect_filled(lane, CornerRadius::same(4), lane_fill);
    painter.rect_stroke(
        lane,
        CornerRadius::same(4),
        Stroke::new(1.0, palette.card_border),
        StrokeKind::Inside,
    );

    // Played part.
    let cursor_x = to_x(model.cursor().time_us()).clamp(lane.left(), lane.right());
    painter.rect_filled(
        Rect::from_min_max(lane.min, pos2(cursor_x, lane.max.y)),
        CornerRadius::same(4),
        palette.accent.gamma_multiply(0.10),
    );

    // Density.
    let bins = (lane.width() / 2.0).floor().max(1.0) as usize;
    let counts = model.density((start, end), bins);
    let max = counts.iter().copied().max().unwrap_or(0).max(1) as f32;
    let bin_width = lane.width() / counts.len() as f32;
    let bar_color = palette.accent.gamma_multiply(if palette.dark { 0.6 } else { 0.45 });
    let usable = lane.height() - 6.0;
    for (i, count) in counts.iter().enumerate() {
        if *count == 0 {
            continue;
        }
        // Square root: single transitions stay visible next to bursts.
        let h = ((*count as f32 / max).sqrt() * usable).max(2.0);
        let x = lane.left() + i as f32 * bin_width;
        painter.rect_filled(
            Rect::from_min_max(
                pos2(x, lane.bottom() - 3.0 - h),
                pos2(x + bin_width.max(1.0), lane.bottom() - 3.0),
            ),
            CornerRadius::ZERO,
            bar_color,
        );
    }

    // Ruler ticks and labels.
    let min_step = span as f64 * 90.0 / rect.width().max(1.0) as f64;
    let step = tick_step(min_step);
    let first = start.div_ceil(step) * step;
    let font = FontId::proportional(10.5);
    let mut t = first;
    while t <= end {
        let x = to_x(t);
        painter.line_segment(
            [pos2(x, ruler.bottom() - 4.0), pos2(x, ruler.bottom())],
            Stroke::new(1.0, text_color),
        );
        let label = if step >= 1000 {
            format_clock(t)
        } else {
            format_seconds(t)
        };
        painter.text(
            pos2(x + 3.0, ruler.top()),
            Align2::LEFT_TOP,
            label,
            font.clone(),
            text_color,
        );
        t = match t.checked_add(step) {
            Some(next) => next,
            None => break,
        };
    }

    // Range selection in progress.
    if let Some((from, to)) = selection {
        let (a, b) = (to_x(from.min(to)), to_x(from.max(to)));
        let band = Rect::from_min_max(pos2(a, rect.top()), pos2(b, rect.bottom()));
        painter.rect_filled(band, CornerRadius::same(2), palette.band_fill);
        painter.rect_stroke(
            band,
            CornerRadius::same(2),
            Stroke::new(1.0, palette.accent),
            StrokeKind::Inside,
        );
    }

    // Position marker.
    let marker = palette.accent;
    painter.line_segment(
        [pos2(cursor_x, ruler.bottom() - 2.0), pos2(cursor_x, lane.bottom())],
        Stroke::new(2.0, marker),
    );
    let handle: Vec<Pos2> = vec![
        pos2(cursor_x - 5.0, ruler.bottom() - 7.0),
        pos2(cursor_x + 5.0, ruler.bottom() - 7.0),
        pos2(cursor_x, ruler.bottom() - 1.0),
    ];
    painter.add(egui::Shape::convex_polygon(handle, marker, Stroke::NONE));

    // The hovered time.
    if let Some(hover) = response.hover_pos() {
        painter.line_segment(
            [pos2(hover.x, lane.top()), pos2(hover.x, lane.bottom())],
            Stroke::new(1.0, Color32::from_gray(128).gamma_multiply(0.6)),
        );
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn tick_steps_are_round() {
        assert_eq!(tick_step(0.0), 1);
        assert_eq!(tick_step(1.0), 1);
        assert_eq!(tick_step(1.5), 2);
        assert_eq!(tick_step(3.0), 5);
        assert_eq!(tick_step(7.0), 10);
        assert_eq!(tick_step(130.0), 200);
        assert_eq!(tick_step(4_000_000.0), 5_000_000);
        assert_eq!(speed_text(0.25), "0.25\u{00D7}");
        assert_eq!(speed_text(10.0), "10\u{00D7}");
    }
}
