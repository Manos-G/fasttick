//! The application shell: [`FastTickApp`].
//!
//! Layout of the window:
//!
//! - top bar: logo, menu bar, mode switcher (Edit, Live, Replay), zoom
//!   controls of the active view;
//! - centre: the dock of the current mode (document tabs with split view,
//!   side panels);
//! - bottom: the status bar (mode, connection, file, node count, zoom).
//!
//! Each frame the app reads the results of background jobs and live
//! events, handles the shortcuts, draws the UI and then runs the actions
//! that the panels requested.

use std::path::{Path, PathBuf};
use std::sync::Arc;

use egui::{Id, RichText, Ui, WidgetText};
use egui_dock::tab_viewer::OnCloseResponse;
use egui_dock::{DockArea, DockState, TabViewer};
use fasttick_core::runtime::{InstanceIndex, Uid};
use fasttick_core::status::NodeStatus;
use fasttick_log::sqlite::SessionInfo;
use fasttick_protocol::breakpoints::{BreakpointSet, Resolution, ResolvedBreakpoint};
use fasttick_protocol::messages::{Hook, HookMode};
use fasttick_protocol::session::ConnectionState;
use serde::{Deserialize, Serialize};

use crate::actions::{AppAction, NamePurpose};
use crate::canvas::{
    CanvasEvent, CanvasNode, CanvasNodeId, DropPlacement, DropSpot, LayoutDirection, NodeSource,
    TreeCanvas,
};
use crate::dialogs::{
    self, ConnectChoice, ConnectForm, NamePrompt, Notice, NoticeLevel, PromptChoice,
};
use crate::dock::{self, Tab};
use crate::documents::{DocId, DocKind, ViewId, ViewOptions, Workspace};
use crate::edit_menu::{EditMenuContext, edit_menu};
use crate::edit_ops::{EditOp, Placement};
use crate::jobs::{DialogPurpose, ExportFormat, JobResult, Jobs, LoadingFile};
use crate::live::{AFTER_BREAKPOINT_UNAVAILABLE, LiveController, LiveUpdate, ResumeAction};
use crate::panels::{self, PanelContext, live_bar};
use crate::replay::ReplayModel;
use crate::settings::{RecentKind, Settings};
use crate::shortcuts::{Command, KeyMap};
use crate::theme::{self, CanvasPalette, ThemeChoice};
use crate::widgets::{self, Icon};

/// The working mode. Each mode has its own documents and dock.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default, Serialize, Deserialize)]
pub enum Mode {
    /// Open and (later) edit tree files.
    #[default]
    Edit,
    /// Watch and debug a running executor.
    Live,
    /// Replay a log.
    Replay,
}

impl Mode {
    /// All modes, in switcher order.
    pub const ALL: [Mode; 3] = [Mode::Edit, Mode::Live, Mode::Replay];

    /// The display name.
    pub fn name(self) -> &'static str {
        match self {
            Mode::Edit => "Edit",
            Mode::Live => "Live",
            Mode::Replay => "Replay",
        }
    }

    fn index(self) -> usize {
        match self {
            Mode::Edit => 0,
            Mode::Live => 1,
            Mode::Replay => 2,
        }
    }

    /// Parses a command-line value (`edit`, `live`, `replay`).
    pub fn parse(text: &str) -> Option<Mode> {
        Mode::ALL
            .into_iter()
            .find(|m| m.name().eq_ignore_ascii_case(text))
    }
}

/// Options from the command line.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct StartOptions {
    /// Files to open.
    pub open: Vec<PathBuf>,
    /// A theme for this session (not saved).
    pub theme: Option<ThemeChoice>,
    /// The mode at start.
    pub mode: Option<Mode>,
    /// An executor to connect to at start (host and port).
    pub connect: Option<(String, u16)>,
}

/// Parses `HOST`, `HOST:PORT`, `[IPV6]` or `[IPV6]:PORT`. The default port
/// is 1667 (the default of the BT.CPP publisher). Returns `None` for an
/// empty host or a bad port.
pub fn parse_endpoint(text: &str) -> Option<(String, u16)> {
    let text = text.trim();
    let default = crate::settings::DEFAULT_PORT;
    let (host, port) = if let Some(rest) = text.strip_prefix('[') {
        let (host, after) = rest.split_once(']')?;
        match after.strip_prefix(':') {
            Some(port) => (host, port.parse().ok()?),
            None if after.is_empty() => (host, default),
            None => return None,
        }
    } else if text.matches(':').count() == 1 {
        let (host, port) = text.split_once(':')?;
        (host, port.parse().ok()?)
    } else {
        (text, default)
    };
    (!host.is_empty() && port != 0).then(|| (host.to_owned(), port))
}

const MODE_KEY: &str = "fasttick.mode";

fn dock_key(mode: Mode) -> String {
    format!("fasttick.dock.{}", mode.name().to_ascii_lowercase())
}

/// A breakpoint file that waits for the user to confirm: the breakpoints
/// and how each one was found in the running tree.
#[derive(Debug, Clone)]
pub struct PendingBreakpoints {
    /// The file.
    pub path: PathBuf,
    /// The breakpoints of the file.
    pub set: BreakpointSet,
    /// How each breakpoint was found ([`BreakpointSet::resolve`]).
    pub resolved: Vec<ResolvedBreakpoint>,
    /// The breakpoints to insert. A breakpoint found by path is chosen by
    /// default; a breakpoint found only by its stored UID is not (the UID
    /// can belong to another node after a change of the tree).
    pub chosen: Vec<bool>,
}

impl PendingBreakpoints {
    /// Resolves a breakpoint set against a tree.
    pub fn new(
        path: PathBuf,
        set: BreakpointSet,
        tree: &fasttick_core::runtime::RuntimeTree,
    ) -> Self {
        let resolved = set.resolve(tree);
        let chosen = resolved
            .iter()
            .map(|r| matches!(r.resolution, Resolution::ByPath | Resolution::AmbiguousPath))
            .collect();
        PendingBreakpoints {
            path,
            set,
            resolved,
            chosen,
        }
    }

    /// The hooks of the chosen breakpoints.
    pub fn hooks(&self) -> Vec<Hook> {
        self.resolved
            .iter()
            .zip(&self.chosen)
            .filter(|(_, chosen)| **chosen)
            .filter_map(|(resolved, _)| {
                let uid = resolved.uid?;
                Some(self.set.breakpoints.get(resolved.index)?.to_hook(uid))
            })
            .collect()
    }
}

/// The sessions of a SQLite log, for the user to choose one.
#[derive(Debug, Clone)]
struct SqlitePicker {
    path: PathBuf,
    sessions: Vec<SessionInfo>,
    selected: usize,
}

/// The application.
pub struct FastTickApp {
    mode: Mode,
    docks: [DockState<Tab>; 3],
    workspace: Workspace,
    settings: Settings,
    theme_override: Option<ThemeChoice>,
    keymap: KeyMap,
    jobs: Jobs,
    live: LiveController,
    live_doc: Option<DocId>,
    show_about: bool,
    show_shortcuts: bool,
    connect_form: Option<ConnectForm>,
    name_prompt: Option<NamePrompt>,
    model_form: Option<crate::model_dialog::ModelForm>,
    notices: Vec<Notice>,
    actions: Vec<AppAction>,
    save_as_view: Option<ViewId>,
    project_view: Option<ViewId>,
    /// The clipboard of the tree editor (XML text of copied nodes).
    clipboard: Option<String>,
    window_title: String,
    pending_breakpoints: Option<PendingBreakpoints>,
    sqlite_picker: Option<SqlitePicker>,
}

impl std::fmt::Debug for FastTickApp {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("FastTickApp")
            .field("mode", &self.mode)
            .field("documents", &self.workspace.docs().len())
            .field("views", &self.workspace.views().len())
            .finish_non_exhaustive()
    }
}

impl FastTickApp {
    /// Creates the app: loads the settings and the dock layouts, installs
    /// the fonts and the theme, and opens the files of `options`.
    pub fn new(cc: &eframe::CreationContext<'_>, options: StartOptions) -> Self {
        let storage = cc.storage;
        let settings = Settings::load(storage);
        let mode = options
            .mode
            .or_else(|| storage.and_then(|s| eframe::get_value(s, MODE_KEY)))
            .unwrap_or_default();
        let docks = Mode::ALL.map(|m| {
            storage
                .and_then(|s| eframe::get_value::<DockState<Tab>>(s, &dock_key(m)))
                .filter(|d| dock::contains(d, Tab::Start))
                .unwrap_or_else(|| dock::default_dock(m))
        });
        let mut app = FastTickApp {
            mode,
            docks,
            workspace: Workspace::default(),
            settings,
            theme_override: options.theme,
            keymap: KeyMap::default(),
            jobs: Jobs::default(),
            live: LiveController::default(),
            live_doc: None,
            show_about: false,
            show_shortcuts: false,
            connect_form: None,
            name_prompt: None,
            model_form: None,
            notices: Vec::new(),
            actions: Vec::new(),
            save_as_view: None,
            project_view: None,
            clipboard: None,
            window_title: String::new(),
            pending_breakpoints: None,
            sqlite_picker: None,
        };
        crate::fonts::install(&cc.egui_ctx);
        cc.egui_ctx.options_mut(|o| o.zoom_with_keyboard = false);
        theme::install(&cc.egui_ctx, app.theme());
        for path in options.open {
            if let Err(message) = app.open_path_now(&path) {
                app.notify(&cc.egui_ctx, NoticeLevel::Error, message);
            }
        }
        if let Some((host, port)) = options.connect {
            app.connect(&cc.egui_ctx, host, port);
        }
        app
    }

    /// The theme in use.
    pub fn theme(&self) -> ThemeChoice {
        self.theme_override.unwrap_or(self.settings.theme)
    }

    /// The current mode.
    pub fn mode(&self) -> Mode {
        self.mode
    }

    /// Changes the mode.
    pub fn set_mode(&mut self, mode: Mode) {
        self.mode = mode;
        // The active view must belong to the new mode.
        let views = dock::document_views(&self.docks[mode.index()]);
        if !self
            .workspace
            .active_view
            .is_some_and(|v| views.contains(&v))
        {
            self.workspace.active_view = views.first().copied();
        }
    }

    /// The documents and views.
    pub fn workspace(&self) -> &Workspace {
        &self.workspace
    }

    /// The documents and views, for changes.
    pub fn workspace_mut(&mut self) -> &mut Workspace {
        &mut self.workspace
    }

    /// The settings.
    pub fn settings(&self) -> &Settings {
        &self.settings
    }

    /// The dock of a mode.
    pub fn dock(&self, mode: Mode) -> &DockState<Tab> {
        &self.docks[mode.index()]
    }

    /// The key map.
    pub fn keymap(&self) -> &KeyMap {
        &self.keymap
    }

    /// The live controller.
    pub fn live(&self) -> &LiveController {
        &self.live
    }

    /// The live controller, for changes (tests and scripts).
    pub fn live_mut(&mut self) -> &mut LiveController {
        &mut self.live
    }

    /// The document of the live tree, if any.
    pub fn live_doc(&self) -> Option<DocId> {
        self.live_doc
    }

    /// The background jobs.
    pub fn jobs(&self) -> &Jobs {
        &self.jobs
    }

    /// Shows a panel in the dock of the current mode (adds it if it is not
    /// there, else makes its tab active).
    pub fn show_panel(&mut self, tab: Tab) {
        dock::show_panel(&mut self.docks[self.mode.index()], tab);
    }

    /// The breakpoint file that waits for confirmation, if any.
    pub fn pending_breakpoints(&self) -> Option<&PendingBreakpoints> {
        self.pending_breakpoints.as_ref()
    }

    /// The replay model of the active replay document.
    pub fn active_replay(&self) -> Option<&ReplayModel> {
        let doc = self.workspace.active_replay_doc()?;
        self.workspace.doc(doc)?.replay.as_ref()
    }

    /// The replay model of the active replay document, for changes.
    pub fn active_replay_mut(&mut self) -> Option<&mut ReplayModel> {
        let doc = self.workspace.active_replay_doc()?;
        self.workspace.replay_mut(doc)
    }

    /// The notices on screen.
    pub fn notices(&self) -> &[Notice] {
        &self.notices
    }

    fn notify(&mut self, ctx: &egui::Context, level: NoticeLevel, text: impl Into<String>) {
        self.notices.push(Notice {
            text: text.into(),
            level,
            since: ctx.input(|i| i.time),
            action: None,
        });
    }

    fn notify_with_action(
        &mut self,
        ctx: &egui::Context,
        level: NoticeLevel,
        text: impl Into<String>,
        button: &str,
        action: AppAction,
    ) {
        self.notices.push(Notice {
            text: text.into(),
            level,
            since: ctx.input(|i| i.time),
            action: Some((button.to_owned(), action)),
        });
    }

    fn view_options(&self) -> ViewOptions {
        ViewOptions {
            direction: self.settings.layout_direction,
            edge_style: self.settings.edge_style,
            minimap: self.settings.show_minimap,
        }
    }

    /// Opens a file now, on the calling thread (command line, tests). The
    /// menu uses background jobs instead.
    pub fn open_path_now(&mut self, path: &Path) -> Result<(), String> {
        match RecentKind::for_path(path) {
            RecentKind::Log => {
                let loaded = crate::jobs::read_log(path)
                    .map_err(|e| format!("Cannot open {}: {e}", path.display()))?;
                let session = if crate::jobs::is_sqlite(path) {
                    loaded
                        .source
                        .as_deref()
                        .and_then(|s| s.strip_prefix("session "))
                        .and_then(|id| id.parse().ok())
                } else {
                    None
                };
                self.add_log_document(path, session, loaded);
            }
            RecentKind::Project | RecentKind::Tree => {
                let loaded = crate::jobs::read_tree(path)
                    .map_err(|e| format!("Cannot open {}: {e}", path.display()))?;
                self.add_tree_document(path, loaded);
            }
        }
        Ok(())
    }

    fn start_open(&mut self, ctx: &egui::Context, path: PathBuf) {
        if !crate::jobs::is_sqlite(&path)
            && let Some(view) = self.find_open_view(&path, None)
        {
            self.focus_view(view);
            return;
        }
        if self.jobs.loading().iter().any(|l| l.path == path) {
            return;
        }
        match RecentKind::for_path(&path) {
            RecentKind::Log => self.jobs.load_log(ctx, path),
            RecentKind::Project | RecentKind::Tree => self.jobs.load_tree(ctx, path),
        }
    }

    fn find_open_view(&self, path: &Path, session: Option<i64>) -> Option<ViewId> {
        let doc = self.workspace.find_doc(|d| match &d.kind {
            DocKind::Replay {
                path: p,
                session: s,
            } => p == path && *s == session,
            DocKind::TreeFile { path: p } => p == path,
            DocKind::Live { .. } => false,
        })?;
        self.workspace.views_of(doc).next().map(|v| v.id)
    }

    fn focus_view(&mut self, view: ViewId) {
        let Some(mode) = self.workspace.doc_of_view(view).map(|d| d.mode()) else {
            return;
        };
        self.set_mode(mode);
        let dock = &mut self.docks[mode.index()];
        if let Some(path) = dock.find_tab(&Tab::Document(view)) {
            let _ = dock.set_active_tab(path);
            dock.set_focused_node_and_surface(path.node_path());
        }
        self.workspace.active_view = Some(view);
    }

    fn add_tree_document(
        &mut self,
        path: &Path,
        loaded: crate::jobs::LoadedTree,
    ) -> Option<ViewId> {
        if let Some(view) = self.find_open_view(path, None) {
            self.focus_view(view);
            return Some(view);
        }
        let title = file_name(path);
        let main = loaded.main_instance();
        let doc = self
            .workspace
            .add_editor_doc(path.to_owned(), title, *loaded.editor);
        let view = self.workspace.open_view(doc, main, self.view_options())?;
        dock::add_document(&mut self.docks[Mode::Edit.index()], Tab::Document(view));
        self.settings.add_recent(path);
        self.focus_view(view);
        Some(view)
    }

    fn add_log_document(
        &mut self,
        path: &Path,
        session: Option<i64>,
        loaded: crate::jobs::LoadedLog,
    ) -> Option<ViewId> {
        if let Some(view) = self.find_open_view(path, session) {
            self.focus_view(view);
            return Some(view);
        }
        let crate::jobs::LoadedLog { log, index, source } = loaded;
        let title = match &source {
            Some(source) => format!("{} ({source})", file_name(path)),
            None => file_name(path),
        };
        let runtime = Arc::new(log.tree.clone());
        let mut model = ReplayModel::new(log, index);
        model.source = source;
        let doc = self.workspace.add_doc(
            DocKind::Replay {
                path: path.to_owned(),
                session,
            },
            title,
            runtime,
            Some(model),
        );
        let view = self
            .workspace
            .open_view(doc, InstanceIndex(0), self.view_options())?;
        dock::add_document(&mut self.docks[Mode::Replay.index()], Tab::Document(view));
        self.settings.add_recent(path);
        self.focus_view(view);
        Some(view)
    }

    /// True if a command can run now.
    pub fn is_enabled(&self, command: Command) -> bool {
        let active = self.active_view_in_mode();
        match command {
            Command::NewProject => true,
            Command::Save => self.active_edit_view().is_some(),
            Command::Undo => self.active_editor().is_some_and(|e| e.can_undo()),
            Command::Redo => self.active_editor().is_some_and(|e| e.can_redo()),
            Command::Cut | Command::Copy | Command::Duplicate | Command::Delete => self
                .active_edit_view()
                .is_some_and(|v| !crate::edit_ops::selected_nodes(&self.workspace, v).is_empty()),
            Command::Paste => self.active_edit_view().is_some() && self.clipboard.is_some(),
            Command::SaveAs => active.is_some_and(|v| {
                self.workspace
                    .doc_of_view(v)
                    .is_some_and(|d| matches!(d.kind, DocKind::TreeFile { .. }))
            }),
            Command::SelectAll
            | Command::FitToView
            | Command::ZoomIn
            | Command::ZoomOut
            | Command::ZoomReset
            | Command::SplitView
            | Command::CloseTab => active.is_some(),
            Command::Disconnect => self.live.is_active(),
            Command::OpenFile | Command::OpenLog => !self.jobs.dialog_open(),
            Command::ToggleBreakpoint => {
                self.mode == Mode::Live
                    && self.live.is_connected()
                    && live_bar::selected_live_uid(&self.workspace, self.live_doc).is_some()
            }
            Command::Continue => self.live.is_connected() && !self.live.hits().is_empty(),
            Command::ToggleRecording => {
                self.live.recording().is_active()
                    || (self.live.is_connected() && !self.live.recording_requested())
            }
            Command::SaveRecording => {
                self.live.recording().has_data()
                    && !self.live.recording().is_active()
                    && !self.jobs.dialog_open()
            }
            Command::SaveBreakpoints => {
                !self.live.hooks().is_empty()
                    && self.live.tree().is_some()
                    && !self.jobs.dialog_open()
            }
            Command::LoadBreakpoints => {
                self.live.is_connected() && self.live.tree().is_some() && !self.jobs.dialog_open()
            }
            Command::PlayPause
            | Command::StepBack
            | Command::StepForward
            | Command::GoToStart
            | Command::GoToEnd => self.mode == Mode::Replay && self.active_replay().is_some(),
            Command::ResetTimelineZoom => {
                self.mode == Mode::Replay
                    && self
                        .active_replay()
                        .is_some_and(|m| m.cursor().range().is_some())
            }
            Command::ExportJson | Command::ExportCsv => {
                self.active_replay().is_some() && !self.jobs.dialog_open()
            }
            Command::Quit
            | Command::Find
            | Command::LayoutTopDown
            | Command::LayoutLeftRight
            | Command::ToggleLayout
            | Command::ThemeLight
            | Command::ThemeDark
            | Command::ThemeSystem
            | Command::ModeEdit
            | Command::ModeLive
            | Command::ModeReplay
            | Command::Connect
            | Command::ResetLayout
            | Command::ShowShortcuts
            | Command::About => true,
        }
    }

    /// The active view if it belongs to the current mode.
    pub fn active_view_in_mode(&self) -> Option<ViewId> {
        let view = self.workspace.active_view?;
        let doc = self.workspace.doc_of_view(view)?;
        (doc.mode() == self.mode).then_some(view)
    }

    /// Runs a command.
    pub fn run_command(&mut self, ctx: &egui::Context, command: Command) {
        if !self.is_enabled(command) {
            return;
        }
        let active = self.active_view_in_mode();
        match command {
            Command::OpenFile => {
                let start = self.settings.recent.first().map(|r| r.path.clone());
                self.jobs.pick_file(ctx, DialogPurpose::OpenFile, start);
            }
            Command::OpenLog => {
                let start = self
                    .settings
                    .recent
                    .iter()
                    .find(|r| r.kind == RecentKind::Log)
                    .map(|r| r.path.clone());
                self.jobs.pick_file(ctx, DialogPurpose::OpenLog, start);
            }
            Command::SaveAs => {
                self.save_as_view = active;
                let start = active
                    .and_then(|v| self.workspace.doc_of_view(v))
                    .and_then(|d| d.kind.path().map(Path::to_owned));
                self.jobs.pick_file(ctx, DialogPurpose::SaveAs, start);
            }
            Command::Quit => ctx.send_viewport_cmd(egui::ViewportCommand::Close),
            Command::SelectAll => {
                if let Some(view) = self.workspace.active_mut() {
                    view.canvas.select_all();
                }
            }
            Command::Find => {
                dock::show_panel(&mut self.docks[self.mode.index()], Tab::Search);
                panels::search::request_focus(ctx);
            }
            Command::FitToView => self.with_active_canvas(|c| c.fit_to_view(true)),
            Command::ZoomIn => self.with_active_canvas(|c| c.zoom_in()),
            Command::ZoomOut => self.with_active_canvas(|c| c.zoom_out()),
            Command::ZoomReset => self.with_active_canvas(|c| c.zoom_reset()),
            Command::LayoutTopDown => self.set_direction(LayoutDirection::TopDown),
            Command::LayoutLeftRight => self.set_direction(LayoutDirection::LeftRight),
            Command::ToggleLayout => {
                let current = active
                    .and_then(|v| self.workspace.view(v))
                    .map_or(self.settings.layout_direction, |v| v.canvas.direction());
                self.set_direction(current.toggled());
            }
            Command::ThemeLight => self.set_theme(ctx, ThemeChoice::Light),
            Command::ThemeDark => self.set_theme(ctx, ThemeChoice::Dark),
            Command::ThemeSystem => self.set_theme(ctx, ThemeChoice::System),
            Command::ModeEdit => self.set_mode(Mode::Edit),
            Command::ModeLive => self.set_mode(Mode::Live),
            Command::ModeReplay => self.set_mode(Mode::Replay),
            Command::Connect => {
                self.connect_form = Some(ConnectForm {
                    host: self.settings.live_host.clone(),
                    port: self.settings.live_port,
                    poll_interval_ms: self.settings.poll_interval_ms,
                });
            }
            Command::Disconnect => self.live.disconnect(),
            Command::ToggleBreakpoint => {
                if let Some(uid) = live_bar::selected_live_uid(&self.workspace, self.live_doc) {
                    self.live.toggle_breakpoint(uid);
                }
            }
            Command::Continue => {
                if let Some(uid) = self.live.hits().first().copied() {
                    self.live.resume(uid, ResumeAction::Continue);
                }
            }
            Command::ToggleRecording => {
                if self.live.recording().is_active() {
                    self.live.stop_recording();
                } else {
                    self.live.start_recording();
                }
            }
            Command::SaveRecording => self.pick_file(ctx, DialogPurpose::SaveRecording),
            Command::SaveBreakpoints => self.pick_file(ctx, DialogPurpose::SaveBreakpoints),
            Command::LoadBreakpoints => self.pick_file(ctx, DialogPurpose::LoadBreakpoints),
            Command::PlayPause => self.with_replay(ReplayModel::toggle_play),
            Command::StepBack => self.with_replay(|m| {
                m.step_back();
            }),
            Command::StepForward => self.with_replay(|m| {
                m.step_forward();
            }),
            Command::GoToStart => self.with_replay(ReplayModel::go_to_start),
            Command::GoToEnd => self.with_replay(ReplayModel::go_to_end),
            Command::ResetTimelineZoom => self.with_replay(|m| m.set_range(None)),
            Command::ExportJson => {
                self.pick_file(ctx, DialogPurpose::ExportTransitions(ExportFormat::Json));
            }
            Command::ExportCsv => {
                self.pick_file(ctx, DialogPurpose::ExportTransitions(ExportFormat::Csv));
            }
            Command::SplitView => {
                if let Some(view) = active
                    && let Some(new_view) = self.workspace.split_view(view)
                {
                    dock::add_beside(
                        &mut self.docks[self.mode.index()],
                        Tab::Document(view),
                        Tab::Document(new_view),
                    );
                    self.workspace.active_view = Some(new_view);
                }
            }
            Command::CloseTab => {
                if let Some(view) = active {
                    self.close_view(view);
                }
            }
            Command::ResetLayout => {
                let mode = self.mode;
                let views = dock::document_views(&self.docks[mode.index()]);
                let mut fresh = dock::default_dock(mode);
                for view in views {
                    dock::add_document(&mut fresh, Tab::Document(view));
                }
                self.docks[mode.index()] = fresh;
            }
            Command::ShowShortcuts => self.show_shortcuts = true,
            Command::About => self.show_about = true,
            Command::NewProject => self.new_document(),
            Command::Save => {
                if let Some(view) = self.active_edit_view() {
                    self.save_view(ctx, view);
                }
            }
            Command::Undo
            | Command::Redo
            | Command::Cut
            | Command::Copy
            | Command::Paste
            | Command::Duplicate
            | Command::Delete => {
                if let Some(view) = self.active_edit_view() {
                    let op = match command {
                        Command::Undo => EditOp::Undo,
                        Command::Redo => EditOp::Redo,
                        Command::Cut => EditOp::Cut,
                        Command::Copy => EditOp::Copy,
                        Command::Paste => EditOp::Paste,
                        Command::Duplicate => EditOp::Duplicate,
                        _ => EditOp::Delete,
                    };
                    self.run_edit(ctx, view, op);
                }
            }
        }
    }

    fn with_replay(&mut self, f: impl FnOnce(&mut ReplayModel)) {
        if let Some(model) = self.active_replay_mut() {
            f(model);
        }
    }

    /// Opens a file dialog. The start folder is the folder of a recent
    /// file of the same kind.
    fn pick_file(&mut self, ctx: &egui::Context, purpose: DialogPurpose) {
        let start = match purpose {
            DialogPurpose::SaveRecording | DialogPurpose::ExportTransitions(_) => self
                .settings
                .recent
                .iter()
                .find(|r| r.kind == RecentKind::Log)
                .map(|r| r.path.clone()),
            _ => self.settings.recent.first().map(|r| r.path.clone()),
        };
        self.jobs.pick_file(ctx, purpose, start);
    }

    /// Writes the finished recording to a `.btlog` file (on a worker
    /// thread). Returns false if there is no recording.
    pub fn save_recording_to(&mut self, ctx: &egui::Context, path: PathBuf) -> bool {
        let fallback = self.live.tree().map(|t| t.xml.clone());
        match self.live.recording().to_file(fallback.as_deref()) {
            Some(file) => {
                self.jobs.save_recording(ctx, path, file);
                true
            }
            None => {
                self.notify(ctx, NoticeLevel::Warning, "There is no recording to save.");
                false
            }
        }
    }

    /// Exports the filtered transitions of the active replay document (on
    /// a worker thread). Returns false if no log is open or the filter is
    /// not valid.
    pub fn export_transitions_to(
        &mut self,
        ctx: &egui::Context,
        path: PathBuf,
        format: ExportFormat,
    ) -> bool {
        let Some(model) = self.active_replay() else {
            self.notify(ctx, NoticeLevel::Warning, "Open a log first.");
            return false;
        };
        match model.export_filter() {
            Ok(filter) => {
                let log = Arc::clone(model.log());
                self.jobs.export_transitions(ctx, path, format, log, filter);
                true
            }
            Err(message) => {
                self.notify(ctx, NoticeLevel::Warning, message);
                false
            }
        }
    }

    /// Writes the breakpoints and forced results to a file (on a worker
    /// thread).
    pub fn save_breakpoints_to(&mut self, ctx: &egui::Context, path: PathBuf) {
        let set = self.live.breakpoint_set();
        self.jobs.save_breakpoints(ctx, path, set);
    }

    /// Resolves a loaded breakpoint set against the running tree and shows
    /// it for confirmation.
    pub fn prepare_breakpoints(&mut self, ctx: &egui::Context, path: PathBuf, set: BreakpointSet) {
        match self.live.tree() {
            Some(tree) => {
                self.pending_breakpoints = Some(PendingBreakpoints::new(path, set, tree));
            }
            None => self.notify(
                ctx,
                NoticeLevel::Warning,
                "Connect to an executor first: FastTick finds the nodes of the breakpoints \
                 in the running tree.",
            ),
        }
    }

    /// Inserts the chosen breakpoints of the pending file. Returns the
    /// number of hooks sent.
    pub fn apply_pending_breakpoints(&mut self) -> usize {
        let Some(pending) = self.pending_breakpoints.take() else {
            return 0;
        };
        self.live.insert_hooks(&pending.hooks())
    }

    /// Selects a node of a document and moves a view to it.
    fn reveal_node(&mut self, doc: DocId, uid: Uid) {
        let Some(view) = self.workspace.view_for_doc(doc) else {
            return;
        };
        if !self.workspace.reveal_uid(view, uid, true) {
            return;
        }
        // Show the tab of the view in its dock (without a mode change).
        if let Some(mode) = self.workspace.doc(doc).map(|d| d.mode()) {
            let dock = &mut self.docks[mode.index()];
            if let Some(path) = dock.find_tab(&Tab::Document(view)) {
                let _ = dock.set_active_tab(path);
            }
            if mode == self.mode {
                self.workspace.active_view = Some(view);
            }
        }
    }

    fn with_active_canvas(&mut self, f: impl FnOnce(&mut crate::canvas::CanvasState)) {
        if self.active_view_in_mode().is_some()
            && let Some(view) = self.workspace.active_mut()
        {
            f(&mut view.canvas);
        }
    }

    fn set_direction(&mut self, direction: LayoutDirection) {
        self.settings.layout_direction = direction;
        if self.active_view_in_mode().is_some()
            && let Some(view) = self.workspace.active_mut()
        {
            view.canvas.set_direction(direction);
        }
    }

    fn set_theme(&mut self, ctx: &egui::Context, choice: ThemeChoice) {
        self.settings.theme = choice;
        self.theme_override = None;
        ctx.set_theme(choice.preference());
    }

    fn close_view(&mut self, view: ViewId) {
        for dock in &mut self.docks {
            if let Some(path) = dock.find_tab(&Tab::Document(view)) {
                dock.remove_tab(path);
            }
        }
        if let Some(doc) = self.workspace.close_view(view)
            && self.live_doc == Some(doc)
        {
            self.live_doc = None;
        }
        if self.workspace.active_view.is_none() {
            self.workspace.active_view = dock::document_views(&self.docks[self.mode.index()])
                .first()
                .copied();
        }
    }

    fn connect(&mut self, ctx: &egui::Context, host: String, port: u16) {
        if host.trim().is_empty() {
            self.notify(ctx, NoticeLevel::Warning, "Enter a host name.");
            return;
        }
        self.settings.live_host = host.clone();
        self.settings.live_port = port;
        self.live
            .connect(ctx, &host, port, self.settings.poll_interval_ms);
        self.set_mode(Mode::Live);
    }

    // ----- Background results -----

    fn handle_jobs(&mut self, ctx: &egui::Context) {
        for result in self.jobs.poll() {
            match result {
                JobResult::Picked { purpose, path } => {
                    let Some(path) = path else {
                        continue;
                    };
                    match purpose {
                        DialogPurpose::OpenFile | DialogPurpose::OpenLog => {
                            self.start_open(ctx, path);
                        }
                        DialogPurpose::SaveAs => {
                            let view = self.save_as_view.take();
                            let doc = view
                                .and_then(|v| self.workspace.doc_of_view(v))
                                .map(|d| d.id);
                            if let Some(doc) = doc
                                && let Some(editor) = self.workspace.editor_mut(doc)
                            {
                                match editor.save_as(&path) {
                                    Ok(()) => {
                                        if let Some(document) = self.workspace.doc_mut(doc) {
                                            document.kind =
                                                DocKind::TreeFile { path: path.clone() };
                                            document.title = file_name(&path);
                                        }
                                        self.settings.add_recent(&path);
                                    }
                                    Err(message) => self.notify(
                                        ctx,
                                        NoticeLevel::Error,
                                        format!("Cannot save: {message}"),
                                    ),
                                }
                            } else if let Some(text) = doc
                                .and_then(|d| self.workspace.doc(d))
                                .map(|d| d.runtime.xml.clone())
                            {
                                self.jobs.save_text(ctx, path, text);
                            }
                        }
                        DialogPurpose::SaveRecording => {
                            self.save_recording_to(ctx, path);
                        }
                        DialogPurpose::AddTreeFile
                        | DialogPurpose::NewTreeFile
                        | DialogPurpose::SaveProject => {
                            if let Some(view) = self.project_view.take() {
                                self.project_file_picked(ctx, view, purpose, path);
                            }
                        }
                        DialogPurpose::SaveBreakpoints => self.save_breakpoints_to(ctx, path),
                        DialogPurpose::LoadBreakpoints => self.jobs.load_breakpoints(ctx, path),
                        DialogPurpose::ExportTransitions(format) => {
                            self.export_transitions_to(ctx, path, format);
                        }
                    }
                }
                JobResult::Tree { path, result } => match result {
                    Ok(loaded) => {
                        self.add_tree_document(&path, loaded);
                    }
                    Err(message) => {
                        self.settings.remove_recent(&path);
                        self.notify(
                            ctx,
                            NoticeLevel::Error,
                            format!("Cannot open {}: {message}", path.display()),
                        );
                    }
                },
                JobResult::Log {
                    path,
                    session,
                    result,
                } => match result {
                    Ok(loaded) => {
                        self.add_log_document(&path, session, *loaded);
                    }
                    Err(message) => {
                        self.settings.remove_recent(&path);
                        self.notify(
                            ctx,
                            NoticeLevel::Error,
                            format!("Cannot open {}: {message}", path.display()),
                        );
                    }
                },
                JobResult::Saved { path, result } => match result {
                    Ok(()) => {
                        self.notify(ctx, NoticeLevel::Info, format!("Saved {}", path.display()))
                    }
                    Err(message) => self.notify(
                        ctx,
                        NoticeLevel::Error,
                        format!("Cannot save {}: {message}", path.display()),
                    ),
                },
                JobResult::SqliteSessions { path, result } => match result {
                    Ok(sessions) if sessions.is_empty() => self.notify(
                        ctx,
                        NoticeLevel::Warning,
                        format!("{} has no sessions.", path.display()),
                    ),
                    Ok(sessions) => {
                        let selected = sessions.len() - 1;
                        self.sqlite_picker = Some(SqlitePicker {
                            path,
                            sessions,
                            selected,
                        });
                    }
                    Err(message) => {
                        self.settings.remove_recent(&path);
                        self.notify(
                            ctx,
                            NoticeLevel::Error,
                            format!("Cannot open {}: {message}", path.display()),
                        );
                    }
                },
                JobResult::RecordingSaved { path, result } => match result {
                    Ok(count) => {
                        self.settings.add_recent(&path);
                        self.notify_with_action(
                            ctx,
                            NoticeLevel::Info,
                            format!("Saved {count} transitions to {}.", path.display()),
                            "Open in replay",
                            AppAction::OpenPath(path),
                        );
                    }
                    Err(message) => self.notify(
                        ctx,
                        NoticeLevel::Error,
                        format!("Cannot save the recording to {}: {message}", path.display()),
                    ),
                },
                JobResult::Exported { path, result } => match result {
                    Ok(count) => self.notify(
                        ctx,
                        NoticeLevel::Info,
                        format!("Exported {count} transitions to {}.", path.display()),
                    ),
                    Err(message) => self.notify(
                        ctx,
                        NoticeLevel::Error,
                        format!("Cannot export to {}: {message}", path.display()),
                    ),
                },
                JobResult::BreakpointsSaved { path, result } => match result {
                    Ok(()) => self.notify(
                        ctx,
                        NoticeLevel::Info,
                        format!("Saved the breakpoints to {}.", path.display()),
                    ),
                    Err(message) => self.notify(
                        ctx,
                        NoticeLevel::Error,
                        format!("Cannot save {}: {message}", path.display()),
                    ),
                },
                JobResult::BreakpointsLoaded { path, result } => match result {
                    Ok(set) => self.prepare_breakpoints(ctx, path, set),
                    Err(message) => self.notify(
                        ctx,
                        NoticeLevel::Error,
                        format!("Cannot read {}: {message}", path.display()),
                    ),
                },
            }
        }
    }

    fn handle_live(&mut self, ctx: &egui::Context) {
        for update in self.live.poll() {
            match update {
                LiveUpdate::Tree(tree) => self.show_live_tree(tree),
                LiveUpdate::Stopped(uid) => self.on_breakpoint(ctx, uid),
                LiveUpdate::Notice(level, text) => self.notify(ctx, level, text),
            }
        }
        if self.live.is_active() {
            // The controller reads the hook list of the executor now and
            // then; it needs a frame for that.
            ctx.request_repaint_after(crate::live::HOOK_DUMP_INTERVAL);
        }
    }

    fn show_live_tree(&mut self, tree: Arc<fasttick_core::runtime::RuntimeTree>) {
        let endpoint = self.live.endpoint_text().unwrap_or_default();
        match self.live_doc.filter(|d| self.workspace.doc(*d).is_some()) {
            Some(doc) => {
                let same = self
                    .workspace
                    .doc(doc)
                    .is_some_and(|d| d.runtime.xml == tree.xml);
                if !same {
                    self.workspace.replace_runtime(doc, tree);
                }
                if let Some(document) = self.workspace.doc_mut(doc) {
                    document.kind = DocKind::Live { endpoint };
                }
            }
            None => {
                let doc = self
                    .workspace
                    .add_doc(DocKind::Live { endpoint }, "Live", tree, None);
                self.live_doc = Some(doc);
                if let Some(view) =
                    self.workspace
                        .open_view(doc, InstanceIndex(0), self.view_options())
                {
                    dock::add_document(&mut self.docks[Mode::Live.index()], Tab::Document(view));
                    if self.mode == Mode::Live {
                        self.workspace.active_view = Some(view);
                    }
                }
            }
        }
    }

    /// The executor stopped at a breakpoint: show the node, unless the user
    /// turned this off.
    fn on_breakpoint(&mut self, ctx: &egui::Context, uid: Uid) {
        if self.settings.auto_focus_on_breakpoint
            && let Some(doc) = self.live_doc
        {
            self.reveal_node(doc, uid);
        }
        if self.mode != Mode::Live {
            let name = self
                .live
                .node_text(uid)
                .map_or_else(|| format!("UID {}", uid.0), |(name, _)| name);
            self.notify_with_action(
                ctx,
                NoticeLevel::Info,
                format!("The executor stopped before {name}."),
                "Show",
                AppAction::Command(Command::ModeLive),
            );
        }
    }

    /// Moves the playback of the replay documents forward.
    fn handle_replay(&mut self, ctx: &egui::Context) {
        let now = ctx.input(|i| i.time);
        let ids: Vec<DocId> = self
            .workspace
            .docs()
            .iter()
            .filter(|d| d.replay.is_some())
            .map(|d| d.id)
            .collect();
        let mut playing = false;
        for doc in ids {
            if let Some(model) = self.workspace.replay_mut(doc) {
                model.advance(now);
                playing |= model.cursor().is_playing();
            }
        }
        if playing {
            ctx.request_repaint();
        }
    }

    // ----- Actions -----

    fn handle_actions(&mut self, ctx: &egui::Context) {
        let actions = std::mem::take(&mut self.actions);
        for action in actions {
            match action {
                AppAction::Command(command) => self.run_command(ctx, command),
                AppAction::OpenPath(path) => self.start_open(ctx, path),
                AppAction::CloseView(view) => self.close_view(view),
                AppAction::SetViewRoot(view, instance) => {
                    self.workspace.set_view_root(view, instance);
                }
                AppAction::OpenTreeInNewTab(doc, instance) => {
                    let mode = self.workspace.doc(doc).map(|d| d.mode());
                    if let Some(mode) = mode
                        && let Some(view) =
                            self.workspace.open_view(doc, instance, self.view_options())
                    {
                        dock::add_document(&mut self.docks[mode.index()], Tab::Document(view));
                        self.focus_view(view);
                    }
                }
                AppAction::RevealInstance(view, instance) => {
                    self.workspace.reveal_instance(view, instance);
                }
                AppAction::RevealNode(doc, uid) => self.reveal_node(doc, uid),
                AppAction::Connect { host, port } => self.connect(ctx, host, port),
                AppAction::PickFile(purpose) => {
                    if !self.jobs.dialog_open() {
                        self.pick_file(ctx, purpose);
                    }
                }
                AppAction::ClearRecent => self.settings.recent.clear(),
                AppAction::Edit(view, op) => self.run_edit(ctx, view, op),
                AppAction::RevealXml { view, file, node } => {
                    self.workspace.reveal_xml(view, file, node, true);
                    self.workspace.active_view = Some(view);
                }
                AppAction::ProjectFile { view, purpose } => {
                    if !self.jobs.dialog_open() {
                        self.project_view = Some(view);
                        self.pick_file(ctx, purpose);
                    }
                }
                AppAction::EditModel { view, model } => {
                    self.model_form = self
                        .workspace
                        .doc_of_view(view)
                        .and_then(|d| d.editor.as_deref())
                        .map(|editor| {
                            let file = crate::edit_ops::view_file(&self.workspace, view)
                                .unwrap_or(editor.file_id());
                            let registry = editor.project().registry();
                            let taken: Vec<String> = registry
                                .all_models(file)
                                .values()
                                .flatten()
                                .map(|m| m.id.clone())
                                .collect();
                            let existing = model
                                .as_deref()
                                .and_then(|id| editor.project().lookup_model(file, id));
                            match existing {
                                Some((m, _)) => {
                                    crate::model_dialog::ModelForm::edit_model(view, m, taken)
                                }
                                None => crate::model_dialog::ModelForm::new_model(view, taken),
                            }
                        });
                }
                AppAction::PromptName { view, purpose } => {
                    let taken = self
                        .workspace
                        .doc_of_view(view)
                        .and_then(|d| d.editor.as_deref())
                        .map(|e| e.trees().into_iter().map(|(_, id)| id).collect())
                        .unwrap_or_default();
                    let value = match &purpose {
                        NamePurpose::RenameTree { old } => old.clone(),
                        _ => String::new(),
                    };
                    self.name_prompt = Some(NamePrompt {
                        view,
                        purpose,
                        value,
                        taken,
                        focused: false,
                    });
                }
            }
        }
    }

    /// The active view if it shows an edit document.
    pub fn active_edit_view(&self) -> Option<ViewId> {
        let view = self.workspace.active_view?;
        self.workspace
            .doc_of_view(view)
            .filter(|d| d.editor.is_some() && d.mode() == self.mode)
            .map(|_| view)
    }

    /// The editor document of the active edit view.
    fn active_editor(&self) -> Option<&crate::editor::EditorDoc> {
        let view = self.active_edit_view()?;
        self.workspace.doc_of_view(view)?.editor.as_deref()
    }

    /// The clipboard text of the tree editor.
    pub fn clipboard(&self) -> Option<&str> {
        self.clipboard.as_deref()
    }

    /// Runs an edit operation on the document of a view, then selects the
    /// new or changed nodes. Errors show as a notice.
    pub fn run_edit(&mut self, ctx: &egui::Context, view: ViewId, op: EditOp) {
        let is_copy = op == EditOp::Copy;
        match crate::edit_ops::run(&mut self.workspace, view, op, &mut self.clipboard) {
            Ok(outcome) => {
                if is_copy && let Some(text) = &self.clipboard {
                    ctx.copy_text(text.clone());
                }
                if let Some(tree) = outcome.show_tree {
                    self.workspace.set_view_root(view, tree);
                }
                self.workspace.refresh_editor_views();
                let file = crate::edit_ops::view_file(&self.workspace, view);
                if let (Some(file), false) = (file, outcome.select.is_empty())
                    && let Some(view_ref) = self.workspace.view_mut(view)
                {
                    let tree = view_ref.canvas.tree_mut();
                    tree.clear_selection();
                    let ids: Vec<_> = outcome
                        .select
                        .iter()
                        .filter_map(|n| tree.find_source(NodeSource::Xml(file, *n)))
                        .collect();
                    for id in &ids {
                        tree.set_selected(*id, true);
                    }
                    if let Some(first) = ids.first() {
                        view_ref.canvas.focus_on(*first, true);
                    }
                }
                if let Some(message) = outcome.message {
                    self.notify(ctx, NoticeLevel::Info, message);
                }
            }
            Err(message) => self.notify(ctx, NoticeLevel::Warning, message),
        }
    }

    /// Runs a project change after its file dialog (tests call it with a
    /// path instead of the dialog).
    pub fn project_file_picked(
        &mut self,
        ctx: &egui::Context,
        view: ViewId,
        purpose: DialogPurpose,
        path: PathBuf,
    ) {
        let Some(doc) = self.workspace.doc_of_view(view).map(|d| d.id) else {
            return;
        };
        let Some(editor) = self.workspace.editor_mut(doc) else {
            return;
        };
        let result = match purpose {
            DialogPurpose::AddTreeFile => editor.add_tree_file(&path).map(|file| {
                (
                    format!("Added {} to the project.", file_name(&path)),
                    editor.tree_refs().iter().position(|r| r.file == file),
                )
            }),
            DialogPurpose::NewTreeFile => editor.new_tree_file(&path).map(|file| {
                (
                    format!("Made the new file {}.", file_name(&path)),
                    editor.tree_refs().iter().position(|r| r.file == file),
                )
            }),
            DialogPurpose::SaveProject => editor
                .save_project_as(&path)
                .map(|()| (format!("Saved the project {}.", file_name(&path)), None)),
            _ => return,
        };
        match result {
            Ok((message, show)) => {
                if purpose == DialogPurpose::SaveProject
                    && let Some(document) = self.workspace.doc_mut(doc)
                {
                    document.kind = DocKind::TreeFile { path: path.clone() };
                    document.title = file_name(&path);
                    self.settings.add_recent(&path);
                }
                self.workspace.refresh_editor_views();
                if let Some(index) = show {
                    self.workspace.set_view_root(view, InstanceIndex(index));
                }
                self.notify(ctx, NoticeLevel::Info, message);
            }
            Err(message) => self.notify(ctx, NoticeLevel::Error, message),
        }
    }

    /// Saves the document of a view. A new file asks for a path.
    fn save_view(&mut self, ctx: &egui::Context, view: ViewId) {
        let Some(doc) = self.workspace.doc_of_view(view).map(|d| d.id) else {
            return;
        };
        let Some(editor) = self.workspace.editor_mut(doc) else {
            return;
        };
        match editor.save() {
            Ok(crate::editor::SaveOutcome::Saved) => {
                if let Some(path) = editor.path() {
                    self.settings.add_recent(&path);
                }
            }
            Ok(crate::editor::SaveOutcome::NeedsPath) => {
                self.save_as_view = Some(view);
                self.jobs.pick_file(ctx, DialogPurpose::SaveAs, None);
            }
            Err(message) => {
                self.notify(ctx, NoticeLevel::Error, format!("Cannot save: {message}"));
            }
        }
    }

    /// Opens a new, empty tree file in edit mode.
    fn new_document(&mut self) {
        let editor = crate::editor::EditorDoc::untitled();
        let main = InstanceIndex(editor.main_tree_index());
        let doc = self
            .workspace
            .add_editor_doc(PathBuf::new(), "Untitled", editor);
        if let Some(view) = self.workspace.open_view(doc, main, self.view_options()) {
            dock::add_document(&mut self.docks[Mode::Edit.index()], Tab::Document(view));
            self.focus_view(view);
        }
    }

    // ----- UI -----

    /// Draws one frame. `eframe::App::ui` calls this; the UI tests call it
    /// through the same trait.
    pub fn show(&mut self, ui: &mut Ui) {
        let ctx = ui.ctx().clone();
        self.handle_jobs(&ctx);
        self.handle_live(&ctx);
        self.handle_replay(&ctx);

        let commands = self.keymap.consume(&ctx, |c| self.is_enabled(c));
        for command in commands {
            self.run_command(&ctx, command);
        }

        let palette = CanvasPalette::for_theme(ctx.theme());

        egui::Panel::top("fasttick_top_bar")
            .frame(
                egui::Frame::side_top_panel(ui.style()).inner_margin(egui::Margin::symmetric(8, 4)),
            )
            .show(ui, |ui| self.top_bar(ui, &palette));
        egui::Panel::bottom("fasttick_status_bar")
            .frame(
                egui::Frame::side_top_panel(ui.style())
                    .inner_margin(egui::Margin::symmetric(10, 3)),
            )
            .show(ui, |ui| self.status_bar(ui, &palette));
        egui::CentralPanel::no_frame()
            .frame(
                egui::Frame::NONE
                    .fill(ui.visuals().panel_fill)
                    .inner_margin(4),
            )
            .show(ui, |ui| {
                if self.mode == Mode::Live {
                    self.live_header(ui, &palette);
                }
                self.dock_area(ui, &palette);
            });

        self.dialogs(&ctx, &palette);
        if let Some(action) = dialogs::notices(&ctx, &mut self.notices) {
            self.actions.push(action);
        }
        self.handle_actions(&ctx);
        self.update_title(&ctx);
    }

    /// The live bar and the breakpoint banner, above the dock of live mode.
    fn live_header(&mut self, ui: &mut Ui, palette: &CanvasPalette) {
        let mut cx = PanelContext {
            mode: self.mode,
            workspace: &mut self.workspace,
            settings: &mut self.settings,
            live: &mut self.live,
            live_doc: self.live_doc,
            loading: self.jobs.loading(),
            keymap: &self.keymap,
            palette,
            actions: &mut self.actions,
        };
        egui::Frame::NONE
            .inner_margin(egui::Margin::symmetric(6, 4))
            .show(ui, |ui| {
                live_bar::show_bar(ui, &mut cx);
                if !cx.live.hits().is_empty() {
                    ui.add_space(4.0);
                    live_bar::show_banner(ui, &mut cx);
                }
            });
        ui.add_space(2.0);
    }

    fn update_title(&mut self, ctx: &egui::Context) {
        let title = match self
            .active_view_in_mode()
            .and_then(|v| self.workspace.doc_of_view(v))
        {
            Some(doc) => format!("{} \u{2014} FastTick", doc.title),
            None => "FastTick".to_owned(),
        };
        if title != self.window_title {
            ctx.send_viewport_cmd(egui::ViewportCommand::Title(title.clone()));
            self.window_title = title;
        }
    }

    fn top_bar(&mut self, ui: &mut Ui, palette: &CanvasPalette) {
        // Three areas: logo and menus at the left, the mode switcher in
        // the middle, the view controls at the right. The menu bar takes
        // all the width that it gets, so each area has its own rectangle.
        let height = ui.spacing().interact_size.y + 4.0;
        let full = ui.available_rect_before_wrap();
        let full = egui::Rect::from_min_size(full.min, egui::vec2(full.width(), height));
        ui.allocate_rect(full, egui::Sense::hover());
        let left = egui::Rect::from_min_max(full.min, egui::pos2(full.min.x + 380.0, full.max.y));
        ui.scope_builder(
            egui::UiBuilder::new()
                .max_rect(left)
                .layout(egui::Layout::left_to_right(egui::Align::Center)),
            |ui| {
                widgets::logo(ui, palette.accent);
                egui::MenuBar::new().ui(ui, |ui| self.menus(ui));
            },
        );
        let switcher = egui::Rect::from_center_size(full.center(), egui::vec2(230.0, height));
        ui.scope_builder(
            egui::UiBuilder::new()
                .max_rect(switcher)
                .layout(egui::Layout::left_to_right(egui::Align::Center)),
            |ui| {
                ui.with_layout(egui::Layout::top_down(egui::Align::Center), |ui| {
                    let mut mode = self.mode;
                    if widgets::segmented(
                        ui,
                        &mut mode,
                        &[
                            (Mode::Edit, "Edit"),
                            (Mode::Live, "Live"),
                            (Mode::Replay, "Replay"),
                        ],
                        " mode",
                        palette.accent,
                    ) {
                        self.set_mode(mode);
                    }
                });
            },
        );
        let right =
            egui::Rect::from_min_max(egui::pos2(switcher.max.x + 8.0, full.min.y), full.max);
        ui.scope_builder(
            egui::UiBuilder::new()
                .max_rect(right)
                .layout(egui::Layout::right_to_left(egui::Align::Center)),
            |ui| self.view_controls(ui),
        );
    }

    fn view_controls(&mut self, ui: &mut Ui) {
        let active = self.active_view_in_mode();
        let enabled = active.is_some();
        let ctx = ui.ctx().clone();
        let direction = active
            .and_then(|v| self.workspace.view(v))
            .map_or(self.settings.layout_direction, |v| v.canvas.direction());
        let (icon, label) = match direction {
            LayoutDirection::TopDown => (Icon::LeftRight, "Lay out left to right"),
            LayoutDirection::LeftRight => (Icon::TopDown, "Lay out top to bottom"),
        };
        if widgets::icon_button(ui, icon, label, true).clicked() {
            self.run_command(&ctx, Command::ToggleLayout);
        }
        if widgets::icon_button(ui, Icon::Fit, "Fit to view", enabled).clicked() {
            self.run_command(&ctx, Command::FitToView);
        }
        if widgets::icon_button(ui, Icon::Plus, "Zoom in", enabled).clicked() {
            self.run_command(&ctx, Command::ZoomIn);
        }
        let zoom = active
            .and_then(|v| self.workspace.view(v))
            .map(|v| v.canvas.viewport().zoom);
        let text = zoom.map_or("\u{2013}".to_owned(), |z| format!("{:.0} %", z * 100.0));
        if ui
            .add_enabled(
                enabled,
                egui::Button::new(RichText::new(text).monospace()).min_size(egui::vec2(58.0, 0.0)),
            )
            .on_hover_text("Zoom to 100 %")
            .clicked()
        {
            self.run_command(&ctx, Command::ZoomReset);
        }
        if widgets::icon_button(ui, Icon::Minus, "Zoom out", enabled).clicked() {
            self.run_command(&ctx, Command::ZoomOut);
        }
    }

    fn menu_item(&mut self, ui: &mut Ui, command: Command) {
        let mut button = egui::Button::new(command.label());
        if let Some(text) = self.keymap.shortcut_text(ui.ctx(), command) {
            button = button.shortcut_text(text);
        }
        let enabled = self.is_enabled(command);
        let response = ui.add_enabled(enabled, button);
        if response.clicked() {
            let ctx = ui.ctx().clone();
            self.run_command(&ctx, command);
        }
        if !enabled {
            response.on_disabled_hover_text(disabled_reason(command));
        }
    }

    fn radio_item(&mut self, ui: &mut Ui, command: Command, selected: bool) {
        let mut button = egui::Button::selectable(selected, command.label());
        if let Some(text) = self.keymap.shortcut_text(ui.ctx(), command) {
            button = button.shortcut_text(text);
        }
        if ui.add(button).clicked() {
            let ctx = ui.ctx().clone();
            self.run_command(&ctx, command);
        }
    }

    fn menus(&mut self, ui: &mut Ui) {
        ui.menu_button("File", |ui| {
            self.menu_item(ui, Command::NewProject);
            self.menu_item(ui, Command::OpenFile);
            self.menu_item(ui, Command::OpenLog);
            submenu(ui, "Recent", |ui| {
                if self.settings.recent.is_empty() {
                    ui.label(RichText::new("No recent files").weak());
                }
                let recent = self.settings.recent.clone();
                for entry in recent {
                    let text = file_name(&entry.path);
                    if ui
                        .button(text)
                        .on_hover_text(entry.path.display().to_string())
                        .clicked()
                    {
                        self.actions.push(AppAction::OpenPath(entry.path.clone()));
                    }
                }
                if !self.settings.recent.is_empty() {
                    ui.separator();
                    if ui.button("Clear recent files").clicked() {
                        self.actions.push(AppAction::ClearRecent);
                    }
                }
            });
            ui.separator();
            self.menu_item(ui, Command::Save);
            self.menu_item(ui, Command::SaveAs);
            ui.separator();
            self.menu_item(ui, Command::Quit);
        });
        ui.menu_button("Edit", |ui| {
            self.menu_item(ui, Command::Undo);
            self.menu_item(ui, Command::Redo);
            ui.separator();
            self.menu_item(ui, Command::Cut);
            self.menu_item(ui, Command::Copy);
            self.menu_item(ui, Command::Paste);
            self.menu_item(ui, Command::Duplicate);
            self.menu_item(ui, Command::Delete);
            ui.separator();
            self.menu_item(ui, Command::SelectAll);
            self.menu_item(ui, Command::Find);
        });
        ui.menu_button("View", |ui| {
            self.menu_item(ui, Command::FitToView);
            self.menu_item(ui, Command::ZoomIn);
            self.menu_item(ui, Command::ZoomOut);
            self.menu_item(ui, Command::ZoomReset);
            ui.separator();
            let direction = self
                .active_view_in_mode()
                .and_then(|v| self.workspace.view(v))
                .map_or(self.settings.layout_direction, |v| v.canvas.direction());
            submenu(ui, "Layout", |ui| {
                self.radio_item(
                    ui,
                    Command::LayoutTopDown,
                    direction == LayoutDirection::TopDown,
                );
                self.radio_item(
                    ui,
                    Command::LayoutLeftRight,
                    direction == LayoutDirection::LeftRight,
                );
                ui.separator();
                self.menu_item(ui, Command::ToggleLayout);
            });
            submenu(ui, "Edges", |ui| {
                for style in [
                    crate::canvas::EdgeStyle::Orthogonal,
                    crate::canvas::EdgeStyle::Curved,
                ] {
                    if ui
                        .add(egui::Button::selectable(
                            self.settings.edge_style == style,
                            style.name(),
                        ))
                        .clicked()
                    {
                        self.settings.edge_style = style;
                        for view in self.workspace.views_in_mode(self.mode) {
                            if let Some(v) = self.workspace.view_mut(view) {
                                v.canvas.set_edge_style(style);
                            }
                        }
                    }
                }
            });
            submenu(ui, "Theme", |ui| {
                let theme = self.theme();
                self.radio_item(ui, Command::ThemeLight, theme == ThemeChoice::Light);
                self.radio_item(ui, Command::ThemeDark, theme == ThemeChoice::Dark);
                self.radio_item(ui, Command::ThemeSystem, theme == ThemeChoice::System);
            });
            submenu(ui, "Panels", |ui| {
                let dock = &mut self.docks[self.mode.index()];
                for tab in Tab::PANELS {
                    let mut shown = dock::contains(dock, tab);
                    if ui.checkbox(&mut shown, tab.title()).changed() {
                        if shown {
                            dock::show_panel(dock, tab);
                        } else {
                            dock::hide_panel(dock, tab);
                        }
                    }
                }
                ui.separator();
                self.menu_item(ui, Command::ResetLayout);
            });
            if ui
                .checkbox(&mut self.settings.show_minimap, "Minimap")
                .changed()
            {
                let show = self.settings.show_minimap;
                for view in self.workspace.views_in_mode(self.mode) {
                    if let Some(v) = self.workspace.view_mut(view) {
                        v.canvas.set_minimap(show);
                    }
                }
            }
            ui.separator();
            self.menu_item(ui, Command::SplitView);
            self.menu_item(ui, Command::CloseTab);
        });
        ui.menu_button("Live", |ui| {
            self.menu_item(ui, Command::Connect);
            self.menu_item(ui, Command::Disconnect);
            ui.separator();
            self.menu_item(ui, Command::ToggleBreakpoint);
            self.menu_item(ui, Command::Continue);
            ui.add_enabled(false, egui::Button::new("Breakpoint after the node"))
                .on_disabled_hover_text(AFTER_BREAKPOINT_UNAVAILABLE);
            ui.separator();
            self.menu_item(ui, Command::ToggleRecording);
            self.menu_item(ui, Command::SaveRecording);
            ui.separator();
            self.menu_item(ui, Command::SaveBreakpoints);
            self.menu_item(ui, Command::LoadBreakpoints);
            ui.separator();
            ui.checkbox(
                &mut self.settings.auto_focus_on_breakpoint,
                "Show the node at a breakpoint",
            );
        });
        ui.menu_button("Replay", |ui| {
            self.menu_item(ui, Command::OpenLog);
            ui.separator();
            self.menu_item(ui, Command::PlayPause);
            self.menu_item(ui, Command::StepBack);
            self.menu_item(ui, Command::StepForward);
            self.menu_item(ui, Command::GoToStart);
            self.menu_item(ui, Command::GoToEnd);
            self.menu_item(ui, Command::ResetTimelineZoom);
            ui.separator();
            self.menu_item(ui, Command::ExportJson);
            self.menu_item(ui, Command::ExportCsv);
        });
        ui.menu_button("Help", |ui| {
            self.menu_item(ui, Command::ShowShortcuts);
            self.menu_item(ui, Command::About);
        });
    }

    fn status_bar(&mut self, ui: &mut Ui, palette: &CanvasPalette) {
        ui.horizontal(|ui| {
            ui.spacing_mut().item_spacing.x = 6.0;
            let small = |text: String| RichText::new(text).size(12.0);
            ui.label(small(format!("{} mode", self.mode.name())).strong());
            ui.separator();
            let (color, text) = match self.live.state() {
                ConnectionState::Disconnected => (palette.skipped, self.live.state_text()),
                ConnectionState::Connecting | ConnectionState::Reconnecting { .. } => {
                    (palette.running, self.live.state_text())
                }
                ConnectionState::Connected { .. } => (palette.success, self.live.state_text()),
            };
            widgets::status_dot(ui, color);
            ui.label(small(text));
            ui.separator();
            let active = self.active_view_in_mode();
            let doc = active.and_then(|v| self.workspace.doc_of_view(v));
            match doc {
                Some(doc) => {
                    let text = match &doc.kind {
                        DocKind::TreeFile { path } | DocKind::Replay { path, .. } => {
                            path.display().to_string()
                        }
                        DocKind::Live { endpoint } => format!("Live tree from {endpoint}"),
                    };
                    ui.label(small(text));
                }
                None => {
                    ui.label(small("No document".to_owned()).weak());
                }
            }
            if let Some(loading) = self.jobs.loading().first() {
                ui.separator();
                ui.spinner();
                let name = file_name(&loading.path);
                let text = match loading.progress.phase() {
                    crate::jobs::LoadPhase::Reading => format!(
                        "Loading {name} ({:.0} %)",
                        loading.progress.fraction() * 100.0
                    ),
                    crate::jobs::LoadPhase::Indexing => format!("Indexing {name}"),
                };
                ui.label(small(text));
                ctx_repaint_soon(ui);
            } else if self.jobs.running() > 0 && !self.jobs.dialog_open() {
                ui.separator();
                ui.spinner();
                ui.label(small("Working".to_owned()));
            }
            if self.live.recording().is_active() {
                ui.separator();
                widgets::status_dot(ui, palette.breakpoint);
                ui.label(small(format!(
                    "Recording ({} transitions)",
                    self.live.recording().len()
                )));
            }
            if !self.live.hits().is_empty() {
                ui.separator();
                widgets::status_dot(ui, palette.breakpoint);
                ui.label(small("Stopped at a breakpoint".to_owned()));
            }
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                if let Some(view) = active.and_then(|v| self.workspace.view(v)) {
                    let stats = view.canvas.stats();
                    ui.label(small(view.canvas.direction().name().to_owned()));
                    ui.separator();
                    ui.label(small(format!(
                        "Zoom {:.0} %",
                        view.canvas.viewport().zoom * 100.0
                    )));
                    ui.separator();
                    ui.label(small(format!(
                        "{} nodes, {} visible",
                        stats.total_nodes, stats.visible_nodes
                    )));
                    let selected = view.canvas.tree().selected_count();
                    if selected > 0 {
                        ui.separator();
                        ui.label(small(format!("{selected} selected")));
                    }
                }
            });
        });
    }

    fn dock_area(&mut self, ui: &mut Ui, palette: &CanvasPalette) {
        let mode = self.mode;
        let mut style = egui_dock::Style::from_egui(ui.style());
        style.tab_bar.height = 28.0;
        style.tab.tab_body.inner_margin = egui::Margin::same(8);
        style.tab.tab_body.stroke =
            egui::Stroke::new(1.0, ui.visuals().widgets.noninteractive.bg_stroke.color);
        style.separator.width = 1.0;
        style.separator.extra = 4.0;
        style.tab_bar.corner_radius = egui::CornerRadius::same(4);
        style.tab_bar.show_scroll_bar_on_overflow = false;
        let dock = &mut self.docks[mode.index()];
        let mut viewer = Viewer {
            mode,
            workspace: &mut self.workspace,
            settings: &mut self.settings,
            live: &mut self.live,
            live_doc: self.live_doc,
            loading: self.jobs.loading(),
            keymap: &self.keymap,
            palette,
            actions: &mut self.actions,
            has_clipboard: self.clipboard.is_some(),
        };
        DockArea::new(dock)
            .id(Id::new(("fasttick_dock", mode.index())))
            .style(style)
            .show_leaf_collapse_buttons(false)
            .show_leaf_close_all_buttons(false)
            .show_inside(ui, &mut viewer);
        // Follow the focused document tab.
        let focused = dock::focused_view(dock);
        let first = dock::document_views(dock).first().copied();
        if let Some(view) = focused {
            self.workspace.active_view = Some(view);
        } else if self.active_view_in_mode().is_none() {
            self.workspace.active_view = first;
        }
    }

    fn dialogs(&mut self, ctx: &egui::Context, palette: &CanvasPalette) {
        if self.show_about {
            self.show_about = dialogs::about(ctx, palette.accent);
        }
        if self.show_shortcuts {
            self.show_shortcuts = dialogs::shortcuts(ctx, &self.keymap);
        }
        self.breakpoint_dialog(ctx);
        self.sqlite_dialog(ctx);
        if let Some(form) = &mut self.model_form {
            match crate::model_dialog::show(ctx, form) {
                PromptChoice::Open => {}
                PromptChoice::Cancel => self.model_form = None,
                PromptChoice::Accept => {
                    if let Some(form) = self.model_form.take() {
                        let op = form.op();
                        self.run_edit(ctx, form.view, op);
                    }
                }
            }
        }
        if let Some(prompt) = &mut self.name_prompt {
            match dialogs::name_prompt(ctx, prompt) {
                (PromptChoice::Open, _) => {}
                (PromptChoice::Cancel, _) => self.name_prompt = None,
                (PromptChoice::Accept, name) => {
                    if let Some(prompt) = self.name_prompt.take() {
                        let op = prompt.purpose.op(name);
                        self.run_edit(ctx, prompt.view, op);
                    }
                }
            }
        }
        if let Some(form) = &mut self.connect_form {
            match dialogs::connect(ctx, form) {
                ConnectChoice::Open => {}
                ConnectChoice::Cancel => self.connect_form = None,
                ConnectChoice::Connect => {
                    let form = self.connect_form.take().unwrap_or(ConnectForm {
                        host: String::new(),
                        port: 0,
                        poll_interval_ms: 100,
                    });
                    self.settings.poll_interval_ms = form.poll_interval_ms;
                    self.settings.sanitize();
                    self.connect(ctx, form.host.trim().to_owned(), form.port);
                }
            }
        }
    }
}

impl FastTickApp {
    /// The dialog that shows a loaded breakpoint file before the hooks go
    /// to the executor.
    fn breakpoint_dialog(&mut self, ctx: &egui::Context) {
        let Some(pending) = &mut self.pending_breakpoints else {
            return;
        };
        let mut open = true;
        let mut insert = false;
        let mut cancel = false;
        let palette = CanvasPalette::for_theme(ctx.theme());
        egui::Window::new("Load breakpoints")
            .collapsible(false)
            .resizable(true)
            .default_width(560.0)
            .anchor(egui::Align2::CENTER_CENTER, [0.0, -40.0])
            .open(&mut open)
            .show(ctx, |ui| {
                ui.label(format!(
                    "{}: {} breakpoints. FastTick looked for each node in the running \
                     tree by its path.",
                    file_name(&pending.path),
                    pending.set.breakpoints.len()
                ));
                ui.add_space(6.0);
                egui::ScrollArea::vertical()
                    .max_height(320.0)
                    .show(ui, |ui| {
                        egui::Grid::new("pending_breakpoints")
                            .num_columns(3)
                            .spacing([10.0, 6.0])
                            .striped(true)
                            .show(ui, |ui| {
                                for (row, resolved) in pending.resolved.iter().enumerate() {
                                    let Some(breakpoint) =
                                        pending.set.breakpoints.get(resolved.index)
                                    else {
                                        continue;
                                    };
                                    let found = resolved.uid.is_some();
                                    let path = if breakpoint.node_path.is_empty() {
                                        "(no path)".to_owned()
                                    } else {
                                        breakpoint.node_path.clone()
                                    };
                                    ui.add_enabled(
                                        found,
                                        egui::Checkbox::new(&mut pending.chosen[row], path),
                                    );
                                    ui.label(match (breakpoint.mode, breakpoint.desired_status) {
                                        (HookMode::Breakpoint, _) => "Breakpoint".to_owned(),
                                        (HookMode::Replace, status) => format!("Force {status}"),
                                    });
                                    let (text, color) = resolution_text(resolved, breakpoint.uid);
                                    let color = match color {
                                        ResolutionColor::Good => palette.success,
                                        ResolutionColor::Warning => ui.visuals().warn_fg_color,
                                        ResolutionColor::Bad => ui.visuals().error_fg_color,
                                    };
                                    ui.label(RichText::new(text).color(color));
                                    ui.end_row();
                                }
                            });
                    });
                ui.add_space(8.0);
                let count = pending
                    .resolved
                    .iter()
                    .zip(&pending.chosen)
                    .filter(|(r, c)| **c && r.uid.is_some())
                    .count();
                ui.horizontal(|ui| {
                    if ui
                        .add_enabled(
                            count > 0,
                            egui::Button::new(
                                RichText::new(format!("Insert {count} hooks")).strong(),
                            ),
                        )
                        .clicked()
                    {
                        insert = true;
                    }
                    if ui.button("Cancel").clicked() {
                        cancel = true;
                    }
                });
            });
        if insert {
            let count = self.apply_pending_breakpoints();
            self.notify(
                ctx,
                NoticeLevel::Info,
                format!("Inserted {count} hooks from the breakpoint file."),
            );
        } else if cancel || !open {
            self.pending_breakpoints = None;
        }
    }

    /// The dialog that picks one session of a SQLite log.
    fn sqlite_dialog(&mut self, ctx: &egui::Context) {
        let Some(picker) = &mut self.sqlite_picker else {
            return;
        };
        let mut open = true;
        let mut choice = None;
        let mut cancel = false;
        egui::Window::new("Choose a session")
            .collapsible(false)
            .resizable(false)
            .anchor(egui::Align2::CENTER_CENTER, [0.0, -40.0])
            .open(&mut open)
            .show(ctx, |ui| {
                ui.label(format!(
                    "{} has {} sessions. Each session is one run of the tree.",
                    file_name(&picker.path),
                    picker.sessions.len()
                ));
                ui.add_space(6.0);
                egui::ScrollArea::vertical()
                    .max_height(300.0)
                    .show(ui, |ui| {
                        for (index, session) in picker.sessions.iter().enumerate() {
                            let text = format!(
                                "Session {}   {}   {} transitions",
                                session.id, session.date, session.transition_count
                            );
                            let response =
                                ui.add(egui::Button::selectable(picker.selected == index, text));
                            if response.clicked() {
                                picker.selected = index;
                            }
                            if response.double_clicked() {
                                choice = Some(session.id);
                            }
                        }
                    });
                ui.add_space(8.0);
                ui.horizontal(|ui| {
                    if ui.button(RichText::new("Open").strong()).clicked() {
                        choice = picker.sessions.get(picker.selected).map(|s| s.id);
                    }
                    if ui.button("Cancel").clicked() {
                        cancel = true;
                    }
                });
            });
        if let Some(id) = choice {
            let path = picker.path.clone();
            self.sqlite_picker = None;
            if let Some(view) = self.find_open_view(&path, Some(id)) {
                self.focus_view(view);
            } else {
                self.jobs.load_sqlite_session(ctx, path, id);
            }
        } else if cancel || !open {
            self.sqlite_picker = None;
        }
    }
}

/// How good a breakpoint resolution is.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum ResolutionColor {
    Good,
    Warning,
    Bad,
}

/// The text that explains how a breakpoint of a file was found.
fn resolution_text(
    resolved: &ResolvedBreakpoint,
    stored: Option<u16>,
) -> (String, ResolutionColor) {
    match resolved.resolution {
        Resolution::ByPath => ("Found by path".to_owned(), ResolutionColor::Good),
        Resolution::AmbiguousPath => (
            "Several nodes have this path; FastTick took one of them".to_owned(),
            ResolutionColor::Warning,
        ),
        Resolution::ByStoredUid => (
            format!(
                "Not found by path. Found only by the stored UID {}: the tree changed, so \
                 this can be another node",
                stored.map_or_else(|| "?".to_owned(), |u| u.to_string())
            ),
            ResolutionColor::Warning,
        ),
        Resolution::Unresolved => ("Not found".to_owned(), ResolutionColor::Bad),
    }
}

/// Asks for a repaint soon (progress displays).
fn ctx_repaint_soon(ui: &Ui) {
    ui.ctx()
        .request_repaint_after(std::time::Duration::from_millis(100));
}

use crate::widgets::submenu;

/// Why a menu command is disabled.
fn disabled_reason(command: Command) -> &'static str {
    match command {
        Command::NewProject
        | Command::Save
        | Command::Undo
        | Command::Redo
        | Command::Cut
        | Command::Copy
        | Command::Paste
        | Command::Duplicate
        | Command::Delete => "This comes with the tree editor.",
        Command::Disconnect => "No connection.",
        Command::SaveAs => "Open a tree file first.",
        _ => "Open a document first.",
    }
}

fn file_name(path: &Path) -> String {
    path.file_name()
        .map(|n| n.to_string_lossy().into_owned())
        .unwrap_or_else(|| path.display().to_string())
}

/// Shows the tabs of the dock.
struct Viewer<'a> {
    mode: Mode,
    workspace: &'a mut Workspace,
    settings: &'a mut Settings,
    live: &'a mut LiveController,
    live_doc: Option<DocId>,
    loading: &'a [LoadingFile],
    keymap: &'a KeyMap,
    palette: &'a CanvasPalette,
    actions: &'a mut Vec<AppAction>,
    has_clipboard: bool,
}

impl Viewer<'_> {
    fn document_ui(&mut self, ui: &mut Ui, view_id: ViewId) {
        let Some(doc) = self.workspace.doc_of_view(view_id) else {
            ui.label("This view is closed.");
            return;
        };
        let doc_id = doc.id;
        let is_live = self.live_doc == Some(doc_id);
        let view_root = self
            .workspace
            .view(view_id)
            .map_or(0, |v| v.root_instance.0);
        let edit_context = doc
            .editor
            .as_deref()
            .filter(|_| doc.mode() == self.mode)
            .map(|editor| {
                let file = editor.tree_file(view_root).unwrap_or(editor.file_id());
                EditMenuContext::new(view_id, editor, file, self.has_clipboard)
            });
        let editable = edit_context.is_some();
        // Statuses and markers of this frame, before the canvas draws.
        self.workspace.refresh_decorations(self.live, self.live_doc);

        let Some(view) = self.workspace.view_mut(view_id) else {
            return;
        };
        let live = &mut *self.live;
        let mut menu_actions = Vec::new();
        let mut menu = |ui: &mut Ui, node: Option<&CanvasNode>| match (&edit_context, node) {
            (Some(context), node) => edit_menu(ui, context, node, &mut menu_actions),
            (None, Some(node)) if is_live => {
                live_node_menu(ui, live, node);
                true
            }
            _ => false,
        };
        let output = TreeCanvas::new(&mut view.canvas, self.palette)
            .id_salt(("canvas", view_id.0))
            .editable(editable)
            .node_menu(&mut menu)
            .show(ui);
        self.actions.append(&mut menu_actions);
        if output.response.clicked()
            || output.response.drag_started()
            || output.response.secondary_clicked()
        {
            self.workspace.active_view = Some(view_id);
        }
        for event in output.events {
            match event {
                CanvasEvent::ToggleSubtree(source) => {
                    self.workspace.toggle_subtree(view_id, source)
                }
                CanvasEvent::SetAllSubtrees(expanded) => {
                    self.workspace.set_all_subtrees(view_id, expanded);
                }
                CanvasEvent::SelectionChanged | CanvasEvent::Activated(_) => {
                    self.workspace.active_view = Some(view_id);
                }
                CanvasEvent::MoveNodes { nodes, spot } => {
                    self.workspace.active_view = Some(view_id);
                    if let Some(op) = self.move_op(view_id, &nodes, spot) {
                        self.actions.push(AppAction::Edit(view_id, op));
                    }
                }
                CanvasEvent::DropNew { type_id, spot } => {
                    self.workspace.active_view = Some(view_id);
                    let op = match spot {
                        DropSpot::Node { target, placement } => {
                            self.xml_node(view_id, target).map(|node| EditOp::AddNode {
                                type_id,
                                target: Some((node, placement_of(placement))),
                            })
                        }
                        _ => Some(EditOp::AddUnconnected { type_id }),
                    };
                    if let Some(op) = op {
                        self.actions.push(AppAction::Edit(view_id, op));
                    }
                }
            }
        }
    }
}

impl Viewer<'_> {
    /// The XML node of a canvas node of an edit view (not for inline
    /// nodes).
    fn xml_node(&self, view: ViewId, id: CanvasNodeId) -> Option<fasttick_core::xml::NodeId> {
        let node = self.workspace.view(view)?.canvas.tree().get(id)?;
        match node.source {
            NodeSource::Xml(_, xml) if !node.inline => Some(xml),
            _ => None,
        }
    }

    /// The edit operation for a move gesture.
    fn move_op(&self, view: ViewId, nodes: &[CanvasNodeId], spot: DropSpot) -> Option<EditOp> {
        let nodes: Vec<_> = nodes
            .iter()
            .filter_map(|id| self.xml_node(view, *id))
            .collect();
        if nodes.is_empty() {
            return None;
        }
        match spot {
            DropSpot::Background => Some(EditOp::DisconnectNodes { nodes }),
            DropSpot::Node { target, placement } => Some(EditOp::MoveNodes {
                nodes,
                target: self.xml_node(view, target)?,
                placement: placement_of(placement),
            }),
            DropSpot::Invalid => None,
        }
    }
}

/// The edit placement for a canvas drop placement.
fn placement_of(placement: DropPlacement) -> Placement {
    match placement {
        DropPlacement::Inside => Placement::Inside,
        DropPlacement::Before => Placement::Before,
        DropPlacement::After => Placement::After,
    }
}

/// The live entries of the context menu of a node: breakpoints and forced
/// results.
fn live_node_menu(ui: &mut Ui, live: &mut LiveController, node: &CanvasNode) {
    let Some(uid) = node.uid else {
        ui.label(RichText::new("This node has no UID.").weak());
        return;
    };
    let connected = live.is_connected();
    if live.is_hit(uid) {
        for action in [
            ResumeAction::Continue,
            ResumeAction::ForceSuccess,
            ResumeAction::ForceFailure,
        ] {
            if ui
                .add_enabled(connected, egui::Button::new(action.label()))
                .clicked()
            {
                live.resume(uid, action);
            }
        }
        ui.separator();
    }
    let hook = live.hook(uid).copied();
    match hook {
        Some(hook) if hook.mode == HookMode::Breakpoint => {
            if ui
                .add_enabled(connected, egui::Button::new("Remove breakpoint"))
                .clicked()
            {
                live.remove_hook(uid);
            }
            let text = if hook.enabled {
                "Disable breakpoint"
            } else {
                "Enable breakpoint"
            };
            if ui.add_enabled(connected, egui::Button::new(text)).clicked() {
                live.set_enabled(uid, !hook.enabled);
            }
        }
        _ => {
            let response = ui
                .add_enabled(
                    connected,
                    egui::Button::new("Add breakpoint before the node"),
                )
                .on_disabled_hover_text("Connect to an executor first.");
            let response = if hook.is_some() {
                response.on_hover_text("This replaces the forced result: a node has one hook.")
            } else {
                response
            };
            if response.clicked() {
                live.add_breakpoint(uid);
            }
        }
    }
    ui.add_enabled(false, egui::Button::new("Add breakpoint after the node"))
        .on_disabled_hover_text(AFTER_BREAKPOINT_UNAVAILABLE);
    ui.separator();
    for (text, status) in [
        ("Force SUCCESS", NodeStatus::Success),
        ("Force FAILURE", NodeStatus::Failure),
    ] {
        let active =
            hook.is_some_and(|h| h.mode == HookMode::Replace && h.desired_status == status);
        if ui
            .add_enabled(connected && !active, egui::Button::new(text))
            .on_hover_text("The node does not tick. It returns this status.")
            .clicked()
        {
            live.force(uid, status);
        }
    }
    let forced = hook.is_some_and(|h| h.mode == HookMode::Replace);
    if ui
        .add_enabled(
            connected && forced,
            egui::Button::new("Clear forced result"),
        )
        .clicked()
    {
        live.remove_hook(uid);
    }
    if !connected {
        ui.label(
            RichText::new("Connect to an executor to use breakpoints.")
                .small()
                .weak(),
        );
    }
}

impl TabViewer for Viewer<'_> {
    type Tab = Tab;

    fn title(&mut self, tab: &mut Tab) -> WidgetText {
        match tab {
            Tab::Document(view_id) => {
                let Some(view) = self.workspace.view(*view_id) else {
                    return "Closed".into();
                };
                let Some(doc) = self.workspace.doc(view.doc) else {
                    return "Closed".into();
                };
                let tree = &view.canvas.tree().title;
                let mut title = if doc.title == *tree || tree.is_empty() {
                    doc.title.clone()
                } else {
                    format!("{} \u{00B7} {tree}", doc.title)
                };
                if self.live_doc == Some(doc.id) && !self.live.is_active() {
                    title.push_str(" (disconnected)");
                }
                if doc.editor.as_ref().is_some_and(|e| e.is_dirty()) {
                    title.push_str(" \u{2022}");
                }
                title.into()
            }
            other => other.title().into(),
        }
    }

    fn id(&mut self, tab: &mut Tab) -> Id {
        Id::new(("fasttick_tab", *tab))
    }

    fn ui(&mut self, ui: &mut Ui, tab: &mut Tab) {
        match *tab {
            Tab::Document(view) => self.document_ui(ui, view),
            other => {
                let mut cx = PanelContext {
                    mode: self.mode,
                    workspace: self.workspace,
                    settings: self.settings,
                    live: self.live,
                    live_doc: self.live_doc,
                    loading: self.loading,
                    keymap: self.keymap,
                    palette: self.palette,
                    actions: self.actions,
                };
                panels::show_panel(other, ui, &mut cx);
            }
        }
    }

    fn on_close(&mut self, tab: &mut Tab) -> OnCloseResponse {
        if let Tab::Document(view) = tab {
            self.actions.push(AppAction::CloseView(*view));
        }
        OnCloseResponse::Close
    }

    fn is_closeable(&self, tab: &Tab) -> bool {
        *tab != Tab::Start
    }

    fn context_menu(&mut self, ui: &mut Ui, tab: &mut Tab, _path: egui_dock::NodePath) {
        if let Tab::Document(view) = tab {
            if ui.button("Split view").clicked() {
                self.workspace.active_view = Some(*view);
                self.actions.push(AppAction::Command(Command::SplitView));
            }
            if ui.button("Close").clicked() {
                self.actions.push(AppAction::CloseView(*view));
            }
        }
    }

    fn tab_style_override(
        &self,
        tab: &Tab,
        global_style: &egui_dock::TabStyle,
    ) -> Option<egui_dock::TabStyle> {
        matches!(tab, Tab::Document(_)).then(|| {
            let mut style = global_style.clone();
            style.tab_body.inner_margin = egui::Margin::ZERO;
            style
        })
    }

    fn scroll_bars(&self, _tab: &Tab) -> [bool; 2] {
        [false, false]
    }

    fn clear_background(&self, tab: &Tab) -> bool {
        !matches!(tab, Tab::Document(_))
    }
}

impl eframe::App for FastTickApp {
    fn ui(&mut self, ui: &mut Ui, _frame: &mut eframe::Frame) {
        self.show(ui);
    }

    fn save(&mut self, storage: &mut dyn eframe::Storage) {
        self.settings.save(storage);
        eframe::set_value(storage, MODE_KEY, &self.mode);
        for mode in Mode::ALL {
            eframe::set_value(
                storage,
                &dock_key(mode),
                &dock::without_documents(&self.docks[mode.index()]),
            );
        }
    }

    fn on_exit(&mut self) {
        self.live.shutdown();
    }

    fn clear_color(&self, visuals: &egui::Visuals) -> [f32; 4] {
        visuals.panel_fill.to_normalized_gamma_f32()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use fasttick_core::runtime::RuntimeTree;
    use fasttick_protocol::breakpoints::Breakpoint;

    #[test]
    fn endpoints() {
        assert_eq!(parse_endpoint("robot"), Some(("robot".into(), 1667)));
        assert_eq!(parse_endpoint(" robot:1700 "), Some(("robot".into(), 1700)));
        assert_eq!(parse_endpoint("[::1]:1800"), Some(("::1".into(), 1800)));
        assert_eq!(parse_endpoint("[::1]"), Some(("::1".into(), 1667)));
        assert_eq!(parse_endpoint("::1"), Some(("::1".into(), 1667)));
        assert_eq!(parse_endpoint(""), None);
        assert_eq!(parse_endpoint(":1700"), None);
        assert_eq!(parse_endpoint("robot:0"), None);
        assert_eq!(parse_endpoint("robot:x"), None);
        assert_eq!(parse_endpoint("[::1]x"), None);
    }

    #[test]
    fn pending_breakpoints_choose_the_nodes_found_by_path() {
        let tree = RuntimeTree::parse(
            r#"<root><BehaviorTree ID="T" _fullpath=""><Sequence name="s" _uid="1"><AlwaysSuccess name="a" _uid="2"/><AlwaysSuccess name="a" _uid="3"/></Sequence></BehaviorTree></root>"#,
        )
        .unwrap();
        let breakpoint = |path: &str, uid: Option<u16>| Breakpoint {
            node_path: path.into(),
            uid,
            position: Default::default(),
            mode: HookMode::Breakpoint,
            enabled: true,
            once: false,
            desired_status: NodeStatus::Skipped,
            label: None,
        };
        let mut set = BreakpointSet::new();
        set.breakpoints = vec![
            breakpoint("s", Some(9)),
            breakpoint("a", Some(3)),
            breakpoint("gone", Some(2)),
            breakpoint("gone", Some(77)),
        ];
        let pending = PendingBreakpoints::new("x.ftbp.json".into(), set, &tree);
        let resolutions: Vec<Resolution> = pending.resolved.iter().map(|r| r.resolution).collect();
        assert_eq!(
            resolutions,
            [
                Resolution::ByPath,
                Resolution::AmbiguousPath,
                Resolution::ByStoredUid,
                Resolution::Unresolved
            ]
        );
        assert_eq!(pending.chosen, [true, true, false, false]);
        let uids: Vec<u16> = pending.hooks().iter().map(|h| h.uid).collect();
        assert_eq!(uids, [1, 3]);
        for (resolved, stored) in pending.resolved.iter().zip([9, 3, 2, 77]) {
            let (text, color) = resolution_text(resolved, Some(stored));
            assert!(!text.is_empty());
            match resolved.resolution {
                Resolution::ByPath => assert_eq!(color, ResolutionColor::Good),
                Resolution::Unresolved => assert_eq!(color, ResolutionColor::Bad),
                _ => assert_eq!(color, ResolutionColor::Warning),
            }
        }
        assert!(
            resolution_text(&pending.resolved[2], Some(2))
                .0
                .contains("UID 2")
        );
    }
}
