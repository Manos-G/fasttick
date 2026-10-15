//! Drop targets for drag and drop on an editable canvas.
//!
//! A drag of selected nodes (move) and a drag from the palette (add) both
//! end at a [`DropSpot`]. The pointer position on the card under the
//! pointer selects the placement: the two ends of the card along the
//! sibling axis put the node before or after the target, the middle puts
//! it inside as the last child. The core crate makes the final check (for
//! example, a decorator that has a child already); the canvas only shows
//! where the node goes.

use egui::{Pos2, Rect};

use super::hit;
use super::layout::LayoutDirection;
use super::model::{CanvasNodeId, CanvasTree, NodeCategory};

/// Where a node goes relative to a target node.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum DropPlacement {
    /// The last child of the target.
    Inside,
    /// The sibling before the target.
    Before,
    /// The sibling after the target.
    After,
}

/// The place under the pointer during a drag.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DropSpot {
    /// An empty place. Moved nodes become unconnected; a new node becomes
    /// unconnected (or the root of an empty tree).
    Background,
    /// A node and a placement.
    Node {
        /// The target node.
        target: CanvasNodeId,
        /// The placement.
        placement: DropPlacement,
    },
    /// A place that cannot take the node (a read-only node, or one of the
    /// moved nodes).
    Invalid,
}

/// The payload of a drag from the palette: the type of the node to add.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NewNodePayload {
    /// The registration ID, for example `Sequence`.
    pub type_id: String,
}

/// The fraction of a card at each end that selects before or after, for a
/// node that can have children.
const END_ZONE: f32 = 0.25;

/// True if nodes of this category can have children.
pub fn takes_children(category: NodeCategory) -> bool {
    matches!(
        category,
        NodeCategory::Control | NodeCategory::Decorator | NodeCategory::Unknown
    )
}

/// The drop spot at world position `p`. `moving` are the dragged nodes
/// (empty for a new node): a node cannot go into itself or into one of its
/// descendants.
pub fn drop_spot(
    tree: &CanvasTree,
    rects: &[Rect],
    direction: LayoutDirection,
    p: Pos2,
    moving: &[CanvasNodeId],
) -> DropSpot {
    let Some(target) = hit::hit_node(rects, p, 0.0) else {
        return DropSpot::Background;
    };
    let Some(node) = tree.get(target) else {
        return DropSpot::Background;
    };
    if node.inline || is_in_moving(tree, target, moving) {
        return DropSpot::Invalid;
    }
    let Some(rect) = rects.get(target.0 as usize) else {
        return DropSpot::Invalid;
    };
    // Expanded SubTree nodes show inline children, but in the file they are
    // leaves.
    let container = takes_children(node.category) && node.subtree.is_none();
    let fraction = match direction {
        LayoutDirection::TopDown => (p.x - rect.min.x) / rect.width().max(1.0),
        LayoutDirection::LeftRight => (p.y - rect.min.y) / rect.height().max(1.0),
    };
    let placement = if node.parent.is_none() {
        // A root (the tree root or an unconnected node) has no siblings in
        // the tree.
        if container {
            DropPlacement::Inside
        } else {
            return DropSpot::Invalid;
        }
    } else if container {
        if fraction < END_ZONE {
            DropPlacement::Before
        } else if fraction > 1.0 - END_ZONE {
            DropPlacement::After
        } else {
            DropPlacement::Inside
        }
    } else if fraction < 0.5 {
        DropPlacement::Before
    } else {
        DropPlacement::After
    };
    DropSpot::Node { target, placement }
}

/// True if `id` is one of `moving` or a descendant of one.
fn is_in_moving(tree: &CanvasTree, id: CanvasNodeId, moving: &[CanvasNodeId]) -> bool {
    let mut current = Some(id);
    while let Some(c) = current {
        if moving.contains(&c) {
            return true;
        }
        current = tree.get(c).and_then(|n| n.parent);
    }
    false
}

/// The nodes that a drag moves: the selected nodes that are editable,
/// without the nodes that have a selected ancestor (they move with it), in
/// tree order. If `grabbed` is not selected, only `grabbed` moves.
pub fn moving_nodes(tree: &CanvasTree, grabbed: CanvasNodeId) -> Vec<CanvasNodeId> {
    let editable = |id: CanvasNodeId| tree.get(id).is_some_and(|n| !n.inline);
    if !tree.is_selected(grabbed) {
        return if editable(grabbed) {
            vec![grabbed]
        } else {
            Vec::new()
        };
    }
    let selected: Vec<CanvasNodeId> = tree.selected().filter(|id| editable(*id)).collect();
    let mut out: Vec<CanvasNodeId> = selected
        .iter()
        .copied()
        .filter(|id| {
            let mut parent = tree.get(*id).and_then(|n| n.parent);
            while let Some(p) = parent {
                if selected.contains(&p) {
                    return false;
                }
                parent = tree.get(p).and_then(|n| n.parent);
            }
            true
        })
        .collect();
    out.sort_by_key(|id| id.0);
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::canvas::model::{CanvasNode, CanvasTreeBuilder, NodeSource};
    use egui::{pos2, vec2};
    use fasttick_core::runtime::NodeIndex;

    /// A root Sequence with two Action children, laid out top-down.
    fn sample() -> (CanvasTree, Vec<Rect>) {
        let mut b = CanvasTreeBuilder::new();
        let mut root = CanvasNode::new(NodeSource::Runtime(NodeIndex(0)), "Sequence", "Sequence");
        root.category = NodeCategory::Control;
        let root = b.push(None, root);
        for i in 1..=2 {
            let mut leaf = CanvasNode::new(NodeSource::Runtime(NodeIndex(i)), "Act", "Act");
            leaf.category = NodeCategory::Action;
            b.push(Some(root), leaf);
        }
        let tree = b.finish("T");
        let size = vec2(100.0, 40.0);
        let rects = vec![
            Rect::from_min_size(pos2(50.0, 0.0), size),
            Rect::from_min_size(pos2(0.0, 100.0), size),
            Rect::from_min_size(pos2(150.0, 100.0), size),
        ];
        (tree, rects)
    }

    fn spot(tree: &CanvasTree, rects: &[Rect], p: Pos2, moving: &[CanvasNodeId]) -> DropSpot {
        drop_spot(tree, rects, LayoutDirection::TopDown, p, moving)
    }

    #[test]
    fn placement_follows_the_pointer() {
        let (tree, rects) = sample();
        let root = CanvasNodeId(0);
        let leaf = CanvasNodeId(1);
        assert_eq!(
            spot(&tree, &rects, pos2(100.0, 20.0), &[]),
            DropSpot::Node {
                target: root,
                placement: DropPlacement::Inside
            }
        );
        // A root takes children only, also at its ends.
        assert_eq!(
            spot(&tree, &rects, pos2(55.0, 20.0), &[]),
            DropSpot::Node {
                target: root,
                placement: DropPlacement::Inside
            }
        );
        assert_eq!(
            spot(&tree, &rects, pos2(10.0, 120.0), &[]),
            DropSpot::Node {
                target: leaf,
                placement: DropPlacement::Before
            }
        );
        assert_eq!(
            spot(&tree, &rects, pos2(90.0, 120.0), &[]),
            DropSpot::Node {
                target: leaf,
                placement: DropPlacement::After
            }
        );
        assert_eq!(
            spot(&tree, &rects, pos2(400.0, 400.0), &[]),
            DropSpot::Background
        );
    }

    #[test]
    fn left_right_uses_the_vertical_axis() {
        let (tree, rects) = sample();
        let leaf = CanvasNodeId(1);
        let at = |y| {
            drop_spot(
                &tree,
                &rects,
                LayoutDirection::LeftRight,
                pos2(50.0, y),
                &[],
            )
        };
        assert_eq!(
            at(105.0),
            DropSpot::Node {
                target: leaf,
                placement: DropPlacement::Before
            }
        );
        assert_eq!(
            at(135.0),
            DropSpot::Node {
                target: leaf,
                placement: DropPlacement::After
            }
        );
    }

    #[test]
    fn a_node_cannot_go_into_itself_or_a_read_only_node() {
        let (mut tree, rects) = sample();
        let root = CanvasNodeId(0);
        assert_eq!(
            spot(&tree, &rects, pos2(100.0, 20.0), &[root]),
            DropSpot::Invalid
        );
        assert_eq!(
            spot(&tree, &rects, pos2(10.0, 120.0), &[root]),
            DropSpot::Invalid
        );
        tree.get_mut(CanvasNodeId(2)).unwrap().inline = true;
        assert_eq!(
            spot(&tree, &rects, pos2(160.0, 120.0), &[]),
            DropSpot::Invalid
        );
    }

    #[test]
    fn moving_nodes_skips_descendants_and_read_only_nodes() {
        let (mut tree, _) = sample();
        let (root, a, b) = (CanvasNodeId(0), CanvasNodeId(1), CanvasNodeId(2));
        // Not selected: only the grabbed node.
        assert_eq!(moving_nodes(&tree, a), vec![a]);
        tree.set_selected(root, true);
        tree.set_selected(b, true);
        assert_eq!(moving_nodes(&tree, b), vec![root]);
        tree.set_selected(root, false);
        tree.set_selected(a, true);
        assert_eq!(moving_nodes(&tree, b), vec![a, b]);
        tree.get_mut(a).unwrap().inline = true;
        assert_eq!(moving_nodes(&tree, b), vec![b]);
    }
}
