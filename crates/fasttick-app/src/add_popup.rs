//! The popup that adds a node: a search field with fuzzy matching (see
//! [`crate::fuzzy`]) and the list of the node types that the file can use.
//!
//! The canvas opens it when the user drags a connection from the dot of a
//! node and releases it, or clicks the dot; the context menu opens it for
//! "Add child…", "Add sibling before…", "Add sibling after…" and "Add
//! node…". Arrow keys move the selection, Enter adds the selected type,
//! Escape or a click outside closes the popup.

use egui::Pos2;
use fasttick_core::models::{ModelSource, NodeKind, NodeModel};
use fasttick_core::xml::NodeId;

use crate::canvas::category_of_kind;
use crate::documents::ViewId;
use crate::edit_ops::{EditOp, Placement};
use crate::fuzzy;
use crate::search_list::{self, SearchChoice, SearchRow, SearchState, SearchTexts};
use crate::theme::CanvasPalette;

/// A node type that the popup can add.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AddCandidate {
    /// The type ID.
    pub id: String,
    /// The kind.
    pub kind: NodeKind,
    /// The description of the model, if any.
    pub description: Option<String>,
    /// True for a built-in node of BT.CPP.
    pub builtin: bool,
}

impl AddCandidate {
    /// The candidates of a list of models.
    pub fn from_models<'a>(
        models: impl IntoIterator<Item = (&'a NodeModel, ModelSource)>,
    ) -> Vec<AddCandidate> {
        models
            .into_iter()
            .map(|(model, source)| AddCandidate {
                id: model.id.clone(),
                kind: model.kind,
                description: model.description().map(str::to_owned).or_else(|| {
                    (model.kind == NodeKind::SubTree)
                        .then(|| format!("A SubTree node that runs the tree {}.", model.id))
                }),
                builtin: source == ModelSource::Builtin,
            })
            .collect()
    }
}

/// The candidates that match `query`, best first. For an empty query: the
/// recent types (newest first), then the types of the project, then the
/// built-in types, each sorted by ID.
pub fn matches<'a>(
    query: &str,
    candidates: &'a [AddCandidate],
    recent: &[String],
) -> Vec<&'a AddCandidate> {
    let mut scored: Vec<(i32, &AddCandidate)> = candidates
        .iter()
        .filter_map(|c| Some((fuzzy::score(query, &c.id)?, c)))
        .collect();
    if query.trim().is_empty() {
        let rank = |c: &AddCandidate| recent.iter().position(|r| *r == c.id).unwrap_or(usize::MAX);
        scored.sort_by(|a, b| {
            rank(a.1)
                .cmp(&rank(b.1))
                .then(a.1.builtin.cmp(&b.1.builtin))
                .then(a.1.id.cmp(&b.1.id))
        });
    } else {
        scored.sort_by(|a, b| b.0.cmp(&a.0).then(a.1.id.cmp(&b.1.id)));
    }
    scored.into_iter().map(|(_, c)| c).collect()
}

/// The state of an open popup.
#[derive(Debug, Clone, PartialEq)]
pub struct AddPopup {
    /// The edit view.
    pub view: ViewId,
    /// Where the node goes: a node and a placement, or `None` for an
    /// unconnected node.
    pub target: Option<(NodeId, Placement)>,
    /// What the popup adds, for example "Add a child of Sequence".
    pub title: String,
    /// The top-left corner on the screen.
    pub pos: Pos2,
    /// The search text and the selection.
    pub search: SearchState,
}

impl AddPopup {
    /// A popup with an empty search.
    pub fn new(
        view: ViewId,
        target: Option<(NodeId, Placement)>,
        title: impl Into<String>,
        pos: Pos2,
    ) -> AddPopup {
        AddPopup {
            view,
            target,
            title: title.into(),
            pos,
            search: SearchState::default(),
        }
    }

    /// The search text.
    pub fn query(&self) -> &str {
        &self.search.query
    }

    /// The edit that adds a node of `type_id`.
    pub fn op(&self, type_id: String) -> EditOp {
        match self.target {
            Some(target) => EditOp::AddNode {
                type_id,
                target: Some(target),
            },
            None => EditOp::AddUnconnected { type_id },
        }
    }
}

/// The result of [`show`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum AddChoice {
    /// The popup stays open.
    Open,
    /// The user closed the popup.
    Cancel,
    /// The user selected a type.
    Add(String),
}

/// Draws the popup.
pub fn show(
    ctx: &egui::Context,
    popup: &mut AddPopup,
    candidates: &[AddCandidate],
    recent: &[String],
    palette: &CanvasPalette,
) -> AddChoice {
    let texts = SearchTexts {
        id: "fasttick_add_popup",
        title: &popup.title,
        hint: "Search node types",
        empty: "No node type matches the search.",
        footer: "\u{2191} \u{2193} select \u{b7} Enter add \u{b7} Esc close",
        width: search_list::WIDTH,
    };
    let rows = |query: &str| -> Vec<SearchRow<String>> {
        matches(query, candidates, recent)
            .into_iter()
            .map(|candidate| SearchRow {
                key: candidate.id.clone(),
                text: candidate.id.clone(),
                tag: if candidate.builtin {
                    format!("{} \u{b7} built-in", candidate.kind.tag())
                } else {
                    candidate.kind.tag().to_owned()
                },
                swatch: Some(palette.category(category_of_kind(candidate.kind))),
                description: Some(
                    candidate
                        .description
                        .clone()
                        .unwrap_or_else(|| format!("{} (no description)", candidate.kind.tag())),
                ),
            })
            .collect()
    };
    match search_list::show(ctx, &mut popup.search, popup.pos, texts, rows) {
        SearchChoice::Open => AddChoice::Open,
        SearchChoice::Cancel => AddChoice::Cancel,
        SearchChoice::Pick(id) => AddChoice::Add(id),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn candidate(id: &str, builtin: bool) -> AddCandidate {
        AddCandidate {
            id: id.to_owned(),
            kind: NodeKind::Action,
            description: None,
            builtin,
        }
    }

    #[test]
    fn empty_query_lists_project_types_first() {
        let list = [
            candidate("Sequence", true),
            candidate("MoveBase", false),
            candidate("AlwaysSuccess", true),
            candidate("Grab", false),
        ];
        let ids = |query: &str, recent: &[String]| -> Vec<String> {
            matches(query, &list, recent)
                .iter()
                .map(|c| c.id.clone())
                .collect()
        };
        assert_eq!(
            ids("", &[]),
            ["Grab", "MoveBase", "AlwaysSuccess", "Sequence"]
        );
        assert_eq!(ids("seq", &[]), ["Sequence"]);
        // The recent types come first, newest first.
        let recent = ["Sequence".to_owned(), "MoveBase".to_owned()];
        assert_eq!(
            ids("", &recent),
            ["Sequence", "MoveBase", "Grab", "AlwaysSuccess"]
        );
    }

    #[test]
    fn the_op_follows_the_target() {
        let popup = AddPopup::new(ViewId(1), None, "Add a node", Pos2::ZERO);
        assert_eq!(
            popup.op("Sleep".into()),
            EditOp::AddUnconnected {
                type_id: "Sleep".into()
            }
        );
    }
}
