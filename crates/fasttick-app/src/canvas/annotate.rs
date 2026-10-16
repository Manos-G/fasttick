//! Groups and canvas notes on the canvas (see
//! `fasttick_core::bt::annotations` for the XML form).
//!
//! A **group** is a box around some nodes of a tree. Its header shows the
//! title and the description. This module:
//!
//! - measures the header text ([`AnnotationText::measure`]) and wraps it
//!   to the width of the box;
//! - gives the layout the extra space around the members ([`padding`]):
//!   the margin, and the header above the top members. So the cards of
//!   other nodes stay out of the box;
//! - computes the box of each group ([`group_boxes`]): the cards of the
//!   members, plus the margin, plus the header;
//! - finds the nodes in a box that the user drags to a new size
//!   ([`snap_members`]);
//! - finds the part of a group or a note under the pointer
//!   ([`hit_group`], [`hit_note`]).
//!
//! A **canvas note** is a text box at a free place. Its box comes from the
//! XML; it grows down when the text needs more lines ([`note_boxes`]).

use std::collections::{BTreeMap, HashMap};

use egui::{Pos2, Rect, Vec2, pos2, vec2};

use super::card::{ELLIPSIS, TextKind, TextMeasure};
use super::layout::LayoutDirection;
use super::model::{CanvasNodeId, CanvasTree, NodeSource};

/// The space between the cards of the members and the border of the box
/// (world units).
pub const MARGIN: f32 = 14.0;
/// The space around the text of a header or a note.
pub const TEXT_PAD: f32 = 8.0;
/// The line height of a group title.
pub const TITLE_LINE: f32 = 17.0;
/// The line height of a description or of note text.
pub const TEXT_LINE: f32 = 15.0;
/// The most lines of a group description. A longer description ends with
/// an ellipsis; the inspector and the tooltip show all of it.
pub const MAX_DESCRIPTION_LINES: usize = 4;
/// The font of a group title.
pub const TITLE_KIND: TextKind = TextKind::Label;
/// The font of a description and of note text.
pub const TEXT_KIND: TextKind = TextKind::Type;
/// The smallest size of a group box while the user drags it.
pub const MIN_DRAG_SIZE: Vec2 = vec2(48.0, 40.0);

/// A word of a text, or a line break.
#[derive(Debug, Clone, PartialEq)]
enum Token {
    Word(String, f32),
    Break,
}

/// Measured text that can wrap to any width: a title line and a body.
#[derive(Debug, Clone, PartialEq, Default)]
pub struct AnnotationText {
    title: Vec<(char, f32)>,
    body: Vec<Token>,
    space: f32,
    title_ellipsis: f32,
    body_ellipsis: f32,
}

impl AnnotationText {
    /// Measures a title (one line, [`TITLE_KIND`]) and a body (words and
    /// line breaks, [`TEXT_KIND`]).
    pub fn measure(title: &str, body: &str, measure: &mut dyn TextMeasure) -> AnnotationText {
        let title = title
            .chars()
            .map(|c| if c.is_control() { ' ' } else { c })
            .map(|c| (c, measure.char_width(c, TITLE_KIND)))
            .collect();
        let mut tokens = Vec::new();
        for (index, paragraph) in body.split('\n').enumerate() {
            if index > 0 {
                tokens.push(Token::Break);
            }
            for word in paragraph.split_whitespace() {
                tokens.push(Token::Word(word.to_owned(), measure.width(word, TEXT_KIND)));
            }
        }
        // No break at the end.
        while tokens.last() == Some(&Token::Break) {
            tokens.pop();
        }
        AnnotationText {
            title,
            body: tokens,
            space: measure.width(" ", TEXT_KIND),
            title_ellipsis: measure.width(ELLIPSIS, TITLE_KIND),
            body_ellipsis: measure.width(ELLIPSIS, TEXT_KIND),
        }
    }

    /// The width of the whole title.
    pub fn title_width(&self) -> f32 {
        self.title.iter().map(|(_, w)| w).sum()
    }

    /// The title, shortened with an ellipsis to `width`.
    pub fn title_line(&self, width: f32) -> String {
        if self.title_width() <= width {
            return self.title.iter().map(|(c, _)| c).collect();
        }
        let budget = width - self.title_ellipsis;
        let mut used = 0.0;
        let mut out = String::new();
        for (c, w) in &self.title {
            if used + w > budget {
                break;
            }
            used += w;
            out.push(*c);
        }
        out.push_str(ELLIPSIS);
        out
    }

    /// The body, wrapped to `width`, with at most `max_lines` lines (the
    /// last line of a longer text ends with an ellipsis). A word that is
    /// wider than `width` gets a line of its own.
    pub fn body_lines(&self, width: f32, max_lines: usize) -> Vec<String> {
        let space = self.space;
        let line_width = |line: &[(&str, f32)]| -> f32 {
            line.iter().map(|(_, w)| w).sum::<f32>() + space * line.len().saturating_sub(1) as f32
        };
        let mut lines: Vec<Vec<(&str, f32)>> = Vec::new();
        let mut current: Option<Vec<(&str, f32)>> = None;
        for token in &self.body {
            match token {
                Token::Break => lines.push(current.take().unwrap_or_default()),
                Token::Word(word, w) => match current.as_mut() {
                    Some(line) if line_width(line) + space + w <= width => {
                        line.push((word, *w));
                    }
                    _ => {
                        lines.extend(current.take());
                        current = Some(vec![(word.as_str(), *w)]);
                    }
                },
            }
            if lines.len() > max_lines {
                break;
            }
        }
        lines.extend(current);
        let cut = lines.len() > max_lines;
        lines.truncate(max_lines);
        if cut && let Some(last) = lines.last_mut() {
            // Make room for the ellipsis.
            while last.len() > 1 && line_width(last) + self.body_ellipsis > width {
                last.pop();
            }
        }
        let count = lines.len();
        lines
            .into_iter()
            .enumerate()
            .map(|(index, line)| {
                let mut text = line.iter().map(|(w, _)| *w).collect::<Vec<_>>().join(" ");
                if cut && index + 1 == count {
                    text.push_str(ELLIPSIS);
                }
                text
            })
            .collect()
    }

    /// True if the body has no words.
    pub fn body_is_empty(&self) -> bool {
        !self.body.iter().any(|t| matches!(t, Token::Word(..)))
    }
}

/// The height of a group header with `lines` description lines.
pub fn header_height(lines: usize) -> f32 {
    let body = if lines == 0 {
        0.0
    } else {
        lines as f32 * TEXT_LINE + 2.0
    };
    TEXT_PAD + TITLE_LINE + body + TEXT_PAD * 0.5
}

/// The measured header text of each group of a tree, by group ID.
pub fn measure_groups(
    tree: &CanvasTree,
    measure: &mut dyn TextMeasure,
) -> HashMap<u32, AnnotationText> {
    tree.groups
        .iter()
        .map(|g| {
            (
                g.id,
                AnnotationText::measure(&g.title, &g.description, measure),
            )
        })
        .collect()
}

/// The measured text of each canvas note of a tree, by note ID.
pub fn measure_notes(
    tree: &CanvasTree,
    measure: &mut dyn TextMeasure,
) -> HashMap<u32, AnnotationText> {
    tree.notes
        .iter()
        .map(|n| (n.id, AnnotationText::measure("", &n.text, measure)))
        .collect()
}

/// The members of each group that the canvas shows, in tree order: the
/// nodes in the group, and the folded nodes that hide members.
pub fn group_members(tree: &CanvasTree) -> BTreeMap<u32, Vec<CanvasNodeId>> {
    let mut members: BTreeMap<u32, Vec<CanvasNodeId>> = BTreeMap::new();
    for (index, node) in tree.nodes().iter().enumerate() {
        if node.inline {
            continue;
        }
        let id = CanvasNodeId(index as u32);
        for group in node.group.iter().chain(&node.hidden_groups) {
            members.entry(*group).or_default().push(id);
        }
    }
    members
}

/// The union of the cards of some nodes.
fn content_of(rects: &[Rect], members: &[CanvasNodeId]) -> Rect {
    members
        .iter()
        .filter_map(|id| rects.get(id.index()))
        .fold(Rect::NOTHING, |b, r| b.union(*r))
}

/// The box around the cards in `content`, with a header of `header` units.
pub fn box_for_content(content: Rect, header: f32) -> Rect {
    let r = content.expand(MARGIN);
    Rect::from_min_max(pos2(r.min.x, r.min.y - header), r.max)
}

/// The width of the text in a box of width `box_width`.
fn text_width(box_width: f32) -> f32 {
    (box_width - 2.0 * TEXT_PAD).max(1.0)
}

/// The header height of a group for a box of width `box_width`.
fn header_for(text: Option<&AnnotationText>, box_width: f32) -> f32 {
    let lines = text.map_or(0, |t| {
        t.body_lines(text_width(box_width), MAX_DESCRIPTION_LINES)
            .len()
    });
    header_height(lines)
}

/// One group box on the canvas.
#[derive(Debug, Clone, PartialEq)]
pub struct GroupBox {
    /// The group ID.
    pub id: u32,
    /// The whole box (world units).
    pub rect: Rect,
    /// The header: the top part of the box with the title and the
    /// description.
    pub header: Rect,
    /// The title, shortened to the width of the box.
    pub title: String,
    /// The description lines.
    pub lines: Vec<String>,
    /// The color name, if set.
    pub color: Option<String>,
    /// The members on the canvas.
    pub members: Vec<CanvasNodeId>,
}

/// The boxes of the groups of a tree that have members on the canvas, the
/// largest first (the canvas draws them in this order, so that a small box
/// on a large one stays visible).
pub fn group_boxes(
    tree: &CanvasTree,
    rects: &[Rect],
    texts: &HashMap<u32, AnnotationText>,
) -> Vec<GroupBox> {
    let members = group_members(tree);
    let mut boxes: Vec<GroupBox> = tree
        .groups
        .iter()
        .filter_map(|group| {
            let list = members.get(&group.id)?;
            let content = content_of(rects, list);
            if !content.is_finite() {
                return None;
            }
            let text = texts.get(&group.id);
            let width = content.width() + 2.0 * MARGIN;
            let header = header_for(text, width);
            let rect = box_for_content(content, header);
            let inner = text_width(rect.width());
            Some(GroupBox {
                id: group.id,
                rect,
                header: Rect::from_min_size(rect.min, vec2(rect.width(), header)),
                title: text.map_or_else(|| group.title.clone(), |t| t.title_line(inner)),
                lines: text.map_or_else(Vec::new, |t| t.body_lines(inner, MAX_DESCRIPTION_LINES)),
                color: group.color.clone(),
                members: list.clone(),
            })
        })
        .collect();
    boxes.sort_by(|a, b| b.rect.area().total_cmp(&a.rect.area()));
    boxes
}

/// Extra space around each card for the layout, `[left, top, right,
/// bottom]` (world units), so that the cards of other nodes stay out of
/// the group boxes.
///
/// The layout is layered: all cards of one level of one tree start at the
/// same place on the depth axis. So the space on the depth axis applies to
/// whole levels: the header and the margin before the first level of a
/// group, the margin after its last level. On the breadth axis, the first
/// and the last member of each level get the margin (and, in the
/// left-right layout, the first one also gets the header).
///
/// `header` gives the header height of each group.
pub fn padding(
    tree: &CanvasTree,
    direction: LayoutDirection,
    header: &HashMap<u32, f32>,
) -> Vec<[f32; 4]> {
    let members = group_members(tree);
    let nodes = tree.nodes();
    let mut pad = vec![[0.0_f32; 4]; nodes.len()];
    if members.is_empty() {
        return pad;
    }
    let root_of: Vec<usize> = (0..nodes.len())
        .map(|mut index| {
            while let Some(parent) = nodes[index].parent {
                index = parent.index();
            }
            index
        })
        .collect();
    // The sides: [left, top, right, bottom].
    let (depth_min, depth_max, breadth_min, breadth_max) = match direction {
        LayoutDirection::TopDown => (1, 3, 0, 2),
        LayoutDirection::LeftRight => (0, 2, 1, 3),
    };
    let top_down = direction == LayoutDirection::TopDown;
    let mut level_pad: HashMap<(usize, u32), [f32; 2]> = HashMap::new();
    for (group, list) in &members {
        let head = header
            .get(group)
            .copied()
            .unwrap_or_else(|| header_height(0));
        // Per tree of the forest: the first and the last level.
        let mut depths: HashMap<usize, (u32, u32)> = HashMap::new();
        // Per level: the first and the last member (tree order is the
        // order along the breadth axis).
        let mut ends: BTreeMap<(usize, u32), (usize, usize)> = BTreeMap::new();
        for id in list {
            let index = id.index();
            let (root, depth) = (root_of[index], nodes[index].depth);
            let range = depths.entry(root).or_insert((depth, depth));
            range.0 = range.0.min(depth);
            range.1 = range.1.max(depth);
            let end = ends.entry((root, depth)).or_insert((index, index));
            end.1 = index;
        }
        for (root, (first, last)) in depths {
            let start = level_pad.entry((root, first)).or_default();
            start[0] = start[0].max(if top_down { head + MARGIN } else { MARGIN });
            let end = level_pad.entry((root, last)).or_default();
            end[1] = end[1].max(MARGIN);
        }
        for (first, last) in ends.values() {
            let lead = if top_down { MARGIN } else { head + MARGIN };
            pad[*first][breadth_min] = pad[*first][breadth_min].max(lead);
            pad[*last][breadth_max] = pad[*last][breadth_max].max(MARGIN);
        }
    }
    for (index, node) in nodes.iter().enumerate() {
        if let Some([before, after]) = level_pad.get(&(root_of[index], node.depth)) {
            pad[index][depth_min] = pad[index][depth_min].max(*before);
            pad[index][depth_max] = pad[index][depth_max].max(*after);
        }
    }
    pad
}

/// The space (world units) between a group box and a card that the
/// layout pushed out of it.
pub const PUSH_GAP: f32 = 12.0;

/// Finds the cards of other nodes that are in a group box, before the
/// first member or after the last member of their level (along the
/// breadth axis), and adds space on that side of that member: the next
/// layout puts the card out of the box. A card between two members of its
/// level stays (the box is not convex there, or another group is nested
/// in this one). `extra` collects the space by `(node index, side)` (side:
/// 0 left, 1 top, 2 right, 3 bottom). Returns true if it added space.
pub fn push_out(
    tree: &CanvasTree,
    direction: LayoutDirection,
    rects: &[Rect],
    boxes: &[GroupBox],
    extra: &mut HashMap<(usize, usize), f32>,
) -> bool {
    let nodes = tree.nodes();
    let axis = direction.breadth_axis();
    let (side_min, side_max) = match direction {
        LayoutDirection::TopDown => (0, 2),
        LayoutDirection::LeftRight => (1, 3),
    };
    let root_of: Vec<usize> = (0..nodes.len())
        .map(|mut index| {
            while let Some(parent) = nodes[index].parent {
                index = parent.index();
            }
            index
        })
        .collect();
    // The space of this pass: the largest push for each side of each card
    // (several cards can push the same side). Two boxes push each other
    // once.
    let mut pass: HashMap<(usize, usize), f32> = HashMap::new();
    let mut pairs: std::collections::HashSet<(u32, u32)> = std::collections::HashSet::new();
    let member_sets: Vec<std::collections::HashSet<usize>> = boxes
        .iter()
        .map(|b| b.members.iter().map(|m| m.index()).collect())
        .collect();
    // The box of each card that is a member of a group.
    let mut owner: Vec<Option<usize>> = vec![None; nodes.len()];
    for (index, group) in boxes.iter().enumerate() {
        for member in &group.members {
            if let Some(slot) = owner.get_mut(member.index()) {
                slot.get_or_insert(index);
            }
        }
    }
    for (group_index, group) in boxes.iter().enumerate() {
        let mut ends: HashMap<(usize, u32), (usize, usize)> = HashMap::new();
        for id in &group.members {
            let index = id.index();
            let Some(node) = nodes.get(index) else {
                continue;
            };
            let end = ends
                .entry((root_of[index], node.depth))
                .or_insert((index, index));
            end.0 = end.0.min(index);
            end.1 = end.1.max(index);
        }
        let area = group.rect.expand(PUSH_GAP - 1.0);
        for (index, node) in nodes.iter().enumerate() {
            let Some(card) = rects.get(index) else {
                continue;
            };
            if member_sets[group_index].contains(&index) {
                continue;
            }
            // A member of another group: that box must stay out.
            let other = owner[index]
                .filter(|o| *o != group_index)
                .map(|o| &boxes[o]);
            if other.is_none() && !card.intersects(area) {
                continue;
            }
            let rect = other.map_or(*card, |other| other.rect);
            if !rect.intersects(area) {
                continue;
            }
            let pair = other.map(|o| (o.id.min(group.id), o.id.max(group.id)));
            if pair.is_some_and(|p| pairs.contains(&p)) {
                continue;
            }
            let Some(&(first, last)) = ends.get(&(root_of[index], node.depth)) else {
                continue;
            };
            let (member, side, overlap) = if index < first {
                (
                    first,
                    side_min,
                    rect.max[axis] - group.rect.min[axis] + PUSH_GAP,
                )
            } else if index > last {
                (
                    last,
                    side_max,
                    group.rect.max[axis] - rect.min[axis] + PUSH_GAP,
                )
            } else {
                continue;
            };
            if overlap > 0.5 {
                let push = pass.entry((member, side)).or_default();
                *push = push.max(overlap);
                if let Some(pair) = pair {
                    pairs.insert(pair);
                }
            }
        }
    }
    for (key, push) in &pass {
        *extra.entry(*key).or_default() += push;
    }
    !pass.is_empty()
}

/// True if a node can join a group by a resize: a node of the tree (not
/// inline) that is in no group or in this group.
fn can_join(tree: &CanvasTree, id: CanvasNodeId, group: u32) -> bool {
    tree.get(id).is_some_and(|node| {
        !node.inline
            && matches!(node.source, NodeSource::Xml(..))
            && node.group.is_none_or(|g| g == group)
    })
}

/// The members of group `group` after the user dragged its box to
/// `area`: the cards whose centre is in the area; then, while the box
/// around them (with the margin and the header) has the centre of one more
/// card that the layout cannot move out of the box, that card too. Cards
/// of other groups are not taken. The result is in tree order; it is empty
/// if no card is in the area.
///
/// The layout moves a card out of a box when the card is before the first
/// or after the last member of its level (see [`push_out`]). It cannot do
/// that for a card between two members of its level, for a card above a
/// member (its parent, for example), or for a card on a level without
/// members.
pub fn snap_members(
    tree: &CanvasTree,
    rects: &[Rect],
    group: u32,
    area: Rect,
    text: Option<&AnnotationText>,
) -> Vec<CanvasNodeId> {
    let nodes = tree.nodes();
    let candidates: Vec<CanvasNodeId> =
        tree.ids().filter(|id| can_join(tree, *id, group)).collect();
    let centre = |id: &CanvasNodeId| rects.get(id.index()).map(Rect::center);
    let root_of = |mut index: usize| {
        while let Some(parent) = nodes[index].parent {
            index = parent.index();
        }
        index
    };
    let mut inside: Vec<bool> = candidates
        .iter()
        .map(|id| centre(id).is_some_and(|c| area.contains(c)))
        .collect();
    // Each step takes at least one card; a few steps are enough in
    // practice.
    for _ in 0..candidates.len().min(64) {
        let members: Vec<CanvasNodeId> = candidates
            .iter()
            .zip(&inside)
            .filter(|(_, inside)| **inside)
            .map(|(id, _)| *id)
            .collect();
        if members.is_empty() {
            return members;
        }
        let content = content_of(rects, &members);
        let snapped = box_for_content(content, header_for(text, content.width() + 2.0 * MARGIN));
        // The first and the last member of each level, and the ancestors
        // of the members.
        let mut ends: HashMap<(usize, u32), (usize, usize)> = HashMap::new();
        let mut above: std::collections::HashSet<usize> = std::collections::HashSet::new();
        for id in &members {
            let index = id.index();
            let end = ends
                .entry((root_of(index), nodes[index].depth))
                .or_insert((index, index));
            end.0 = end.0.min(index);
            end.1 = end.1.max(index);
            let mut current = nodes[index].parent;
            while let Some(parent) = current {
                if !above.insert(parent.index()) {
                    break;
                }
                current = nodes[parent.index()].parent;
            }
        }
        let mut added = false;
        for (position, id) in candidates.iter().enumerate() {
            if inside[position] || !centre(id).is_some_and(|c| snapped.contains(c)) {
                continue;
            }
            let index = id.index();
            let stays = match ends.get(&(root_of(index), nodes[index].depth)) {
                None => true,
                Some(&(first, last)) => (first < index && index < last) || above.contains(&index),
            };
            if stays {
                inside[position] = true;
                added = true;
            }
        }
        if !added {
            return members;
        }
    }
    candidates
        .into_iter()
        .zip(inside)
        .filter(|(_, inside)| *inside)
        .map(|(id, _)| id)
        .collect()
}

/// The box that a group with these members gets.
pub fn snapped_box(
    rects: &[Rect],
    members: &[CanvasNodeId],
    text: Option<&AnnotationText>,
) -> Rect {
    let content = content_of(rects, members);
    box_for_content(content, header_for(text, content.width() + 2.0 * MARGIN))
}

/// The sides of a box that a drag moves.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct Sides {
    /// The left side.
    pub left: bool,
    /// The top side.
    pub top: bool,
    /// The right side.
    pub right: bool,
    /// The bottom side.
    pub bottom: bool,
}

impl Sides {
    /// True if no side moves.
    pub fn is_empty(self) -> bool {
        !(self.left || self.top || self.right || self.bottom)
    }

    /// The box after the drag: the moving sides move by `delta`. The box
    /// keeps at least `min` size.
    pub fn apply(self, rect: Rect, delta: Vec2, min: Vec2) -> Rect {
        let mut r = rect;
        if self.left {
            r.min.x = (rect.min.x + delta.x).min(rect.max.x - min.x);
        }
        if self.right {
            r.max.x = (rect.max.x + delta.x).max(rect.min.x + min.x);
        }
        if self.top {
            r.min.y = (rect.min.y + delta.y).min(rect.max.y - min.y);
        }
        if self.bottom {
            r.max.y = (rect.max.y + delta.y).max(rect.min.y + min.y);
        }
        r
    }

    /// The pointer for these sides.
    pub fn cursor(self) -> egui::CursorIcon {
        use egui::CursorIcon;
        match (self.left, self.top, self.right, self.bottom) {
            (true, true, ..) | (_, _, true, true) => CursorIcon::ResizeNwSe,
            (true, _, _, true) | (_, true, true, _) => CursorIcon::ResizeNeSw,
            (true, ..) | (_, _, true, _) => CursorIcon::ResizeHorizontal,
            _ => CursorIcon::ResizeVertical,
        }
    }
}

/// The sides of `rect` that are within `grab` of `p` (empty if `p` is not
/// near the border). A corner gives two sides.
pub fn sides_at(rect: Rect, p: Pos2, grab: f32) -> Sides {
    if !rect.expand(grab).contains(p) {
        return Sides::default();
    }
    // Near a corner, a larger area counts, so that corners are easy to hit.
    let corner = grab * 1.6;
    let near = |a: f32, b: f32, g: f32| (a - b).abs() <= g;
    let mut sides = Sides {
        left: near(p.x, rect.min.x, grab),
        top: near(p.y, rect.min.y, grab),
        right: near(p.x, rect.max.x, grab),
        bottom: near(p.y, rect.max.y, grab),
    };
    let horizontal = near(p.x, rect.min.x, corner) || near(p.x, rect.max.x, corner);
    let vertical = near(p.y, rect.min.y, corner) || near(p.y, rect.max.y, corner);
    if horizontal && vertical {
        sides.left |= near(p.x, rect.min.x, corner);
        sides.right |= near(p.x, rect.max.x, corner);
        sides.top |= near(p.y, rect.min.y, corner);
        sides.bottom |= near(p.y, rect.max.y, corner);
    }
    // A thin box: one side of each axis.
    if sides.left && sides.right {
        sides.right = (p.x - rect.max.x).abs() < (p.x - rect.min.x).abs();
        sides.left = !sides.right;
    }
    if sides.top && sides.bottom {
        sides.bottom = (p.y - rect.max.y).abs() < (p.y - rect.min.y).abs();
        sides.top = !sides.bottom;
    }
    sides
}

/// A part of a group box.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum GroupPart {
    /// The header (title and description).
    Header,
    /// The border: a drag resizes the box.
    Border(Sides),
}

/// The group part under `p`: the border first (within `grab`), then the
/// header. The smallest box wins (it is drawn on top).
pub fn hit_group(boxes: &[GroupBox], p: Pos2, grab: f32) -> Option<(u32, GroupPart)> {
    for group in boxes.iter().rev() {
        let sides = sides_at(group.rect, p, grab);
        if !sides.is_empty() {
            return Some((group.id, GroupPart::Border(sides)));
        }
        if group.header.contains(p) {
            return Some((group.id, GroupPart::Header));
        }
    }
    None
}

/// One canvas note on the canvas.
#[derive(Debug, Clone, PartialEq)]
pub struct NoteBox {
    /// The note ID.
    pub id: u32,
    /// The box (world units). It is at least as high as the text.
    pub rect: Rect,
    /// The box in the XML.
    pub stored: Rect,
    /// The text lines.
    pub lines: Vec<String>,
    /// The color name, if set.
    pub color: Option<String>,
}

/// The boxes of the canvas notes of a tree, in ID order.
pub fn note_boxes(tree: &CanvasTree, texts: &HashMap<u32, AnnotationText>) -> Vec<NoteBox> {
    tree.notes
        .iter()
        .map(|note| {
            let [x, y, w, h] = note.rect;
            let stored = Rect::from_min_size(pos2(x, y), vec2(w, h));
            let lines = texts
                .get(&note.id)
                .map_or_else(Vec::new, |t| t.body_lines(text_width(w), usize::MAX));
            let needed = 2.0 * TEXT_PAD + lines.len().max(1) as f32 * TEXT_LINE;
            NoteBox {
                id: note.id,
                rect: Rect::from_min_size(stored.min, vec2(w, h.max(needed))),
                stored,
                lines,
                color: note.color.clone(),
            }
        })
        .collect()
}

/// A part of a note.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum NotePart {
    /// The inside: a drag moves the note.
    Body,
    /// The border: a drag resizes the note.
    Border(Sides),
}

/// The note part under `p`. The last note wins (it is drawn on top).
pub fn hit_note(notes: &[NoteBox], p: Pos2, grab: f32) -> Option<(u32, NotePart)> {
    for note in notes.iter().rev() {
        let sides = sides_at(note.rect, p, grab);
        if !sides.is_empty() {
            return Some((note.id, NotePart::Border(sides)));
        }
        if note.rect.contains(p) {
            return Some((note.id, NotePart::Body));
        }
    }
    None
}

#[cfg(test)]
mod tests {
    use fasttick_core::bt::annotations::GroupInfo;
    use fasttick_core::project::FileId;
    use fasttick_core::xml::{Document, NodeId};

    use super::*;
    use crate::canvas::card::FixedMeasure;
    use crate::canvas::model::{CanvasNode, CanvasTreeBuilder};

    /// Six element IDs of a document.
    fn ids() -> Vec<NodeId> {
        let doc = Document::parse("<r><n/><n/><n/><n/><n/><n/></r>").unwrap();
        let root = doc.document_element().unwrap();
        doc.element_children(root).collect()
    }

    fn node(n: u32) -> CanvasNode {
        let id = ids()[n as usize - 1];
        CanvasNode::new(NodeSource::Xml(FileId(0), id), format!("n{n}"), "A")
    }

    /// root -> a, b, c; b -> d, e. a and b are in group 1.
    fn tree() -> CanvasTree {
        let mut b = CanvasTreeBuilder::new();
        let root = b.push(None, node(1));
        let mut a = node(2);
        a.group = Some(1);
        b.push(Some(root), a);
        let mut bb = node(3);
        bb.group = Some(1);
        let b_id = b.push(Some(root), bb);
        b.push(Some(root), node(4));
        b.push(Some(b_id), node(5));
        b.push(Some(b_id), node(6));
        let mut tree = b.finish("T");
        tree.groups = vec![GroupInfo {
            id: 1,
            title: "Pick".into(),
            description: "one two three four".into(),
            color: None,
        }];
        tree
    }

    #[test]
    fn wraps_text() {
        let text = AnnotationText::measure(
            "Long title",
            "aa bb cc\ndd ee ff gg",
            &mut FixedMeasure(10.0),
        );
        assert_eq!(text.title_width(), 100.0);
        assert_eq!(text.title_line(100.0), "Long title");
        assert_eq!(text.title_line(60.0), "Long \u{2026}");
        assert_eq!(text.body_lines(80.0, 10), ["aa bb cc", "dd ee ff", "gg"]);
        assert_eq!(text.body_lines(50.0, 10), ["aa bb", "cc", "dd ee", "ff gg"]);
        assert_eq!(text.body_lines(50.0, 2), ["aa bb", "cc\u{2026}"]);
        // A long word gets a line of its own.
        let long = AnnotationText::measure("", "x abcdefghij y", &mut FixedMeasure(10.0));
        assert_eq!(long.body_lines(40.0, 10), ["x", "abcdefghij", "y"]);
        // Empty lines stay; the end has no empty line.
        let empty = AnnotationText::measure("", "a\n\nb\n\n", &mut FixedMeasure(10.0));
        assert_eq!(empty.body_lines(40.0, 10), ["a", "", "b"]);
        assert!(AnnotationText::measure("t", " \n ", &mut FixedMeasure(1.0)).body_is_empty());
    }

    #[test]
    fn padding_top_down() {
        let tree = tree();
        let header = HashMap::from([(1, 40.0)]);
        let pad = padding(&tree, LayoutDirection::TopDown, &header);
        // Level 1 (a, b, c): the header and the margin above.
        for index in [1, 2, 3] {
            assert_eq!(pad[index][1], 40.0 + MARGIN, "node {index}");
            assert_eq!(pad[index][3], MARGIN, "node {index}");
        }
        // a is the first member, b the last.
        assert_eq!(pad[1][0], MARGIN);
        assert_eq!(pad[1][2], 0.0);
        assert_eq!(pad[2][2], MARGIN);
        assert_eq!(pad[3], [0.0, 40.0 + MARGIN, 0.0, MARGIN]);
        // Other levels: nothing.
        assert_eq!(pad[0], [0.0; 4]);
        assert_eq!(pad[4], [0.0; 4]);
    }

    #[test]
    fn padding_left_right() {
        let tree = tree();
        let header = HashMap::from([(1, 40.0)]);
        let pad = padding(&tree, LayoutDirection::LeftRight, &header);
        // The depth axis is x: the margin before and after the level.
        assert_eq!(pad[3][0], MARGIN);
        assert_eq!(pad[3][2], MARGIN);
        // The first member gets the header above it.
        assert_eq!(pad[1][1], 40.0 + MARGIN);
        assert_eq!(pad[2][3], MARGIN);
    }

    fn rects() -> Vec<Rect> {
        // root, a, b, c, d, e
        [
            (100.0, 0.0),
            (0.0, 100.0),
            (100.0, 100.0),
            (200.0, 100.0),
            (60.0, 200.0),
            (140.0, 200.0),
        ]
        .iter()
        .map(|(x, y)| Rect::from_min_size(pos2(*x, *y), vec2(80.0, 40.0)))
        .collect()
    }

    #[test]
    fn group_box_geometry() {
        let tree = tree();
        let texts = measure_groups(&tree, &mut FixedMeasure(6.0));
        let boxes = group_boxes(&tree, &rects(), &texts);
        assert_eq!(boxes.len(), 1);
        let group = &boxes[0];
        assert_eq!(group.members, [CanvasNodeId(1), CanvasNodeId(2)]);
        // The cards of a and b: x 0..180, y 100..140.
        assert_eq!(group.rect.min.x, -MARGIN);
        assert_eq!(group.rect.max.x, 180.0 + MARGIN);
        assert_eq!(group.rect.max.y, 140.0 + MARGIN);
        // The description fits on one line (18 chars * 6 = 108 < 192).
        assert_eq!(group.lines, ["one two three four"]);
        assert_eq!(group.rect.min.y, 100.0 - MARGIN - header_height(1));
        assert_eq!(group.header.height(), header_height(1));
        assert_eq!(group.title, "Pick");
    }

    #[test]
    fn snap_takes_the_cards_inside_and_closes_the_box() {
        let tree = tree();
        let rects = rects();
        let text = AnnotationText::measure("Pick", "", &mut FixedMeasure(6.0));
        // An area around the centres of a and d only.
        let area = Rect::from_min_max(pos2(30.0, 110.0), pos2(110.0, 230.0));
        let members = snap_members(&tree, &rects, 1, area, Some(&text));
        // a and d are in the area. The box around them (x -14..154) has
        // the centre of b (x 140): b is the parent of d, the layout cannot
        // move it out, so it joins. Then the box (x -14..194) has the
        // centre of e (x 180), but e is after the last member of its level
        // (d): the layout moves it out of the box, so it does not join.
        assert_eq!(members, [CanvasNodeId(1), CanvasNodeId(2), CanvasNodeId(4)]);
        // An empty area.
        let empty = Rect::from_min_size(pos2(1000.0, 1000.0), vec2(10.0, 10.0));
        assert!(snap_members(&tree, &rects, 1, empty, Some(&text)).is_empty());
    }

    #[test]
    fn snap_skips_other_groups_and_inline_nodes() {
        let mut tree = tree();
        tree.get_mut(CanvasNodeId(3)).unwrap().group = Some(2);
        tree.get_mut(CanvasNodeId(4)).unwrap().inline = true;
        let all = Rect::from_min_max(pos2(-100.0, -100.0), pos2(1000.0, 1000.0));
        let members = snap_members(&tree, &rects(), 1, all, None);
        assert_eq!(
            members,
            [
                CanvasNodeId(0),
                CanvasNodeId(1),
                CanvasNodeId(2),
                CanvasNodeId(5)
            ]
        );
    }

    #[test]
    fn sides_and_hits() {
        let rect = Rect::from_min_max(pos2(0.0, 0.0), pos2(100.0, 80.0));
        assert!(sides_at(rect, pos2(50.0, 40.0), 4.0).is_empty());
        assert!(sides_at(rect, pos2(200.0, 40.0), 4.0).is_empty());
        let left = sides_at(rect, pos2(2.0, 40.0), 4.0);
        assert!(left.left && !left.top && !left.right && !left.bottom);
        let corner = sides_at(rect, pos2(98.0, 77.0), 4.0);
        assert!(corner.right && corner.bottom);
        // Near the corner (within 1.6 * grab of both sides).
        let near_corner = sides_at(rect, pos2(103.0, 74.0), 4.0);
        assert!(near_corner.right && near_corner.bottom);
        assert_eq!(left.cursor(), egui::CursorIcon::ResizeHorizontal);
        assert_eq!(corner.cursor(), egui::CursorIcon::ResizeNwSe);
        let moved = corner.apply(rect, vec2(20.0, -100.0), MIN_DRAG_SIZE);
        assert_eq!(moved.max, pos2(120.0, MIN_DRAG_SIZE.y));

        let tree = tree();
        let boxes = group_boxes(&tree, &rects(), &HashMap::new());
        let group = &boxes[0];
        let header_point = group.header.center();
        assert_eq!(
            hit_group(&boxes, header_point, 4.0),
            Some((1, GroupPart::Header))
        );
        assert!(matches!(
            hit_group(&boxes, pos2(group.rect.min.x, group.rect.center().y), 4.0),
            Some((1, GroupPart::Border(s))) if s.left
        ));
        assert_eq!(hit_group(&boxes, pos2(500.0, 500.0), 4.0), None);
    }

    #[test]
    fn notes_grow_to_their_text() {
        let mut tree = tree();
        tree.notes = vec![fasttick_core::bt::annotations::CanvasNoteInfo {
            id: 3,
            text: "aa bb cc dd".into(),
            rect: [10.0, 20.0, 66.0, 20.0],
            color: None,
        }];
        let texts = measure_notes(&tree, &mut FixedMeasure(10.0));
        let notes = note_boxes(&tree, &texts);
        // Width 66 - 16 = 50: "aa bb", "cc dd".
        assert_eq!(notes[0].lines, ["aa bb", "cc dd"]);
        assert_eq!(notes[0].rect.height(), 2.0 * TEXT_PAD + 2.0 * TEXT_LINE);
        assert_eq!(notes[0].stored.height(), 20.0);
        assert_eq!(
            hit_note(&notes, pos2(40.0, 40.0), 3.0),
            Some((3, NotePart::Body))
        );
        assert!(matches!(
            hit_note(&notes, pos2(76.0, 40.0), 3.0),
            Some((3, NotePart::Border(s))) if s.right
        ));
    }
}
