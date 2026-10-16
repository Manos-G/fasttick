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
//!
//! The methods of [`FastTickApp`] are in these files: `files` (open, close,
//! save), `commands` (menu and shortcut commands), `background` (jobs,
//! live, replay), `handlers` (actions, clipboard, edits), `ui` (the window),
//! `popups` (add node, command search, breakpoint and SQLite dialogs) and
//! `remote` (files on robots).

use std::path::{Path, PathBuf};
use std::sync::Arc;

use egui::{Id, RichText, Ui, WidgetText};
use egui_dock::tab_viewer::OnCloseResponse;
use egui_dock::{DockArea, DockState, TabViewer};
use fasttick_core::project::{MissingFiles, MissingNames};
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
use crate::shortcuts::{ClipboardEvent, Command, KeyMap};
use crate::theme::{self, CanvasPalette, ThemeChoice};
use crate::widgets::{self, Icon};

mod background;
mod commands;
mod files;
mod handlers;
mod popups;
mod recovery;
mod remote;
mod ui;

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

/// A close that waits for the answer of the user about unsaved changes.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PendingClose {
    /// The last view of a document.
    View(ViewId),
    /// The window (the application).
    Window,
}

/// The answer about unsaved changes.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum UnsavedChoice {
    Save,
    Discard,
    Cancel,
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
    add_popup: Option<crate::add_popup::AddPopup>,
    command_search: Option<crate::command_search::CommandSearch>,
    remote: crate::remote_ui::RemoteState,
    recovery: Option<crate::recovery::Recovery>,
    recovery_offer: Vec<crate::recovery::Orphan>,
    recovery_failed: bool,
    notices: Vec<Notice>,
    actions: Vec<AppAction>,
    save_as_view: Option<ViewId>,
    svg_view: Option<ViewId>,
    project_view: Option<ViewId>,
    /// The clipboard of the tree editor (XML text of copied nodes).
    clipboard: Option<String>,
    window_title: String,
    pending_breakpoints: Option<PendingBreakpoints>,
    sqlite_picker: Option<SqlitePicker>,
    pending_close: Option<PendingClose>,
    close_confirmed: bool,
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
                .map(|mut d| {
                    // Layouts that older versions saved can point to a
                    // document tab that is gone.
                    dock::repair_active_tabs(&mut d);
                    d
                })
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
            add_popup: None,
            command_search: None,
            remote: crate::remote_ui::RemoteState::new(crate::remote::RemoteCache::default_root()),
            recovery: None,
            recovery_offer: Vec::new(),
            recovery_failed: false,
            notices: Vec::new(),
            actions: Vec::new(),
            save_as_view: None,
            svg_view: None,
            project_view: None,
            clipboard: None,
            window_title: String::new(),
            pending_breakpoints: None,
            sqlite_picker: None,
            pending_close: None,
            close_confirmed: false,
        };
        // The real application keeps copies of unsaved changes; tests set a
        // folder with `use_recovery_dir`.
        if (storage.is_some() || std::env::var_os(crate::recovery::ENV_DIR).is_some())
            && let Some(root) = crate::recovery::Recovery::default_root()
        {
            app.use_recovery_dir(&root);
        }
        crate::fonts::install(&cc.egui_ctx);
        cc.egui_ctx.options_mut(|o| o.zoom_with_keyboard = false);
        theme::install(&cc.egui_ctx, app.theme());
        for path in options.open {
            if let Err(message) = app.open_path_now(&cc.egui_ctx, &path) {
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

/// "1 tree", "2 trees", ...
fn counted(count: usize, one: &str, many: &str) -> String {
    format!("{count} {}", if count == 1 { one } else { many })
}

/// "5 trees", "1 tree and 2 node types", ...
fn missing_summary(missing: &MissingNames) -> String {
    let trees = (!missing.trees.is_empty()).then(|| counted(missing.trees.len(), "tree", "trees"));
    let types = (!missing.node_types.is_empty())
        .then(|| counted(missing.node_types.len(), "node type", "node types"));
    match (trees, types) {
        (Some(trees), Some(types)) => format!("{trees} and {types}"),
        (Some(text), None) | (None, Some(text)) => text,
        (None, None) => "nothing".to_owned(),
    }
}

/// "trees A, B and node types C".
fn missing_list(missing: &MissingNames) -> String {
    let join = |set: &std::collections::BTreeSet<String>| {
        set.iter()
            .map(String::as_str)
            .collect::<Vec<_>>()
            .join(", ")
    };
    let mut parts = Vec::new();
    if !missing.trees.is_empty() {
        let label = if missing.trees.len() == 1 {
            "tree"
        } else {
            "trees"
        };
        parts.push(format!("{label} {}", join(&missing.trees)));
    }
    if !missing.node_types.is_empty() {
        let label = if missing.node_types.len() == 1 {
            "node type"
        } else {
            "node types"
        };
        parts.push(format!("{label} {}", join(&missing.node_types)));
    }
    parts.join(" and ")
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
        let edit_context = doc
            .editor
            .as_deref()
            .filter(|_| doc.mode() == self.mode)
            .map(|editor| EditMenuContext::new(view_id, editor, self.has_clipboard));
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
                CanvasEvent::ToggleFold(source) => self.workspace.toggle_fold(view_id, source),
                CanvasEvent::UnfoldAll => self.workspace.unfold_all(view_id),
                CanvasEvent::SelectionChanged | CanvasEvent::Activated(_) => {
                    self.workspace.active_view = Some(view_id);
                }
                CanvasEvent::MoveNodes { nodes, spot } => {
                    self.workspace.active_view = Some(view_id);
                    if let Some(op) = self.move_op(view_id, &nodes, spot) {
                        self.actions.push(AppAction::Edit(view_id, op));
                    }
                }
                CanvasEvent::AddChild { spot, at } => {
                    self.workspace.active_view = Some(view_id);
                    if let DropSpot::Node { target, placement } = spot
                        && let Some(node) = self.xml_node(view_id, target)
                        && let Some(canvas_node) = self
                            .workspace
                            .view(view_id)
                            .and_then(|v| v.canvas.tree().get(target))
                    {
                        let label = canvas_node.label.clone();
                        // A new child of a folded node must be visible.
                        let unfold = (placement == DropPlacement::Inside
                            && canvas_node.folded.is_some())
                        .then_some(canvas_node.source);
                        let title = match placement {
                            DropPlacement::Inside => format!("Add a child of {label}"),
                            DropPlacement::Before => format!("Add a node before {label}"),
                            DropPlacement::After => format!("Add a node after {label}"),
                        };
                        if let Some(source) = unfold {
                            self.workspace.toggle_fold(view_id, source);
                        }
                        self.actions.push(AppAction::OpenAddPopup {
                            view: view_id,
                            target: Some((node, placement_of(placement))),
                            title,
                            pos: at,
                        });
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
        self.finish_recovery();
    }

    fn clear_color(&self, visuals: &egui::Visuals) -> [f32; 4] {
        visuals.panel_fill.to_normalized_gamma_f32()
    }
}

#[cfg(test)]
mod tests;
