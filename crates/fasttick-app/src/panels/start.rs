//! The start page of each mode: the main actions of the mode, the recent
//! files, what is new in this version and some tips.

use egui::{RichText, Ui};
use fasttick_protocol::session::ConnectionState;

use super::PanelContext;
use crate::actions::AppAction;
use crate::app::Mode;
use crate::settings::RecentKind;
use crate::shortcuts::Command;
use crate::widgets;

/// The most recent files that the page shows.
const MAX_RECENT: usize = 12;

/// The page is in two columns if it is at least this wide.
const TWO_COLUMNS: f32 = 820.0;

/// What is new in this version: a title and a short text.
pub const WHATS_NEW: &[(&str, &str)] = &[
    (
        "Add nodes from the dot",
        "Drag from the dot under a node and release where the new node must go, \
         or push Shift+A. A search window with fuzzy matching asks for the type; \
         the types that you used last come first.",
    ),
    (
        "Search commands",
        "Push Ctrl+Shift+P to find a command, a tree, a recent file or a theme by \
         its name. Ctrl+P goes to a tree of the document.",
    ),
    (
        "Export an image",
        "File > Export tree as SVG writes the tree of the tab as an SVG image, \
         with the statuses in live and replay mode.",
    ),
    (
        "Fold large trees",
        "Double-click a node to hide or show the nodes below it.",
    ),
    (
        "Blackboard keys",
        "The keys menu of a port lists the blackboard keys that the tree uses.",
    ),
    (
        "Pre- and post-conditions",
        "The conditions show in a box on the cards. FastTick checks each script \
         with the grammar of BT.CPP and shows the message of BT.CPP.",
    ),
    (
        "Missing SubTree files",
        "When a tree uses SubTrees from other files of its folder, FastTick finds \
         the files and adds them to the project with one click.",
    ),
    (
        "Descriptions",
        "Put the pointer on a node or on a palette entry to see what the node does.",
    ),
    (
        "All ports on the cards",
        "The cards show all ports and remappings. A long value, for example a \
         script, continues on the next lines.",
    ),
    (
        "Twelve themes",
        "OpenWire, Catppuccin and VS Code themes, in View > Theme.",
    ),
];

/// Short tips, as (keys, text).
const TIPS: &[(&str, &str)] = &[
    ("F1", "Show all keyboard shortcuts."),
    (
        "Ctrl+Shift+P",
        "Find a command, a tree or a theme by its name.",
    ),
    ("Shift+A", "Add a node to the selected node."),
    ("F", "Fit the tree to the view."),
    ("Ctrl+L", "Change the layout direction."),
    (
        "Double-click",
        "Expand a SubTree, or fold the nodes below a node.",
    ),
    ("Ctrl+F", "Search the nodes of the tree."),
    ("Ctrl+1/2/3", "Change to edit, live or replay mode."),
];

/// Draws the start page.
pub fn show(ui: &mut Ui, cx: &mut PanelContext<'_>) {
    egui::ScrollArea::vertical()
        .auto_shrink([false, false])
        .show(ui, |ui| {
            ui.add_space(28.0);
            let width = ui.available_width().min(1040.0);
            let left = ((ui.available_width() - width) * 0.5).max(0.0);
            ui.horizontal(|ui| {
                ui.add_space(left);
                ui.vertical(|ui| {
                    ui.set_width(width);
                    header(ui, cx);
                    ui.add_space(22.0);
                    if width >= TWO_COLUMNS {
                        ui.columns(2, |columns| {
                            columns[0].vertical(|ui| {
                                start_section(ui, cx);
                                ui.add_space(20.0);
                                recent(ui, cx);
                            });
                            columns[1].vertical(|ui| {
                                whats_new(ui);
                                ui.add_space(20.0);
                                tips(ui);
                            });
                        });
                    } else {
                        start_section(ui, cx);
                        ui.add_space(20.0);
                        recent(ui, cx);
                        ui.add_space(20.0);
                        whats_new(ui);
                        ui.add_space(20.0);
                        tips(ui);
                    }
                    ui.add_space(24.0);
                });
            });
        });
}

/// The logo, the name, the version and what FastTick is.
fn header(ui: &mut Ui, cx: &mut PanelContext<'_>) {
    ui.horizontal(|ui| {
        ui.scope(|ui| {
            ui.spacing_mut().interact_size.y = 44.0;
            widgets::logo(ui, cx.palette.accent);
        });
        ui.vertical(|ui| {
            ui.horizontal(|ui| {
                ui.label(medium("FastTick", 26.0));
                ui.label(
                    RichText::new(format!("version {}", env!("CARGO_PKG_VERSION")))
                        .small()
                        .weak(),
                );
            });
            ui.label(
                RichText::new("Tree editor, live debugger and log replayer for BehaviorTree.CPP 4")
                    .weak(),
            );
        });
    });
}

/// Text in the medium font.
fn medium(text: &str, size: f32) -> RichText {
    RichText::new(text)
        .family(egui::FontFamily::Name(crate::fonts::MEDIUM.into()))
        .size(size)
        .strong()
}

/// A section title.
fn heading(ui: &mut Ui, text: &str) {
    ui.label(medium(text, 16.0));
    ui.add_space(6.0);
}

/// The main actions of the mode, and the ways to the other modes.
fn start_section(ui: &mut Ui, cx: &mut PanelContext<'_>) {
    heading(ui, "Start");
    match cx.mode {
        Mode::Edit => edit_actions(ui, cx),
        Mode::Live => live_actions(ui, cx),
        Mode::Replay => replay_actions(ui, cx),
    }
    ui.add_space(10.0);
    ui.horizontal_wrapped(|ui| {
        for (mode, command, text) in [
            (Mode::Edit, Command::ModeEdit, "Edit trees"),
            (Mode::Live, Command::ModeLive, "Debug a running tree"),
            (Mode::Replay, Command::ModeReplay, "Replay a log"),
        ] {
            if mode != cx.mode && ui.link(text).clicked() {
                cx.actions.push(AppAction::Command(command));
            }
        }
    });
}

fn action_button(ui: &mut Ui, cx: &mut PanelContext<'_>, command: Command, text: &str) {
    let shortcut = cx.keymap.shortcut_text(ui.ctx(), command);
    let mut button =
        egui::Button::new(RichText::new(text).size(14.5)).min_size(egui::vec2(260.0, 34.0));
    if let Some(shortcut) = shortcut {
        button = button.shortcut_text(shortcut);
    }
    if ui.add(button).clicked() {
        cx.actions.push(AppAction::Command(command));
    }
}

fn edit_actions(ui: &mut Ui, cx: &mut PanelContext<'_>) {
    ui.label("Open a tree XML file or a project, or make a new project.");
    ui.add_space(8.0);
    action_button(
        ui,
        cx,
        Command::OpenFile,
        "Open tree XML or project\u{2026}",
    );
    ui.add_space(4.0);
    action_button(ui, cx, Command::NewProject, "New project");
}

fn replay_actions(ui: &mut Ui, cx: &mut PanelContext<'_>) {
    ui.label(
        "Open a .btlog file that FileLogger2 wrote, a SQLite log of SqliteLogger, or a \
         recording that FastTick saved.",
    );
    ui.add_space(8.0);
    action_button(ui, cx, Command::OpenLog, "Open log\u{2026}");
    for loading in cx.loading {
        ui.add_space(10.0);
        let name = loading
            .path
            .file_name()
            .map(|n| n.to_string_lossy().into_owned())
            .unwrap_or_default();
        let (text, fraction) = match loading.progress.phase() {
            crate::jobs::LoadPhase::Reading => {
                (format!("Reading {name}"), loading.progress.fraction())
            }
            crate::jobs::LoadPhase::Indexing => (format!("Indexing {name}"), 1.0),
        };
        ui.add(
            egui::ProgressBar::new(fraction)
                .desired_width(300.0)
                .text(text)
                .animate(true),
        );
        ui.ctx()
            .request_repaint_after(std::time::Duration::from_millis(100));
    }
}

fn live_actions(ui: &mut Ui, cx: &mut PanelContext<'_>) {
    ui.label("Connect to an executor that runs the BT.CPP publisher.");
    ui.add_space(8.0);
    ui.vertical(|ui| {
        ui.set_width(320.0);
        connect_form(ui, cx);
    });
}

fn connect_form(ui: &mut Ui, cx: &mut PanelContext<'_>) {
    egui::Frame::group(ui.style())
        .inner_margin(12.0)
        .show(ui, |ui| {
            ui.set_width(ui.available_width());
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
    heading(ui, "Recent");
    // The files of this mode first, then the others.
    let first: &[RecentKind] = match cx.mode {
        Mode::Edit => &[RecentKind::Tree, RecentKind::Project],
        Mode::Replay => &[RecentKind::Log],
        Mode::Live => &[],
    };
    let mut entries: Vec<_> = cx.settings.recent.to_vec();
    entries.sort_by_key(|r| !first.contains(&r.kind));
    entries.truncate(MAX_RECENT);
    if entries.is_empty() {
        ui.label(RichText::new("The files that you open show here.").weak());
        return;
    }
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
        let exists = entry.path.exists();
        ui.horizontal(|ui| {
            let kind = match entry.kind {
                RecentKind::Tree => "tree",
                RecentKind::Project => "project",
                RecentKind::Log => "log",
            };
            ui.add_sized(
                [52.0, 18.0],
                egui::Label::new(RichText::new(kind).small().weak()),
            );
            let mut text = RichText::new(&name);
            if !exists {
                text = text.strikethrough().weak();
            }
            let response = ui
                .add_enabled(exists, egui::Button::new(text).frame(false))
                .on_hover_text(entry.path.display().to_string())
                .on_disabled_hover_text("The file is not there any more.");
            if response.clicked() {
                cx.actions.push(AppAction::OpenPath(entry.path.clone()));
            }
            ui.label(RichText::new(dir).small().weak());
        });
    }
    ui.add_space(4.0);
    if ui.small_button("Clear the list").clicked() {
        cx.actions.push(AppAction::ClearRecent);
    }
}

/// What is new in this version.
fn whats_new(ui: &mut Ui) {
    heading(ui, "What's new");
    for (title, text) in WHATS_NEW {
        ui.label(medium(title, 13.5));
        ui.label(RichText::new(*text).weak());
        ui.add_space(6.0);
    }
}

/// Keyboard and mouse tips.
fn tips(ui: &mut Ui) {
    heading(ui, "Tips");
    egui::Grid::new("start_tips")
        .num_columns(2)
        .spacing([12.0, 4.0])
        .show(ui, |ui| {
            for (keys, text) in TIPS {
                ui.label(RichText::new(*keys).monospace());
                ui.label(*text);
                ui.end_row();
            }
        });
    ui.add_space(6.0);
    ui.label(
        RichText::new("The user guide is in docs/USER_GUIDE.md of the source code.")
            .small()
            .weak(),
    );
}
