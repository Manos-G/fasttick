//! The commands of the menus, the shortcuts and the command search: which
//! can run now, and what they do.

use super::*;

impl FastTickApp {
    /// True if a command can run now.
    pub fn is_enabled(&self, command: Command) -> bool {
        let active = self.active_view_in_mode();
        match command {
            Command::NewProject => true,
            Command::Save => self.active_edit_view().is_some(),
            Command::Undo => self.active_editor().is_some_and(|e| e.can_undo()),
            Command::Redo => self.active_editor().is_some_and(|e| e.can_redo()),
            Command::MoveUp | Command::MoveDown => self
                .active_edit_view()
                .is_some_and(|v| crate::edit_ops::primary_node(&self.workspace, v).is_some()),
            Command::Cut | Command::Copy | Command::Duplicate | Command::Delete => self
                .active_edit_view()
                .is_some_and(|v| !crate::edit_ops::selected_nodes(&self.workspace, v).is_empty()),
            Command::Paste => self.active_edit_view().is_some() && self.clipboard.is_some(),
            Command::AddNode => self.active_edit_view().is_some() && self.add_popup.is_none(),
            Command::SearchCommands => self.command_search.is_none(),
            Command::ExportSvg => active.is_some() && !self.jobs.dialog_open(),
            Command::OpenRemote => true,
            Command::UploadRemote => {
                self.remote.uploading == 0 && !self.remote.index.pending().is_empty()
            }
            Command::ReloadRemote => self.active_remote_doc().is_some_and(|doc| {
                self.workspace.doc(doc).is_some_and(|d| {
                    d.editor.as_ref().is_some_and(|e| {
                        !e.is_dirty() && !e.all_paths().iter().any(|p| self.remote.is_pending(p))
                    })
                })
            }),
            Command::DebugRobot => self.active_remote_doc().is_some(),
            Command::GoToTree => active.is_some() && self.command_search.is_none(),
            Command::SaveAs => active.is_some_and(|v| {
                self.workspace
                    .doc_of_view(v)
                    .is_some_and(|d| matches!(d.kind, DocKind::TreeFile { .. }))
            }),
            Command::SelectAll
            | Command::FitToView
            | Command::FitSelection
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
            Command::ExportSvg => {
                self.svg_view = active;
                self.pick_file(ctx, DialogPurpose::ExportSvg);
            }
            Command::OpenRemote => {
                if self.remote.dialog.is_none() {
                    self.remote.dialog = Some(crate::remote_ui::OpenDialog::new(
                        self.settings.remote_hosts.first(),
                    ));
                }
            }
            Command::UploadRemote => {
                let pending = self.remote.index.pending();
                let items = self.remote.upload_items(&pending, false);
                self.start_uploads(ctx, items);
            }
            Command::ReloadRemote => {
                if let Some(doc) = self.active_remote_doc() {
                    self.reload_remote_doc(ctx, doc);
                }
            }
            Command::DebugRobot => {
                if let Some(url) = self.active_remote_url() {
                    // An alias of the ssh configuration is not a network
                    // name: ask ssh for the real name.
                    let config = self.remote.quiet_ssh_config(ctx);
                    let host = crate::remote::ssh::resolve_hostname(&config, &url.host)
                        .unwrap_or(url.host.host);
                    let port = self.settings.live_port;
                    self.connect(ctx, host, port);
                }
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
            Command::FitSelection => self.with_active_canvas(|c| c.fit_selection()),
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
                    self.request_close_view(view);
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
            Command::MoveUp | Command::MoveDown => {
                if let Some(view) = self.active_edit_view() {
                    let delta = if command == Command::MoveUp { -1 } else { 1 };
                    self.run_edit(ctx, view, EditOp::MoveSibling(delta));
                }
            }
            Command::AddNode => {
                self.command_search = None;
                self.open_add_for_selection(ctx);
            }
            Command::SearchCommands | Command::GoToTree => {
                let scope = if command == Command::GoToTree {
                    crate::command_search::Scope::Trees
                } else {
                    crate::command_search::Scope::All
                };
                self.add_popup = None;
                self.command_search = Some(crate::command_search::CommandSearch::new(scope));
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

    pub(super) fn with_replay(&mut self, f: impl FnOnce(&mut ReplayModel)) {
        if let Some(model) = self.active_replay_mut() {
            f(model);
        }
    }

    /// Opens a file dialog. The start folder is the folder of a recent
    /// file of the same kind.
    pub(super) fn pick_file(&mut self, ctx: &egui::Context, purpose: DialogPurpose) {
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

    /// Writes the tree of a view as an SVG image (on a worker thread), in
    /// the colours of the current theme. Returns false if the view is gone.
    pub fn export_svg_to(&mut self, ctx: &egui::Context, view: ViewId, path: PathBuf) -> bool {
        let palette = theme::canvas_palette(self.theme(), ctx.theme());
        let Some(view) = self.workspace.view_mut(view) else {
            self.notify(ctx, NoticeLevel::Warning, "The tree is no longer open.");
            return false;
        };
        let svg = view.canvas.to_svg(ctx, &palette);
        self.jobs.save_text(ctx, path, svg);
        true
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
    pub(super) fn reveal_node(&mut self, doc: DocId, uid: Uid) {
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

    pub(super) fn with_active_canvas(&mut self, f: impl FnOnce(&mut crate::canvas::CanvasState)) {
        if self.active_view_in_mode().is_some()
            && let Some(view) = self.workspace.active_mut()
        {
            f(&mut view.canvas);
        }
    }

    pub(super) fn set_direction(&mut self, direction: LayoutDirection) {
        self.settings.layout_direction = direction;
        if self.active_view_in_mode().is_some()
            && let Some(view) = self.workspace.active_mut()
        {
            view.canvas.set_direction(direction);
        }
    }

    pub(super) fn set_theme(&mut self, ctx: &egui::Context, choice: ThemeChoice) {
        self.settings.theme = choice;
        self.theme_override = None;
        theme::set_visuals(ctx, choice);
    }
}
