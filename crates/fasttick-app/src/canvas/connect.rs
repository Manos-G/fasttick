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
//!
//! An unconnected node (see [`CanvasNode::detached`]) also shows a dot on
//! its parent side (the top edge in the top-down layout). A drag from that
//! dot draws a connection to a new parent. Where the user releases it:
//!
//! - on a node that can take one more child (or on its dot): the
//!   unconnected node (with its subtree) becomes its last child;
//! - on a node that takes no child, whose parent can take one more child:
//!   the unconnected node goes before or after that node (the half of the
//!   card selects it);
//! - elsewhere: nothing.

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
    nearest_dot(tree, rects, p, grab, |r| dot_center(r, direction), has_dot)
}

/// True if the node shows a dot on its parent side: it is an unconnected
/// node (see [`CanvasNode::detached`]).
pub fn has_parent_dot(node: &CanvasNode) -> bool {
    node.detached && node.parent.is_none() && !node.inline
}

/// The centre of the parent dot of a card, in world units: a little
/// outside the middle of the parent side.
pub fn parent_dot_center(rect: Rect, direction: LayoutDirection) -> Pos2 {
    match direction {
        LayoutDirection::TopDown => pos2(rect.center().x, rect.min.y - DOT_OFFSET),
        LayoutDirection::LeftRight => pos2(rect.min.x - DOT_OFFSET, rect.center().y),
    }
}

/// The unconnected node whose parent dot is nearest to `p` (world units),
/// not further than `grab` (world units).
pub fn parent_dot_at(
    tree: &CanvasTree,
    rects: &[Rect],
    direction: LayoutDirection,
    p: Pos2,
    grab: f32,
) -> Option<CanvasNodeId> {
    nearest_dot(
        tree,
        rects,
        p,
        grab,
        |r| parent_dot_center(r, direction),
        has_parent_dot,
    )
}

fn nearest_dot(
    tree: &CanvasTree,
    rects: &[Rect],
    p: Pos2,
    grab: f32,
    center: impl Fn(Rect) -> Pos2,
    shows_dot: impl Fn(&CanvasNode) -> bool,
) -> Option<CanvasNodeId> {
    let mut best: Option<(f32, CanvasNodeId)> = None;
    for (index, rect) in rects.iter().enumerate() {
        let distance = center(*rect).distance(p);
        if distance > grab || best.is_some_and(|(d, _)| d <= distance) {
            continue;
        }
        let id = CanvasNodeId(index as u32);
        if tree.get(id).is_some_and(&shows_dot) {
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

/// The result of a connection from the parent dot of an unconnected node.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AttachEnd {
    /// Connect the node at this place (a [`DropSpot::Node`]).
    Attach(DropSpot),
    /// The node under the pointer cannot take one more child.
    Full(CanvasNodeId),
    /// No node that can be the parent: an empty place, the node itself, a
    /// node of its own subtree or a read-only node.
    Nothing,
}

/// What a connection from the parent dot of the unconnected node `child`
/// does when it ends at `p` (world units). The parent is the card under
/// the pointer, or the card whose child dot is nearer than `grab` (world
/// units).
pub fn attach_end(
    tree: &CanvasTree,
    rects: &[Rect],
    direction: LayoutDirection,
    child: CanvasNodeId,
    p: Pos2,
    grab: f32,
) -> AttachEnd {
    let target =
        super::hit::hit_node(rects, p, 0.0).or_else(|| dot_at(tree, rects, direction, p, grab));
    let Some(target) = target else {
        return AttachEnd::Nothing;
    };
    let Some(node) = tree.get(target) else {
        return AttachEnd::Nothing;
    };
    if node.inline || is_ancestor(tree, child, target) {
        return AttachEnd::Nothing;
    }
    if has_dot(node) {
        return AttachEnd::Attach(DropSpot::Node {
            target,
            placement: DropPlacement::Inside,
        });
    }
    // A node that takes no child: a sibling, if its parent has room.
    let parent_has_room = node
        .parent
        .and_then(|parent| tree.get(parent))
        .is_some_and(|parent| has_dot(parent) && !parent.inline);
    let Some(rect) = rects.get(target.0 as usize).filter(|_| parent_has_room) else {
        return AttachEnd::Full(target);
    };
    let first_half = match direction {
        LayoutDirection::TopDown => p.x < rect.center().x,
        LayoutDirection::LeftRight => p.y < rect.center().y,
    };
    AttachEnd::Attach(DropSpot::Node {
        target,
        placement: if first_half {
            DropPlacement::Before
        } else {
            DropPlacement::After
        },
    })
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
    fn unconnected_nodes_have_a_parent_dot() {
        let (mut tree, rects) = sample();
        tree.get_mut(CanvasNodeId(4)).unwrap().detached = true;
        let shown: Vec<bool> = (0..5)
            .map(|i| has_parent_dot(tree.get(CanvasNodeId(i)).unwrap()))
            .collect();
        assert_eq!(shown, [false, false, false, false, true]);
        // The dot is at the middle of the top edge, a little outside.
        assert_eq!(
            parent_dot_center(rects[4], LayoutDirection::TopDown),
            pos2(650.0, -DOT_OFFSET)
        );
        assert_eq!(
            parent_dot_center(rects[4], LayoutDirection::LeftRight),
            pos2(600.0 - DOT_OFFSET, 20.0)
        );
        let at = |p| parent_dot_at(&tree, &rects, LayoutDirection::TopDown, p, 10.0);
        assert_eq!(at(pos2(652.0, -5.0)), Some(CanvasNodeId(4)));
        assert_eq!(at(pos2(650.0, 40.0)), None, "the bottom edge");
        assert_eq!(
            at(pos2(200.0, -3.0)),
            None,
            "the tree root has no parent dot"
        );
    }

    #[test]
    fn the_release_point_selects_the_parent_and_the_place() {
        let (mut tree, rects) = sample();
        let unconnected = CanvasNodeId(4);
        tree.get_mut(unconnected).unwrap().detached = true;
        let end = |p| {
            attach_end(
                &tree,
                &rects,
                LayoutDirection::TopDown,
                unconnected,
                p,
                10.0,
            )
        };
        let inside = |target| {
            AttachEnd::Attach(DropSpot::Node {
                target,
                placement: DropPlacement::Inside,
            })
        };
        // On the Sequence or on its dot: the last child.
        assert_eq!(end(pos2(160.0, 20.0)), inside(CanvasNodeId(0)));
        assert_eq!(end(pos2(200.0, 40.0 + DOT_OFFSET)), inside(CanvasNodeId(0)));
        // On the empty Inverter: its child.
        assert_eq!(end(pos2(350.0, 120.0)), inside(CanvasNodeId(3)));
        // On an action of the Sequence: before or after it.
        assert_eq!(
            end(pos2(10.0, 120.0)),
            AttachEnd::Attach(DropSpot::Node {
                target: CanvasNodeId(1),
                placement: DropPlacement::Before
            })
        );
        assert_eq!(
            end(pos2(240.0, 120.0)),
            AttachEnd::Attach(DropSpot::Node {
                target: CanvasNodeId(2),
                placement: DropPlacement::After
            })
        );
        // The node itself, and an empty place.
        assert_eq!(end(pos2(620.0, 20.0)), AttachEnd::Nothing);
        assert_eq!(end(pos2(500.0, 300.0)), AttachEnd::Nothing);
        // A full Inverter: no child, and its own child goes nowhere else.
        let mut b = CanvasTreeBuilder::new();
        let inverter = b.push(None, node(0, NodeCategory::Decorator));
        let leaf = b.push(Some(inverter), node(1, NodeCategory::Action));
        let loose = b.push(None, node(2, NodeCategory::Action));
        let mut full = b.finish("T");
        full.get_mut(loose).unwrap().detached = true;
        let size = vec2(100.0, 40.0);
        let full_rects = vec![
            Rect::from_min_size(pos2(0.0, 0.0), size),
            Rect::from_min_size(pos2(0.0, 100.0), size),
            Rect::from_min_size(pos2(300.0, 0.0), size),
        ];
        let full_end = |p| attach_end(&full, &full_rects, LayoutDirection::TopDown, loose, p, 10.0);
        assert_eq!(full_end(pos2(50.0, 20.0)), AttachEnd::Full(inverter));
        assert_eq!(full_end(pos2(50.0, 120.0)), AttachEnd::Full(leaf));
        // A node of another tree (read-only).
        tree.get_mut(CanvasNodeId(3)).unwrap().inline = true;
        let inline = attach_end(
            &tree,
            &rects,
            LayoutDirection::TopDown,
            unconnected,
            pos2(350.0, 120.0),
            10.0,
        );
        assert_eq!(inline, AttachEnd::Nothing);
    }

    #[test]
    fn an_unconnected_node_cannot_go_into_its_own_subtree() {
        let mut b = CanvasTreeBuilder::new();
        let top = b.push(None, node(0, NodeCategory::Control));
        let inner = b.push(Some(top), node(1, NodeCategory::Control));
        let mut tree = b.finish("T");
        tree.get_mut(top).unwrap().detached = true;
        let size = vec2(100.0, 40.0);
        let rects = vec![
            Rect::from_min_size(pos2(0.0, 0.0), size),
            Rect::from_min_size(pos2(0.0, 100.0), size),
        ];
        let end = attach_end(
            &tree,
            &rects,
            LayoutDirection::TopDown,
            top,
            pos2(50.0, 120.0),
            10.0,
        );
        assert_eq!(end, AttachEnd::Nothing);
        assert!(has_dot(tree.get(inner).unwrap()));
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
