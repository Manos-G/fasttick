//! Automatic tree layout.
//!
//! [`layout`] places the nodes of a [`LayoutTree`] as a tidy tree. The
//! nodes can have different sizes (the GUI measures the text of each
//! node). The input is a forest: the main tree and, after it, the
//! unconnected subtrees of the canvas. See `docs/DESIGN.md` section 2.7.
//!
//! The module has no GUI types. It uses its own [`Vec2`] and [`Rect`],
//! which convert to and from `(f32, f32)` and `[f32; 2]`.
//!
//! # Rules of the drawing
//!
//! - Two nodes do not overlap. Two children of the same parent have at
//!   least [`LayoutParams::sibling_gap`] between them; two nodes with
//!   different parents have at least [`LayoutParams::subtree_gap`].
//! - Each child is below its parent ([`LayoutDirection::TopDown`]) or to
//!   the right of it ([`LayoutDirection::LeftRight`]), after a gap of
//!   [`LayoutParams::level_gap`]. The children keep their order: the first
//!   child is at the left (top-down) or at the top (left-right).
//! - A parent is centred between the centres of its first and last child.
//! - Subtrees are as compact as possible. Small subtrees between two large
//!   ones get equal spaces.
//! - Two equal subtrees get the same drawing, wherever they are. (With
//!   [`LayoutParams::layered`], the positions along the depth axis also
//!   depend on the other nodes of each level.)
//! - The drawing of a mirrored tree is the mirror image of the drawing.
//! - The result is deterministic.
//!
//! # Algorithm
//!
//! The tidy-tree algorithm of van der Ploeg (2014), which extends the
//! Walker algorithm to nodes of different sizes. It takes linear time. The
//! layout runs it twice, on the tree and on the mirrored tree, and takes
//! the mean of the two results. This makes the drawing symmetric. The mean
//! keeps all the rules above, because each rule is a linear inequality or
//! equation that both results obey.
//!
//! [`LayoutParams::layered`] selects one of two placements along the depth
//! axis:
//!
//! - Layered (the default): all nodes of one depth in one tree start at the
//!   same position. A level is as deep as its largest node.
//! - Not layered: each child starts directly after its parent and the level
//!   gap. This gives a more compact drawing when the node sizes vary a lot.
//!
//! The trees of the forest are placed one after the other along the
//! breadth axis (to the right for top-down, below for left-right), with
//! [`LayoutParams::forest_gap`] between their bounding boxes. All roots
//! start at depth position 0, and the first tree starts at the origin.
//!
//! # Example
//!
//! ```
//! use fasttick_core::layout::{LayoutParams, LayoutTree, Vec2, layout};
//!
//! let mut tree = LayoutTree::new();
//! let root = tree.add_root((100.0, 40.0));
//! let a = tree.add_child(root, (60.0, 30.0)).unwrap();
//! let b = tree.add_child(root, (80.0, 30.0)).unwrap();
//!
//! let params = LayoutParams::default();
//! let result = layout(&tree, &params).unwrap();
//! let (ra, rb) = (result.rect(a).unwrap(), result.rect(b).unwrap());
//! // The children are on one level, below the root, after the gap.
//! assert_eq!(ra.min.y, 40.0 + params.level_gap);
//! assert_eq!(rb.min.x - ra.max.x, params.sibling_gap);
//! // The root is centred between the centres of its children.
//! let root_centre = result.rect(root).unwrap().center().x;
//! assert_eq!(root_centre, (ra.center().x + rb.center().x) / 2.0);
//! assert_eq!(result.bounds.min, Vec2::ZERO);
//! ```

mod geom;
mod tidy;

use std::ops::Range;

pub use crate::project::LayoutDirection;
pub use geom::{Rect, Vec2};

use tidy::{NONE, Pass, Shape};

/// One node of a [`LayoutTree`].
#[derive(Debug, Clone, PartialEq, Default)]
pub struct LayoutNode {
    /// The width and the height of the node. A value that is negative or
    /// not finite counts as zero.
    pub size: Vec2,
    /// The children, in order: indices into [`LayoutTree::nodes`].
    pub children: Vec<usize>,
}

impl LayoutNode {
    /// Makes a node without children.
    pub fn new(size: impl Into<Vec2>) -> LayoutNode {
        LayoutNode {
            size: size.into(),
            children: Vec::new(),
        }
    }

    /// Makes a node with children.
    pub fn with_children(size: impl Into<Vec2>, children: Vec<usize>) -> LayoutNode {
        LayoutNode {
            size: size.into(),
            children,
        }
    }
}

/// The input of [`layout`]: a forest of nodes with sizes.
///
/// Each node must be reachable from exactly one root on exactly one path:
/// a node is a root or the child of one node, never both, and the children
/// lists have no cycles. [`layout`] returns a [`LayoutError`] if this is not
/// true.
#[derive(Debug, Clone, PartialEq, Default)]
pub struct LayoutTree {
    /// The nodes. The layout gives one position for each node, at the same
    /// index.
    pub nodes: Vec<LayoutNode>,
    /// The roots, in order: the main tree first, then the unconnected
    /// subtrees.
    pub roots: Vec<usize>,
}

impl LayoutTree {
    /// Makes an empty tree.
    pub fn new() -> LayoutTree {
        LayoutTree::default()
    }

    /// Makes an empty tree with space for `capacity` nodes.
    pub fn with_capacity(capacity: usize) -> LayoutTree {
        LayoutTree {
            nodes: Vec::with_capacity(capacity),
            roots: Vec::new(),
        }
    }

    /// The number of nodes.
    pub fn len(&self) -> usize {
        self.nodes.len()
    }

    /// True if the tree has no nodes.
    pub fn is_empty(&self) -> bool {
        self.nodes.is_empty()
    }

    /// Adds a node as a new root, after the other roots. Returns its index.
    pub fn add_root(&mut self, size: impl Into<Vec2>) -> usize {
        let index = self.nodes.len();
        self.nodes.push(LayoutNode::new(size));
        self.roots.push(index);
        index
    }

    /// Adds a node as the last child of `parent`. Returns its index, or
    /// `None` (and adds nothing) if `parent` is not a node index.
    pub fn add_child(&mut self, parent: usize, size: impl Into<Vec2>) -> Option<usize> {
        if parent >= self.nodes.len() {
            return None;
        }
        let index = self.nodes.len();
        self.nodes.push(LayoutNode::new(size));
        self.nodes[parent].children.push(index);
        Some(index)
    }

    /// The mirrored tree: the same nodes, with the order of the roots and
    /// of each children list reversed.
    pub fn mirrored(&self) -> LayoutTree {
        LayoutTree {
            nodes: self
                .nodes
                .iter()
                .map(|node| LayoutNode {
                    size: node.size,
                    children: node.children.iter().rev().copied().collect(),
                })
                .collect(),
            roots: self.roots.iter().rev().copied().collect(),
        }
    }
}

/// The spacing and the direction of a layout, in world units.
///
/// A gap that is negative or not finite counts as zero.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct LayoutParams {
    /// The direction of the tree.
    pub direction: LayoutDirection,
    /// The minimum gap between two children of the same parent.
    pub sibling_gap: f32,
    /// The minimum gap between two nodes that have different parents (for
    /// example the children of two sibling subtrees).
    pub subtree_gap: f32,
    /// The gap between a parent and its children along the depth axis.
    pub level_gap: f32,
    /// The gap between the bounding boxes of two trees of the forest.
    pub forest_gap: f32,
    /// True: all nodes of one depth start at the same position along the
    /// depth axis. False: each child starts directly after its parent and
    /// the level gap.
    pub layered: bool,
}

impl LayoutParams {
    /// The default spacing for a direction. The layout is layered.
    pub fn new(direction: LayoutDirection) -> LayoutParams {
        match direction {
            LayoutDirection::TopDown => LayoutParams {
                direction,
                sibling_gap: 18.0,
                subtree_gap: 34.0,
                level_gap: 56.0,
                forest_gap: 80.0,
                layered: true,
            },
            LayoutDirection::LeftRight => LayoutParams {
                direction,
                sibling_gap: 12.0,
                subtree_gap: 22.0,
                level_gap: 64.0,
                forest_gap: 64.0,
                layered: true,
            },
        }
    }
}

impl Default for LayoutParams {
    fn default() -> LayoutParams {
        LayoutParams::new(LayoutDirection::TopDown)
    }
}

/// The result of [`layout`].
#[derive(Debug, Clone, PartialEq, Default)]
pub struct Layout {
    /// The direction of the layout.
    pub direction: LayoutDirection,
    /// The top-left corner of each node, at the index of the node.
    pub positions: Vec<Vec2>,
    /// The size of each node, after the correction of bad values (see
    /// [`LayoutNode::size`]).
    pub sizes: Vec<Vec2>,
    /// The depth of each node. A root has depth 0.
    pub depths: Vec<u32>,
    /// For each node: the position along the depth axis (y for top-down,
    /// x for left-right) in the middle of the gap between the node and its
    /// children. Edges drawn with right angles can turn there. In a layered
    /// layout, all nodes of one level of one tree have the same value.
    pub bus_positions: Vec<f32>,
    /// The bounding box of each tree of the forest, in the order of
    /// [`LayoutTree::roots`].
    pub tree_bounds: Vec<Rect>,
    /// The bounding box of all nodes. [`Rect::ZERO`] if there are no nodes.
    pub bounds: Rect,
}

impl Layout {
    /// The number of nodes.
    pub fn len(&self) -> usize {
        self.positions.len()
    }

    /// True if the layout has no nodes.
    pub fn is_empty(&self) -> bool {
        self.positions.is_empty()
    }

    /// The rectangle of a node.
    pub fn rect(&self, node: usize) -> Option<Rect> {
        let position = *self.positions.get(node)?;
        let size = *self.sizes.get(node)?;
        Some(Rect::from_min_size(position, size))
    }

    /// The rectangles of all nodes, in node order.
    pub fn rects(&self) -> impl Iterator<Item = Rect> + '_ {
        self.positions
            .iter()
            .zip(&self.sizes)
            .map(|(&position, &size)| Rect::from_min_size(position, size))
    }

    /// The point where the edges to the children leave a node: the middle
    /// of its bottom side (top-down) or of its right side (left-right).
    pub fn edge_start(&self, node: usize) -> Option<Vec2> {
        let rect = self.rect(node)?;
        Some(match self.direction {
            LayoutDirection::TopDown => Vec2::new(rect.center().x, rect.max.y),
            LayoutDirection::LeftRight => Vec2::new(rect.max.x, rect.center().y),
        })
    }

    /// The point where the edge from the parent enters a node: the middle
    /// of its top side (top-down) or of its left side (left-right).
    pub fn edge_end(&self, node: usize) -> Option<Vec2> {
        let rect = self.rect(node)?;
        Some(match self.direction {
            LayoutDirection::TopDown => Vec2::new(rect.center().x, rect.min.y),
            LayoutDirection::LeftRight => Vec2::new(rect.min.x, rect.center().y),
        })
    }

    /// The position along the depth axis (y for top-down, x for
    /// left-right) in the middle of the gap between a node and its
    /// children. See [`Layout::bus_positions`].
    pub fn bus_position(&self, node: usize) -> Option<f32> {
        self.bus_positions.get(node).copied()
    }

    /// Moves all nodes by `offset`.
    pub fn translate(&mut self, offset: Vec2) {
        for position in &mut self.positions {
            *position += offset;
        }
        for rect in &mut self.tree_bounds {
            *rect = rect.translated(offset);
        }
        self.bounds = self.bounds.translated(offset);
        let depth_offset = match self.direction {
            LayoutDirection::TopDown => offset.y,
            LayoutDirection::LeftRight => offset.x,
        };
        for bus in &mut self.bus_positions {
            *bus += depth_offset;
        }
    }
}

/// An error in the structure of a [`LayoutTree`].
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum LayoutError {
    /// A root is not a node index.
    #[error("root {root} is not a node index (the tree has {len} nodes)")]
    RootOutOfRange {
        /// The root value.
        root: usize,
        /// The number of nodes.
        len: usize,
    },
    /// A child is not a node index.
    #[error(
        "node {parent} has the child {child}, which is not a node index (the tree has {len} nodes)"
    )]
    ChildOutOfRange {
        /// The parent.
        parent: usize,
        /// The child value.
        child: usize,
        /// The number of nodes.
        len: usize,
    },
    /// A node is reached more than once from the roots: it is a root and a
    /// child, a child of two nodes, a root twice, or part of a cycle.
    #[error("node {node} is reached more than once from the roots")]
    NodeReachedTwice {
        /// The node.
        node: usize,
    },
    /// A node cannot be reached from a root.
    #[error("node {node} cannot be reached from a root")]
    NodeNotReachable {
        /// The node.
        node: usize,
    },
    /// The tree has more nodes than the layout can index.
    #[error("the tree has too many nodes ({len})")]
    TooManyNodes {
        /// The number of nodes.
        len: usize,
    },
}

/// A size or a gap that the layout can use: finite and not negative.
fn sanitize(value: f32) -> f64 {
    if value.is_finite() && value > 0.0 {
        f64::from(value)
    } else {
        0.0
    }
}

/// The structure of a valid forest, in the form that the passes use.
struct Structure {
    parent: Vec<u32>,
    depth: Vec<u32>,
    first: Vec<u32>,
    count: Vec<u32>,
    list: Vec<u32>,
    /// All nodes, each tree in pre-order, the trees in root order.
    preorder: Vec<u32>,
    /// The part of `preorder` that holds each tree.
    trees: Vec<Range<usize>>,
}

impl Structure {
    fn build(tree: &LayoutTree) -> Result<Structure, LayoutError> {
        let n = tree.nodes.len();
        if n >= NONE as usize {
            return Err(LayoutError::TooManyNodes { len: n });
        }
        let mut s = Structure {
            parent: vec![NONE; n],
            depth: vec![0; n],
            first: vec![0; n],
            count: vec![0; n],
            list: Vec::with_capacity(n),
            preorder: Vec::with_capacity(n),
            trees: Vec::with_capacity(tree.roots.len()),
        };
        let mut seen = vec![false; n];
        let mut stack: Vec<u32> = Vec::new();
        for &root in &tree.roots {
            if root >= n {
                return Err(LayoutError::RootOutOfRange { root, len: n });
            }
            if seen[root] {
                return Err(LayoutError::NodeReachedTwice { node: root });
            }
            seen[root] = true;
            let start = s.preorder.len();
            stack.push(root as u32);
            while let Some(t) = stack.pop() {
                s.preorder.push(t);
                let tu = t as usize;
                let children = &tree.nodes[tu].children;
                // `list` holds each node at most once, so its length and
                // the child counts are below `n`, which fits in u32.
                s.first[tu] = s.list.len() as u32;
                s.count[tu] = children.len() as u32;
                for &child in children {
                    if child >= n {
                        return Err(LayoutError::ChildOutOfRange {
                            parent: tu,
                            child,
                            len: n,
                        });
                    }
                    if seen[child] {
                        return Err(LayoutError::NodeReachedTwice { node: child });
                    }
                    seen[child] = true;
                    s.parent[child] = t;
                    s.depth[child] = s.depth[tu] + 1;
                    s.list.push(child as u32);
                }
                stack.extend(children.iter().rev().map(|&c| c as u32));
            }
            s.trees.push(start..s.preorder.len());
        }
        if let Some(node) = seen.iter().position(|&reached| !reached) {
            return Err(LayoutError::NodeNotReachable { node });
        }
        Ok(s)
    }
}

/// Places the nodes of a forest. See the [module documentation](self).
///
/// The time is linear in the number of nodes.
///
/// The symmetric mean of the two passes keeps every rule of the module
/// documentation. It can leave a little more space than one pass between
/// the first (or the last) subtree of a node and its siblings: one pass
/// moves the subtree against one sibling, the mirrored pass against
/// another, and the mean is between the two.
pub fn layout(tree: &LayoutTree, params: &LayoutParams) -> Result<Layout, LayoutError> {
    layout_with(tree, params, Passes::Symmetric)
}

/// Which passes [`layout_with`] uses.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Passes {
    /// The mean of the normal and the mirrored pass.
    Symmetric,
    /// Only the normal pass (for tests of the pass itself).
    #[cfg(test)]
    NormalOnly,
}

fn layout_with(
    tree: &LayoutTree,
    params: &LayoutParams,
    passes: Passes,
) -> Result<Layout, LayoutError> {
    let s = Structure::build(tree)?;
    let n = tree.nodes.len();
    let direction = params.direction;
    let level_gap = sanitize(params.level_gap);
    let forest_gap = sanitize(params.forest_gap);

    // Sizes along the breadth axis and the depth axis.
    let sizes: Vec<Vec2> = tree
        .nodes
        .iter()
        .map(|node| Vec2::new(sanitize(node.size.x) as f32, sanitize(node.size.y) as f32))
        .collect();
    let (breadth, depth_size): (Vec<f64>, Vec<f64>) = sizes
        .iter()
        .map(|size| match direction {
            LayoutDirection::TopDown => (f64::from(size.x), f64::from(size.y)),
            LayoutDirection::LeftRight => (f64::from(size.y), f64::from(size.x)),
        })
        .unzip();

    // Positions along the depth axis. `bottom` is where the band of the
    // children starts; the bus is in the middle of the gap before it. Each
    // tree of the forest has its own levels, so an unconnected node cannot
    // move the nodes of the main tree.
    let mut top = vec![0.0f64; n];
    let mut bottom = vec![0.0f64; n];
    if params.layered {
        let mut start: Vec<f64> = Vec::new();
        for range in &s.trees {
            let nodes = &s.preorder[range.clone()];
            start.clear();
            for &t in nodes {
                let (t, d) = (t as usize, s.depth[t as usize] as usize);
                // `start` holds the extents first, then the starts.
                if start.len() <= d {
                    start.resize(d + 1, 0.0);
                }
                start[d] = start[d].max(depth_size[t]);
            }
            let mut position = 0.0;
            for value in &mut start {
                let extent = *value;
                *value = position;
                position += extent + level_gap;
            }
            start.push(position);
            for &t in nodes {
                let (t, d) = (t as usize, s.depth[t as usize] as usize);
                top[t] = start[d];
                bottom[t] = start[d + 1];
            }
        }
    } else {
        for &t in &s.preorder {
            let tu = t as usize;
            let parent = s.parent[tu];
            top[tu] = if parent == NONE {
                0.0
            } else {
                bottom[parent as usize]
            };
            bottom[tu] = top[tu] + depth_size[tu] + level_gap;
        }
    }
    let bus_positions: Vec<f32> = bottom
        .iter()
        .map(|&b| (b - level_gap * 0.5) as f32)
        .collect();

    // The two passes along the breadth axis.
    let shape = Shape {
        first: &s.first,
        count: &s.count,
        list: &s.list,
        parent: &s.parent,
        width: &breadth,
        bottom: &bottom,
        sibling_gap: sanitize(params.sibling_gap),
        subtree_gap: sanitize(params.subtree_gap),
    };
    let normal = Pass::run(&shape, &s.preorder, false);
    let reversed = Pass::run(&shape, &s.preorder, true);

    // The mean of the two results, each tree relative to its root, and the
    // trees one after the other.
    let mut left = vec![0.0f64; n];
    let mut cursor = 0.0f64;
    for range in &s.trees {
        let nodes = &s.preorder[range.clone()];
        let Some(&root) = nodes.first() else {
            continue;
        };
        let root = root as usize;
        let root_normal = normal[root] + breadth[root] * 0.5;
        // The reversed pass gives the mirrored drawing: negate it.
        let root_reversed = -(reversed[root] + breadth[root] * 0.5);
        let mut min = f64::INFINITY;
        let mut max = f64::NEG_INFINITY;
        for &t in nodes {
            let t = t as usize;
            let half = breadth[t] * 0.5;
            let a = (normal[t] + half) - root_normal;
            let b = (-(reversed[t] + half)) - root_reversed;
            let centre = match passes {
                Passes::Symmetric => (a + b) * 0.5,
                #[cfg(test)]
                Passes::NormalOnly => a,
            };
            left[t] = centre - half;
            min = min.min(left[t]);
            max = max.max(centre + half);
        }
        let offset = cursor - min;
        for &t in nodes {
            left[t as usize] += offset;
        }
        cursor += (max - min) + forest_gap;
    }

    let positions: Vec<Vec2> = (0..n)
        .map(|t| {
            let (b, d) = (left[t] as f32, top[t] as f32);
            match direction {
                LayoutDirection::TopDown => Vec2::new(b, d),
                LayoutDirection::LeftRight => Vec2::new(d, b),
            }
        })
        .collect();

    let mut result = Layout {
        direction,
        positions,
        sizes,
        depths: s.depth,
        bus_positions,
        tree_bounds: Vec::with_capacity(s.trees.len()),
        bounds: Rect::ZERO,
    };
    let mut all: Option<Rect> = None;
    for range in &s.trees {
        let mut bounds: Option<Rect> = None;
        for &t in &s.preorder[range.clone()] {
            let rect = result.rect(t as usize).unwrap_or(Rect::ZERO);
            bounds = Some(bounds.map_or(rect, |b| b.union(rect)));
        }
        let bounds = bounds.unwrap_or(Rect::ZERO);
        all = Some(all.map_or(bounds, |a| a.union(bounds)));
        result.tree_bounds.push(bounds);
    }
    result.bounds = all.unwrap_or(Rect::ZERO);
    Ok(result)
}

#[cfg(test)]
mod tests;
