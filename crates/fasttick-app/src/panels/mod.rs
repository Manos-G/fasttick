//! Side panels. One function for each panel; [`show_panel`] picks it.

pub mod blackboard;
pub mod breakpoints;
pub mod inspector;
pub mod live_bar;
pub mod node_editor;
pub mod palette;
pub mod problems;
pub mod search;
pub mod start;
pub mod statistics;
pub mod timeline;
pub mod transitions;
pub mod tree_list;
pub mod xml_view;

use egui::{RichText, Ui};

use crate::actions::AppAction;
use crate::app::Mode;
use crate::dock::Tab;
use crate::documents::{DocId, Workspace};
use crate::jobs::LoadingFile;
use crate::live::LiveController;
use crate::settings::Settings;
use crate::shortcuts::KeyMap;
use crate::theme::CanvasPalette;

/// What a panel can read and change.
#[derive(Debug)]
pub struct PanelContext<'a> {
    /// The current mode.
    pub mode: Mode,
    /// The documents and views.
    pub workspace: &'a mut Workspace,
    /// The settings.
    pub settings: &'a mut Settings,
    /// The live connection.
    pub live: &'a mut LiveController,
    /// The document of the live tree, if any.
    pub live_doc: Option<DocId>,
    /// The logs that are loading now.
    pub loading: &'a [LoadingFile],
    /// The key map (for shortcut hints).
    pub keymap: &'a KeyMap,
    /// The canvas colours.
    pub palette: &'a CanvasPalette,
    /// Requests to the app.
    pub actions: &'a mut Vec<AppAction>,
}

/// Draws a panel tab.
pub fn show_panel(tab: Tab, ui: &mut Ui, cx: &mut PanelContext<'_>) {
    match tab {
        Tab::Start => start::show(ui, cx),
        Tab::Trees => tree_list::show(ui, cx),
        Tab::Inspector => inspector::show(ui, cx),
        Tab::Palette => palette::show(ui, cx),
        Tab::Problems => problems::show(ui, cx),
        Tab::Xml => xml_view::show(ui, cx),
        Tab::Search => search::show(ui, cx),
        Tab::Blackboard => blackboard::show(ui, cx),
        Tab::Breakpoints => breakpoints::show(ui, cx),
        Tab::Timeline => timeline::show(ui, cx),
        Tab::Transitions => transitions::show(ui, cx),
        Tab::Statistics => statistics::show(ui, cx),
        Tab::Document(_) => {}
    }
}

/// A status with its badge (the same colour and shape as on the canvas)
/// and its name. IDLE has a hollow circle.
pub(crate) fn status_cell(
    ui: &mut Ui,
    status: fasttick_core::status::NodeStatus,
    palette: &CanvasPalette,
) {
    ui.horizontal(|ui| {
        ui.spacing_mut().item_spacing.x = 4.0;
        let (rect, _) = ui.allocate_exact_size(egui::vec2(11.0, 11.0), egui::Sense::hover());
        if status == fasttick_core::status::NodeStatus::Idle {
            ui.painter().circle_stroke(
                rect.center(),
                4.0,
                egui::Stroke::new(1.2, ui.visuals().weak_text_color()),
            );
        } else {
            let mut shapes = Vec::new();
            crate::canvas::paint::status_badge(
                &mut shapes,
                rect.center(),
                11.0,
                status,
                false,
                palette,
            );
            ui.painter().extend(shapes);
        }
        ui.label(RichText::new(status.as_str()).small());
    });
}

/// The active view and its editor document, if the active view is an edit
/// view of `mode`.
pub(crate) fn active_edit(
    workspace: &Workspace,
    mode: Mode,
) -> Option<(crate::documents::ViewId, &crate::editor::EditorDoc)> {
    let view = workspace.active()?;
    let doc = workspace.doc(view.doc)?;
    if doc.mode() != mode {
        return None;
    }
    Some((view.id, doc.editor.as_deref()?))
}

/// A short weak text at the top of an empty panel.
pub(crate) fn empty(ui: &mut Ui, text: &str) {
    ui.add_space(8.0);
    ui.label(RichText::new(text).weak());
}
