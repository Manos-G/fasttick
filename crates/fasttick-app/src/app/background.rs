//! The results of background work: jobs, the live connection and replay
//! playback.

use super::*;

impl FastTickApp {
    pub(super) fn handle_jobs(&mut self, ctx: &egui::Context) {
        for result in self.jobs.poll() {
            match result {
                JobResult::Picked { purpose, paths } => {
                    if matches!(
                        purpose,
                        DialogPurpose::AddTreeFile | DialogPurpose::AddModelFile
                    ) {
                        if let Some(view) = self.project_view.take() {
                            for path in paths {
                                self.project_file_picked(ctx, view, purpose, path);
                            }
                        }
                        continue;
                    }
                    let Some(path) = paths.into_iter().next() else {
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
                        | DialogPurpose::AddModelFile
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
                        DialogPurpose::ExportSvg => {
                            if let Some(view) = self.svg_view.take() {
                                self.export_svg_to(ctx, view, path);
                            }
                        }
                    }
                }
                JobResult::Tree { path, result } => match result {
                    Ok(loaded) => {
                        let missing = loaded.missing.clone();
                        if let Some(view) = self.add_tree_document(&path, loaded) {
                            self.offer_missing_files(ctx, view, &path, &missing);
                        }
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
                JobResult::Remote(result) => self.remote_result(ctx, *result),
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

    pub(super) fn handle_live(&mut self, ctx: &egui::Context) {
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

    pub(super) fn show_live_tree(&mut self, tree: Arc<fasttick_core::runtime::RuntimeTree>) {
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
    pub(super) fn on_breakpoint(&mut self, ctx: &egui::Context, uid: Uid) {
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
    pub(super) fn handle_replay(&mut self, ctx: &egui::Context) {
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
}
