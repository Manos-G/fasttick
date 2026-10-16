//! Popups and dialogs: add node, command search, breakpoint files, SQLite
//! sessions.

use super::*;

impl FastTickApp {
    /// The search popup that adds a node.
    pub(super) fn add_popup_dialog(&mut self, ctx: &egui::Context, palette: &CanvasPalette) {
        let Some(popup) = &mut self.add_popup else {
            return;
        };
        let view = popup.view;
        let candidates = crate::edit_ops::view_file(&self.workspace, view)
            .zip(
                self.workspace
                    .doc_of_view(view)
                    .and_then(|d| d.editor.as_deref()),
            )
            .map(|(file, editor)| {
                let project = editor.project();
                let models = project.registry().all_models(file);
                crate::add_popup::AddCandidate::from_models(models.values().flat_map(|models| {
                    models.iter().map(|model| {
                        let source = project
                            .lookup_model(file, &model.id)
                            .map_or(fasttick_core::models::ModelSource::Unattached, |(_, s)| s);
                        (*model, source)
                    })
                }))
            });
        let Some(mut candidates) = candidates else {
            // The view is gone.
            self.add_popup = None;
            return;
        };
        // A tree cannot contain itself.
        if let Some(title) = self
            .workspace
            .view(view)
            .map(|v| v.canvas.tree().title.clone())
        {
            candidates
                .retain(|c| !(c.kind == fasttick_core::models::NodeKind::SubTree && c.id == title));
        }
        match crate::add_popup::show(
            ctx,
            popup,
            &candidates,
            &self.settings.recent_types,
            palette,
        ) {
            crate::add_popup::AddChoice::Open => {}
            crate::add_popup::AddChoice::Cancel => self.add_popup = None,
            crate::add_popup::AddChoice::Add(type_id) => {
                self.settings.add_recent_type(&type_id);
                if let Some(popup) = self.add_popup.take() {
                    let op = popup.op(type_id);
                    self.run_edit(ctx, view, op);
                }
            }
        }
    }

    /// Opens the search popup for the primary selected node: a new child
    /// of a node that takes children, else a new sibling after it. Without
    /// a selection the new node is unconnected.
    pub(super) fn open_add_for_selection(&mut self, ctx: &egui::Context) {
        let Some(view_id) = self.active_edit_view() else {
            return;
        };
        let Some(view) = self.workspace.view(view_id) else {
            return;
        };
        let tree = view.canvas.tree();
        let primary = view
            .canvas
            .primary()
            .filter(|id| tree.is_selected(*id))
            .or_else(|| tree.selected().next())
            .and_then(|id| tree.get(id))
            .filter(|n| !n.inline);
        let target = primary.and_then(|node| {
            let NodeSource::Xml(_, xml) = node.source else {
                return None;
            };
            if crate::canvas::connect::has_dot(node) {
                Some((
                    xml,
                    Placement::Inside,
                    format!("Add a child of {}", node.label),
                ))
            } else if node.parent.is_some() {
                Some((
                    xml,
                    Placement::After,
                    format!("Add a node after {}", node.label),
                ))
            } else {
                None
            }
        });
        let pos = ctx
            .input(|i| i.pointer.latest_pos())
            .unwrap_or_else(|| ctx.content_rect().center() - egui::vec2(170.0, 120.0));
        let popup = match target {
            Some((xml, placement, title)) => {
                crate::add_popup::AddPopup::new(view_id, Some((xml, placement)), title, pos)
            }
            None => crate::add_popup::AddPopup::new(view_id, None, "Add an unconnected node", pos),
        };
        self.add_popup = Some(popup);
    }

    /// The search popup, if it is open.
    pub fn add_popup(&self) -> Option<&crate::add_popup::AddPopup> {
        self.add_popup.as_ref()
    }

    /// The command search, if it is open.
    pub fn command_search(&self) -> Option<&crate::command_search::CommandSearch> {
        self.command_search.as_ref()
    }

    /// The items of the command search: the commands that can run now, the
    /// trees of the active document, the recent files that exist and the
    /// themes. [`Scope::Trees`](crate::command_search::Scope::Trees) gives
    /// the trees only.
    pub fn command_search_items(
        &self,
        ctx: &egui::Context,
        scope: crate::command_search::Scope,
    ) -> Vec<crate::command_search::SearchItem> {
        use crate::command_search::{Scope, SearchItem, SearchTarget};
        let all = scope == Scope::All;
        let mut items = Vec::new();
        if all {
            for command in Command::ALL {
                let theme = matches!(
                    command,
                    Command::ThemeLight | Command::ThemeDark | Command::ThemeSystem
                );
                if theme || command == Command::SearchCommands || !self.is_enabled(command) {
                    continue;
                }
                items.push(SearchItem {
                    target: SearchTarget::Command(command),
                    text: format!("{}: {}", command.group(), command.label()),
                    tag: self.keymap.shortcut_text(ctx, command).unwrap_or_default(),
                    description: None,
                });
            }
        }
        if let Some(view) = self
            .active_view_in_mode()
            .and_then(|v| self.workspace.view(v))
            && let Some(doc) = self.workspace.doc(view.doc)
        {
            let trees: Vec<(InstanceIndex, String, String)> = match &doc.editor {
                Some(editor) => {
                    let refs = editor.tree_refs();
                    let several_files = {
                        let mut files: Vec<_> = refs.iter().map(|r| r.file).collect();
                        files.dedup();
                        files.len() > 1
                    };
                    editor
                        .trees()
                        .into_iter()
                        .map(|(index, id)| {
                            let file = refs
                                .get(index)
                                .filter(|_| several_files)
                                .map(|r| editor.file_name(r.file))
                                .unwrap_or_default();
                            (InstanceIndex(index), id, file)
                        })
                        .collect()
                }
                None => panels::tree_list::rows(&doc.runtime, doc.linking)
                    .into_iter()
                    .map(|row| (row.instance, row.tree_id, row.path))
                    .collect(),
            };
            for (instance, id, place) in trees {
                let current = instance == view.root_instance;
                items.push(SearchItem {
                    target: SearchTarget::Tree(view.id, instance),
                    text: if all { format!("Tree: {id}") } else { id },
                    tag: if current {
                        "shown".to_owned()
                    } else {
                        place.clone()
                    },
                    description: (!place.is_empty()).then_some(place),
                });
            }
        }
        if all {
            for recent in &self.settings.recent {
                if !crate::remote::is_remote_url(&recent.path) && !recent.path.exists() {
                    continue;
                }
                let name = recent.path.file_name().map_or_else(
                    || recent.path.display().to_string(),
                    |n| n.to_string_lossy().into_owned(),
                );
                items.push(SearchItem {
                    target: SearchTarget::Recent(recent.path.clone()),
                    text: format!("Open recent: {name}"),
                    tag: match recent.kind {
                        RecentKind::Tree => "tree",
                        RecentKind::Project => "project",
                        RecentKind::Log => "log",
                    }
                    .to_owned(),
                    description: Some(recent.path.display().to_string()),
                });
            }
            let current = self.theme();
            for choice in ThemeChoice::ALL {
                let name = if choice.family() == "FastTick" {
                    format!("FastTick {}", choice.name())
                } else {
                    choice.name().to_owned()
                };
                items.push(SearchItem {
                    target: SearchTarget::Theme(choice),
                    text: format!("Theme: {name}"),
                    tag: if choice == current {
                        "current".to_owned()
                    } else {
                        String::new()
                    },
                    description: None,
                });
            }
        }
        items
    }

    /// The command search popup.
    pub(super) fn command_search_dialog(&mut self, ctx: &egui::Context) {
        let Some(scope) = self.command_search.as_ref().map(|s| s.scope) else {
            return;
        };
        let items = self.command_search_items(ctx, scope);
        let Some(search) = &mut self.command_search else {
            return;
        };
        use crate::command_search::SearchTarget;
        use crate::search_list::SearchChoice;
        match crate::command_search::show(ctx, search, &items) {
            SearchChoice::Open => {}
            SearchChoice::Cancel => self.command_search = None,
            SearchChoice::Pick(target) => {
                self.command_search = None;
                match target {
                    SearchTarget::Command(command) => self.run_command(ctx, command),
                    SearchTarget::Tree(view, instance) => {
                        self.workspace.set_view_root(view, instance);
                        self.focus_view(view);
                    }
                    SearchTarget::Recent(path) => self.start_open(ctx, path),
                    SearchTarget::Theme(choice) => self.set_theme(ctx, choice),
                }
            }
        }
    }

    /// The dialog that shows a loaded breakpoint file before the hooks go
    /// to the executor.
    pub(super) fn breakpoint_dialog(&mut self, ctx: &egui::Context) {
        let palette = theme::canvas_palette(self.theme(), ctx.theme());
        let Some(pending) = &mut self.pending_breakpoints else {
            return;
        };
        let mut open = true;
        let mut insert = false;
        let mut cancel = false;
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
    pub(super) fn sqlite_dialog(&mut self, ctx: &egui::Context) {
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
