//! Unit tests of the layout. The property tests are in
//! `tests/layout_props.rs`.

use super::*;

fn params(direction: LayoutDirection) -> LayoutParams {
    LayoutParams {
        direction,
        sibling_gap: 10.0,
        subtree_gap: 20.0,
        level_gap: 30.0,
        forest_gap: 50.0,
        layered: true,
    }
}

fn close(a: f32, b: f32) -> bool {
    (a - b).abs() < 1e-3
}

#[test]
fn empty_tree() {
    let result = layout(&LayoutTree::new(), &LayoutParams::default()).unwrap();
    assert!(result.is_empty());
    assert_eq!(result.len(), 0);
    assert_eq!(result.bounds, Rect::ZERO);
    assert!(result.tree_bounds.is_empty());
    assert!(result.bus_positions.is_empty());
    assert_eq!(result.rect(0), None);
    assert_eq!(result.bus_position(0), None);
}

#[test]
fn single_node() {
    let mut tree = LayoutTree::new();
    tree.add_root((40.0, 20.0));
    let result = layout(&tree, &params(LayoutDirection::TopDown)).unwrap();
    assert_eq!(result.positions, vec![Vec2::ZERO]);
    assert_eq!(
        result.bounds,
        Rect::from_min_size(Vec2::ZERO, Vec2::new(40.0, 20.0))
    );
    assert_eq!(result.bus_positions, vec![35.0]);
    assert_eq!(result.bus_position(0), Some(35.0));
}

#[test]
fn two_leaves_and_a_wide_parent() {
    let mut tree = LayoutTree::new();
    let root = tree.add_root((200.0, 40.0));
    let a = tree.add_child(root, (20.0, 10.0)).unwrap();
    let b = tree.add_child(root, (30.0, 20.0)).unwrap();
    let result = layout(&tree, &params(LayoutDirection::TopDown)).unwrap();
    let (ra, rb, rr) = (
        result.rect(a).unwrap(),
        result.rect(b).unwrap(),
        result.rect(root).unwrap(),
    );
    assert!(close(rb.min.x - ra.max.x, 10.0));
    assert!(close(ra.min.y, 70.0));
    assert!(close(rb.min.y, 70.0));
    assert!(close(rr.center().x, (ra.center().x + rb.center().x) / 2.0));
    assert_eq!(result.bounds.min, Vec2::ZERO);
    assert!(close(result.bounds.width(), 200.0));
    assert!(close(result.bounds.height(), 40.0 + 30.0 + 20.0));
    assert_eq!(result.depths, vec![0, 1, 1]);
    assert_eq!(result.edge_start(root), Some(Vec2::new(100.0, 40.0)));
    assert_eq!(result.edge_end(a), Some(Vec2::new(ra.center().x, ra.min.y)));
    assert_eq!(result.rects().count(), 3);
}

#[test]
fn cousins_use_the_subtree_gap() {
    // Two parents, each with one child of the same width.
    let mut tree = LayoutTree::new();
    let root = tree.add_root((10.0, 10.0));
    let p1 = tree.add_child(root, (10.0, 10.0)).unwrap();
    let p2 = tree.add_child(root, (10.0, 10.0)).unwrap();
    let c1 = tree.add_child(p1, (10.0, 10.0)).unwrap();
    let c2 = tree.add_child(p2, (10.0, 10.0)).unwrap();
    let result = layout(&tree, &params(LayoutDirection::TopDown)).unwrap();
    let gap = |a: usize, b: usize| result.rect(b).unwrap().min.x - result.rect(a).unwrap().max.x;
    assert!(close(gap(c1, c2), 20.0));
    // The parents are above their children, so they have the same gap.
    assert!(close(gap(p1, p2), 20.0));
}

#[test]
fn a_subtree_fits_below_a_wide_leaf_sibling() {
    // A tidy tree may put the children of a node below a wide sibling.
    let mut tree = LayoutTree::new();
    let root = tree.add_root((10.0, 10.0));
    let wide = tree.add_child(root, (300.0, 10.0)).unwrap();
    let p = tree.add_child(root, (10.0, 10.0)).unwrap();
    let c1 = tree.add_child(p, (100.0, 10.0)).unwrap();
    let c2 = tree.add_child(p, (100.0, 10.0)).unwrap();
    let result = layout(&tree, &params(LayoutDirection::TopDown)).unwrap();
    let r = |i: usize| result.rect(i).unwrap();
    assert!(close(r(p).min.x - r(wide).max.x, 10.0));
    // c1 starts below the wide leaf.
    assert!(r(c1).min.x < r(wide).max.x);
    assert!(close(r(c2).min.x - r(c1).max.x, 10.0));
}

#[test]
fn small_subtrees_between_large_ones_are_spaced_equally() {
    // root: [big, leaf, leaf, big]; each big subtree is a parent with
    // three wide children.
    let mut tree = LayoutTree::new();
    let root = tree.add_root((10.0, 10.0));
    let big1 = tree.add_child(root, (10.0, 10.0)).unwrap();
    let l1 = tree.add_child(root, (10.0, 10.0)).unwrap();
    let l2 = tree.add_child(root, (10.0, 10.0)).unwrap();
    let big2 = tree.add_child(root, (10.0, 10.0)).unwrap();
    for big in [big1, big2] {
        for _ in 0..3 {
            tree.add_child(big, (100.0, 10.0)).unwrap();
        }
    }
    let result = layout(&tree, &params(LayoutDirection::TopDown)).unwrap();
    let centre = |i: usize| result.rect(i).unwrap().center().x;
    let d1 = centre(l1) - centre(big1);
    let d2 = centre(l2) - centre(l1);
    let d3 = centre(big2) - centre(l2);
    assert!(close(d1, d2), "{d1} {d2}");
    assert!(close(d2, d3), "{d2} {d3}");
    // The large subtrees are as near as their children allow.
    let last_of_big1 = tree.nodes[big1].children[2];
    let first_of_big2 = tree.nodes[big2].children[0];
    let gap = result.rect(first_of_big2).unwrap().min.x - result.rect(last_of_big1).unwrap().max.x;
    assert!(close(gap, 20.0), "{gap}");
}

#[test]
fn left_right_swaps_the_axes() {
    let mut tree = LayoutTree::new();
    let root = tree.add_root((100.0, 40.0));
    let a = tree.add_child(root, (60.0, 30.0)).unwrap();
    let b = tree.add_child(root, (80.0, 20.0)).unwrap();
    let result = layout(&tree, &params(LayoutDirection::LeftRight)).unwrap();
    let (ra, rb) = (result.rect(a).unwrap(), result.rect(b).unwrap());
    assert!(close(ra.min.x, 130.0));
    assert!(close(rb.min.x, 130.0));
    // The first child is at the top.
    assert!(close(rb.min.y - ra.max.y, 10.0));
    assert_eq!(result.bus_positions, vec![115.0, 225.0, 225.0]);
    assert_eq!(result.bus_position(root), Some(115.0));
    assert_eq!(
        result.edge_start(root),
        Some(Vec2::new(100.0, result.rect(root).unwrap().center().y))
    );
    assert_eq!(result.edge_end(a), Some(Vec2::new(130.0, ra.center().y)));
}

#[test]
fn left_right_is_top_down_with_the_axes_swapped() {
    let mut tree = LayoutTree::new();
    let root = tree.add_root((100.0, 40.0));
    let a = tree.add_child(root, (60.0, 30.0)).unwrap();
    tree.add_child(a, (10.0, 70.0)).unwrap();
    tree.add_child(a, (20.0, 50.0)).unwrap();
    tree.add_child(root, (80.0, 20.0)).unwrap();
    let mut swapped = tree.clone();
    for node in &mut swapped.nodes {
        node.size = node.size.swapped();
    }
    let lr = layout(&tree, &params(LayoutDirection::LeftRight)).unwrap();
    let td = layout(&swapped, &params(LayoutDirection::TopDown)).unwrap();
    for (p, q) in lr.positions.iter().zip(&td.positions) {
        assert_eq!(*p, q.swapped());
    }
    assert_eq!(lr.bounds.size(), td.bounds.size().swapped());
}

#[test]
fn not_layered_starts_children_after_their_parent() {
    let mut tree = LayoutTree::new();
    let root = tree.add_root((10.0, 10.0));
    let tall = tree.add_child(root, (10.0, 100.0)).unwrap();
    let short = tree.add_child(root, (10.0, 10.0)).unwrap();
    let below_tall = tree.add_child(tall, (10.0, 10.0)).unwrap();
    let below_short = tree.add_child(short, (10.0, 10.0)).unwrap();
    let mut p = params(LayoutDirection::TopDown);
    p.layered = false;
    let result = layout(&tree, &p).unwrap();
    // Each node has its own bus position.
    assert_eq!(result.bus_position(tall), Some(40.0 + 100.0 + 15.0));
    assert!(close(
        result.rect(below_tall).unwrap().min.y,
        10.0 + 30.0 + 100.0 + 30.0
    ));
    assert!(close(
        result.rect(below_short).unwrap().min.y,
        10.0 + 30.0 + 10.0 + 30.0
    ));
    assert_eq!(result.bus_position(short), Some(40.0 + 10.0 + 15.0));
}

#[test]
fn layered_and_not_layered_differ_next_to_a_tall_node() {
    // root: [tall leaf, short node]; the short node has a wide child.
    let mut tree = LayoutTree::new();
    let root = tree.add_root((10.0, 10.0));
    let tall = tree.add_child(root, (10.0, 100.0)).unwrap();
    let short = tree.add_child(root, (10.0, 10.0)).unwrap();
    let wide = tree.add_child(short, (300.0, 10.0)).unwrap();
    let r = |l: &Layout, i: usize| l.rect(i).unwrap();

    // Not layered: the wide child starts after its short parent, so it is
    // next to the tall leaf (they overlap on the y axis). The two have
    // different parents, so the subtree gap applies.
    let mut p = params(LayoutDirection::TopDown);
    p.layered = false;
    let compact = layout(&tree, &p).unwrap();
    assert!(close(r(&compact, wide).min.y, 10.0 + 30.0 + 10.0 + 30.0));
    assert!(close(
        r(&compact, wide).min.x - r(&compact, tall).max.x,
        20.0
    ));

    // Layered: the wide child is on level 2, below the tall leaf, so it
    // can extend below it.
    p.layered = true;
    let layered = layout(&tree, &p).unwrap();
    assert!(close(r(&layered, wide).min.y, 10.0 + 30.0 + 100.0 + 30.0));
    assert!(r(&layered, wide).min.x < r(&layered, tall).min.x);
    assert!(close(
        r(&layered, short).min.x - r(&layered, tall).max.x,
        10.0
    ));
}

#[test]
fn forest_trees_follow_each_other() {
    let mut tree = LayoutTree::new();
    let r1 = tree.add_root((50.0, 10.0));
    tree.add_child(r1, (100.0, 10.0)).unwrap();
    let r2 = tree.add_root((30.0, 30.0));
    let r3 = tree.add_root((10.0, 10.0));
    let result = layout(&tree, &params(LayoutDirection::TopDown)).unwrap();
    assert_eq!(result.tree_bounds.len(), 3);
    assert!(close(result.tree_bounds[0].min.x, 0.0));
    assert!(close(result.tree_bounds[0].width(), 100.0));
    assert!(close(result.rect(r2).unwrap().min.x, 150.0));
    assert!(close(result.rect(r3).unwrap().min.x, 230.0));
    for root in [r1, r2, r3] {
        assert_eq!(result.rect(root).unwrap().min.y, 0.0);
    }
    assert!(close(result.bounds.width(), 240.0));
    assert!(close(result.bounds.height(), 50.0));
}

#[test]
fn bad_sizes_and_gaps_count_as_zero() {
    let mut tree = LayoutTree::new();
    let root = tree.add_root((f32::NAN, -5.0));
    tree.add_child(root, (f32::INFINITY, 10.0)).unwrap();
    tree.add_child(root, (10.0, f32::NEG_INFINITY)).unwrap();
    let p = LayoutParams {
        sibling_gap: f32::NAN,
        subtree_gap: -1.0,
        level_gap: f32::INFINITY,
        ..LayoutParams::default()
    };
    let result = layout(&tree, &p).unwrap();
    for rect in result.rects() {
        assert!(rect.min.is_finite() && rect.max.is_finite());
        assert!(rect.width() >= 0.0 && rect.height() >= 0.0);
    }
    assert_eq!(result.sizes[0], Vec2::ZERO);
    assert_eq!(result.bus_positions, vec![0.0, 10.0, 10.0]);
}

#[test]
fn structure_errors() {
    let node = |children: Vec<usize>| LayoutNode::with_children((1.0, 1.0), children);
    let check = |nodes: Vec<LayoutNode>, roots: Vec<usize>, expected: LayoutError| {
        let tree = LayoutTree { nodes, roots };
        assert_eq!(layout(&tree, &LayoutParams::default()), Err(expected));
    };
    check(
        vec![node(vec![])],
        vec![1],
        LayoutError::RootOutOfRange { root: 1, len: 1 },
    );
    check(
        vec![node(vec![5])],
        vec![0],
        LayoutError::ChildOutOfRange {
            parent: 0,
            child: 5,
            len: 1,
        },
    );
    // A cycle back to the root.
    check(
        vec![node(vec![1]), node(vec![0])],
        vec![0],
        LayoutError::NodeReachedTwice { node: 0 },
    );
    // A child twice in one list.
    check(
        vec![node(vec![1, 1]), node(vec![])],
        vec![0],
        LayoutError::NodeReachedTwice { node: 1 },
    );
    // A root that is also a child.
    check(
        vec![node(vec![1]), node(vec![])],
        vec![0, 1],
        LayoutError::NodeReachedTwice { node: 1 },
    );
    check(
        vec![node(vec![]), node(vec![])],
        vec![0],
        LayoutError::NodeNotReachable { node: 1 },
    );
    // A cycle that no root reaches.
    check(
        vec![node(vec![]), node(vec![2]), node(vec![1])],
        vec![0],
        LayoutError::NodeNotReachable { node: 1 },
    );
    check(
        vec![node(vec![])],
        vec![],
        LayoutError::NodeNotReachable { node: 0 },
    );
    assert!(
        LayoutError::NodeNotReachable { node: 1 }
            .to_string()
            .contains("cannot be reached")
    );
}

#[test]
fn add_child_with_a_bad_parent() {
    let mut tree = LayoutTree::with_capacity(4);
    assert_eq!(tree.add_child(0, (1.0, 1.0)), None);
    assert!(tree.is_empty());
    let root = tree.add_root((1.0, 1.0));
    assert_eq!(tree.add_child(root, (1.0, 1.0)), Some(1));
    assert_eq!(tree.len(), 2);
}

#[test]
fn mirrored_tree() {
    let tree = LayoutTree {
        nodes: vec![
            LayoutNode::with_children((1.0, 1.0), vec![1, 2]),
            LayoutNode::new((2.0, 2.0)),
            LayoutNode::new((3.0, 3.0)),
            LayoutNode::new((4.0, 4.0)),
        ],
        roots: vec![0, 3],
    };
    let m = tree.mirrored();
    assert_eq!(m.roots, vec![3, 0]);
    assert_eq!(m.nodes[0].children, vec![2, 1]);
    assert_eq!(m.mirrored(), tree);
}

#[test]
fn translate_moves_everything() {
    let mut tree = LayoutTree::new();
    let root = tree.add_root((10.0, 10.0));
    tree.add_child(root, (10.0, 10.0)).unwrap();
    let mut result = layout(&tree, &params(LayoutDirection::TopDown)).unwrap();
    let before = result.clone();
    result.translate(Vec2::new(5.0, 7.0));
    assert_eq!(
        result.positions[1],
        before.positions[1] + Vec2::new(5.0, 7.0)
    );
    assert_eq!(result.bounds, before.bounds.translated(Vec2::new(5.0, 7.0)));
    assert_eq!(
        result.tree_bounds[0],
        before.tree_bounds[0].translated(Vec2::new(5.0, 7.0))
    );
    assert_eq!(result.bus_positions, vec![32.0, 72.0]);
    assert_eq!(
        result.bus_position(0),
        before.bus_position(0).map(|v| v + 7.0)
    );
}

#[test]
fn deep_chain_does_not_overflow_the_stack() {
    let mut tree = LayoutTree::new();
    let mut last = tree.add_root((10.0, 10.0));
    for _ in 0..100_000 {
        last = tree.add_child(last, (10.0, 10.0)).unwrap();
    }
    let result = layout(&tree, &params(LayoutDirection::TopDown)).unwrap();
    assert!(result.positions.iter().all(|p| p.x == 0.0));
    assert_eq!(result.bus_position(100_000), Some(100_000.0 * 40.0 + 25.0));
}

#[test]
fn default_params() {
    let p = LayoutParams::default();
    assert_eq!(p.direction, LayoutDirection::TopDown);
    assert!(p.layered);
    let q = LayoutParams::new(LayoutDirection::LeftRight);
    assert_eq!(q.direction, LayoutDirection::LeftRight);
    assert!(q.sibling_gap > 0.0 && q.subtree_gap >= q.sibling_gap);
}

/// The nodes of the subtree of `root`, in pre-order.
fn subtree_nodes(tree: &LayoutTree, root: usize) -> Vec<usize> {
    let mut out = Vec::new();
    let mut stack = vec![root];
    while let Some(t) = stack.pop() {
        out.push(t);
        stack.extend(tree.nodes[t].children.iter().rev());
    }
    out
}

/// The smallest free space between the subtree of `block` and the subtrees
/// of the other children of `parent`, on the levels where both have nodes.
fn single_pass_slack(
    tree: &LayoutTree,
    params: &LayoutParams,
    result: &Layout,
    parent: usize,
    block: usize,
) -> f32 {
    let children = &tree.nodes[parent].children;
    let position = children.iter().position(|&c| c == block).unwrap_or(0);
    let inside = subtree_nodes(tree, block);
    let mut parent_of = vec![usize::MAX; tree.nodes.len()];
    for (p, node) in tree.nodes.iter().enumerate() {
        for &c in &node.children {
            parent_of[c] = p;
        }
    }
    let mut min = f32::INFINITY;
    for (k, &other) in children.iter().enumerate() {
        if k == position {
            continue;
        }
        for a in subtree_nodes(tree, other) {
            for &b in &inside {
                if result.depths[a] != result.depths[b] {
                    continue;
                }
                let required = if parent_of[a] == parent_of[b] {
                    params.sibling_gap
                } else {
                    params.subtree_gap
                };
                let (ra, rb) = (
                    result.rect(a).unwrap_or(Rect::ZERO),
                    result.rect(b).unwrap_or(Rect::ZERO),
                );
                let free = if k < position {
                    rb.min.x - ra.max.x - required
                } else {
                    ra.min.x - rb.max.x - required
                };
                min = min.min(free);
            }
        }
    }
    min
}

proptest::proptest! {
    /// One normal pass moves the first and the last subtree of each node
    /// against a sibling: neither can move nearer to its siblings.
    #[test]
    fn one_pass_moves_outer_subtrees_against_a_sibling(
        sizes in proptest::collection::vec((0.0f32..120.0, 0.0f32..60.0, proptest::prelude::any::<u32>()), 1..50),
    ) {
        let mut tree = LayoutTree::new();
        tree.add_root((sizes[0].0, sizes[0].1));
        for (i, &(w, h, pick)) in sizes.iter().enumerate().skip(1) {
            let parent = pick as usize % i;
            tree.add_child(parent, (w, h));
        }
        let params = params(LayoutDirection::TopDown);
        let result = layout_with(&tree, &params, Passes::NormalOnly).unwrap();
        let scale = result.bounds.width().max(1.0);
        for (p, node) in tree.nodes.iter().enumerate() {
            if node.children.len() < 2 {
                continue;
            }
            for block in [node.children[0], node.children[node.children.len() - 1]] {
                let free = single_pass_slack(&tree, &params, &result, p, block);
                proptest::prop_assert!(free.abs() <= scale * 1e-4 + 1e-3,
                    "subtree {} of {}: free space {}", block, p, free);
            }
        }
    }
}
