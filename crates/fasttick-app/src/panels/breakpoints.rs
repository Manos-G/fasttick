//! The breakpoints panel (live mode): all hooks of the executor, as
//! breakpoints and as forced results.
//!
//! The list comes from the hook mirror of [`crate::live::LiveController`]:
//! the changes of this session at once, and the hook list of the executor
//! (HOOKS_DUMP) a moment later. Hooks of other clients show too.

use egui::{RichText, Ui};
use fasttick_core::runtime::Uid;
use fasttick_core::status::NodeStatus;
use fasttick_protocol::messages::Hook;

use super::{PanelContext, empty};
use crate::actions::AppAction;
use crate::app::Mode;
use crate::jobs::DialogPurpose;
use crate::live::{AFTER_BREAKPOINT_UNAVAILABLE, ResumeAction};
use crate::shortcuts::Command;
use crate::widgets;

/// Draws the breakpoints panel.
pub fn show(ui: &mut Ui, cx: &mut PanelContext<'_>) {
    if cx.mode != Mode::Live {
        empty(ui, "Breakpoints are available in live mode.");
        return;
    }
    let connected = cx.live.is_connected();
    ui.horizontal_wrapped(|ui| {
        let has_hooks = !cx.live.hooks().is_empty();
        if ui
            .add_enabled(has_hooks, egui::Button::new("Save\u{2026}"))
            .on_hover_text("Save the breakpoints and forced results to a file.")
            .clicked()
        {
            cx.actions
                .push(AppAction::PickFile(DialogPurpose::SaveBreakpoints));
        }
        if ui
            .add_enabled(connected, egui::Button::new("Load\u{2026}"))
            .on_hover_text("Load breakpoints from a file and insert them.")
            .on_disabled_hover_text("Connect to an executor first.")
            .clicked()
        {
            cx.actions.push(AppAction::Command(Command::LoadBreakpoints));
        }
        if ui
            .add_enabled(connected, egui::Button::new("Refresh"))
            .on_hover_text("Read the hook list of the executor again.")
            .clicked()
        {
            cx.live.request_dump();
        }
        if ui
            .add_enabled(connected && has_hooks, egui::Button::new("Remove all"))
            .on_hover_text("Remove all hooks of the executor, also of other clients.")
            .clicked()
        {
            cx.live.remove_all_hooks();
        }
    });
    if !connected {
        ui.label(
            RichText::new(
                "Not connected. When the connection closes, the executor removes the \
                 breakpoints of this session.",
            )
            .small()
            .weak(),
        );
    }
    ui.add_space(6.0);

    egui::ScrollArea::vertical()
        .auto_shrink([false, false])
        .show(ui, |ui| {
            let breakpoints: Vec<Hook> = cx.live.breakpoints().copied().collect();
            ui.label(RichText::new(format!("Breakpoints ({})", breakpoints.len())).strong());
            if breakpoints.is_empty() {
                ui.label(
                    RichText::new(
                        "No breakpoints. Right-click a node and choose \
                         \u{201C}Add breakpoint before the node\u{201D}, or press F9.",
                    )
                    .weak(),
                );
            } else {
                egui::Grid::new("breakpoint_rows")
                    .num_columns(4)
                    .spacing([8.0, 4.0])
                    .striped(true)
                    .show(ui, |ui| {
                        for hook in &breakpoints {
                            breakpoint_row(ui, cx, hook, connected);
                            ui.end_row();
                        }
                    });
            }
            ui.add_space(10.0);

            let forced: Vec<Hook> = cx.live.forced().copied().collect();
            ui.label(RichText::new(format!("Forced results ({})", forced.len())).strong());
            if forced.is_empty() {
                ui.label(
                    RichText::new(
                        "No forced results. Right-click a node and choose \
                         \u{201C}Force SUCCESS\u{201D} or \u{201C}Force FAILURE\u{201D}.",
                    )
                    .weak(),
                );
            } else {
                egui::Grid::new("forced_rows")
                    .num_columns(3)
                    .spacing([8.0, 4.0])
                    .striped(true)
                    .show(ui, |ui| {
                        for hook in &forced {
                            forced_row(ui, cx, hook, connected);
                            ui.end_row();
                        }
                    });
            }
            ui.add_space(10.0);
            ui.label(
                RichText::new(format!(
                    "Breakpoints stop before the node. Stops after a node are not \
                     available: {AFTER_BREAKPOINT_UNAVAILABLE}."
                ))
                .small()
                .weak(),
            );
        });
}

/// The node label of a hook: name and path, or the UID if the tree does
/// not have it. Clicking it shows the node.
fn node_label(ui: &mut Ui, cx: &mut PanelContext<'_>, uid: Uid) {
    match cx.live.node_text(uid) {
        Some((name, path)) => {
            let name = if name.is_empty() {
                format!("UID {}", uid.0)
            } else {
                name
            };
            ui.vertical(|ui| {
                let response = ui
                    .add(egui::Button::new(RichText::new(name).strong()).frame(false))
                    .on_hover_text("Show the node");
                if response.clicked()
                    && let Some(doc) = cx.live_doc
                {
                    cx.actions.push(AppAction::RevealNode(doc, uid));
                }
                ui.label(RichText::new(path).small().weak());
            });
        }
        None => {
            ui.label(format!("UID {} (not in the tree)", uid.0));
        }
    }
}

fn breakpoint_row(ui: &mut Ui, cx: &mut PanelContext<'_>, hook: &Hook, connected: bool) {
    let uid = hook.node_uid();
    let hit = cx.live.is_hit(uid);
    let color = if hit || hook.enabled {
        cx.palette.breakpoint
    } else {
        ui.visuals().weak_text_color()
    };
    ui.horizontal(|ui| {
        widgets::status_dot(ui, color);
        node_label(ui, cx, uid);
    });
    ui.add_enabled_ui(connected, |ui| {
        ui.vertical(|ui| {
            let mut enabled = hook.enabled;
            if ui
                .checkbox(&mut enabled, "Enabled")
                .on_hover_text("A disabled breakpoint stays in the list but does not stop.")
                .changed()
            {
                cx.live.set_enabled(uid, enabled);
            }
            let mut once = hook.once;
            if ui
                .checkbox(&mut once, "Once")
                .on_hover_text("Remove the breakpoint after it stops one time.")
                .changed()
            {
                cx.live.set_once(uid, once);
            }
        });
    });
    if hit {
        if ui
            .add_enabled(connected, egui::Button::new("Continue"))
            .clicked()
        {
            cx.live.resume(uid, ResumeAction::Continue);
        }
    } else {
        ui.label("");
    }
    if ui
        .add_enabled(connected, egui::Button::new("Remove"))
        .clicked()
    {
        cx.live.remove_hook(uid);
    }
}

fn forced_row(ui: &mut Ui, cx: &mut PanelContext<'_>, hook: &Hook, connected: bool) {
    let uid = hook.node_uid();
    let (text, color) = match hook.desired_status {
        NodeStatus::Success => ("SUCCESS", cx.palette.success),
        NodeStatus::Failure => ("FAILURE", cx.palette.failure),
        other => (other.as_str(), ui.visuals().weak_text_color()),
    };
    ui.horizontal(|ui| {
        widgets::status_dot(ui, color);
        node_label(ui, cx, uid);
    });
    let mut label = RichText::new(text).strong().color(color);
    if !hook.enabled {
        label = label.strikethrough();
    }
    ui.label(label).on_hover_text(if hook.enabled {
        "The node does not tick. It returns this status."
    } else {
        "This forced result is disabled."
    });
    if ui
        .add_enabled(connected, egui::Button::new("Clear"))
        .on_hover_text("Clear the forced result: the node ticks normally again.")
        .clicked()
    {
        cx.live.remove_hook(uid);
    }
}
