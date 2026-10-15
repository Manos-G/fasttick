//! The dock: document tabs and side panels ([`Tab`]), the default
//! arrangement of each mode, and the helpers that place new tabs.
//!
//! Each mode (edit, live, replay) has its own `DockState<Tab>`, so each
//! mode keeps its own panel arrangement. The arrangement is saved without
//! the document tabs.
//!
//! To add a panel: add a variant to [`Tab`], give it a title and a zone in
//! [`Tab::title`] and [`Tab::zone`], add it to [`Tab::PANELS`], put it in
//! the default arrangements in [`default_dock`] if it should show by
//! default, and draw it in `panels::show_panel`.

use egui_dock::{DockState, NodeIndex, SurfaceIndex};
use serde::{Deserialize, Serialize};

use crate::app::Mode;
use crate::documents::ViewId;

/// A dock tab.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum Tab {
    /// The start page of a mode (open, connect, recent files).
    Start,
    /// A view of a document (a tree canvas).
    Document(ViewId),
    /// The trees of the file, or the SubTree instances of a running tree.
    Trees,
    /// The node palette (edit mode).
    Palette,
    /// The properties of the selected node.
    Inspector,
    /// The blackboards of a running tree.
    Blackboard,
    /// The breakpoints.
    Breakpoints,
    /// Validation problems.
    Problems,
    /// The XML of the document.
    Xml,
    /// Search.
    Search,
    /// The status transitions (live recording, replay).
    Transitions,
    /// Statistics of a log.
    Statistics,
    /// The replay timeline: playback controls and the time bar.
    Timeline,
}

/// Where a panel goes when it opens.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Zone {
    /// The left column.
    Left,
    /// The centre (documents).
    Center,
    /// The right column.
    Right,
    /// The bottom row.
    Bottom,
}

impl Tab {
    /// All panel tabs (not documents and not the start page).
    pub const PANELS: [Tab; 11] = [
        Tab::Trees,
        Tab::Palette,
        Tab::Inspector,
        Tab::Blackboard,
        Tab::Breakpoints,
        Tab::Problems,
        Tab::Xml,
        Tab::Search,
        Tab::Transitions,
        Tab::Statistics,
        Tab::Timeline,
    ];

    /// The title of a panel tab. Documents get their title from the
    /// workspace.
    pub fn title(self) -> &'static str {
        match self {
            Tab::Start => "Start",
            Tab::Document(_) => "Document",
            Tab::Trees => "Trees",
            Tab::Palette => "Palette",
            Tab::Inspector => "Inspector",
            Tab::Blackboard => "Blackboard",
            Tab::Breakpoints => "Breakpoints",
            Tab::Problems => "Problems",
            Tab::Xml => "XML",
            Tab::Search => "Search",
            Tab::Transitions => "Transitions",
            Tab::Statistics => "Statistics",
            Tab::Timeline => "Timeline",
        }
    }

    /// The default zone.
    pub fn zone(self) -> Zone {
        match self {
            Tab::Start | Tab::Document(_) => Zone::Center,
            Tab::Trees | Tab::Palette => Zone::Left,
            Tab::Inspector | Tab::Blackboard | Tab::Breakpoints => Zone::Right,
            Tab::Problems
            | Tab::Xml
            | Tab::Search
            | Tab::Transitions
            | Tab::Statistics
            | Tab::Timeline => Zone::Bottom,
        }
    }

    /// True for document tabs and the start page.
    pub fn is_central(self) -> bool {
        self.zone() == Zone::Center
    }
}

/// The default arrangement of a mode.
pub fn default_dock(mode: Mode) -> DockState<Tab> {
    let (left, right, bottom): (Vec<Tab>, Vec<Tab>, Vec<Tab>) = match mode {
        Mode::Edit => (
            vec![Tab::Trees, Tab::Palette],
            vec![Tab::Inspector],
            vec![Tab::Problems, Tab::Xml, Tab::Search],
        ),
        Mode::Live => (
            vec![Tab::Trees],
            vec![Tab::Inspector, Tab::Blackboard, Tab::Breakpoints],
            vec![Tab::Transitions, Tab::Search],
        ),
        Mode::Replay => (
            vec![Tab::Trees],
            vec![Tab::Inspector],
            vec![Tab::Transitions, Tab::Statistics, Tab::Search],
        ),
    };
    let mut dock = DockState::new(vec![Tab::Start]);
    let surface = dock.main_surface_mut();
    // Order of the splits: bottom row under the centre column only, then
    // the side columns over the full height.
    let [center, _left] = surface.split_left(NodeIndex::root(), 0.18, left);
    // The right column (the inspector in edit mode) is wide enough for
    // long port values.
    let [center, _right] = surface.split_right(center, 0.71, right);
    match mode {
        Mode::Replay => {
            // The tables below, and the timeline between the tree and the
            // tables, so that both are visible at the same time.
            let [center, _bottom] = surface.split_below(center, 0.62, bottom);
            surface.split_below(center, 0.7, vec![Tab::Timeline]);
        }
        Mode::Edit | Mode::Live => {
            surface.split_below(center, 0.74, bottom);
        }
    }
    dock
}

/// The dock without the document tabs, for saving.
pub fn without_documents(dock: &DockState<Tab>) -> DockState<Tab> {
    dock.filter_map_tabs(|tab| (!matches!(tab, Tab::Document(_))).then_some(*tab))
}

/// True if the dock contains a tab.
pub fn contains(dock: &DockState<Tab>, tab: Tab) -> bool {
    dock.find_tab(&tab).is_some()
}

/// Adds a document tab next to the other documents (or the start page),
/// and makes it active. If the dock has no central tab, the tab goes to
/// the largest leaf.
pub fn add_document(dock: &mut DockState<Tab>, tab: Tab) {
    if let Some(path) = dock
        .find_tab_from(|t| matches!(t, Tab::Document(_)))
        .or_else(|| dock.find_tab(&Tab::Start))
    {
        let node = path.node_path();
        dock.set_focused_node_and_surface(node);
        dock.push_to_focused_leaf(tab);
        return;
    }
    push_to_largest_leaf(dock, tab);
}

/// Adds a tab beside a document tab, in a new split to its right (split
/// view).
pub fn add_beside(dock: &mut DockState<Tab>, beside: Tab, tab: Tab) {
    let Some(path) = dock.find_tab(&beside) else {
        add_document(dock, tab);
        return;
    };
    let node = path.node_path();
    if let Some(surface) = dock.get_surface_mut(node.surface)
        && let Some(tree) = surface.node_tree_mut()
    {
        tree.split_right(node.node, 0.5, vec![tab]);
        return;
    }
    add_document(dock, tab);
}

/// Shows a panel: focuses it if it is in the dock, else adds it to a leaf
/// of its zone (a leaf that holds another panel of the same zone).
pub fn show_panel(dock: &mut DockState<Tab>, tab: Tab) {
    if let Some(path) = dock.find_tab(&tab) {
        let _ = dock.set_active_tab(path);
        dock.set_focused_node_and_surface(path.node_path());
        return;
    }
    let zone = tab.zone();
    if let Some(path) = dock.find_tab_from(|t| t.zone() == zone && *t != tab) {
        dock.set_focused_node_and_surface(path.node_path());
        dock.push_to_focused_leaf(tab);
        return;
    }
    // No panel of this zone is open: split the main surface.
    let surface = dock.main_surface_mut();
    if surface.is_empty() {
        surface.push_to_first_leaf(tab);
        return;
    }
    match zone {
        Zone::Left => {
            surface.split_left(NodeIndex::root(), 0.2, vec![tab]);
        }
        Zone::Right => {
            surface.split_right(NodeIndex::root(), 0.78, vec![tab]);
        }
        Zone::Bottom => {
            surface.split_below(NodeIndex::root(), 0.72, vec![tab]);
        }
        Zone::Center => {
            surface.push_to_first_leaf(tab);
        }
    }
}

/// Removes a panel from the dock. Returns true if it was there.
pub fn hide_panel(dock: &mut DockState<Tab>, tab: Tab) -> bool {
    match dock.find_tab(&tab) {
        Some(path) => dock.remove_tab(path).is_some(),
        None => false,
    }
}

fn push_to_largest_leaf(dock: &mut DockState<Tab>, tab: Tab) {
    let largest = dock
        .iter_leaves()
        .filter(|(path, _)| path.surface == SurfaceIndex::main())
        .max_by(|(_, a), (_, b)| {
            let area_a = a.rect.area();
            let area_b = b.rect.area();
            area_a.total_cmp(&area_b)
        })
        .map(|(path, _)| path);
    match largest {
        Some(path) => {
            dock.set_focused_node_and_surface(path);
            dock.push_to_focused_leaf(tab);
        }
        None => dock.main_surface_mut().push_to_first_leaf(tab),
    }
}

/// All document views in a dock.
pub fn document_views(dock: &DockState<Tab>) -> Vec<ViewId> {
    dock.iter_all_tabs()
        .filter_map(|(_, tab)| match tab {
            Tab::Document(view) => Some(*view),
            _ => None,
        })
        .collect()
}

/// The view of the active tab of the focused leaf, if it is a document.
pub fn focused_view(dock: &mut DockState<Tab>) -> Option<ViewId> {
    match dock.find_active_focused() {
        Some((_, Tab::Document(view))) => Some(*view),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn default_docks_have_all_zones() {
        for mode in [Mode::Edit, Mode::Live, Mode::Replay] {
            let dock = default_dock(mode);
            assert!(contains(&dock, Tab::Start));
            assert!(contains(&dock, Tab::Trees));
            assert!(contains(&dock, Tab::Inspector));
            assert!(dock.iter_all_tabs().any(|(_, t)| t.zone() == Zone::Bottom));
        }
        assert!(contains(&default_dock(Mode::Edit), Tab::Palette));
        assert!(contains(&default_dock(Mode::Live), Tab::Breakpoints));
        assert!(contains(&default_dock(Mode::Live), Tab::Blackboard));
        assert!(contains(&default_dock(Mode::Replay), Tab::Statistics));
        assert!(contains(&default_dock(Mode::Replay), Tab::Timeline));
        assert!(!contains(&default_dock(Mode::Live), Tab::Timeline));
    }

    #[test]
    fn documents_go_to_the_centre() {
        let mut dock = default_dock(Mode::Edit);
        add_document(&mut dock, Tab::Document(ViewId(1)));
        add_document(&mut dock, Tab::Document(ViewId(2)));
        let start = dock.find_tab(&Tab::Start).unwrap().node_path();
        let first = dock
            .find_tab(&Tab::Document(ViewId(1)))
            .unwrap()
            .node_path();
        let second = dock
            .find_tab(&Tab::Document(ViewId(2)))
            .unwrap()
            .node_path();
        assert_eq!(start, first);
        assert_eq!(first, second);
        assert_eq!(document_views(&dock), vec![ViewId(1), ViewId(2)]);
        assert_eq!(focused_view(&mut dock), Some(ViewId(2)));

        // Split view: a new leaf.
        add_beside(
            &mut dock,
            Tab::Document(ViewId(2)),
            Tab::Document(ViewId(3)),
        );
        let third = dock
            .find_tab(&Tab::Document(ViewId(3)))
            .unwrap()
            .node_path();
        assert_ne!(third, second);

        // Saving drops the documents.
        let saved = without_documents(&dock);
        assert!(document_views(&saved).is_empty());
        assert!(contains(&saved, Tab::Start));
        assert!(contains(&saved, Tab::Problems));
    }

    #[test]
    fn documents_without_a_centre() {
        let mut dock = DockState::new(vec![Tab::Trees]);
        add_document(&mut dock, Tab::Document(ViewId(7)));
        assert!(contains(&dock, Tab::Document(ViewId(7))));
        let mut empty: DockState<Tab> = DockState::new(vec![]);
        add_document(&mut empty, Tab::Document(ViewId(8)));
        assert!(contains(&empty, Tab::Document(ViewId(8))));
    }

    #[test]
    fn show_and_hide_panels() {
        let mut dock = default_dock(Mode::Edit);
        assert!(hide_panel(&mut dock, Tab::Problems));
        assert!(!contains(&dock, Tab::Problems));
        assert!(!hide_panel(&mut dock, Tab::Problems));
        show_panel(&mut dock, Tab::Problems);
        // It goes to the leaf of the other bottom panels.
        let problems = dock.find_tab(&Tab::Problems).unwrap().node_path();
        let xml = dock.find_tab(&Tab::Xml).unwrap().node_path();
        assert_eq!(problems, xml);
        // A zone with no open panel gets a new split.
        hide_panel(&mut dock, Tab::Inspector);
        show_panel(&mut dock, Tab::Blackboard);
        assert!(contains(&dock, Tab::Blackboard));
        // Showing an open panel does not add a second one.
        show_panel(&mut dock, Tab::Blackboard);
        assert_eq!(
            dock.iter_all_tabs()
                .filter(|(_, t)| **t == Tab::Blackboard)
                .count(),
            1
        );
    }

    #[test]
    fn dock_serializes() {
        use std::collections::HashMap;
        #[derive(Default)]
        struct Memory(HashMap<String, String>);
        impl eframe::Storage for Memory {
            fn get_string(&self, key: &str) -> Option<String> {
                self.0.get(key).cloned()
            }
            fn set_string(&mut self, key: &str, value: String) {
                self.0.insert(key.to_owned(), value);
            }
            fn remove_string(&mut self, key: &str) {
                self.0.remove(key);
            }
            fn flush(&mut self) {}
        }
        let mut dock = default_dock(Mode::Live);
        add_document(&mut dock, Tab::Document(ViewId(4)));
        let mut storage = Memory::default();
        eframe::set_value(&mut storage, "dock", &without_documents(&dock));
        let back: DockState<Tab> = eframe::get_value(&storage, "dock").unwrap();
        let tabs = |d: &DockState<Tab>| d.iter_all_tabs().map(|(_, t)| *t).collect::<Vec<_>>();
        assert_eq!(tabs(&back), tabs(&without_documents(&dock)));
        assert!(!contains(&back, Tab::Document(ViewId(4))));
    }
}
