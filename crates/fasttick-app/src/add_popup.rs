//! The popup that adds a node: a search field with fuzzy matching (see
//! [`crate::fuzzy`]) and the list of the node types that the file can use.
//!
//! The canvas opens it when the user drags a connection from the dot of a
//! node and releases it, or clicks the dot; the context menu opens it for
//! "Add child…", "Add sibling before…", "Add sibling after…" and "Add
//! node…". Arrow keys move the selection, Enter adds the selected type,
//! Escape or a click outside closes the popup.

use egui::{Align2, CornerRadius, Key, Modifiers, Pos2, RichText, Sense, Stroke};
use fasttick_core::models::{ModelSource, NodeKind, NodeModel};
use fasttick_core::xml::NodeId;

use crate::canvas::category_of_kind;
use crate::documents::ViewId;
use crate::edit_ops::{EditOp, Placement};
use crate::fuzzy;
use crate::theme::CanvasPalette;

/// The width of the popup.
const WIDTH: f32 = 340.0;
/// The height of one row of the list.
const ROW_HEIGHT: f32 = 24.0;
/// The most rows that show without scrolling.
const VISIBLE_ROWS: usize = 12;

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
                description: model.description().map(str::to_owned),
                builtin: source == ModelSource::Builtin,
            })
            .collect()
    }
}

/// The candidates that match `query`, best first. For an empty query: the
/// types of the project first, then the built-in types, each sorted by ID.
pub fn matches<'a>(query: &str, candidates: &'a [AddCandidate]) -> Vec<&'a AddCandidate> {
    let mut scored: Vec<(i32, &AddCandidate)> = candidates
        .iter()
        .filter_map(|c| Some((fuzzy::score(query, &c.id)?, c)))
        .collect();
    if query.trim().is_empty() {
        scored.sort_by(|a, b| a.1.builtin.cmp(&b.1.builtin).then(a.1.id.cmp(&b.1.id)));
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
    /// The search text.
    pub query: String,
    /// The index of the selected row.
    pub selected: usize,
    focused: bool,
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
            query: String::new(),
            selected: 0,
            focused: false,
        }
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
    palette: &CanvasPalette,
) -> AddChoice {
    let mut choice = AddChoice::Open;
    // The keys of the list, before the search field sees them.
    let (down, up, enter, escape) = ctx.input_mut(|i| {
        (
            i.consume_key(Modifiers::NONE, Key::ArrowDown),
            i.consume_key(Modifiers::NONE, Key::ArrowUp),
            i.consume_key(Modifiers::NONE, Key::Enter),
            i.consume_key(Modifiers::NONE, Key::Escape),
        )
    });
    let found = matches(&popup.query, candidates);
    if !found.is_empty() {
        if down {
            popup.selected = (popup.selected + 1).min(found.len() - 1);
        }
        if up {
            popup.selected = popup.selected.saturating_sub(1);
        }
        popup.selected = popup.selected.min(found.len() - 1);
        if enter {
            choice = AddChoice::Add(found[popup.selected].id.clone());
        }
    }
    if escape {
        choice = AddChoice::Cancel;
    }

    let area = egui::Area::new(egui::Id::new("fasttick_add_popup"))
        .order(egui::Order::Foreground)
        .fixed_pos(popup.pos)
        .pivot(Align2::LEFT_TOP)
        .constrain(true)
        .show(ctx, |ui| {
            egui::Frame::popup(ui.style()).show(ui, |ui| {
                ui.set_width(WIDTH);
                ui.label(RichText::new(&popup.title).small().weak());
                let edit = ui.add(
                    egui::TextEdit::singleline(&mut popup.query)
                        .id_salt("fasttick_add_popup_search")
                        .hint_text("Search node types")
                        .desired_width(f32::INFINITY),
                );
                if !popup.focused {
                    edit.request_focus();
                    popup.focused = true;
                }
                if edit.changed() {
                    popup.selected = 0;
                }
                ui.add_space(4.0);
                // The list of this frame uses the query of this frame.
                let found = matches(&popup.query, candidates);
                if found.is_empty() {
                    ui.label(RichText::new("No node type matches the search.").weak());
                }
                let selected = popup.selected.min(found.len().saturating_sub(1));
                egui::ScrollArea::vertical()
                    .max_height(ROW_HEIGHT * VISIBLE_ROWS as f32)
                    .auto_shrink([false, true])
                    .show(ui, |ui| {
                        for (index, candidate) in found.iter().enumerate() {
                            let response = row(ui, candidate, index == selected, palette);
                            if index == selected && (down || up) {
                                response.scroll_to_me(None);
                            }
                            if response.clicked() {
                                choice = AddChoice::Add(candidate.id.clone());
                            }
                        }
                    });
                if let Some(candidate) = found.get(selected) {
                    ui.separator();
                    let text = candidate
                        .description
                        .clone()
                        .unwrap_or_else(|| format!("{} (no description)", candidate.kind.tag()));
                    ui.label(RichText::new(text).small());
                }
                ui.label(
                    RichText::new("↑ ↓ select · Enter add · Esc close")
                        .small()
                        .weak(),
                );
            });
        });
    // A click outside closes the popup.
    let clicked_outside = ctx.input(|i| {
        i.pointer.any_pressed()
            && i.pointer
                .interact_pos()
                .is_some_and(|p| !area.response.rect.contains(p))
    });
    if clicked_outside && choice == AddChoice::Open {
        choice = AddChoice::Cancel;
    }
    choice
}

/// One row: the category colour, the type ID and the kind.
fn row(
    ui: &mut egui::Ui,
    candidate: &AddCandidate,
    selected: bool,
    palette: &CanvasPalette,
) -> egui::Response {
    let (rect, response) =
        ui.allocate_exact_size(egui::vec2(ui.available_width(), ROW_HEIGHT), Sense::click());
    let visuals = ui.visuals();
    if selected {
        ui.painter().rect(
            rect,
            CornerRadius::same(4),
            visuals.selection.bg_fill,
            Stroke::new(1.0, visuals.selection.stroke.color),
            egui::StrokeKind::Inside,
        );
    } else if response.hovered() {
        ui.painter().rect_filled(
            rect,
            CornerRadius::same(4),
            visuals.widgets.hovered.weak_bg_fill,
        );
    }
    let color = palette.category(category_of_kind(candidate.kind));
    let swatch = egui::Rect::from_center_size(
        egui::pos2(rect.min.x + 12.0, rect.center().y),
        egui::vec2(10.0, 10.0),
    );
    ui.painter()
        .rect_filled(swatch, CornerRadius::same(2), color);
    let text_color = visuals.text_color();
    ui.painter().text(
        egui::pos2(rect.min.x + 24.0, rect.center().y),
        Align2::LEFT_CENTER,
        &candidate.id,
        egui::TextStyle::Body.resolve(ui.style()),
        text_color,
    );
    let tag = if candidate.builtin {
        format!("{} · built-in", candidate.kind.tag())
    } else {
        candidate.kind.tag().to_owned()
    };
    ui.painter().text(
        egui::pos2(rect.max.x - 8.0, rect.center().y),
        Align2::RIGHT_CENTER,
        tag,
        egui::TextStyle::Small.resolve(ui.style()),
        visuals.weak_text_color(),
    );
    response.widget_info(|| {
        egui::WidgetInfo::selected(
            egui::WidgetType::SelectableLabel,
            true,
            selected,
            &candidate.id,
        )
    });
    response
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
        let ids: Vec<&str> = matches("", &list).iter().map(|c| c.id.as_str()).collect();
        assert_eq!(ids, ["Grab", "MoveBase", "AlwaysSuccess", "Sequence"]);
        let ids: Vec<&str> = matches("seq", &list)
            .iter()
            .map(|c| c.id.as_str())
            .collect();
        assert_eq!(ids, ["Sequence"]);
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
