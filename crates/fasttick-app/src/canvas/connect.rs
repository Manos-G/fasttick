//! Add a child from the dot of a node.
//!
//! On an editable canvas, each node that can take one more child shows a
//! dot at the middle of its child side (the bottom edge in the top-down
//! layout, the right edge in the left-right layout). A drag from the dot
//! draws a connection. Where the user releases it:
//!
//! - on an unconnected node: that node becomes the last child;
//! - elsewhere: the app asks for the type of the new node (see
//!   [`crate::add_popup`]). The pointer position along the sibling axis
//!   selects the place among the children, so a release between two
//!   children puts the new node between them.
//!
//! A click on the dot also asks for a type; the new node is the last
//! child.

use egui::{Pos2, Rect, pos2};

use super::drop::{DropPlacement, DropSpot};
use super::layout::LayoutDirection;
use super::model::{CanvasNode, CanvasNodeId, CanvasTree, NodeCategory};

/// The radius of the dot, in world units.
pub const DOT_RADIUS: f32 = 7.0;

/// The distance (world units) of the centre of the dot outside the card
/// edge, so that the dot does not cover the last port line.
pub const DOT_OFFSET: f32 = 3.0;

/// The largest distance (screen points) from the centre of a dot at which
/// the pointer grabs it.
pub const DOT_GRAB: f32 = 11.0;

/// The radius of the dot on the screen at a zoom.
pub fn dot_radius_px(zoom: f32) -> f32 {
    (DOT_RADIUS * zoom).clamp(4.0, 10.0)
}

/// The distance (screen points) from the centre of a dot at which the
/// pointer grabs it at a zoom: a little more than the dot, so that a small
/// card can still be moved.
pub fn grab_px(zoom: f32) -> f32 {
    (dot_radius_px(zoom) + 4.0).min(DOT_GRAB)
}

/// True if the node shows a dot: it can take one more child in the file.
/// A decorator takes one child; a control takes any number (BT.CPP checks
/// the limits of some controls, and the edit shows its message). A SubTree
/// node and a node of another tree (inline) take none.
pub fn has_dot(node: &CanvasNode) -> bool {
    if node.inline || node.subtree.is_some() {
        return false;
    }
    match node.category {
        NodeCategory::Control | NodeCategory::Unknown => true,
        NodeCategory::Decorator => node.children.is_empty(),
        _ => false,
    }
}

/// The centre of the dot of a card, in world units: a little outside the
/// middle of the child side.
pub fn dot_center(rect: Rect, direction: LayoutDirection) -> Pos2 {
    match direction {
        LayoutDirection::TopDown => pos2(rect.center().x, rect.max.y + DOT_OFFSET),
        LayoutDirection::LeftRight => pos2(rect.max.x + DOT_OFFSET, rect.center().y),
    }
}

/// The node whose dot is nearest to `p` (world units), not further than
/// `grab` (world units).
pub fn dot_at(
    tree: &CanvasTree,
    rects: &[Rect],
    direction: LayoutDirection,
    p: Pos2,
    grab: f32,
) -> Option<CanvasNodeId> {
    let mut best: Option<(f32, CanvasNodeId)> = None;
    for (index, rect) in rects.iter().enumerate() {
        let distance = dot_center(*rect, direction).distance(p);
        if distance > grab || best.is_some_and(|(d, _)| d <= distance) {
            continue;
        }
        let id = CanvasNodeId(index as u32);
        if tree.get(id).is_some_and(has_dot) {
            best = Some((distance, id));
        }
    }
    best.map(|(_, id)| id)
}

/// The result of a connection from the dot of `parent`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ConnectEnd {
    /// Make this unconnected node the last child of the parent.
    Connect(CanvasNodeId),
    /// Ask for the type of a new node at this place.
    Add(DropSpot),
}

/// What a connection from the dot of `parent` does when it ends at `p`
/// (world units).
pub fn connect_end(
    tree: &CanvasTree,
    rects: &[Rect],
    direction: LayoutDirection,
    parent: CanvasNodeId,
    p: Pos2,
) -> ConnectEnd {
    if let Some(target) = super::hit::hit_node(rects, p, 0.0)
        && target != parent
        && tree
            .get(target)
            .is_some_and(|n| n.parent.is_none() && !n.inline)
        && !is_ancestor(tree, target, parent)
    {
        return ConnectEnd::Connect(target);
    }
    ConnectEnd::Add(add_spot(tree, rects, direction, parent, p))
}

/// The place of a new child of `parent` for a pointer at `p`: before the
/// first child that is after the pointer along the sibling axis, else the
/// last child.
pub fn add_spot(
    tree: &CanvasTree,
    rects: &[Rect],
    direction: LayoutDirection,
    parent: CanvasNodeId,
    p: Pos2,
) -> DropSpot {
    let along = |point: Pos2| match direction {
        LayoutDirection::TopDown => point.x,
        LayoutDirection::LeftRight => point.y,
    };
    let children = tree
        .get(parent)
        .map(|n| n.children.as_slice())
        .unwrap_or_default();
    let next = children.iter().copied().find(|child| {
        rects
            .get(child.0 as usize)
            .is_some_and(|r| along(r.center()) > along(p))
    });
    match next {
        Some(child) => DropSpot::Node {
            target: child,
            placement: DropPlacement::Before,
        },
        None => DropSpot::Node {
            target: parent,
            placement: DropPlacement::Inside,
        },
    }
}

/// True if `ancestor` is `node` or an ancestor of it.
fn is_ancestor(tree: &CanvasTree, ancestor: CanvasNodeId, node: CanvasNodeId) -> bool {
    let mut current = Some(node);
    while let Some(c) = current {
        if c == ancestor {
            return true;
        }
        current = tree.get(c).and_then(|n| n.parent);
    }
    false
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::canvas::model::{CanvasTreeBuilder, NodeSource};
    use egui::vec2;
    use fasttick_core::runtime::NodeIndex;

    fn node(index: usize, category: NodeCategory) -> CanvasNode {
        let mut n = CanvasNode::new(NodeSource::Runtime(NodeIndex(index)), "n", "T");
        n.category = category;
        n
    }

    /// A Sequence (0) with two actions (1, 2) side by side, an empty
    /// Inverter (3) under the Sequence, and an unconnected action (4).
    fn sample() -> (CanvasTree, Vec<Rect>) {
        let mut b = CanvasTreeBuilder::new();
        let root = b.push(None, node(0, NodeCategory::Control));
        b.push(Some(root), node(1, NodeCategory::Action));
        b.push(Some(root), node(2, NodeCategory::Action));
        b.push(Some(root), node(3, NodeCategory::Decorator));
        b.push(None, node(4, NodeCategory::Action));
        let tree = b.finish("T");
        let size = vec2(100.0, 40.0);
        let rects = vec![
            Rect::from_min_size(pos2(150.0, 0.0), size),
            Rect::from_min_size(pos2(0.0, 100.0), size),
            Rect::from_min_size(pos2(150.0, 100.0), size),
            Rect::from_min_size(pos2(300.0, 100.0), size),
            Rect::from_min_size(pos2(600.0, 0.0), size),
        ];
        (tree, rects)
    }

    #[test]
    fn containers_with_room_have_a_dot() {
        let (mut tree, rects) = sample();
        let ids: Vec<bool> = (0..5)
            .map(|i| has_dot(tree.get(CanvasNodeId(i)).unwrap()))
            .collect();
        assert_eq!(ids, [true, false, false, true, false]);
        // The dot is at the middle of the bottom edge, a little outside.
        assert_eq!(
            dot_center(rects[0], LayoutDirection::TopDown),
            pos2(200.0, 40.0 + DOT_OFFSET)
        );
        assert_eq!(
            dot_center(rects[0], LayoutDirection::LeftRight),
            pos2(250.0 + DOT_OFFSET, 20.0)
        );
        let at = |p| dot_at(&tree, &rects, LayoutDirection::TopDown, p, 10.0);
        assert_eq!(at(pos2(203.0, 44.0)), Some(CanvasNodeId(0)));
        assert_eq!(at(pos2(200.0, 60.0)), None);
        // A leaf has no dot.
        assert_eq!(at(pos2(50.0, 140.0)), None);
        // An inline node has no dot.
        tree.get_mut(CanvasNodeId(0)).unwrap().inline = true;
        assert_eq!(
            dot_at(
                &tree,
                &rects,
                LayoutDirection::TopDown,
                pos2(200.0, 40.0),
                10.0
            ),
            None
        );
    }

    #[test]
    fn the_release_point_selects_the_place() {
        let (tree, rects) = sample();
        let root = CanvasNodeId(0);
        let end = |p| connect_end(&tree, &rects, LayoutDirection::TopDown, root, p);
        // Left of the first child: before it.
        assert_eq!(
            end(pos2(10.0, 200.0)),
            ConnectEnd::Add(DropSpot::Node {
                target: CanvasNodeId(1),
                placement: DropPlacement::Before
            })
        );
        // Between the second and the third child.
        assert_eq!(
            end(pos2(280.0, 200.0)),
            ConnectEnd::Add(DropSpot::Node {
                target: CanvasNodeId(3),
                placement: DropPlacement::Before
            })
        );
        // After the last child: the last child.
        assert_eq!(
            end(pos2(500.0, 200.0)),
            ConnectEnd::Add(DropSpot::Node {
                target: root,
                placement: DropPlacement::Inside
            })
        );
        // On an unconnected node: connect it.
        assert_eq!(end(pos2(620.0, 20.0)), ConnectEnd::Connect(CanvasNodeId(4)));
        // On a node of the tree: a new node, not a move.
        assert!(matches!(end(pos2(20.0, 120.0)), ConnectEnd::Add(_)));
        // A connection from the Inverter to the root (its ancestor) is not
        // a connect.
        let from_inverter = connect_end(
            &tree,
            &rects,
            LayoutDirection::TopDown,
            CanvasNodeId(3),
            pos2(160.0, 10.0),
        );
        assert!(matches!(from_inverter, ConnectEnd::Add(_)));
    }
}
