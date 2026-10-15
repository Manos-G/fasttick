//! The command search: a popup that finds a command, a tree of the active
//! document, a recent file or a theme by its name (Ctrl+Shift+P). Ctrl+P
//! opens it for the trees only ("Go to tree").
//!
//! The app makes the items (see [`SearchItem`]); this module filters them
//! with the fuzzy matcher ([`crate::fuzzy`]) and draws the popup with
//! [`crate::search_list`].

use std::path::PathBuf;

use egui::Pos2;
use fasttick_core::runtime::InstanceIndex;

use crate::documents::ViewId;
use crate::fuzzy;
use crate::search_list::{self, SearchChoice, SearchRow, SearchState, SearchTexts};
use crate::shortcuts::Command;
use crate::theme::ThemeChoice;

/// The width of the popup.
const WIDTH: f32 = 520.0;

/// What an item does.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SearchTarget {
    /// Run a command.
    Command(Command),
    /// Show a tree in a view.
    Tree(ViewId, InstanceIndex),
    /// Open a recent file.
    Recent(PathBuf),
    /// Use a theme.
    Theme(ThemeChoice),
}

/// One item of the search.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SearchItem {
    /// What the item does.
    pub target: SearchTarget,
    /// The text that the search matches, for example "Edit: Undo".
    pub text: String,
    /// A short text at the right side, for example the shortcut.
    pub tag: String,
    /// A longer text for the selected item.
    pub description: Option<String>,
}

/// The items that the search shows.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Scope {
    /// Commands, trees, recent files and themes.
    All,
    /// The trees of the active document.
    Trees,
}

/// The state of an open command search.
#[derive(Debug, Clone, PartialEq)]
pub struct CommandSearch {
    /// The items that the search shows.
    pub scope: Scope,
    /// The search text and the selection.
    pub search: SearchState,
}

impl CommandSearch {
    /// A search with an empty query.
    pub fn new(scope: Scope) -> CommandSearch {
        CommandSearch {
            scope,
            search: SearchState::default(),
        }
    }
}

/// The items that match `query`, best first. An empty query keeps the
/// order of the items.
pub fn matches<'a>(query: &str, items: &'a [SearchItem]) -> Vec<&'a SearchItem> {
    if query.trim().is_empty() {
        return items.iter().collect();
    }
    let mut scored: Vec<(i32, usize, &SearchItem)> = items
        .iter()
        .enumerate()
        .filter_map(|(index, item)| Some((fuzzy::score(query, &item.text)?, index, item)))
        .collect();
    scored.sort_by(|a, b| b.0.cmp(&a.0).then(a.1.cmp(&b.1)));
    scored.into_iter().map(|(_, _, item)| item).collect()
}

/// Draws the popup at the top of the window.
pub fn show(
    ctx: &egui::Context,
    state: &mut CommandSearch,
    items: &[SearchItem],
) -> SearchChoice<SearchTarget> {
    let screen = ctx.content_rect();
    let pos = Pos2::new(
        screen.center().x - WIDTH / 2.0,
        screen.min.y + (screen.height() * 0.12).min(80.0),
    );
    let (title, hint, empty) = match state.scope {
        Scope::All => (
            "Run a command, go to a tree, open a recent file or use a theme",
            "Search commands",
            "Nothing matches the search.",
        ),
        Scope::Trees => (
            "Go to a tree of this document",
            "Search trees",
            "No tree matches the search.",
        ),
    };
    let texts = SearchTexts {
        id: "fasttick_command_search",
        title,
        hint,
        empty,
        footer: "\u{2191} \u{2193} select \u{b7} Enter run \u{b7} Esc close",
        width: WIDTH,
    };
    let rows = |query: &str| -> Vec<SearchRow<SearchTarget>> {
        matches(query, items)
            .into_iter()
            .map(|item| SearchRow {
                key: item.target.clone(),
                text: item.text.clone(),
                tag: item.tag.clone(),
                swatch: None,
                description: item.description.clone(),
            })
            .collect()
    };
    search_list::show(ctx, &mut state.search, pos, texts, rows)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn item(text: &str, command: Command) -> SearchItem {
        SearchItem {
            target: SearchTarget::Command(command),
            text: text.to_owned(),
            tag: String::new(),
            description: None,
        }
    }

    #[test]
    fn the_best_match_comes_first() {
        let items = [
            item("File: Save", Command::Save),
            item("File: Save as\u{2026}", Command::SaveAs),
            item("Edit: Undo", Command::Undo),
            item("View: Fit to view", Command::FitToView),
        ];
        let texts = |query: &str| -> Vec<&str> {
            matches(query, &items)
                .iter()
                .map(|i| i.text.as_str())
                .collect()
        };
        // An empty query keeps the order.
        assert_eq!(texts("").len(), 4);
        assert_eq!(texts("")[0], "File: Save");
        assert_eq!(texts("undo"), ["Edit: Undo"]);
        assert_eq!(texts("fit")[0], "View: Fit to view");
        assert_eq!(texts("save as")[0], "File: Save as\u{2026}");
        assert!(texts("zzz").is_empty());
    }
}
