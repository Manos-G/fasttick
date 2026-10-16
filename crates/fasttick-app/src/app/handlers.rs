//! The actions that panels and dialogs push, the clipboard, and edits.

use super::*;

impl FastTickApp {
    pub(super) fn handle_actions(&mut self, ctx: &egui::Context) {
        let actions = std::mem::take(&mut self.actions);
        for action in actions {
            match action {
                AppAction::Command(command) => self.run_command(ctx, command),
                AppAction::OpenPath(path) => self.start_open(ctx, path),
                AppAction::CloseView(view) => self.request_close_view(view),
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
                AppAction::AddMissingFiles(doc) => self.add_missing_files(ctx, doc),
                AppAction::ReloadRemote(doc) => self.reload_remote_doc(ctx, doc),
                AppAction::FindKey(key) => {
                    dock::show_panel(&mut self.docks[self.mode.index()], Tab::Search);
                    panels::search::find_key(ctx, &key);
                }
                AppAction::OpenAddPopup {
                    view,
                    target,
                    title,
                    pos,
                } => {
                    self.workspace.active_view = Some(view);
                    self.add_popup =
                        Some(crate::add_popup::AddPopup::new(view, target, title, pos));
                }
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
    pub(super) fn active_editor(&self) -> Option<&crate::editor::EditorDoc> {
        let view = self.active_edit_view()?;
        self.workspace.doc_of_view(view)?.editor.as_deref()
    }

    /// Runs a copy, cut or paste of the platform (Ctrl+C, Ctrl+X,
    /// Ctrl+V). A paste takes the text of the system clipboard if it is
    /// XML, so nodes copied as text in another program paste as nodes.
    pub fn clipboard_event(&mut self, ctx: &egui::Context, event: ClipboardEvent) {
        match event {
            ClipboardEvent::Copy => self.run_command(ctx, Command::Copy),
            ClipboardEvent::Cut => self.run_command(ctx, Command::Cut),
            ClipboardEvent::Paste(text) => {
                if self.active_edit_view().is_none() {
                    return;
                }
                let text = text.trim();
                if text.starts_with('<') {
                    self.clipboard = Some(text.to_owned());
                } else if self.clipboard.as_deref().map(str::trim) != Some(text) {
                    self.notify(
                        ctx,
                        NoticeLevel::Warning,
                        "The clipboard has no nodes. Copy nodes in FastTick, or copy the XML \
                         of nodes, for example <Sleep msec=\"100\"/>.",
                    );
                    return;
                }
                self.run_command(ctx, Command::Paste);
            }
        }
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
                    let tree = view_ref.canvas.tree();
                    let ids: Vec<_> = outcome
                        .select
                        .iter()
                        .filter_map(|n| tree.find_source(NodeSource::Xml(file, *n)))
                        .collect();
                    let tree = view_ref.canvas.tree_mut();
                    tree.clear_selection();
                    for id in &ids {
                        tree.set_selected(*id, true);
                    }
                    // The view moves only if the first node is not visible,
                    // so the cards stay where the user saw them.
                    if let Some(first) = ids.first() {
                        view_ref.canvas.reveal(*first);
                    }
                }
                // A new group or note: its text opens for editing.
                if let Some(target) = outcome.edit_text
                    && let Some(view_ref) = self.workspace.view_mut(view)
                {
                    view_ref.canvas.start_edit(target);
                    self.workspace.active_view = Some(view);
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
            DialogPurpose::AddModelFile => editor.add_model_file(&path).map(|_| {
                (
                    format!("Added the node models of {}.", file_name(&path)),
                    None,
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
}
