//! Remote files (see [`crate::remote_ui`]).

use super::*;

impl FastTickApp {
    /// Adds a file to the recent files. A local copy of a remote file goes
    /// in as its `ssh://` address.
    pub(super) fn add_recent(&mut self, path: &Path) {
        match self.remote.url_of(path) {
            Some(url) => self.settings.add_recent(Path::new(&url.to_string())),
            None => self.settings.add_recent(path),
        }
    }

    /// The remote state (for tests).
    pub fn remote(&self) -> &crate::remote_ui::RemoteState {
        &self.remote
    }

    /// The remote state, to change it (for tests and tools).
    pub fn remote_mut(&mut self) -> &mut crate::remote_ui::RemoteState {
        &mut self.remote
    }

    /// The settings, to change them (for tests).
    pub fn settings_mut(&mut self) -> &mut Settings {
        &mut self.settings
    }

    /// The edit document of the active view, if it is a local copy of a
    /// remote file.
    pub(super) fn active_remote_doc(&self) -> Option<DocId> {
        let doc = self.workspace.doc_of_view(self.active_edit_view()?)?;
        let DocKind::TreeFile { path } = &doc.kind else {
            return None;
        };
        self.remote.url_of(path).map(|_| doc.id)
    }

    /// Opens a remote file: copies it (and the files that it needs) to the
    /// cache on a worker thread, then opens the copy. A file that is open
    /// already gets the focus.
    pub fn open_remote_url(&mut self, ctx: &egui::Context, url: crate::remote::RemoteUrl) {
        let local = self.remote.cache.local_path(&url);
        if let Some(view) = self.find_open_view(&local, None) {
            self.remote.dialog = None;
            self.focus_view(view);
            return;
        }
        if let Some(dialog) = &mut self.remote.dialog {
            dialog.busy = true;
            dialog.error = None;
        }
        let config = self.remote.ssh_config(ctx);
        let cache = self.remote.cache.clone();
        let pending = self.remote.pending();
        self.jobs.run_remote(ctx, "remote-open", move || {
            crate::remote_ui::open_job(config, cache, pending, url)
        });
    }

    /// After a save: uploads the saved files that are local copies of
    /// remote files.
    pub(super) fn after_save(&mut self, ctx: &egui::Context, saved: &[PathBuf]) {
        let items = self.remote.upload_items(saved, false);
        if items.is_empty() {
            return;
        }
        self.remote.mark_pending(&items);
        self.start_uploads(ctx, items);
    }

    /// Uploads local copies: one job for each computer.
    pub(super) fn start_uploads(
        &mut self,
        ctx: &egui::Context,
        items: Vec<crate::remote_ui::UploadItem>,
    ) {
        let mut by_host: Vec<(crate::remote::RemoteHost, Vec<crate::remote_ui::UploadItem>)> =
            Vec::new();
        for item in items {
            match by_host.iter_mut().find(|(host, _)| *host == item.url.host) {
                Some((_, list)) => list.push(item),
                None => by_host.push((item.url.host.clone(), vec![item])),
            }
        }
        for (host, items) in by_host {
            let restart = self
                .settings
                .remote_host(&host)
                .map(|h| h.restart_command.clone())
                .filter(|c| !c.trim().is_empty());
            let config = self.remote.ssh_config(ctx);
            self.remote.uploading += 1;
            self.jobs.run_remote(ctx, "remote-upload", move || {
                crate::remote_ui::upload_job(config, host, items, restart)
            });
        }
    }

    /// The edit document whose project has a file.
    pub(super) fn doc_with_file(&self, local: &Path) -> Option<DocId> {
        self.workspace.find_doc(|d| {
            d.editor
                .as_ref()
                .is_some_and(|e| e.all_paths().iter().any(|p| p == local))
        })
    }

    /// Copies the files of a remote document from the robot again and
    /// opens them. Changes that are not uploaded are lost.
    pub(super) fn reload_remote_doc(&mut self, ctx: &egui::Context, doc: DocId) {
        let Some(document) = self.workspace.doc(doc) else {
            return;
        };
        let DocKind::TreeFile { path } = &document.kind else {
            return;
        };
        let Some(url) = self.remote.url_of(path) else {
            return;
        };
        let files = document
            .editor
            .as_ref()
            .map(|e| e.all_paths())
            .unwrap_or_default();
        for file in &files {
            if let Some(entry) = self.remote.index.files.get_mut(file) {
                entry.pending = false;
            }
        }
        self.remote.save_index();
        let views: Vec<ViewId> = self.workspace.views_of(doc).map(|v| v.id).collect();
        for view in views {
            self.close_view(view);
        }
        self.open_remote_url(ctx, url);
    }

    /// Applies the result of a remote job.
    pub(super) fn remote_result(
        &mut self,
        ctx: &egui::Context,
        result: crate::remote_ui::RemoteResult,
    ) {
        use crate::remote_ui::{RemoteResult, UploadOutcome};
        match result {
            RemoteResult::Listed {
                host,
                result,
                connect_error,
            } => {
                if connect_error {
                    self.remote.forget_passwords();
                }
                if let Some(dialog) = &mut self.remote.dialog {
                    dialog.busy = false;
                    match result {
                        Ok(listing) => {
                            dialog.folder = listing.dir.clone();
                            dialog.listing = Some((host, listing));
                            dialog.selected = None;
                            dialog.error = None;
                        }
                        Err(message) => dialog.error = Some(message),
                    }
                }
            }
            RemoteResult::Opened {
                url,
                result,
                connect_error,
            } => {
                if connect_error {
                    self.remote.forget_passwords();
                }
                let opened = match result {
                    Ok(opened) => opened,
                    Err(message) => {
                        match &mut self.remote.dialog {
                            Some(dialog) => {
                                dialog.busy = false;
                                dialog.error = Some(message);
                            }
                            None => self.notify(
                                ctx,
                                NoticeLevel::Error,
                                format!("Cannot open {url}: {message}"),
                            ),
                        }
                        return;
                    }
                };
                for (local, file) in opened.files {
                    self.remote.index.files.insert(local, file);
                }
                self.remote.save_index();
                let folder = crate::remote::posix_parent(&url.path);
                let host_settings = match self.remote.dialog.take() {
                    Some(dialog) => dialog.host_settings(&folder),
                    None => {
                        let mut settings = self
                            .settings
                            .remote_host(&url.host)
                            .cloned()
                            .unwrap_or_default();
                        settings.address = url.host.to_string();
                        settings.folder = folder;
                        settings
                    }
                };
                self.settings.add_remote_host(host_settings);
                self.settings.live_host = url.host.host.clone();
                if !opened.kept.is_empty() {
                    let names: Vec<String> = opened.kept.iter().map(|p| file_name(p)).collect();
                    self.notify_with_action(
                        ctx,
                        NoticeLevel::Warning,
                        format!(
                            "{} changes that are not on {} yet: {}. FastTick opened your version.",
                            counted(names.len(), "file has", "files have"),
                            url.host.label(),
                            names.join(", ")
                        ),
                        "Upload to robot",
                        AppAction::Command(Command::UploadRemote),
                    );
                }
                if !opened.skipped.is_empty() {
                    self.notify(
                        ctx,
                        NoticeLevel::Warning,
                        format!(
                            "FastTick did not copy these files, because they are too large: {}.",
                            opened.skipped.join(", ")
                        ),
                    );
                }
                self.start_open(ctx, opened.local);
            }
            RemoteResult::Uploaded {
                host,
                outcomes,
                restart,
            } => {
                self.remote.uploading = self.remote.uploading.saturating_sub(1);
                let mut uploaded: Vec<String> = Vec::new();
                let mut failed: Option<String> = None;
                let mut conflicts = 0;
                for (local, outcome) in outcomes {
                    match outcome {
                        UploadOutcome::Done(base) => {
                            if let Some(entry) = self.remote.index.files.get_mut(&local) {
                                entry.base = base;
                                entry.pending = false;
                            }
                            uploaded.push(file_name(&local));
                        }
                        UploadOutcome::Conflict => {
                            conflicts += 1;
                            if !self.remote.conflicts.contains(&local) {
                                self.remote.conflicts.push_back(local);
                            }
                        }
                        UploadOutcome::Failed(message) => {
                            failed.get_or_insert(message);
                        }
                    }
                }
                self.remote.save_index();
                if let Some(message) = failed {
                    self.remote.forget_passwords();
                    self.notify_with_action(
                        ctx,
                        NoticeLevel::Warning,
                        format!(
                            "Not uploaded to {}: {message} Your changes are saved on this \
                             computer.",
                            host.label()
                        ),
                        "Try again",
                        AppAction::Command(Command::UploadRemote),
                    );
                } else if conflicts == 0 && !uploaded.is_empty() {
                    self.notify(
                        ctx,
                        NoticeLevel::Info,
                        format!("Saved {} to {}.", uploaded.join(", "), host.label()),
                    );
                }
                match restart {
                    Some(Ok(output)) => {
                        let mut text =
                            format!("Ran the command after the save on {}.", host.label());
                        if !output.is_empty() {
                            let short: String = output.chars().take(300).collect();
                            text.push_str(&format!(" Output: {short}"));
                        }
                        self.notify(ctx, NoticeLevel::Info, text);
                    }
                    Some(Err(message)) => self.notify(
                        ctx,
                        NoticeLevel::Warning,
                        format!(
                            "The command after the save failed on {}: {message}",
                            host.label()
                        ),
                    ),
                    None => {}
                }
            }
        }
    }

    /// The dialogs of remote files: the questions of ssh, "Open from
    /// robot" and the question about a file that changed on the robot.
    pub(super) fn remote_dialogs(&mut self, ctx: &egui::Context) {
        if let Some(dialog) = &mut self.remote.dialog {
            use crate::remote_ui::OpenDialogAction;
            let recent = self.settings.remote_hosts.clone();
            match crate::remote_ui::open_dialog(ctx, dialog, &recent) {
                OpenDialogAction::None => {}
                OpenDialogAction::Close => self.remote.dialog = None,
                OpenDialogAction::List(host, dir) => {
                    dialog.busy = true;
                    dialog.error = None;
                    let config = self.remote.ssh_config(ctx);
                    self.jobs.run_remote(ctx, "remote-list", move || {
                        crate::remote_ui::list_job(config, host, dir)
                    });
                }
                OpenDialogAction::Open(url) => self.open_remote_url(ctx, url),
            }
        }
        self.remote_conflict_dialog(ctx);
        // Last: the questions of ssh are on top of the other dialogs.
        self.remote.show_prompt(ctx);
    }

    /// The question about a local copy whose remote file changed after
    /// FastTick copied it.
    pub(super) fn remote_conflict_dialog(&mut self, ctx: &egui::Context) {
        let Some(local) = self.remote.conflicts.front().cloned() else {
            return;
        };
        let Some(url) = self.remote.url_of(&local) else {
            self.remote.conflicts.pop_front();
            return;
        };
        let mut choice = None;
        egui::Window::new("Changed on the robot")
            .id(Id::new("fasttick_remote_conflict"))
            .collapsible(false)
            .resizable(false)
            .default_width(460.0)
            .anchor(egui::Align2::CENTER_CENTER, egui::Vec2::ZERO)
            .show(ctx, |ui| {
                ui.label(format!(
                    "{} changed on {} after FastTick copied it. FastTick did not upload \
                     your version.",
                    url.file_name(),
                    url.host.label()
                ));
                ui.label(RichText::new(url.to_string()).small().monospace().weak());
                ui.add_space(6.0);
                ui.horizontal(|ui| {
                    if ui
                        .button("Overwrite the robot's file")
                        .on_hover_text("Upload your version. The change on the robot is lost.")
                        .clicked()
                    {
                        choice = Some(0);
                    }
                    if ui
                        .button("Reload from robot")
                        .on_hover_text("Open the robot's version. Your changes are lost.")
                        .clicked()
                    {
                        choice = Some(1);
                    }
                    if ui
                        .button("Not now")
                        .on_hover_text(
                            "Keep your changes on this computer. Use File > Upload to robot later.",
                        )
                        .clicked()
                    {
                        choice = Some(2);
                    }
                });
            });
        match choice {
            Some(0) => {
                self.remote.conflicts.pop_front();
                let items = self.remote.upload_items(&[local], true);
                self.start_uploads(ctx, items);
            }
            Some(1) => {
                self.remote.conflicts.pop_front();
                match self.doc_with_file(&local) {
                    Some(doc) => self.reload_remote_doc(ctx, doc),
                    None => {
                        if let Some(entry) = self.remote.index.files.get_mut(&local) {
                            entry.pending = false;
                        }
                        self.remote.save_index();
                        self.open_remote_url(ctx, url);
                    }
                }
            }
            Some(_) => {
                self.remote.conflicts.pop_front();
            }
            None => {}
        }
    }
}
