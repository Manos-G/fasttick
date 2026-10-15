//! A popup with a search field and a list that the search filters.
//!
//! The add popup ([`crate::add_popup`]) and the command search
//! ([`crate::command_search`]) use it. The caller gives a function that
//! makes the rows for a query (filtered and sorted, best first). Arrow keys
//! move the selection, Enter picks the selected row, Escape or a click
//! outside closes the popup.

use egui::{Align2, Color32, CornerRadius, Key, Modifiers, Pos2, RichText, Sense, Stroke};

/// The width of the add popup.
pub const WIDTH: f32 = 340.0;
/// The height of one row of the list.
const ROW_HEIGHT: f32 = 24.0;
/// The most rows that show without scrolling.
const VISIBLE_ROWS: usize = 12;

/// One row of the list.
#[derive(Debug, Clone, PartialEq)]
pub struct SearchRow<K> {
    /// What the row picks.
    pub key: K,
    /// The main text.
    pub text: String,
    /// A short weak text at the right side, for example a shortcut.
    pub tag: String,
    /// A small colour square before the text.
    pub swatch: Option<Color32>,
    /// A text below the list for the selected row.
    pub description: Option<String>,
}

/// The search text and the selection.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct SearchState {
    /// The search text.
    pub query: String,
    /// The index of the selected row.
    pub selected: usize,
    focused: bool,
    /// True after the first frame of the popup.
    shown: bool,
}

/// The fixed texts of a popup.
#[derive(Debug, Clone, Copy)]
pub struct SearchTexts<'a> {
    /// The ID of the popup area.
    pub id: &'a str,
    /// A small text above the search field.
    pub title: &'a str,
    /// The hint in the empty search field.
    pub hint: &'a str,
    /// The text when no row matches.
    pub empty: &'a str,
    /// The key help below the list.
    pub footer: &'a str,
    /// The width of the popup.
    pub width: f32,
}

/// The result of [`show`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SearchChoice<K> {
    /// The popup stays open.
    Open,
    /// The user closed the popup.
    Cancel,
    /// The user picked a row.
    Pick(K),
}

/// Draws the popup with its top-left corner at `pos`.
pub fn show<K: Clone>(
    ctx: &egui::Context,
    state: &mut SearchState,
    pos: Pos2,
    texts: SearchTexts<'_>,
    rows: impl Fn(&str) -> Vec<SearchRow<K>>,
) -> SearchChoice<K> {
    let mut choice = SearchChoice::Open;
    // The keys of the list, before the search field sees them.
    let (down, up, enter, escape) = ctx.input_mut(|i| {
        (
            i.consume_key(Modifiers::NONE, Key::ArrowDown),
            i.consume_key(Modifiers::NONE, Key::ArrowUp),
            i.consume_key(Modifiers::NONE, Key::Enter),
            i.consume_key(Modifiers::NONE, Key::Escape),
        )
    });
    let found = rows(&state.query);
    if !found.is_empty() {
        if down {
            state.selected = (state.selected + 1).min(found.len() - 1);
        }
        if up {
            state.selected = state.selected.saturating_sub(1);
        }
        state.selected = state.selected.min(found.len() - 1);
        if enter {
            choice = SearchChoice::Pick(found[state.selected].key.clone());
        }
    }
    if escape {
        choice = SearchChoice::Cancel;
    }

    let area = egui::Area::new(egui::Id::new(texts.id))
        .order(egui::Order::Foreground)
        .fixed_pos(pos)
        .pivot(Align2::LEFT_TOP)
        .constrain(true)
        .show(ctx, |ui| {
            egui::Frame::popup(ui.style()).show(ui, |ui| {
                ui.set_width(texts.width);
                ui.label(RichText::new(texts.title).small().weak());
                let edit = ui.add(
                    egui::TextEdit::singleline(&mut state.query)
                        .id_salt((texts.id, "search"))
                        .hint_text(texts.hint)
                        .desired_width(f32::INFINITY),
                );
                // The first frame of a new area is a sizing pass, which does
                // not take the focus: ask until the field has it.
                if !state.focused {
                    // The stored focus, not `has_focus`: that one is false
                    // while the window itself has no focus.
                    if ui.ctx().memory(|m| m.focused()) == Some(edit.id) {
                        state.focused = true;
                    } else {
                        edit.request_focus();
                        // The focus shows in the next frame: draw it now,
                        // not at the next input event.
                        ui.ctx().request_repaint();
                    }
                }
                if edit.changed() {
                    state.selected = 0;
                }
                ui.add_space(4.0);
                // The list of this frame uses the query of this frame.
                let found = rows(&state.query);
                if found.is_empty() {
                    ui.label(RichText::new(texts.empty).weak());
                }
                let selected = state.selected.min(found.len().saturating_sub(1));
                egui::ScrollArea::vertical()
                    .max_height(ROW_HEIGHT * VISIBLE_ROWS as f32)
                    .auto_shrink([false, true])
                    .show(ui, |ui| {
                        for (index, row) in found.iter().enumerate() {
                            let response = row_widget(ui, row, index == selected);
                            if index == selected && (down || up) {
                                response.scroll_to_me(None);
                            }
                            if response.clicked() {
                                choice = SearchChoice::Pick(row.key.clone());
                            }
                        }
                    });
                if let Some(text) = found.get(selected).and_then(|r| r.description.as_ref()) {
                    ui.separator();
                    ui.label(RichText::new(text).small());
                }
                ui.label(RichText::new(texts.footer).small().weak());
            });
        });
    // A click outside closes the popup. The click that opened the popup
    // (for example on a menu entry) can be in the first frame: ignore it.
    let was_shown = std::mem::replace(&mut state.shown, true);
    let clicked_outside = was_shown
        && ctx.input(|i| {
            i.pointer.any_pressed()
                && i.pointer
                    .interact_pos()
                    .is_some_and(|p| !area.response.rect.contains(p))
        });
    if clicked_outside && matches!(choice, SearchChoice::Open) {
        choice = SearchChoice::Cancel;
    }
    choice
}

/// One row: the swatch, the text and the tag.
fn row_widget<K>(ui: &mut egui::Ui, row: &SearchRow<K>, selected: bool) -> egui::Response {
    let (rect, response) =
        ui.allocate_exact_size(egui::vec2(ui.available_width(), ROW_HEIGHT), Sense::click());
    let visuals = ui.visuals();
    if selected {
        ui.painter().rect(
            rect,
            CornerRadius::same(4),
            visuals.selection.bg_fill,
            Stroke::new(1.0, visuals.selection.stroke.color),
            egui::StrokeKind::Inside,
        );
    } else if response.hovered() {
        ui.painter().rect_filled(
            rect,
            CornerRadius::same(4),
            visuals.widgets.hovered.weak_bg_fill,
        );
    }
    let mut text_x = rect.min.x + 8.0;
    if let Some(color) = row.swatch {
        let swatch = egui::Rect::from_center_size(
            egui::pos2(rect.min.x + 12.0, rect.center().y),
            egui::vec2(10.0, 10.0),
        );
        ui.painter()
            .rect_filled(swatch, CornerRadius::same(2), color);
        text_x = rect.min.x + 24.0;
    }
    // The tag first: the text must not run into it.
    let tag = ui.painter().layout_no_wrap(
        row.tag.clone(),
        egui::TextStyle::Small.resolve(ui.style()),
        visuals.weak_text_color(),
    );
    let tag_pos = egui::pos2(
        rect.max.x - 8.0 - tag.size().x,
        rect.center().y - tag.size().y / 2.0,
    );
    let text_width = (tag_pos.x - 8.0 - text_x).max(0.0);
    let mut job = egui::text::LayoutJob::simple_singleline(
        row.text.clone(),
        egui::TextStyle::Body.resolve(ui.style()),
        visuals.text_color(),
    );
    job.wrap = egui::text::TextWrapping::truncate_at_width(text_width);
    let text = ui.painter().layout_job(job);
    ui.painter().galley(
        egui::pos2(text_x, rect.center().y - text.size().y / 2.0),
        text,
        visuals.text_color(),
    );
    ui.painter().galley(tag_pos, tag, visuals.weak_text_color());
    response.widget_info(|| {
        egui::WidgetInfo::selected(egui::WidgetType::SelectableLabel, true, selected, &row.text)
    });
    response
}
