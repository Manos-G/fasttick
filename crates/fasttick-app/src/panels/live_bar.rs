//! The bar at the top of live mode: the connection (host, port, state),
//! the poll interval, the breakpoint button, the recording controls; and
//! the banner that shows when the executor waits at a breakpoint.

use egui::{Color32, RichText, Ui};
use fasttick_core::runtime::Uid;
use fasttick_protocol::session::ConnectionState;

use super::PanelContext;
use crate::actions::AppAction;
use crate::documents::{DocId, Workspace};
use crate::jobs::DialogPurpose;
use crate::live::ResumeAction;
use crate::settings::{MAX_POLL_MS, MIN_POLL_MS};
use crate::shortcuts::Command;
use crate::theme::mix;
use crate::widgets::{self, Icon};

/// The slider range of the poll interval, in milliseconds.
pub const POLL_SLIDER_MS: (u32, u32) = (20, 2000);

/// The UID of the selected node in the live tree: the primary selection of
/// the active view of the live document, or of its first view.
pub fn selected_live_uid(workspace: &Workspace, live_doc: Option<DocId>) -> Option<Uid> {
    let view = workspace.view(workspace.view_for_doc(live_doc?)?)?;
    let tree = view.canvas.tree();
    let id = view
        .canvas
        .primary()
        .filter(|id| tree.is_selected(*id))
        .or_else(|| tree.selected().next())?;
    tree.get(id)?.uid
}

/// The colour of a connection state.
pub fn state_color(state: ConnectionState, cx: &PanelContext<'_>) -> Color32 {
    match state {
        ConnectionState::Connected { .. } => cx.palette.success,
        ConnectionState::Disconnected => cx.palette.skipped,
        ConnectionState::Connecting | ConnectionState::Reconnecting { .. } => cx.palette.running,
    }
}

/// Draws the live bar.
pub fn show_bar(ui: &mut Ui, cx: &mut PanelContext<'_>) {
    ui.horizontal_wrapped(|ui| {
        ui.spacing_mut().item_spacing.x = 6.0;
        connection(ui, cx);
        ui.separator();
        poll_interval(ui, cx);
        ui.separator();
        breakpoint_button(ui, cx);
        ui.separator();
        recording(ui, cx);
    });
}

fn connection(ui: &mut Ui, cx: &mut PanelContext<'_>) {
    let state = cx.live.state();
    widgets::status_dot(ui, state_color(state, cx));
    ui.label(RichText::new(cx.live.state_text()).strong());
    match state {
        ConnectionState::Disconnected => {
            let label = ui.label("Host");
            ui.add(egui::TextEdit::singleline(&mut cx.settings.live_host).desired_width(120.0))
                .labelled_by(label.id);
            let label = ui.label("Port");
            ui.add(egui::DragValue::new(&mut cx.settings.live_port).range(1..=65534))
                .labelled_by(label.id)
                .on_hover_text("The server port. The notifications use the next port.");
            let valid = !cx.settings.live_host.trim().is_empty();
            if ui
                .add_enabled(valid, egui::Button::new(RichText::new("Connect").strong()))
                .clicked()
            {
                cx.actions.push(AppAction::Connect {
                    host: cx.settings.live_host.trim().to_owned(),
                    port: cx.settings.live_port,
                });
            }
        }
        _ => {
            if let ConnectionState::Connected { uuid } = state {
                let text = uuid.to_string();
                let short: String = text.chars().take(8).collect();
                ui.label(RichText::new(format!("tree {short}")).small().weak())
                    .on_hover_text(format!(
                        "The UUID of the running tree: {text}. A new UUID means that the \
                         executor restarted."
                    ));
            }
            if ui.button("Disconnect").clicked() {
                cx.actions.push(AppAction::Command(Command::Disconnect));
            }
        }
    }
}

fn poll_interval(ui: &mut Ui, cx: &mut PanelContext<'_>) {
    let label = ui.label("Poll");
    let (low, high) = POLL_SLIDER_MS;
    let mut ms = cx.settings.poll_interval_ms.clamp(low, high);
    let response = ui
        .add(
            egui::Slider::new(&mut ms, low..=high)
                .logarithmic(true)
                .suffix(" ms")
                .integer(),
        )
        .labelled_by(label.id)
        .on_hover_text(
            "How often FastTick reads the node statuses. The connection also sends a \
             heartbeat each second.",
        );
    if response.changed() {
        cx.settings.poll_interval_ms = ms.clamp(MIN_POLL_MS, MAX_POLL_MS);
        cx.live.set_poll_interval(cx.settings.poll_interval_ms);
    }
}

fn breakpoint_button(ui: &mut Ui, cx: &mut PanelContext<'_>) {
    let selected = selected_live_uid(cx.workspace, cx.live_doc);
    let enabled = cx.live.is_connected() && selected.is_some();
    let has = selected
        .and_then(|uid| cx.live.hook(uid))
        .is_some_and(|hook| hook.mode == fasttick_protocol::messages::HookMode::Breakpoint);
    let label = if has {
        "Remove the breakpoint of the selected node (F9)"
    } else {
        "Add a breakpoint before the selected node (F9)"
    };
    let response = widgets::icon_button_colored(
        ui,
        Icon::Breakpoint,
        label,
        enabled,
        Some(cx.palette.breakpoint),
    );
    let response = if enabled {
        response
    } else {
        response.on_hover_text("Connect and select a node first.")
    };
    if response.clicked()
        && let Some(uid) = selected
    {
        cx.live.toggle_breakpoint(uid);
    }
}

fn recording(ui: &mut Ui, cx: &mut PanelContext<'_>) {
    let now = std::time::Instant::now();
    let recording = cx.live.recording();
    if recording.is_active() {
        let count = recording.len();
        let seconds = recording.duration(now).as_secs();
        let loss = recording.possible_loss();
        if widgets::icon_button_colored(
            ui,
            Icon::Stop,
            "Stop recording",
            true,
            Some(cx.palette.breakpoint),
        )
        .clicked()
        {
            cx.live.stop_recording();
        }
        ui.label(
            RichText::new(format!("Recording: {count} transitions, {seconds} s"))
                .color(cx.palette.breakpoint),
        );
        if loss {
            ui.label(
                RichText::new("Some transitions can be lost").color(ui.visuals().warn_fg_color),
            )
            .on_hover_text(
                "The executor keeps only the last 1000 transitions. FastTick reads them \
                 every 100 ms. When the tree changes faster, older transitions are lost.",
            );
        }
        ui.ctx()
            .request_repaint_after(std::time::Duration::from_millis(500));
        return;
    }
    let can_start = cx.live.is_connected() && !cx.live.recording_requested();
    let start = widgets::icon_button_colored(
        ui,
        Icon::Record,
        "Start recording",
        can_start,
        Some(cx.palette.breakpoint),
    );
    let start = if can_start {
        start
    } else {
        start.on_hover_text("Connect to an executor to record its transitions.")
    };
    if start.clicked() {
        cx.live.start_recording();
    }
    let recording = cx.live.recording();
    if recording.has_data() {
        let count = recording.len();
        let loss = recording.possible_loss();
        let mut text = format!("Recorded {count} transitions");
        if loss {
            text.push_str(" (some can be lost)");
        }
        ui.label(text);
        if ui
            .button("Save\u{2026}")
            .on_hover_text("Save the recording as a .btlog file.")
            .clicked()
        {
            cx.actions
                .push(AppAction::PickFile(DialogPurpose::SaveRecording));
        }
        if ui.small_button("Discard").clicked() {
            cx.live.discard_recording();
        }
    } else if cx.live.recording_requested() {
        ui.spinner();
    } else {
        ui.label(RichText::new("Record").weak());
    }
}

/// Draws the banner for the nodes that wait at a breakpoint. Draws
/// nothing if no node waits.
pub fn show_banner(ui: &mut Ui, cx: &mut PanelContext<'_>) {
    let hits = cx.live.hits().to_vec();
    if hits.is_empty() {
        return;
    }
    let color = cx.palette.breakpoint;
    egui::Frame::new()
        .fill(mix(ui.visuals().panel_fill, color, 0.14))
        .stroke(egui::Stroke::new(1.0, color))
        .corner_radius(5)
        .inner_margin(egui::Margin::symmetric(10, 6))
        .show(ui, |ui| {
            ui.set_width(ui.available_width());
            for uid in hits {
                ui.horizontal(|ui| {
                    widgets::status_dot(ui, color);
                    let (name, path) = cx
                        .live
                        .node_text(uid)
                        .unwrap_or_else(|| (format!("UID {}", uid.0), String::new()));
                    ui.label(RichText::new("Stopped before").strong());
                    ui.label(RichText::new(&name).strong().monospace());
                    if !path.is_empty() && path != name {
                        ui.label(RichText::new(&path).weak());
                    }
                    ui.add_space(12.0);
                    let connected = cx.live.is_connected();
                    for action in [
                        ResumeAction::Continue,
                        ResumeAction::ForceSuccess,
                        ResumeAction::ForceFailure,
                    ] {
                        let hint = match action {
                            ResumeAction::Continue => "The node ticks normally (F8).",
                            ResumeAction::ForceSuccess => {
                                "The node does not tick. It returns SUCCESS."
                            }
                            ResumeAction::ForceFailure => {
                                "The node does not tick. It returns FAILURE."
                            }
                        };
                        let mut text = RichText::new(action.label());
                        if action == ResumeAction::Continue {
                            text = text.strong();
                        }
                        if ui
                            .add_enabled(connected, egui::Button::new(text))
                            .on_hover_text(hint)
                            .clicked()
                        {
                            cx.live.resume(uid, action);
                        }
                    }
                    if ui.button("Show node").clicked()
                        && let Some(doc) = cx.live_doc
                    {
                        cx.actions.push(AppAction::RevealNode(doc, uid));
                    }
                });
            }
        });
}
