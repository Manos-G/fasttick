//! The start page of each mode: the main action of the mode and the
//! recent files.

use egui::{RichText, Ui};
use fasttick_protocol::session::ConnectionState;

use super::PanelContext;
use crate::actions::AppAction;
use crate::app::Mode;
use crate::settings::RecentKind;
use crate::shortcuts::Command;
use crate::widgets;

/// Draws the start page.
pub fn show(ui: &mut Ui, cx: &mut PanelContext<'_>) {
    egui::ScrollArea::vertical().show(ui, |ui| {
        ui.add_space(36.0);
        ui.vertical_centered(|ui| {
            ui.set_max_width(520.0);
            ui.scope(|ui| {
                ui.spacing_mut().interact_size.y = 44.0;
                widgets::logo(ui, cx.palette.accent);
            });
            ui.label(RichText::new("FastTick").size(26.0).strong());
            ui.label(
                RichText::new("Tree editor, live debugger and log replayer for BehaviorTree.CPP 4")
                    .weak(),
            );
            ui.add_space(20.0);
            match cx.mode {
                Mode::Edit => edit_actions(ui, cx),
                Mode::Live => live_actions(ui, cx),
                Mode::Replay => replay_actions(ui, cx),
            }
            ui.add_space(24.0);
            recent(ui, cx);
        });
    });
}

fn action_button(ui: &mut Ui, cx: &mut PanelContext<'_>, command: Command, text: &str) {
    let shortcut = cx.keymap.shortcut_text(ui.ctx(), command);
    let mut button =
        egui::Button::new(RichText::new(text).size(14.5)).min_size(egui::vec2(220.0, 34.0));
    if let Some(shortcut) = shortcut {
        button = button.shortcut_text(shortcut);
    }
    if ui.add(button).clicked() {
        cx.actions.push(AppAction::Command(command));
    }
}

fn edit_actions(ui: &mut Ui, cx: &mut PanelContext<'_>) {
    ui.label("Open a tree XML file to show its trees. Editing comes in a later version.");
    ui.add_space(8.0);
    action_button(ui, cx, Command::OpenFile, "Open tree XML\u{2026}");
}

fn replay_actions(ui: &mut Ui, cx: &mut PanelContext<'_>) {
    ui.label("Open a .btlog file that FileLogger2 wrote.");
    ui.add_space(8.0);
    action_button(ui, cx, Command::OpenLog, "Open log\u{2026}");
}

fn live_actions(ui: &mut Ui, cx: &mut PanelContext<'_>) {
    ui.label("Connect to an executor that runs the BT.CPP publisher.");
    ui.add_space(8.0);
    egui::Frame::group(ui.style())
        .inner_margin(12.0)
        .show(ui, |ui| {
            egui::Grid::new("start_connect")
                .num_columns(2)
                .spacing([10.0, 8.0])
                .show(ui, |ui| {
                    ui.label("Host");
                    ui.add(
                        egui::TextEdit::singleline(&mut cx.settings.live_host).desired_width(180.0),
                    );
                    ui.end_row();
                    ui.label("Port");
                    ui.add(egui::DragValue::new(&mut cx.settings.live_port).range(1..=65534));
                    ui.end_row();
                });
            ui.add_space(6.0);
            let state = cx.live.state();
            ui.horizontal(|ui| {
                let connected = !matches!(state, ConnectionState::Disconnected);
                if connected {
                    if ui.button("Disconnect").clicked() {
                        cx.actions.push(AppAction::Command(Command::Disconnect));
                    }
                } else if ui
                    .add(
                        egui::Button::new(RichText::new("Connect").strong())
                            .min_size(egui::vec2(120.0, 28.0)),
                    )
                    .clicked()
                {
                    cx.actions.push(AppAction::Connect {
                        host: cx.settings.live_host.trim().to_owned(),
                        port: cx.settings.live_port,
                    });
                }
                ui.label(RichText::new(cx.live.state_text()).weak());
            });
            if let Some(message) = &cx.live.last_message {
                ui.label(
                    RichText::new(message)
                        .small()
                        .color(ui.visuals().warn_fg_color),
                );
            }
        });
}

fn recent(ui: &mut Ui, cx: &mut PanelContext<'_>) {
    let wanted: &[RecentKind] = match cx.mode {
        Mode::Edit => &[RecentKind::Tree, RecentKind::Project],
        Mode::Replay => &[RecentKind::Log],
        Mode::Live => &[],
    };
    let entries: Vec<_> = cx
        .settings
        .recent
        .iter()
        .filter(|r| wanted.contains(&r.kind))
        .cloned()
        .collect();
    if entries.is_empty() {
        return;
    }
    ui.label(RichText::new("Recent").strong());
    ui.add_space(4.0);
    for entry in entries {
        let name = entry
            .path
            .file_name()
            .map(|n| n.to_string_lossy().into_owned())
            .unwrap_or_else(|| entry.path.display().to_string());
        let dir = entry
            .path
            .parent()
            .map(|p| p.display().to_string())
            .unwrap_or_default();
        let response = ui
            .add(egui::Button::new(RichText::new(&name)).frame(false))
            .on_hover_text(entry.path.display().to_string());
        if response.clicked() {
            cx.actions.push(AppAction::OpenPath(entry.path.clone()));
        }
        ui.label(RichText::new(dir).small().weak());
        ui.add_space(2.0);
    }
}
