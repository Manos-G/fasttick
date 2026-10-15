//! The blackboard panel (live mode): choose the blackboards to watch, and
//! see their entries as a collapsible JSON tree with a filter.
//!
//! The tree shows objects as key and value rows, arrays as indexed
//! entries (`[0]`, `[1]`, ...) and scalars aligned to the right. A
//! `"__type"` key is shown as a type hint next to its object (see
//! [`crate::live::blackboard`]).

use egui::{Align, Color32, Id, Layout, RichText, Ui};
use serde_json::Value;

use super::{PanelContext, empty};
use crate::app::Mode;
use crate::live::blackboard::{self, MAX_INTERVAL_MS, MIN_INTERVAL_MS};

/// The filter text of the panel (kept in egui memory).
fn filter_id() -> Id {
    Id::new("fasttick_blackboard_filter")
}

/// Draws the blackboard panel.
pub fn show(ui: &mut Ui, cx: &mut PanelContext<'_>) {
    match cx.mode {
        Mode::Live => {}
        Mode::Replay => {
            empty(ui, "Logs do not contain blackboard values.");
            return;
        }
        Mode::Edit => {
            empty(ui, "Blackboards show in live mode.");
            return;
        }
    }
    let Some(tree) = cx.live.tree().cloned() else {
        empty(
            ui,
            "Connect to an executor to see the blackboards of the running tree.",
        );
        return;
    };
    let connected = cx.live.is_connected();

    // Controls: interval, periodic read, read now.
    ui.horizontal_wrapped(|ui| {
        let watch = cx.live.blackboard();
        let mut auto = watch.auto_refresh();
        let mut interval = watch.interval_ms();
        let auto_changed = ui
            .checkbox(&mut auto, "Every")
            .on_hover_text("Read the watched blackboards again and again.")
            .changed();
        let interval_changed = ui
            .add_enabled(
                auto,
                egui::DragValue::new(&mut interval)
                    .range(MIN_INTERVAL_MS..=MAX_INTERVAL_MS)
                    .speed(10.0)
                    .suffix(" ms"),
            )
            .on_hover_text("The time between two reads.")
            .changed();
        if auto_changed || interval_changed {
            cx.live.update_blackboard_watch(|w| {
                w.set_auto_refresh(auto);
                w.set_interval_ms(interval);
            });
        }
        if ui
            .add_enabled(connected, egui::Button::new("Read now"))
            .clicked()
        {
            cx.live.refresh_blackboards();
        }
        if let Some(at) = cx.live.blackboard().updated_at() {
            let age = at.elapsed().as_secs_f32();
            ui.label(RichText::new(format!("updated {age:.1} s ago")).small().weak());
            if cx.live.blackboard().auto_refresh() {
                ui.ctx()
                    .request_repaint_after(std::time::Duration::from_millis(500));
            }
        }
    });

    // The blackboards of the tree: one for each SubTree instance.
    egui::CollapsingHeader::new(RichText::new("Watched blackboards").strong())
        .id_salt("blackboard_choice")
        .default_open(true)
        .show(ui, |ui| {
            for instance in &tree.instances {
                let name = instance.blackboard_name().to_owned();
                let mut on = cx.live.blackboard().is_selected(&name);
                let text = if name == instance.tree_id {
                    name.clone()
                } else {
                    format!("{name}  ({})", instance.tree_id)
                };
                if ui.checkbox(&mut on, text).changed() {
                    cx.live
                        .update_blackboard_watch(|w| w.set_selected(&name, on));
                }
            }
        });

    let mut filter: String = ui.data(|d| d.get_temp(filter_id())).unwrap_or_default();
    let response = ui.add(
        egui::TextEdit::singleline(&mut filter)
            .hint_text("Filter keys and values")
            .desired_width(f32::INFINITY),
    );
    if response.changed() {
        ui.data_mut(|d| d.insert_temp(filter_id(), filter.clone()));
    }
    let filter = filter.trim().to_lowercase();
    ui.add_space(4.0);

    let selected: Vec<String> = cx.live.blackboard().selected().iter().cloned().collect();
    if selected.is_empty() {
        empty(ui, "Choose a blackboard to watch.");
        return;
    }
    let colors = ValueColors::new(ui, cx.palette.text_reference);
    egui::ScrollArea::vertical()
        .auto_shrink([false, false])
        .show(ui, |ui| {
            for name in selected {
                let id = Id::new(("blackboard", &name));
                match cx.live.blackboard().value_of(&name) {
                    Some(value) => {
                        if !blackboard::entry_matches(&name, value, &filter)
                            && !filter.is_empty()
                        {
                            continue;
                        }
                        egui::CollapsingHeader::new(RichText::new(&name).strong())
                            .id_salt(id)
                            .default_open(true)
                            .show(ui, |ui| {
                                let children = blackboard::children(value);
                                if children.is_empty() {
                                    ui.label(RichText::new("No entries.").weak());
                                }
                                let force = name.to_lowercase().contains(&filter);
                                for (key, child) in children {
                                    json_entry(
                                        ui,
                                        id.with(&key),
                                        &key,
                                        child,
                                        &filter,
                                        force,
                                        &colors,
                                    );
                                }
                            });
                    }
                    None => {
                        ui.label(RichText::new(&name).strong());
                        let text = if cx.live.blackboard().updated_at().is_some() {
                            "The executor has no blackboard with this name."
                        } else {
                            "No values yet."
                        };
                        ui.label(RichText::new(text).weak());
                    }
                }
            }
        });
}

/// The text colours of the JSON value types.
struct ValueColors {
    string: Color32,
    number: Color32,
    other: Color32,
    weak: Color32,
}

impl ValueColors {
    fn new(ui: &Ui, reference: Color32) -> Self {
        ValueColors {
            string: ui.visuals().text_color(),
            number: reference,
            other: ui.visuals().warn_fg_color,
            weak: ui.visuals().weak_text_color(),
        }
    }

    fn of(&self, value: &Value) -> Color32 {
        match value {
            Value::String(_) => self.string,
            Value::Number(_) => self.number,
            Value::Bool(_) | Value::Null => self.other,
            Value::Array(_) | Value::Object(_) => self.weak,
        }
    }
}

/// Draws one entry of the JSON tree. `force` shows the entry and all its
/// children (a parent matched the filter).
fn json_entry(
    ui: &mut Ui,
    id: Id,
    key: &str,
    value: &Value,
    filter: &str,
    force: bool,
    colors: &ValueColors,
) {
    let show_all = force || filter.is_empty() || key.to_lowercase().contains(filter);
    if !show_all && !blackboard::entry_matches(key, value, filter) {
        return;
    }
    if blackboard::is_container(value) {
        let mut state =
            egui::collapsing_header::CollapsingState::load_with_default_open(ui.ctx(), id, false);
        if !filter.is_empty() && !show_all {
            // A child matches: open the entry to show it.
            state.set_open(true);
        }
        let (_, header, _) = state
            .show_header(ui, |ui| {
                // A click on the key opens or closes the entry too.
                let clicked = ui
                    .add(
                        egui::Label::new(RichText::new(key).monospace())
                            .sense(egui::Sense::click()),
                    )
                    .clicked();
                if let Some(hint) = blackboard::type_hint(value) {
                    ui.label(RichText::new(hint).small().italics().color(colors.weak))
                        .on_hover_text("The C++ type (the \"__type\" key)");
                }
                ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                    ui.label(
                        RichText::new(blackboard::scalar_text(value))
                            .small()
                            .color(colors.weak),
                    );
                });
                clicked
            })
            .body(|ui| {
                for (child_key, child) in blackboard::children(value) {
                    json_entry(
                        ui,
                        id.with(&child_key),
                        &child_key,
                        child,
                        filter,
                        show_all,
                        colors,
                    );
                }
            });
        if header.inner {
            let mut state =
                egui::collapsing_header::CollapsingState::load_with_default_open(ui.ctx(), id, false);
            state.toggle(ui);
            state.store(ui.ctx());
        }
    } else {
        ui.horizontal(|ui| {
            // Line up with the text of the collapsible headers.
            ui.add_space(ui.spacing().indent);
            ui.label(RichText::new(key).monospace());
            ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                let text = blackboard::scalar_text(value);
                ui.add(
                    egui::Label::new(RichText::new(text).monospace().color(colors.of(value)))
                        .truncate(),
                );
            });
        });
    }
}
