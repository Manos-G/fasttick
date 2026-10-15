//! The layout boundary of the canvas.
//!
//! [`layout`] places the cards of a [`CanvasTree`]. It reads the card sizes
//! ([`CardText::size`](super::model::CardText::size)) and gives one world
//! rectangle for each node.
//!
//! TODO(after the merge of `fasttick_core::layout`): implement
//! [`LayoutEngine`] with the tidy-tree layout of the core crate (Walker
//! algorithm, variable node sizes) and use it in [`layout`]. The
//! temporary [`LayeredLayout`] below is simple and correct (no overlaps,
//! parents centred over their children, one row or column for each level),
//! but it does not pack subtrees as tightly as a tidy tree.

use egui::{Pos2, Rect, Vec2, pos2, vec2};

use super::model::{CanvasNodeId, CanvasTree};

/// The direction of the tree.
#[derive(
    Debug, Clone, Copy, PartialEq, Eq, Hash, Default, serde::Serialize, serde::Deserialize,
)]
pub enum LayoutDirection {
    /// The root at the top, children below.
    #[default]
    TopDown,
    /// The root at the left, children to the right.
    LeftRight,
}

impl LayoutDirection {
    /// The display name.
    pub fn name(self) -> &'static str {
        match self {
            LayoutDirection::TopDown => "Top-down",
            LayoutDirection::LeftRight => "Left-right",
        }
    }

    /// The other direction.
    pub fn toggled(self) -> Self {
        match self {
            LayoutDirection::TopDown => LayoutDirection::LeftRight,
            LayoutDirection::LeftRight => LayoutDirection::TopDown,
        }
    }

    /// The axis along the levels (0 = x, 1 = y).
    pub fn depth_axis(self) -> usize {
        match self {
            LayoutDirection::TopDown => 1,
            LayoutDirection::LeftRight => 0,
        }
    }

    /// The axis along the siblings (0 = x, 1 = y).
    pub fn breadth_axis(self) -> usize {
        1 - self.depth_axis()
    }
}

/// Spacing of the layout, in world units.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct LayoutParams {
    /// The gap between two leaf siblings.
    pub sibling_gap: f32,
    /// The gap between two siblings when one of them has children.
    pub subtree_gap: f32,
    /// The gap between two levels.
    pub level_gap: f32,
}

impl LayoutParams {
    /// The default spacing for a direction.
    pub fn for_direction(direction: LayoutDirection) -> Self {
        match direction {
            LayoutDirection::TopDown => LayoutParams {
                sibling_gap: 18.0,
                subtree_gap: 34.0,
                level_gap: 56.0,
            },
            LayoutDirection::LeftRight => LayoutParams {
                sibling_gap: 12.0,
                subtree_gap: 22.0,
                level_gap: 64.0,
            },
        }
    }
}

/// The result of a layout.
#[derive(Debug, Clone, PartialEq, Default)]
pub struct TreeLayout {
    /// The direction.
    pub direction: LayoutDirection,
    /// The world rectangle of each node, by [`CanvasNodeId`].
    pub rects: Vec<Rect>,
    /// The rectangle that contains all nodes.
    pub bounds: Rect,
    /// The start of each level on the depth axis.
    pub level_starts: Vec<f32>,
    /// The spacing that the layout used.
    pub level_gap: f32,
}

impl TreeLayout {
    /// The rectangle of a node.
    pub fn rect(&self, id: CanvasNodeId) -> Option<Rect> {
        self.rects.get(id.index()).copied()
    }

    /// The point on the depth axis where the edges from a parent at
    /// `depth` turn towards their children: the middle of the gap before
    /// the next level.
    pub fn bus_position(&self, depth: u32) -> Option<f32> {
        let next = self.level_starts.get(depth as usize + 1)?;
        Some(next - self.level_gap * 0.5)
    }

    /// The point where the edge leaves the parent card.
    pub fn edge_start(&self, rect: Rect) -> Pos2 {
        match self.direction {
            LayoutDirection::TopDown => pos2(rect.center().x, rect.max.y),
            LayoutDirection::LeftRight => pos2(rect.max.x, rect.center().y),
        }
    }

    /// The point where the edge enters the child card.
    pub fn edge_end(&self, rect: Rect) -> Pos2 {
        match self.direction {
            LayoutDirection::TopDown => pos2(rect.center().x, rect.min.y),
            LayoutDirection::LeftRight => pos2(rect.min.x, rect.center().y),
        }
    }
}

/// A layout algorithm.
pub trait LayoutEngine {
    /// Places the nodes of `tree`. The result has one rectangle for each
    /// node, with the size of [`CardText::size`](super::model::CardText::size).
    fn layout(
        &self,
        tree: &CanvasTree,
        direction: LayoutDirection,
        params: &LayoutParams,
    ) -> TreeLayout;
}

/// Places the nodes of a tree with the default engine and spacing.
pub fn layout(tree: &CanvasTree, direction: LayoutDirection) -> TreeLayout {
    // TODO(after the merge of `fasttick_core::layout`): use the core engine.
    LayeredLayout.layout(tree, direction, &LayoutParams::for_direction(direction))
}

/// A temporary layered layout. See the module documentation.
///
/// Each subtree takes a band on the breadth axis that is as wide as its
/// widest part. Siblings sit side by side, and a parent is centred between
/// its first and last child. All nodes of one level start at the same
/// position on the depth axis. Linear time.
#[derive(Debug, Clone, Copy, Default)]
pub struct LayeredLayout;

impl LayoutEngine for LayeredLayout {
    fn layout(
        &self,
        tree: &CanvasTree,
        direction: LayoutDirection,
        params: &LayoutParams,
    ) -> TreeLayout {
        let nodes = tree.nodes();
        let n = nodes.len();
        if n == 0 {
            return TreeLayout {
                direction,
                level_gap: params.level_gap,
                ..TreeLayout::default()
            };
        }
        let breadth_axis = direction.breadth_axis();
        let depth_axis = direction.depth_axis();
        let size = |i: usize| -> Vec2 {
            let s = nodes[i].card.size;
            vec2(sanitize(s.x), sanitize(s.y))
        };

        // Levels.
        let levels = tree.level_count();
        let mut level_extent = vec![0.0_f32; levels];
        for (i, node) in nodes.iter().enumerate() {
            let d = node.depth as usize;
            level_extent[d] = level_extent[d].max(size(i)[depth_axis]);
        }
        let mut level_starts = Vec::with_capacity(levels);
        let mut position = 0.0;
        for extent in &level_extent {
            level_starts.push(position);
            position += extent + params.level_gap;
        }

        // Post-order (the reverse of the depth-first order): the extent of
        // each subtree relative to the centre of its root, and the offset of
        // each child from its parent.
        let mut ext_lo = vec![0.0_f32; n];
        let mut ext_hi = vec![0.0_f32; n];
        let mut offset = vec![0.0_f32; n];
        let mut centres: Vec<f32> = Vec::new();
        for i in (0..n).rev() {
            let half = size(i)[breadth_axis] * 0.5;
            let children = &nodes[i].children;
            if children.is_empty() {
                ext_lo[i] = -half;
                ext_hi[i] = half;
                continue;
            }
            centres.clear();
            let mut cursor = 0.0_f32;
            let mut previous: Option<usize> = None;
            for child in children {
                let c = child.index();
                if let Some(p) = previous {
                    let gap = if nodes[p].children.is_empty() && nodes[c].children.is_empty() {
                        params.sibling_gap
                    } else {
                        params.subtree_gap
                    };
                    cursor += gap;
                }
                let centre = cursor - ext_lo[c];
                centres.push(centre);
                cursor = centre + ext_hi[c];
                previous = Some(c);
            }
            let first = centres[0];
            let last = centres[centres.len() - 1];
            let parent_centre = (first + last) * 0.5;
            for (child, centre) in children.iter().zip(&centres) {
                offset[child.index()] = centre - parent_centre;
            }
            ext_lo[i] = (-parent_centre).min(-half);
            ext_hi[i] = (cursor - parent_centre).max(half);
        }

        // Pre-order: absolute centres. Roots (only one in a valid tree) are
        // placed side by side.
        let mut centre = vec![0.0_f32; n];
        let mut root_cursor = 0.0_f32;
        for i in 0..n {
            centre[i] = match nodes[i].parent {
                Some(parent) => centre[parent.index()] + offset[i],
                None => {
                    let c = root_cursor - ext_lo[i];
                    root_cursor = c + ext_hi[i] + params.subtree_gap;
                    c
                }
            };
        }

        let mut rects = Vec::with_capacity(n);
        let mut bounds = Rect::NOTHING;
        for (i, node) in nodes.iter().enumerate() {
            let s = size(i);
            let depth_start = level_starts[node.depth as usize];
            let breadth_start = centre[i] - s[breadth_axis] * 0.5;
            let min = match direction {
                LayoutDirection::TopDown => pos2(breadth_start, depth_start),
                LayoutDirection::LeftRight => pos2(depth_start, breadth_start),
            };
            let rect = Rect::from_min_size(min, s);
            bounds = bounds.union(rect);
            rects.push(rect);
        }

        TreeLayout {
            direction,
            rects,
            bounds,
            level_starts,
            level_gap: params.level_gap,
        }
    }
}

/// A size value that the layout can use: finite and not negative.
fn sanitize(value: f32) -> f32 {
    if value.is_finite() {
        value.max(0.0)
    } else {
        0.0
    }
}

/// The rectangle of each subtree (the node and all its descendants).
pub fn subtree_bounds(tree: &CanvasTree, layout: &TreeLayout) -> Vec<Rect> {
    let mut bounds: Vec<Rect> = layout.rects.clone();
    bounds.resize(tree.len(), Rect::NOTHING);
    // Children follow their parent in depth-first order, so a reverse
    // pass sees each child before its parent.
    for i in (0..tree.len()).rev() {
        if let Some(parent) = tree.nodes()[i].parent {
            let child = bounds[i];
            let slot = &mut bounds[parent.index()];
            *slot = slot.union(child);
        }
    }
    bounds
}

/// The rectangle that contains the edges from a parent to its children,
/// or `None` for a leaf. The canvas uses it to skip edges outside the
/// view.
pub fn edge_bounds(tree: &CanvasTree, layout: &TreeLayout, id: CanvasNodeId) -> Option<Rect> {
    let node = tree.get(id)?;
    if node.children.is_empty() {
        return None;
    }
    let mut rect = Rect::from_min_max(
        layout.edge_start(layout.rect(id)?),
        layout.edge_start(layout.rect(id)?),
    );
    for child in &node.children {
        rect.extend_with(layout.edge_end(layout.rect(*child)?));
    }
    Some(rect)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::canvas::model::{CanvasNode, CanvasTreeBuilder, NodeSource};
    use fasttick_core::runtime::NodeIndex;

    /// Builds a tree from (parent, width, height) triples in depth-first
    /// order.
    fn tree(spec: &[(Option<u32>, f32, f32)]) -> CanvasTree {
        let mut builder = CanvasTreeBuilder::new();
        for (i, (parent, w, h)) in spec.iter().enumerate() {
            let mut node = CanvasNode::new(NodeSource::Runtime(NodeIndex(i)), "n", "T");
            node.card.size = vec2(*w, *h);
            builder.push(parent.map(CanvasNodeId), node);
        }
        builder.finish("T")
    }

    fn no_overlaps(layout: &TreeLayout) {
        for (i, a) in layout.rects.iter().enumerate() {
            for (j, b) in layout.rects.iter().enumerate().skip(i + 1) {
                assert!(
                    !a.intersects(*b) || a.intersect(*b).area() < 1e-3,
                    "nodes {i} and {j} overlap: {a:?} {b:?}"
                );
            }
        }
    }

    fn sample() -> CanvasTree {
        //        0
        //   1    4     7
        //  2 3   5 6
        tree(&[
            (None, 100.0, 40.0),
            (Some(0), 80.0, 30.0),
            (Some(1), 60.0, 30.0),
            (Some(1), 200.0, 50.0),
            (Some(0), 50.0, 30.0),
            (Some(4), 60.0, 30.0),
            (Some(4), 60.0, 30.0),
            (Some(0), 120.0, 30.0),
        ])
    }

    #[test]
    fn top_down_basic_properties() {
        let tree = sample();
        let result = layout(&tree, LayoutDirection::TopDown);
        let params = LayoutParams::for_direction(LayoutDirection::TopDown);
        assert_eq!(result.rects.len(), tree.len());
        no_overlaps(&result);
        // Sizes are kept.
        for (node, rect) in tree.nodes().iter().zip(&result.rects) {
            assert_eq!(rect.size(), node.card.size);
        }
        // Levels: all nodes of a level start at the same y, below the
        // tallest node of the level before.
        assert_eq!(result.level_starts, vec![0.0, 40.0 + 56.0, 40.0 + 56.0 + 50.0 + 56.0]);
        for (node, rect) in tree.nodes().iter().zip(&result.rects) {
            assert_eq!(rect.min.y, result.level_starts[node.depth as usize]);
        }
        // A parent is centred between its first and last child.
        for id in tree.ids() {
            let node = tree.get(id).unwrap();
            if let (Some(first), Some(last)) = (node.children.first(), node.children.last()) {
                let mid = (result.rects[first.index()].center().x
                    + result.rects[last.index()].center().x)
                    * 0.5;
                assert!((result.rects[id.index()].center().x - mid).abs() < 1e-3);
            }
        }
        // Children are in order, with at least the sibling gap.
        for node in tree.nodes() {
            for pair in node.children.windows(2) {
                let a = result.rects[pair[0].index()];
                let b = result.rects[pair[1].index()];
                assert!(b.min.x - a.max.x >= params.sibling_gap - 1e-3);
            }
        }
        // The bounds start at the origin and contain all nodes.
        assert_eq!(result.bounds.min, Pos2::ZERO);
        for rect in &result.rects {
            assert!(result.bounds.contains_rect(*rect));
        }
        assert_eq!(result.bus_position(0), Some(96.0 - 28.0));
        assert_eq!(result.bus_position(2), None);
    }

    #[test]
    fn left_right_swaps_axes() {
        let tree = sample();
        let result = layout(&tree, LayoutDirection::LeftRight);
        no_overlaps(&result);
        assert_eq!(result.level_starts[1], 100.0 + 64.0);
        for (node, rect) in tree.nodes().iter().zip(&result.rects) {
            assert_eq!(rect.min.x, result.level_starts[node.depth as usize]);
        }
        let start = result.edge_start(result.rects[0]);
        assert_eq!(start.x, result.rects[0].max.x);
        let end = result.edge_end(result.rects[1]);
        assert_eq!(end.x, result.rects[1].min.x);
    }

    #[test]
    fn wide_parent_over_narrow_children() {
        let tree = tree(&[(None, 400.0, 40.0), (Some(0), 20.0, 20.0), (Some(0), 20.0, 20.0)]);
        let result = layout(&tree, LayoutDirection::TopDown);
        no_overlaps(&result);
        assert_eq!(result.bounds.width(), 400.0);
        let mid = (result.rects[1].center().x + result.rects[2].center().x) * 0.5;
        assert!((result.rects[0].center().x - mid).abs() < 1e-3);
    }

    #[test]
    fn degenerate_input() {
        let empty = CanvasTree::default();
        let result = layout(&empty, LayoutDirection::TopDown);
        assert!(result.rects.is_empty());
        let bad = tree(&[(None, f32::NAN, -5.0), (Some(0), f32::INFINITY, 10.0)]);
        let result = layout(&bad, LayoutDirection::TopDown);
        for rect in &result.rects {
            assert!(rect.min.x.is_finite() && rect.min.y.is_finite());
            assert!(rect.width() >= 0.0 && rect.height() >= 0.0);
        }
    }

    #[test]
    fn deep_chain_is_linear() {
        let mut spec = vec![(None, 100.0, 30.0)];
        for i in 0..2000 {
            spec.push((Some(i), 100.0, 30.0));
        }
        let tree = tree(&spec);
        let result = layout(&tree, LayoutDirection::TopDown);
        assert_eq!(result.level_starts.len(), 2001);
        // A chain is a straight line.
        assert!(result.rects.iter().all(|r| r.min.x == 0.0));
    }

    #[test]
    fn subtree_and_edge_bounds() {
        let tree = sample();
        let result = layout(&tree, LayoutDirection::TopDown);
        let bounds = subtree_bounds(&tree, &result);
        assert_eq!(bounds[0], result.bounds);
        let b1 = bounds[1];
        for i in [1, 2, 3] {
            assert!(b1.contains_rect(result.rects[i]));
        }
        assert!(!b1.contains_rect(result.rects[5]));
        assert_eq!(bounds[2], result.rects[2]);

        assert_eq!(edge_bounds(&tree, &result, CanvasNodeId(2)), None);
        let edges = edge_bounds(&tree, &result, CanvasNodeId(0)).unwrap();
        assert_eq!(edges.min.y, result.rects[0].max.y);
        assert_eq!(edges.max.y, result.level_starts[1]);
        assert_eq!(edges.min.x, result.rects[1].center().x);
        assert_eq!(edges.max.x, result.rects[7].center().x);
    }
}
