//! Open, close and save documents.

use super::*;

impl FastTickApp {
    /// Opens a file now, on the calling thread (command line, tests). The
    /// menu uses background jobs instead.
    pub fn open_path_now(&mut self, ctx: &egui::Context, path: &Path) -> Result<(), String> {
        // A remote file needs the network: it opens with a background job.
        if crate::remote::is_remote_url(path) {
            self.start_open(ctx, path.to_path_buf());
            return Ok(());
        }
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
                let missing = loaded.missing.clone();
                if let Some(view) = self.add_tree_document(path, loaded) {
                    self.offer_missing_files(ctx, view, path, &missing);
                }
            }
        }
        Ok(())
    }

    pub(super) fn start_open(&mut self, ctx: &egui::Context, path: PathBuf) {
        if crate::remote::is_remote_url(&path) {
            match crate::remote::RemoteUrl::parse(&path.to_string_lossy()) {
                Ok(url) => self.open_remote_url(ctx, url),
                Err(message) => self.notify(ctx, NoticeLevel::Error, message),
            }
            return;
        }
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

    pub(super) fn find_open_view(&self, path: &Path, session: Option<i64>) -> Option<ViewId> {
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

    pub(super) fn focus_view(&mut self, view: ViewId) {
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

    pub(super) fn add_tree_document(
        &mut self,
        path: &Path,
        loaded: crate::jobs::LoadedTree,
    ) -> Option<ViewId> {
        if let Some(view) = self.find_open_view(path, None) {
            self.focus_view(view);
            return Some(view);
        }
        let title = self.remote.title(path).unwrap_or_else(|| file_name(path));
        let main = loaded.main_instance();
        let mut editor = loaded.editor;
        if let Some(mirror) = self.remote.mirror_root(path) {
            editor.set_mirror_root(Some(mirror));
        }
        let doc = self
            .workspace
            .add_editor_doc(path.to_owned(), title, *editor);
        let view = self.workspace.open_view(doc, main, self.view_options())?;
        dock::add_document(&mut self.docks[Mode::Edit.index()], Tab::Document(view));
        self.add_recent(path);
        self.focus_view(view);
        Some(view)
    }

    /// Offers the XML files of the folder that have the missing trees and
    /// node models of a new document.
    pub(super) fn offer_missing_files(
        &mut self,
        ctx: &egui::Context,
        view: ViewId,
        path: &Path,
        found: &MissingFiles,
    ) {
        if found.is_empty() {
            return;
        }
        let Some(doc) = self.workspace.doc_of_view(view).map(|d| d.id) else {
            return;
        };
        let files = found.files().map(|p| file_name(p)).collect::<Vec<_>>();
        let text = format!(
            "{} uses {} that are not in the project. {} in the same folder {}: {}.",
            file_name(path),
            missing_summary(&found.missing),
            counted(files.len(), "XML file", "XML files"),
            if files.len() == 1 {
                "has them"
            } else {
                "have them"
            },
            files.join(", ")
        );
        self.notify_with_action(
            ctx,
            NoticeLevel::Warning,
            text,
            "Add the files",
            AppAction::AddMissingFiles(doc),
        );
    }

    /// Adds the XML files of the folder that have the missing trees and
    /// node models of a document.
    pub(super) fn add_missing_files(&mut self, ctx: &egui::Context, doc: DocId) {
        let Some(editor) = self.workspace.editor_mut(doc) else {
            return;
        };
        let result = editor.add_missing_files();
        self.workspace.refresh_editor_views();
        match result {
            Ok(found) if found.missing.is_empty() => self.notify(
                ctx,
                NoticeLevel::Info,
                "Nothing is missing: the project has all its trees and node models.",
            ),
            Ok(found) if found.is_empty() => self.notify(
                ctx,
                NoticeLevel::Warning,
                format!(
                    "No XML file in the folder has the missing {}.",
                    missing_list(&found.missing)
                ),
            ),
            Ok(found) => {
                let files = found.files().map(|p| file_name(p)).collect::<Vec<_>>();
                let mut text = format!(
                    "Added {}: {}.",
                    counted(files.len(), "file", "files"),
                    files.join(", ")
                );
                if !found.still_missing.is_empty() {
                    text.push_str(&format!(
                        " These are still missing: {}.",
                        missing_list(&found.still_missing)
                    ));
                }
                self.notify(ctx, NoticeLevel::Info, text);
            }
            Err(message) => self.notify(
                ctx,
                NoticeLevel::Error,
                format!("Cannot add the files: {message}"),
            ),
        }
    }

    pub(super) fn add_log_document(
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
        let name = self.remote.title(path).unwrap_or_else(|| file_name(path));
        let title = match &source {
            Some(source) => format!("{name} ({source})"),
            None => name,
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
        self.add_recent(path);
        self.focus_view(view);
        Some(view)
    }

    /// Closes a view. The last view of a document with changes that are
    /// not saved asks first.
    pub(super) fn request_close_view(&mut self, view: ViewId) {
        let doc = self.workspace.doc_of_view(view);
        let dirty = doc.is_some_and(|d| d.editor.as_ref().is_some_and(|e| e.is_dirty()));
        let last = doc.is_some_and(|d| self.workspace.views_of(d.id).count() <= 1);
        if dirty && last {
            self.pending_close = Some(PendingClose::View(view));
        } else {
            self.close_view(view);
        }
    }

    /// The documents with changes that are not saved: (title, a view).
    pub fn unsaved_documents(&self) -> Vec<(String, ViewId)> {
        self.workspace
            .docs()
            .iter()
            .filter(|d| d.editor.as_ref().is_some_and(|e| e.is_dirty()))
            .filter_map(|d| Some((d.title.clone(), self.workspace.views_of(d.id).next()?.id)))
            .collect()
    }

    /// The close that waits for an answer about unsaved changes.
    pub fn pending_close(&self) -> Option<PendingClose> {
        self.pending_close
    }

    /// Saves the document of a view without a dialog. Returns false (with
    /// a notice) if the document needs a path or the save failed.
    pub(super) fn save_now(&mut self, ctx: &egui::Context, view: ViewId) -> bool {
        let Some(doc) = self.workspace.doc_of_view(view).map(|d| d.id) else {
            return true;
        };
        let Some(editor) = self.workspace.editor_mut(doc) else {
            return true;
        };
        let paths = editor.unsaved_paths();
        match editor.save() {
            Ok(crate::editor::SaveOutcome::Saved) => {
                self.after_save(ctx, &paths);
                true
            }
            Ok(crate::editor::SaveOutcome::NeedsPath) => {
                self.focus_view(view);
                self.save_as_view = Some(view);
                self.jobs.pick_file(ctx, DialogPurpose::SaveAs, None);
                false
            }
            Err(message) => {
                self.notify(ctx, NoticeLevel::Error, format!("Cannot save: {message}"));
                false
            }
        }
    }

    /// Opens the files that the user drops on the window (trees, projects
    /// and logs, by extension), and shows a hint while files are over it.
    pub(super) fn handle_dropped_files(&mut self, ctx: &egui::Context) {
        let (hovered, dropped) = ctx.input(|i| {
            (
                !i.raw.hovered_files.is_empty(),
                i.raw
                    .dropped_files
                    .iter()
                    .map(|f| f.path().to_path_buf())
                    .filter(|p| p.is_absolute())
                    .collect::<Vec<_>>(),
            )
        });
        for path in dropped {
            self.start_open(ctx, path);
        }
        if hovered {
            let painter = ctx.layer_painter(egui::LayerId::new(
                egui::Order::Foreground,
                Id::new("fasttick_file_drop"),
            ));
            let rect = ctx.content_rect();
            painter.rect_filled(rect, 0.0, egui::Color32::from_black_alpha(140));
            painter.text(
                rect.center(),
                egui::Align2::CENTER_CENTER,
                "Drop the files to open them",
                egui::FontId::proportional(22.0),
                egui::Color32::WHITE,
            );
        }
    }

    /// The question about unsaved changes before a close.
    pub(super) fn unsaved_dialog(&mut self, ctx: &egui::Context) {
        let Some(pending) = self.pending_close else {
            return;
        };
        let unsaved = self.unsaved_documents();
        let (text, save_label) = match pending {
            PendingClose::View(view) => {
                let title = self
                    .workspace
                    .doc_of_view(view)
                    .map(|d| d.title.clone())
                    .unwrap_or_default();
                (
                    format!(
                        "{title} has changes that are not saved. Save them before you close it?"
                    ),
                    "Save",
                )
            }
            PendingClose::Window => (
                format!(
                    "These files have changes that are not saved: {}. Save them before you close FastTick?",
                    unsaved
                        .iter()
                        .map(|(t, _)| t.as_str())
                        .collect::<Vec<_>>()
                        .join(", ")
                ),
                "Save all",
            ),
        };
        let mut choice = None;
        egui::Window::new("Changes are not saved")
            .collapsible(false)
            .resizable(false)
            .anchor(egui::Align2::CENTER_CENTER, [0.0, -60.0])
            .show(ctx, |ui| {
                ui.set_max_width(420.0);
                ui.label(text);
                ui.add_space(10.0);
                ui.horizontal(|ui| {
                    if ui.button(RichText::new(save_label).strong()).clicked() {
                        choice = Some(UnsavedChoice::Save);
                    }
                    if ui.button("Do not save").clicked() {
                        choice = Some(UnsavedChoice::Discard);
                    }
                    if ui.button("Cancel").clicked() {
                        choice = Some(UnsavedChoice::Cancel);
                    }
                });
            });
        if ctx.input(|i| i.key_pressed(egui::Key::Escape)) {
            choice = Some(UnsavedChoice::Cancel);
        }
        let Some(choice) = choice else {
            return;
        };
        self.pending_close = None;
        match (pending, choice) {
            (_, UnsavedChoice::Cancel) => {}
            (PendingClose::View(view), UnsavedChoice::Save) => {
                if self.save_now(ctx, view) {
                    self.close_view(view);
                }
            }
            (PendingClose::View(view), UnsavedChoice::Discard) => self.close_view(view),
            (PendingClose::Window, UnsavedChoice::Save) => {
                if unsaved.iter().all(|(_, view)| self.save_now(ctx, *view)) {
                    self.close_confirmed = true;
                    ctx.send_viewport_cmd(egui::ViewportCommand::Close);
                }
            }
            (PendingClose::Window, UnsavedChoice::Discard) => {
                self.close_confirmed = true;
                ctx.send_viewport_cmd(egui::ViewportCommand::Close);
            }
        }
    }

    pub(super) fn close_view(&mut self, view: ViewId) {
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

    pub(super) fn connect(&mut self, ctx: &egui::Context, host: String, port: u16) {
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

    /// Saves the document of a view. A new file asks for a path.
    pub(super) fn save_view(&mut self, ctx: &egui::Context, view: ViewId) {
        let Some(doc) = self.workspace.doc_of_view(view).map(|d| d.id) else {
            return;
        };
        let Some(editor) = self.workspace.editor_mut(doc) else {
            return;
        };
        let paths = editor.unsaved_paths();
        match editor.save() {
            Ok(crate::editor::SaveOutcome::Saved) => {
                if let Some(path) = editor.path() {
                    self.add_recent(&path);
                }
                self.after_save(ctx, &paths);
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
    pub(super) fn new_document(&mut self) {
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
}
