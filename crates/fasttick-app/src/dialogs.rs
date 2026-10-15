//! Dialogs: About, keyboard shortcuts, connect; and short notices.

use std::collections::BTreeMap;

use egui::{Align2, Color32, RichText, Ui};

use crate::shortcuts::{CANVAS_HELP, Command, KeyMap};

/// The URL of the third-party licence list.
pub fn third_party_licenses_url() -> String {
    format!(
        "{}/blob/main/THIRD_PARTY_LICENSES.md",
        env!("CARGO_PKG_REPOSITORY")
    )
}

/// Draws the About dialog. Returns false when the user closes it.
pub fn about(ctx: &egui::Context, accent: Color32) -> bool {
    let mut open = true;
    let mut close = false;
    egui::Window::new("About FastTick")
        .collapsible(false)
        .resizable(false)
        .anchor(Align2::CENTER_CENTER, [0.0, 0.0])
        .open(&mut open)
        .show(ctx, |ui| {
            ui.set_width(380.0);
            ui.horizontal(|ui| {
                crate::widgets::logo(ui, accent);
                ui.label(RichText::new("FastTick").size(20.0).strong());
                ui.label(RichText::new(env!("CARGO_PKG_VERSION")).weak());
            });
            ui.add_space(4.0);
            ui.label("A tree editor, live debugger and log replayer for BehaviorTree.CPP 4.");
            ui.add_space(8.0);
            ui.label(RichText::new("Licences").strong());
            ui.label("FastTick is free software under the MIT licence.");
            ui.label("Fonts: Roboto (Apache-2.0). A monospace font of the system is used if one is installed.");
            ui.label("Built with egui, eframe and egui_dock (MIT or Apache-2.0), and other crates.");
            ui.label("Live connections use libzmq (MPL-2.0), linked statically.");
            ui.horizontal_wrapped(|ui| {
                ui.label("The full list is in");
                ui.hyperlink_to("THIRD_PARTY_LICENSES.md", third_party_licenses_url());
            });
            ui.add_space(8.0);
            ui.vertical_centered(|ui| {
                if ui.button("Close").clicked() {
                    close = true;
                }
            });
        });
    open && !close
}

/// Draws the keyboard shortcut dialog. Returns false when the user
/// closes it.
pub fn shortcuts(ctx: &egui::Context, keymap: &KeyMap) -> bool {
    let mut open = true;
    egui::Window::new("Keyboard shortcuts")
        .collapsible(false)
        .resizable(true)
        .default_width(520.0)
        .default_height(520.0)
        .anchor(Align2::CENTER_CENTER, [0.0, 0.0])
        .open(&mut open)
        .show(ctx, |ui| {
            egui::ScrollArea::vertical().show(ui, |ui| shortcut_list(ui, keymap));
        });
    open && !ctx.input(|i| i.key_pressed(egui::Key::Escape))
}

/// The list of all shortcuts, grouped.
pub fn shortcut_list(ui: &mut Ui, keymap: &KeyMap) {
    // Group rank -> (group name, entries).
    type Group<'a> = (&'a str, Vec<(Command, String)>);
    let mut groups: BTreeMap<usize, Group<'_>> = BTreeMap::new();
    let order = ["File", "Edit", "View", "Mode", "Live", "Help"];
    for binding in keymap.bindings() {
        let group = binding.command.group();
        let rank = order
            .iter()
            .position(|g| *g == group)
            .unwrap_or(order.len());
        let text = crate::shortcuts::format_shortcut(ui.ctx(), &binding.shortcut);
        let entries = &mut groups.entry(rank).or_insert_with(|| (group, Vec::new())).1;
        // Several bindings of one command share a row.
        match entries.iter_mut().find(|(command, _)| *command == binding.command) {
            Some((_, existing)) => {
                existing.push_str(", ");
                existing.push_str(&text);
            }
            None => entries.push((binding.command, text)),
        }
    }
    // One grid for all groups, so that the columns line up.
    egui::Grid::new("shortcuts")
        .num_columns(2)
        .spacing([32.0, 4.0])
        .min_col_width(200.0)
        .show(ui, |ui| {
            for (index, (_, (group, entries))) in groups.into_iter().enumerate() {
                group_header(ui, group, index > 0);
                for (command, text) in entries {
                    ui.label(command.label());
                    ui.label(RichText::new(text).monospace());
                    ui.end_row();
                }
            }
            group_header(ui, "Canvas", true);
            for (keys, action) in CANVAS_HELP {
                ui.label(*action);
                ui.label(RichText::new(*keys).monospace());
                ui.end_row();
            }
        });
}

/// A group title row in the shortcut grid, after an empty row (a grid
/// does not accept `add_space`).
fn group_header(ui: &mut Ui, group: &str, gap: bool) {
    if gap {
        ui.label("");
        ui.end_row();
    }
    ui.label(RichText::new(group).strong());
    ui.end_row();
}

/// The state of the connect dialog.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ConnectForm {
    /// Host name or address.
    pub host: String,
    /// Server port.
    pub port: u16,
    /// Status poll interval in milliseconds.
    pub poll_interval_ms: u32,
}

/// What the user did in the connect dialog.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ConnectChoice {
    /// The dialog stays open.
    Open,
    /// The user pressed Connect.
    Connect,
    /// The user cancelled.
    Cancel,
}

/// Draws the connect dialog.
pub fn connect(ctx: &egui::Context, form: &mut ConnectForm) -> ConnectChoice {
    let mut choice = ConnectChoice::Open;
    let mut open = true;
    egui::Window::new("Connect to an executor")
        .collapsible(false)
        .resizable(false)
        .anchor(Align2::CENTER_CENTER, [0.0, -60.0])
        .open(&mut open)
        .show(ctx, |ui| {
            ui.label("The executor must run the BT.CPP publisher.");
            ui.add_space(6.0);
            egui::Grid::new("connect_grid")
                .num_columns(2)
                .spacing([10.0, 8.0])
                .show(ui, |ui| {
                    ui.label("Host");
                    let host =
                        ui.add(egui::TextEdit::singleline(&mut form.host).desired_width(200.0));
                    if host.lost_focus() && ui.input(|i| i.key_pressed(egui::Key::Enter)) {
                        choice = ConnectChoice::Connect;
                    }
                    ui.end_row();
                    ui.label("Port");
                    ui.add(egui::DragValue::new(&mut form.port).range(1..=65534))
                        .on_hover_text("The server port. The notifications use the next port.");
                    ui.end_row();
                    ui.label("Poll interval");
                    ui.add(
                        egui::DragValue::new(&mut form.poll_interval_ms)
                            .range(crate::settings::MIN_POLL_MS..=crate::settings::MAX_POLL_MS)
                            .suffix(" ms"),
                    );
                    ui.end_row();
                });
            ui.add_space(8.0);
            ui.horizontal(|ui| {
                let valid = !form.host.trim().is_empty();
                if ui
                    .add_enabled(valid, egui::Button::new(RichText::new("Connect").strong()))
                    .clicked()
                {
                    choice = ConnectChoice::Connect;
                }
                if ui.button("Cancel").clicked() {
                    choice = ConnectChoice::Cancel;
                }
            });
        });
    if !open || ctx.input(|i| i.key_pressed(egui::Key::Escape)) {
        choice = ConnectChoice::Cancel;
    }
    choice
}

/// The level of a notice.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum NoticeLevel {
    /// Information.
    Info,
    /// A warning.
    Warning,
    /// An error.
    Error,
}

/// A short message in the corner of the window.
#[derive(Debug, Clone, PartialEq)]
pub struct Notice {
    /// The text.
    pub text: String,
    /// The level.
    pub level: NoticeLevel,
    /// The time when the notice appeared (egui time, seconds).
    pub since: f64,
}

/// How long a notice stays, in seconds.
pub const NOTICE_SECONDS: f64 = 7.0;

/// Draws the notices and removes the old ones.
pub fn notices(ctx: &egui::Context, notices: &mut Vec<Notice>) {
    let now = ctx.input(|i| i.time);
    notices.retain(|n| now - n.since < NOTICE_SECONDS);
    if notices.is_empty() {
        return;
    }
    ctx.request_repaint_after(std::time::Duration::from_millis(500));
    egui::Area::new(egui::Id::new("fasttick_notices"))
        .anchor(Align2::RIGHT_BOTTOM, [-16.0, -40.0])
        .order(egui::Order::Foreground)
        .interactable(true)
        .show(ctx, |ui| {
            let mut dismiss = None;
            for (index, notice) in notices.iter().enumerate() {
                let color = match notice.level {
                    NoticeLevel::Info => ui.visuals().hyperlink_color,
                    NoticeLevel::Warning => ui.visuals().warn_fg_color,
                    NoticeLevel::Error => ui.visuals().error_fg_color,
                };
                egui::Frame::popup(ui.style()).show(ui, |ui| {
                    ui.set_max_width(420.0);
                    ui.horizontal(|ui| {
                        crate::widgets::status_dot(ui, color);
                        ui.label(&notice.text);
                        if ui.small_button("Close").clicked() {
                            dismiss = Some(index);
                        }
                    });
                });
                ui.add_space(4.0);
            }
            if let Some(index) = dismiss {
                notices.remove(index);
            }
        });
}
