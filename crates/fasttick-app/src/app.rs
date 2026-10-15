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

use crate::actions::AppAction;
use crate::canvas::{CanvasEvent, CanvasNode, LayoutDirection, TreeCanvas};
use crate::dialogs::{self, ConnectChoice, ConnectForm, Notice, NoticeLevel};
use crate::dock::{self, Tab};
use crate::documents::{DocId, DocKind, ViewId, ViewOptions, Workspace};
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
    pub fn new(path: PathBuf, set: BreakpointSet, tree: &fasttick_core::runtime::RuntimeTree) -> Self {
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
    notices: Vec<Notice>,
    actions: Vec<AppAction>,
    save_as_view: Option<ViewId>,
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
            notices: Vec::new(),
            actions: Vec::new(),
            save_as_view: None,
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
            RecentKind::Project => return Err(project_message(path)),
            RecentKind::Tree => {
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
            RecentKind::Project => {
                let message = project_message(&path);
                self.notify(ctx, NoticeLevel::Info, message);
            }
            RecentKind::Tree => self.jobs.load_tree(ctx, path),
        }
    }

    fn find_open_view(&self, path: &Path, session: Option<i64>) -> Option<ViewId> {
        let doc = self.workspace.find_doc(|d| match &d.kind {
            DocKind::Replay { path: p, session: s } => p == path && *s == session,
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
        let doc = self.workspace.add_doc(
            DocKind::TreeFile {
                path: path.to_owned(),
            },
            title,
            loaded.tree,
            None,
        );
        if let Some(document) = self.workspace.doc_mut(doc) {
            document.main_instance = main;
        }
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
            Command::NewProject
            | Command::Save
            | Command::Undo
            | Command::Redo
            | Command::Cut
            | Command::Copy
            | Command::Paste
            | Command::Duplicate
            | Command::Delete => false,
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
            Command::Find => dock::show_panel(&mut self.docks[self.mode.index()], Tab::Search),
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
            Command::NewProject
            | Command::Save
            | Command::Undo
            | Command::Redo
            | Command::Cut
            | Command::Copy
            | Command::Paste
            | Command::Duplicate
            | Command::Delete => {}
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
                            let text = self
                                .save_as_view
                                .take()
                                .and_then(|v| self.workspace.doc_of_view(v))
                                .map(|d| d.runtime.xml.clone());
                            if let Some(text) = text {
                                self.jobs.save_text(ctx, path, text);
                            }
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
                JobResult::Log { path, result } => match result {
                    Ok(loaded) => {
                        self.add_log_document(&path, *loaded);
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
            }
        }
    }

    fn handle_live(&mut self, ctx: &egui::Context) {
        for event in self.live.poll() {
            match event {
                SessionEvent::Tree { tree, .. } => {
                    let endpoint = self.live.endpoint_text().unwrap_or_default();
                    match self.live_doc.filter(|d| self.workspace.doc(*d).is_some()) {
                        Some(doc) => {
                            self.workspace.replace_runtime(doc, tree);
                            if let Some(document) = self.workspace.doc_mut(doc) {
                                document.kind = DocKind::Live { endpoint };
                            }
                        }
                        None => {
                            let doc = self.workspace.add_doc(
                                DocKind::Live { endpoint },
                                "Live",
                                tree,
                                None,
                            );
                            self.live_doc = Some(doc);
                            if let Some(view) =
                                self.workspace
                                    .open_view(doc, InstanceIndex(0), self.view_options())
                            {
                                dock::add_document(
                                    &mut self.docks[Mode::Live.index()],
                                    Tab::Document(view),
                                );
                                if self.mode == Mode::Live {
                                    self.workspace.active_view = Some(view);
                                }
                            }
                        }
                    }
                }
                SessionEvent::Status { snapshot } => {
                    if let Some(doc) = self.live_doc {
                        self.workspace.for_each_tree(doc, |tree| {
                            for entry in &snapshot.entries {
                                tree.set_status(entry.uid, entry.current, entry.previous);
                            }
                        });
                    }
                }
                SessionEvent::BreakpointReached { uid } => {
                    if self.settings.auto_focus_on_breakpoint
                        && let Some(doc) = self.live_doc
                    {
                        let views: Vec<ViewId> =
                            self.workspace.views_of(doc).map(|v| v.id).collect();
                        for view in views {
                            if let Some(v) = self.workspace.view_mut(view)
                                && let Some(id) = v.canvas.tree().find_uid(uid)
                            {
                                v.canvas.focus_on(id, true);
                            }
                        }
                    }
                }
                SessionEvent::ExecutorRestarted { .. } => {
                    self.notify(
                        ctx,
                        NoticeLevel::Info,
                        "The executor restarted. FastTick reads its tree again.",
                    );
                }
                SessionEvent::Error(message) => {
                    self.notify(ctx, NoticeLevel::Error, message);
                }
                _ => {}
            }
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
                AppAction::Connect { host, port } => self.connect(ctx, host, port),
                AppAction::SetReplayPosition(doc, position) => {
                    self.workspace.set_replay_position(doc, position);
                }
                AppAction::ClearRecent => self.settings.recent.clear(),
            }
        }
    }

    // ----- UI -----

    /// Draws one frame. `eframe::App::ui` calls this; the UI tests call it
    /// through the same trait.
    pub fn show(&mut self, ui: &mut Ui) {
        let ctx = ui.ctx().clone();
        self.handle_jobs(&ctx);
        self.handle_live(&ctx);

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
            .show(ui, |ui| self.dock_area(ui, &palette));

        self.dialogs(&ctx, &palette);
        dialogs::notices(&ctx, &mut self.notices);
        self.handle_actions(&ctx);
        self.update_title(&ctx);
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
        let left = egui::Rect::from_min_max(full.min, egui::pos2(full.min.x + 300.0, full.max.y));
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
            ui.checkbox(
                &mut self.settings.auto_focus_on_breakpoint,
                "Show the node at a breakpoint",
            );
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
                        DocKind::TreeFile { path } | DocKind::Replay { path } => {
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
            if self.jobs.running() > 0 {
                ui.separator();
                ui.spinner();
                ui.label(small("Loading".to_owned()));
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
            live: &self.live,
            live_doc: self.live_doc,
            keymap: &self.keymap,
            palette,
            actions: &mut self.actions,
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

/// A submenu entry. egui's default arrow glyph is not in the bundled font,
/// so the entry uses "›" (U+203A), which Roboto has.
fn submenu(ui: &mut Ui, text: &str, content: impl FnOnce(&mut Ui)) {
    egui::containers::menu::SubMenuButton::from_button(
        egui::Button::new(text).right_text("\u{203A}"),
    )
    .ui(ui, content);
}

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

fn project_message(path: &Path) -> String {
    format!(
        "{}: project files come with the tree editor. Open a tree XML file.",
        path.display()
    )
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
    live: &'a LiveController,
    live_doc: Option<DocId>,
    keymap: &'a KeyMap,
    palette: &'a CanvasPalette,
    actions: &'a mut Vec<AppAction>,
}

impl Viewer<'_> {
    fn document_ui(&mut self, ui: &mut Ui, view_id: ViewId) {
        let Some(doc) = self.workspace.doc_of_view(view_id) else {
            ui.label("This view is closed.");
            return;
        };
        let doc_id = doc.id;
        let is_live = self.live_doc == Some(doc_id);
        let replay = doc.replay.as_ref().map(|r| {
            (
                r.position,
                r.index.len(),
                r.index.time_of_index(r.position.saturating_sub(1)),
                r.index.last_time(),
            )
        });

        if is_live {
            self.live_strip(ui, view_id);
        }
        if let Some((position, len, time, last)) = replay {
            egui::Panel::bottom(Id::new(("replay_strip", view_id)))
                .resizable(false)
                .frame(
                    egui::Frame::NONE
                        .inner_margin(egui::Margin::symmetric(8, 4))
                        .fill(ui.visuals().panel_fill),
                )
                .show(ui, |ui| {
                    replay_strip(ui, doc_id, position, len, time, last, self.actions)
                });
        }

        let Some(view) = self.workspace.view_mut(view_id) else {
            return;
        };
        let output = TreeCanvas::new(&mut view.canvas, self.palette)
            .id_salt(("canvas", view_id.0))
            .show(ui);
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
            }
        }
    }

    fn live_strip(&mut self, ui: &mut Ui, view_id: ViewId) {
        egui::Panel::top(Id::new(("live_strip", view_id)))
            .resizable(false)
            .frame(
                egui::Frame::NONE
                    .inner_margin(egui::Margin::symmetric(8, 4))
                    .fill(ui.visuals().panel_fill),
            )
            .show(ui, |ui| {
                ui.horizontal(|ui| {
                    let color = match self.live.state() {
                        ConnectionState::Connected { .. } => self.palette.success,
                        ConnectionState::Disconnected => self.palette.skipped,
                        _ => self.palette.running,
                    };
                    widgets::status_dot(ui, color);
                    ui.label(self.live.state_text());
                    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                        if self.live.is_active() {
                            if ui.button("Disconnect").clicked() {
                                self.actions.push(AppAction::Command(Command::Disconnect));
                            }
                        } else if ui.button("Connect\u{2026}").clicked() {
                            self.actions.push(AppAction::Command(Command::Connect));
                        }
                    });
                });
            });
    }
}

fn format_time(us: u64) -> String {
    format!("{:.3} s", us as f64 / 1e6)
}

fn replay_strip(
    ui: &mut Ui,
    doc: DocId,
    position: usize,
    len: usize,
    time: Option<u64>,
    last: Option<u64>,
    actions: &mut Vec<AppAction>,
) {
    ui.horizontal(|ui| {
        let mut target = None;
        if widgets::icon_button(ui, Icon::First, "Go to the start", position > 0).clicked() {
            target = Some(0);
        }
        if widgets::icon_button(ui, Icon::Previous, "One transition back", position > 0).clicked() {
            target = Some(position.saturating_sub(1));
        }
        if widgets::icon_button(ui, Icon::Next, "One transition forward", position < len).clicked()
        {
            target = Some(position + 1);
        }
        if widgets::icon_button(ui, Icon::Last, "Go to the end", position < len).clicked() {
            target = Some(len);
        }
        let mut value = position;
        ui.spacing_mut().slider_width = (ui.available_width() - 260.0).max(80.0);
        if ui
            .add(egui::Slider::new(&mut value, 0..=len).show_value(false))
            .on_hover_text("Replay position (number of transitions)")
            .changed()
        {
            target = Some(value);
        }
        let time_text = match (time, position) {
            (_, 0) => format!("0.000 s / {}", format_time(last.unwrap_or(0))),
            (Some(t), _) => format!("{} / {}", format_time(t), format_time(last.unwrap_or(t))),
            (None, _) => String::new(),
        };
        ui.label(RichText::new(format!("{position} / {len}")).monospace());
        ui.label(RichText::new(time_text).monospace().weak());
        if let Some(target) = target {
            actions.push(AppAction::SetReplayPosition(doc, target));
        }
    });
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
