//! Side panels. One function for each panel; [`show_panel`] picks it.
//!
//! Panels that are not built yet show a short placeholder text
//! ([`placeholder_text`]).

pub mod inspector;
pub mod start;
pub mod tree_list;

use egui::{RichText, Ui};

use crate::actions::AppAction;
use crate::app::Mode;
use crate::dock::Tab;
use crate::documents::Workspace;
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
    pub live: &'a LiveController,
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
        Tab::Document(_) => {}
        other => placeholder(ui, other),
    }
}

/// The placeholder text of a panel that is not built yet.
pub fn placeholder_text(tab: Tab) -> &'static str {
    match tab {
        Tab::Palette => "The node palette comes with the tree editor.",
        Tab::Blackboard => "The blackboard viewer comes with live debugging.",
        Tab::Breakpoints => "The breakpoint list comes with live debugging.",
        Tab::Problems => "The problems list comes with validation.",
        Tab::Xml => "The XML view comes with the tree editor.",
        Tab::Search => "Search comes in a later version.",
        Tab::Transitions => "The transitions table comes with replay.",
        Tab::Statistics => "The statistics table comes with replay.",
        Tab::Start | Tab::Document(_) | Tab::Trees | Tab::Inspector => "",
    }
}

fn placeholder(ui: &mut Ui, tab: Tab) {
    ui.add_space(8.0);
    ui.vertical_centered(|ui| {
        ui.label(RichText::new(tab.title()).strong());
        ui.label(RichText::new(placeholder_text(tab)).weak());
    });
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_unbuilt_panel_has_a_placeholder() {
        for tab in Tab::PANELS {
            if !matches!(tab, Tab::Trees | Tab::Inspector) {
                assert!(!placeholder_text(tab).is_empty(), "{tab:?}");
            }
        }
    }
}
