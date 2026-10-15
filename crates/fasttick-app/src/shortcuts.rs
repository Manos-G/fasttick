//! Commands and the central keyboard shortcut registry ([`KeyMap`]).
//!
//! Menus, toolbar buttons and the shortcut handler all use [`Command`]. The
//! menus show the shortcut of each command from the key map, and the help
//! dialog lists all of them. To add a command: add a variant, give it a
//! label and a group, add a binding in [`KeyMap::default`] if it has a
//! shortcut, and handle it in `FastTickApp::run_command`.

use egui::{Key, KeyboardShortcut, Modifiers};

/// A user command.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Command {
    /// Make a new project.
    NewProject,
    /// Open a project or a tree XML file.
    OpenFile,
    /// Open a `.btlog` file.
    OpenLog,
    /// Save the active document.
    Save,
    /// Save the active document under a new name.
    SaveAs,
    /// Close the window.
    Quit,
    /// Undo the last edit.
    Undo,
    /// Redo the last undone edit.
    Redo,
    /// Cut the selection.
    Cut,
    /// Copy the selection.
    Copy,
    /// Paste.
    Paste,
    /// Duplicate the selection.
    Duplicate,
    /// Delete the selection.
    Delete,
    /// Select all nodes.
    SelectAll,
    /// Add a node to the selected node (a search window asks for the type).
    AddNode,
    /// Find nodes.
    Find,
    /// Show the whole tree.
    FitToView,
    /// Zoom in.
    ZoomIn,
    /// Zoom out.
    ZoomOut,
    /// Zoom to 100 %.
    ZoomReset,
    /// Lay out the tree from top to bottom.
    LayoutTopDown,
    /// Lay out the tree from left to right.
    LayoutLeftRight,
    /// Change between the two layout directions.
    ToggleLayout,
    /// Use the light theme.
    ThemeLight,
    /// Use the dark theme.
    ThemeDark,
    /// Follow the system theme.
    ThemeSystem,
    /// Change to edit mode.
    ModeEdit,
    /// Change to live mode.
    ModeLive,
    /// Change to replay mode.
    ModeReplay,
    /// Connect to an executor.
    Connect,
    /// Disconnect from the executor.
    Disconnect,
    /// Add or remove a breakpoint on the selected node (live mode).
    ToggleBreakpoint,
    /// Let the node at the breakpoint continue (live mode).
    Continue,
    /// Start or stop a recording (live mode).
    ToggleRecording,
    /// Save the recording as a `.btlog` file.
    SaveRecording,
    /// Save the breakpoints to a file.
    SaveBreakpoints,
    /// Load breakpoints from a file.
    LoadBreakpoints,
    /// Start or stop playback (replay mode).
    PlayPause,
    /// Take back one transition.
    StepBack,
    /// Apply one transition.
    StepForward,
    /// Go to the start of the log (or of the time range).
    GoToStart,
    /// Go to the end of the log (or of the time range).
    GoToEnd,
    /// Show the whole log on the timeline.
    ResetTimelineZoom,
    /// Export the filtered transitions as JSON.
    ExportJson,
    /// Export the filtered transitions as CSV.
    ExportCsv,
    /// Show the active document a second time, beside the first.
    SplitView,
    /// Close the active document tab.
    CloseTab,
    /// Restore the default panel arrangement.
    ResetLayout,
    /// Write the shown tree as an SVG image.
    ExportSvg,
    /// Find a command, a tree, a recent file or a theme by its name.
    SearchCommands,
    /// Find a tree of the active document by its name.
    GoToTree,
    /// Show the keyboard shortcuts.
    ShowShortcuts,
    /// Show the About dialog.
    About,
}

impl Command {
    /// All commands, in help-dialog order.
    pub const ALL: [Command; 53] = [
        Command::NewProject,
        Command::OpenFile,
        Command::OpenLog,
        Command::Save,
        Command::SaveAs,
        Command::ExportSvg,
        Command::Quit,
        Command::Undo,
        Command::Redo,
        Command::Cut,
        Command::Copy,
        Command::Paste,
        Command::Duplicate,
        Command::Delete,
        Command::SelectAll,
        Command::AddNode,
        Command::Find,
        Command::FitToView,
        Command::ZoomIn,
        Command::ZoomOut,
        Command::ZoomReset,
        Command::LayoutTopDown,
        Command::LayoutLeftRight,
        Command::ToggleLayout,
        Command::ThemeLight,
        Command::ThemeDark,
        Command::ThemeSystem,
        Command::SplitView,
        Command::CloseTab,
        Command::ResetLayout,
        Command::SearchCommands,
        Command::GoToTree,
        Command::ModeEdit,
        Command::ModeLive,
        Command::ModeReplay,
        Command::Connect,
        Command::Disconnect,
        Command::ToggleBreakpoint,
        Command::Continue,
        Command::ToggleRecording,
        Command::SaveRecording,
        Command::SaveBreakpoints,
        Command::LoadBreakpoints,
        Command::PlayPause,
        Command::StepBack,
        Command::StepForward,
        Command::GoToStart,
        Command::GoToEnd,
        Command::ResetTimelineZoom,
        Command::ExportJson,
        Command::ExportCsv,
        Command::ShowShortcuts,
        Command::About,
    ];

    /// The menu label.
    pub fn label(self) -> &'static str {
        match self {
            Command::NewProject => "New project",
            Command::OpenFile => "Open project or XML\u{2026}",
            Command::OpenLog => "Open log\u{2026}",
            Command::Save => "Save",
            Command::SaveAs => "Save as\u{2026}",
            Command::Quit => "Quit",
            Command::Undo => "Undo",
            Command::Redo => "Redo",
            Command::Cut => "Cut",
            Command::Copy => "Copy",
            Command::Paste => "Paste",
            Command::Duplicate => "Duplicate",
            Command::Delete => "Delete",
            Command::SelectAll => "Select all",
            Command::AddNode => "Add node\u{2026}",
            Command::Find => "Find\u{2026}",
            Command::FitToView => "Fit to view",
            Command::ZoomIn => "Zoom in",
            Command::ZoomOut => "Zoom out",
            Command::ZoomReset => "Zoom to 100 %",
            Command::LayoutTopDown => "Top-down",
            Command::LayoutLeftRight => "Left-right",
            Command::ToggleLayout => "Change layout direction",
            Command::ThemeLight => "Light",
            Command::ThemeDark => "Dark",
            Command::ThemeSystem => "System",
            Command::ModeEdit => "Edit mode",
            Command::ModeLive => "Live mode",
            Command::ModeReplay => "Replay mode",
            Command::Connect => "Connect\u{2026}",
            Command::Disconnect => "Disconnect",
            Command::ToggleBreakpoint => "Toggle breakpoint",
            Command::Continue => "Continue",
            Command::ToggleRecording => "Start or stop recording",
            Command::SaveRecording => "Save recording\u{2026}",
            Command::SaveBreakpoints => "Save breakpoints\u{2026}",
            Command::LoadBreakpoints => "Load breakpoints\u{2026}",
            Command::PlayPause => "Play or pause",
            Command::StepBack => "One transition back",
            Command::StepForward => "One transition forward",
            Command::GoToStart => "Go to the start",
            Command::GoToEnd => "Go to the end",
            Command::ResetTimelineZoom => "Show the whole log",
            Command::ExportJson => "Export transitions as JSON\u{2026}",
            Command::ExportCsv => "Export transitions as CSV\u{2026}",
            Command::SplitView => "Split view",
            Command::CloseTab => "Close tab",
            Command::ResetLayout => "Reset panel layout",
            Command::ExportSvg => "Export tree as SVG\u{2026}",
            Command::SearchCommands => "Search commands\u{2026}",
            Command::GoToTree => "Go to tree\u{2026}",
            Command::ShowShortcuts => "Keyboard shortcuts",
            Command::About => "About FastTick",
        }
    }

    /// The group in the help dialog.
    pub fn group(self) -> &'static str {
        match self {
            Command::NewProject
            | Command::OpenFile
            | Command::OpenLog
            | Command::Save
            | Command::SaveAs
            | Command::ExportSvg
            | Command::Quit => "File",
            Command::Undo
            | Command::Redo
            | Command::Cut
            | Command::Copy
            | Command::Paste
            | Command::Duplicate
            | Command::Delete
            | Command::SelectAll
            | Command::AddNode
            | Command::Find => "Edit",
            Command::FitToView
            | Command::ZoomIn
            | Command::ZoomOut
            | Command::ZoomReset
            | Command::LayoutTopDown
            | Command::LayoutLeftRight
            | Command::ToggleLayout
            | Command::ThemeLight
            | Command::ThemeDark
            | Command::ThemeSystem
            | Command::SplitView
            | Command::CloseTab
            | Command::ResetLayout
            | Command::SearchCommands
            | Command::GoToTree => "View",
            Command::ModeEdit | Command::ModeLive | Command::ModeReplay => "Mode",
            Command::Connect
            | Command::Disconnect
            | Command::ToggleBreakpoint
            | Command::Continue
            | Command::ToggleRecording
            | Command::SaveRecording
            | Command::SaveBreakpoints
            | Command::LoadBreakpoints => "Live",
            Command::PlayPause
            | Command::StepBack
            | Command::StepForward
            | Command::GoToStart
            | Command::GoToEnd
            | Command::ResetTimelineZoom
            | Command::ExportJson
            | Command::ExportCsv => "Replay",
            Command::ShowShortcuts | Command::About => "Help",
        }
    }
}

/// A key binding.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Binding {
    /// The command.
    pub command: Command,
    /// The key combination.
    pub shortcut: KeyboardShortcut,
}

/// The key map: all global shortcuts.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct KeyMap {
    bindings: Vec<Binding>,
}

/// Canvas keys and mouse actions. The canvas handles these itself; the
/// help dialog lists them.
pub const CANVAS_HELP: &[(&str, &str)] = &[
    ("Click", "Select a node"),
    ("Shift + click", "Add a node to the selection"),
    ("Ctrl + click", "Add or remove a node"),
    ("Drag on the background", "Select with a rectangle"),
    (
        "Drag a node (edit mode)",
        "Move it: onto a node, or onto an empty place to disconnect it",
    ),
    ("Escape during a drag", "Cancel the move"),
    ("Drag a node (live, replay)", "Pan"),
    ("Double-click a SubTree", "Expand or collapse it"),
    ("Middle drag, Space + drag", "Pan"),
    ("Mouse wheel, touchpad", "Pan"),
    ("Ctrl + wheel, pinch", "Zoom at the pointer"),
    ("Arrow keys", "Go to the parent, a child or a sibling"),
    ("Home", "Go to the root"),
    ("Enter", "Expand or collapse the SubTree"),
    ("Escape", "Clear the selection"),
    ("Right-click", "Node and view actions"),
];

impl Default for KeyMap {
    fn default() -> Self {
        let cmd = Modifiers::COMMAND;
        let cmd_shift = Modifiers::COMMAND | Modifiers::SHIFT;
        let none = Modifiers::NONE;
        let bind = |command, modifiers, key| Binding {
            command,
            shortcut: KeyboardShortcut::new(modifiers, key),
        };
        KeyMap {
            bindings: vec![
                bind(Command::NewProject, cmd, Key::N),
                bind(Command::OpenFile, cmd, Key::O),
                bind(Command::OpenLog, cmd_shift, Key::O),
                bind(Command::Save, cmd, Key::S),
                bind(Command::SaveAs, cmd_shift, Key::S),
                bind(Command::Quit, cmd, Key::Q),
                bind(Command::Undo, cmd, Key::Z),
                bind(Command::Redo, cmd_shift, Key::Z),
                bind(Command::Redo, cmd, Key::Y),
                bind(Command::Cut, cmd, Key::X),
                bind(Command::Copy, cmd, Key::C),
                bind(Command::Paste, cmd, Key::V),
                bind(Command::Duplicate, cmd, Key::D),
                bind(Command::Delete, none, Key::Delete),
                bind(Command::SelectAll, cmd, Key::A),
                bind(Command::AddNode, Modifiers::SHIFT, Key::A),
                bind(Command::Find, cmd, Key::F),
                bind(Command::FitToView, none, Key::F),
                bind(Command::ZoomIn, cmd, Key::Equals),
                bind(Command::ZoomIn, cmd, Key::Plus),
                bind(Command::ZoomOut, cmd, Key::Minus),
                bind(Command::ZoomReset, cmd, Key::Num0),
                bind(Command::ToggleLayout, cmd, Key::L),
                bind(Command::SplitView, cmd, Key::Backslash),
                bind(Command::CloseTab, cmd, Key::W),
                bind(Command::SearchCommands, cmd_shift, Key::P),
                bind(Command::GoToTree, cmd, Key::P),
                bind(Command::ModeEdit, cmd, Key::Num1),
                bind(Command::ModeLive, cmd, Key::Num2),
                bind(Command::ModeReplay, cmd, Key::Num3),
                bind(Command::Connect, none, Key::F5),
                bind(Command::Disconnect, Modifiers::SHIFT, Key::F5),
                bind(Command::Continue, none, Key::F8),
                bind(Command::ToggleBreakpoint, none, Key::F9),
                bind(Command::ToggleRecording, cmd, Key::R),
                bind(Command::PlayPause, none, Key::P),
                bind(Command::StepBack, none, Key::Comma),
                bind(Command::StepForward, none, Key::Period),
                bind(Command::GoToStart, cmd, Key::Home),
                bind(Command::GoToEnd, cmd, Key::End),
                bind(Command::ShowShortcuts, none, Key::F1),
            ],
        }
    }
}

impl KeyMap {
    /// All bindings.
    pub fn bindings(&self) -> &[Binding] {
        &self.bindings
    }

    /// The first shortcut of a command.
    pub fn shortcut(&self, command: Command) -> Option<KeyboardShortcut> {
        self.bindings
            .iter()
            .find(|b| b.command == command)
            .map(|b| b.shortcut)
    }

    /// The shortcut text of a command, for example `Ctrl+Shift+O`.
    pub fn shortcut_text(&self, ctx: &egui::Context, command: Command) -> Option<String> {
        self.shortcut(command).map(|s| format_shortcut(ctx, &s))
    }

    /// Commands that have two bindings with the same keys. Should be empty.
    pub fn conflicts(&self) -> Vec<(Command, Command)> {
        let mut out = Vec::new();
        for (i, a) in self.bindings.iter().enumerate() {
            for b in &self.bindings[i + 1..] {
                if a.shortcut == b.shortcut && a.command != b.command {
                    out.push((a.command, b.command));
                }
            }
        }
        out
    }

    /// Takes the pressed shortcuts out of the input and returns their
    /// commands. Only commands for which `enabled` is true are taken, so a
    /// disabled command does not block the key for other widgets.
    ///
    /// Shortcuts without Ctrl or Alt (for example `F` for "Fit to view")
    /// are ignored while a text field has the keyboard focus, and so are
    /// the clipboard and select-all commands: the text field uses them.
    pub fn consume(&self, ctx: &egui::Context, enabled: impl Fn(Command) -> bool) -> Vec<Command> {
        let typing = ctx.text_edit_focused();
        // A binding with more modifiers goes first: egui matches Ctrl+S
        // also when Ctrl+Shift+S is pressed.
        let mut order: Vec<&Binding> = self.bindings.iter().collect();
        order.sort_by_key(|b| std::cmp::Reverse(modifier_count(b.shortcut.modifiers)));
        let mut commands = Vec::new();
        ctx.input_mut(|input| {
            for binding in order {
                let plain = !binding.shortcut.modifiers.command
                    && !binding.shortcut.modifiers.ctrl
                    && !binding.shortcut.modifiers.alt
                    && !is_function_key(binding.shortcut.logical_key);
                let text_command = matches!(
                    binding.command,
                    Command::SelectAll | Command::Cut | Command::Copy | Command::Paste
                );
                if ((plain || text_command) && typing) || !enabled(binding.command) {
                    continue;
                }
                if input.consume_shortcut(&binding.shortcut) && !commands.contains(&binding.command)
                {
                    commands.push(binding.command);
                }
            }
        });
        commands
    }
}

/// A clipboard command of the platform. On desktop, egui-winit sends
/// Ctrl+C, Ctrl+X and Ctrl+V as copy, cut and paste events, not as key
/// events, so the key map does not see them.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ClipboardEvent {
    /// Ctrl+C.
    Copy,
    /// Ctrl+X.
    Cut,
    /// Ctrl+V, with the text of the system clipboard.
    Paste(String),
}

/// Takes the copy, cut and paste events out of the input, unless a text
/// field has the keyboard focus or a label has selected text: those use
/// the events.
pub fn take_clipboard_events(ctx: &egui::Context) -> Vec<ClipboardEvent> {
    let label_selection = ctx
        .with_plugin(|s: &mut egui::text_selection::LabelSelectionState| s.has_selection())
        .unwrap_or(false);
    if ctx.text_edit_focused() || label_selection {
        return Vec::new();
    }
    let mut out = Vec::new();
    ctx.input_mut(|input| {
        input.events.retain(|event| {
            let taken = match event {
                egui::Event::Copy => ClipboardEvent::Copy,
                egui::Event::Cut => ClipboardEvent::Cut,
                egui::Event::Paste(text) => ClipboardEvent::Paste(text.clone()),
                _ => return true,
            };
            out.push(taken);
            false
        });
    });
    out
}

/// The text of a shortcut. Like `Context::format_shortcut`, but with
/// symbols for the punctuation keys (`Ctrl+=` instead of `Ctrl+Equals`).
pub fn format_shortcut(ctx: &egui::Context, shortcut: &KeyboardShortcut) -> String {
    let symbol = match shortcut.logical_key {
        Key::Equals => Some("="),
        Key::Plus => Some("+"),
        Key::Minus => Some("-"),
        Key::Backslash => Some("\\"),
        Key::Slash => Some("/"),
        Key::Comma => Some(","),
        Key::Period => Some("."),
        _ => None,
    };
    let Some(symbol) = symbol else {
        return ctx.format_shortcut(shortcut);
    };
    // Format the modifiers with a placeholder key, then put the symbol in.
    let modifiers_only = KeyboardShortcut::new(shortcut.modifiers, Key::A);
    let text = ctx.format_shortcut(&modifiers_only);
    match text.strip_suffix('A') {
        Some(prefix) => format!("{prefix}{symbol}"),
        None => text,
    }
}

fn modifier_count(m: Modifiers) -> u8 {
    u8::from(m.alt) + u8::from(m.shift) + u8::from(m.ctrl || m.command) + u8::from(m.mac_cmd)
}

fn is_function_key(key: Key) -> bool {
    matches!(
        key,
        Key::F1
            | Key::F2
            | Key::F3
            | Key::F4
            | Key::F5
            | Key::F6
            | Key::F7
            | Key::F8
            | Key::F9
            | Key::F10
            | Key::F11
            | Key::F12
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn no_conflicts_and_all_labelled() {
        let map = KeyMap::default();
        assert!(map.conflicts().is_empty(), "{:?}", map.conflicts());
        for command in Command::ALL {
            assert!(!command.label().is_empty());
            assert!(!command.group().is_empty());
        }
        // Every bound command is in ALL.
        for binding in map.bindings() {
            assert!(Command::ALL.contains(&binding.command));
        }
        assert_eq!(
            map.shortcut(Command::OpenLog),
            Some(KeyboardShortcut::new(
                Modifiers::COMMAND | Modifiers::SHIFT,
                Key::O
            ))
        );
        assert_eq!(map.shortcut(Command::About), None);
    }

    fn press(ctx: &egui::Context, modifiers: Modifiers, key: Key, map: &KeyMap) -> Vec<Command> {
        let mut input = egui::RawInput::default();
        input.events.push(egui::Event::Key {
            key,
            physical_key: None,
            pressed: true,
            repeat: false,
            modifiers,
        });
        let mut out = Vec::new();
        let mut output = ctx.run_ui(input, |ui| {
            out = map.consume(ui.ctx(), |_| true);
        });
        output.textures_delta.clear();
        out
    }

    #[test]
    fn shortcut_text_uses_symbols() {
        let ctx = egui::Context::default();
        let map = KeyMap::default();
        assert_eq!(
            map.shortcut_text(&ctx, Command::ZoomIn).as_deref(),
            Some("Ctrl+=")
        );
        assert_eq!(
            map.shortcut_text(&ctx, Command::ZoomOut).as_deref(),
            Some("Ctrl+-")
        );
        assert_eq!(
            map.shortcut_text(&ctx, Command::SplitView).as_deref(),
            Some("Ctrl+\\")
        );
        assert_eq!(
            map.shortcut_text(&ctx, Command::OpenLog).as_deref(),
            Some("Ctrl+Shift+O")
        );
        assert_eq!(
            map.shortcut_text(&ctx, Command::FitToView).as_deref(),
            Some("F")
        );
    }

    #[test]
    fn consume_prefers_more_modifiers() {
        let ctx = egui::Context::default();
        let map = KeyMap::default();
        assert_eq!(
            press(&ctx, Modifiers::COMMAND | Modifiers::SHIFT, Key::S, &map),
            vec![Command::SaveAs]
        );
        assert_eq!(
            press(&ctx, Modifiers::COMMAND, Key::S, &map),
            vec![Command::Save]
        );
        assert_eq!(
            press(&ctx, Modifiers::NONE, Key::F, &map),
            vec![Command::FitToView]
        );
        assert_eq!(
            press(&ctx, Modifiers::COMMAND, Key::F, &map),
            vec![Command::Find]
        );
        assert_eq!(
            press(&ctx, Modifiers::NONE, Key::F1, &map),
            vec![Command::ShowShortcuts]
        );
        assert!(press(&ctx, Modifiers::NONE, Key::G, &map).is_empty());
    }

    #[test]
    fn disabled_commands_are_not_consumed() {
        let ctx = egui::Context::default();
        let map = KeyMap::default();
        let mut input = egui::RawInput::default();
        input.events.push(egui::Event::Key {
            key: Key::C,
            physical_key: None,
            pressed: true,
            repeat: false,
            modifiers: Modifiers::COMMAND,
        });
        let mut output = ctx.run_ui(input, |ui| {
            let commands = map.consume(ui.ctx(), |c| c != Command::Copy);
            assert!(commands.is_empty());
            // The key event is still there for other widgets.
            assert!(ui.input(|i| i.key_pressed(Key::C)));
        });
        output.textures_delta.clear();
    }
}
