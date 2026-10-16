//! The main window: top bar, menus, status bar, dock and dialogs.

use super::*;

impl FastTickApp {
    /// Draws one frame. `eframe::App::ui` calls this; the UI tests call it
    /// through the same trait.
    pub fn show(&mut self, ui: &mut Ui) {
        let ctx = ui.ctx().clone();
        self.handle_jobs(&ctx);
        self.handle_live(&ctx);
        self.handle_replay(&ctx);

        // Closing the window with unsaved changes asks first.
        if ctx.input(|i| i.viewport().close_requested())
            && !self.close_confirmed
            && !self.unsaved_documents().is_empty()
        {
            ctx.send_viewport_cmd(egui::ViewportCommand::CancelClose);
            self.pending_close = Some(PendingClose::Window);
        }

        self.handle_dropped_files(&ctx);
        self.check_remote_changes(&ctx);

        let commands = self.keymap.consume(&ctx, |c| self.is_enabled(c));
        for command in commands {
            self.run_command(&ctx, command);
        }
        for event in crate::shortcuts::take_clipboard_events(&ctx) {
            self.clipboard_event(&ctx, event);
        }

        let palette = theme::canvas_palette(self.theme(), ctx.theme());

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
        self.update_recovery(&ctx);
        self.update_title(&ctx);
    }

    /// The live bar and the breakpoint banner, above the dock of live mode.
    pub(super) fn live_header(&mut self, ui: &mut Ui, palette: &CanvasPalette) {
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

    pub(super) fn update_title(&mut self, ctx: &egui::Context) {
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

    pub(super) fn top_bar(&mut self, ui: &mut Ui, palette: &CanvasPalette) {
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

    pub(super) fn view_controls(&mut self, ui: &mut Ui) {
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

    pub(super) fn menu_item(&mut self, ui: &mut Ui, command: Command) {
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

    pub(super) fn radio_item(&mut self, ui: &mut Ui, command: Command, selected: bool) {
        let mut button = egui::Button::selectable(selected, command.label());
        if let Some(text) = self.keymap.shortcut_text(ui.ctx(), command) {
            button = button.shortcut_text(text);
        }
        if ui.add(button).clicked() {
            let ctx = ui.ctx().clone();
            self.run_command(&ctx, command);
        }
    }

    pub(super) fn menus(&mut self, ui: &mut Ui) {
        ui.menu_button("File", |ui| {
            self.menu_item(ui, Command::NewProject);
            self.menu_item(ui, Command::OpenFile);
            self.menu_item(ui, Command::OpenLog);
            self.menu_item(ui, Command::OpenRemote);
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
            self.menu_item(ui, Command::UploadRemote);
            self.menu_item(ui, Command::ReloadRemote);
            self.menu_item(ui, Command::DebugRobot);
            ui.separator();
            self.menu_item(ui, Command::ExportSvg);
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
            self.menu_item(ui, Command::DeleteWithChildren);
            ui.separator();
            self.menu_item(ui, Command::SelectAll);
            self.menu_item(ui, Command::AddNode);
            self.menu_item(ui, Command::MoveUp);
            self.menu_item(ui, Command::MoveDown);
            self.menu_item(ui, Command::Find);
        });
        ui.menu_button("View", |ui| {
            self.menu_item(ui, Command::SearchCommands);
            self.menu_item(ui, Command::GoToTree);
            ui.separator();
            self.menu_item(ui, Command::FitToView);
            self.menu_item(ui, Command::FitSelection);
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
                let mut family = "FastTick";
                for choice in ThemeChoice::ALL {
                    if choice.family() == "FastTick" {
                        continue;
                    }
                    if choice.family() != family {
                        family = choice.family();
                        ui.separator();
                        ui.label(RichText::new(family).small().weak());
                    }
                    if ui
                        .add(egui::Button::selectable(theme == choice, choice.name()))
                        .clicked()
                    {
                        let ctx = ui.ctx().clone();
                        self.set_theme(&ctx, choice);
                    }
                }
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
            self.menu_item(ui, Command::DebugRobot);
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

    pub(super) fn status_bar(&mut self, ui: &mut Ui, palette: &CanvasPalette) {
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
                            match self.remote.url_of(path) {
                                Some(url) => {
                                    let pending = doc.editor.as_ref().is_some_and(|e| {
                                        e.all_paths().iter().any(|p| self.remote.is_pending(p))
                                    });
                                    if pending {
                                        format!("{url} (changes not uploaded)")
                                    } else {
                                        url.to_string()
                                    }
                                }
                                None => path.display().to_string(),
                            }
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

    pub(super) fn dock_area(&mut self, ui: &mut Ui, palette: &CanvasPalette) {
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

    pub(super) fn dialogs(&mut self, ctx: &egui::Context, palette: &CanvasPalette) {
        if self.show_about {
            self.show_about = dialogs::about(ctx, palette.accent);
        }
        if self.show_shortcuts {
            self.show_shortcuts = dialogs::shortcuts(ctx, &self.keymap);
        }
        self.breakpoint_dialog(ctx);
        self.unsaved_dialog(ctx);
        self.recovery_dialog(ctx);
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
        self.add_popup_dialog(ctx, palette);
        self.command_search_dialog(ctx);
        self.remote_dialogs(ctx);
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
