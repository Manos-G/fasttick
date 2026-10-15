//! Property tests of the tidy-tree layout on random forests with random
//! node sizes and random gaps.

use fasttick_core::layout::{
    Layout, LayoutDirection, LayoutNode, LayoutParams, LayoutTree, Rect, layout,
};
use proptest::prelude::*;

/// One raw node: a parent choice, the width, the height and flags.
type RawNode = (u32, f32, f32, u8);

/// Builds a forest from raw nodes. Node 0 is a root. A node is a new root
/// when `flags % 16 == 0`. Otherwise its parent is the node before it
/// (`flags & 2`, which gives deep trees) or a random earlier node. With
/// `flags & 1`, the node becomes the first child, else the last. With
/// `flags & 0x20`, the width is zero.
fn build(raw: &[RawNode]) -> LayoutTree {
    let mut tree = LayoutTree::new();
    for (i, &(choice, w, h, flags)) in raw.iter().enumerate() {
        let w = if flags & 0x20 != 0 { 0.0 } else { w };
        tree.nodes.push(LayoutNode::new((w, h)));
        if i == 0 || flags % 16 == 0 {
            tree.roots.push(i);
            continue;
        }
        let parent = if flags & 2 != 0 {
            i - 1
        } else {
            choice as usize % i
        };
        let children = &mut tree.nodes[parent].children;
        if flags & 1 != 0 {
            children.insert(0, i);
        } else {
            children.push(i);
        }
    }
    tree
}

fn raw_nodes(max: usize) -> impl Strategy<Value = Vec<RawNode>> {
    prop::collection::vec(
        (any::<u32>(), 0.0f32..220.0, 0.0f32..120.0, any::<u8>()),
        1..max,
    )
}

fn params_strategy() -> impl Strategy<Value = LayoutParams> {
    (
        prop_oneof![
            Just(LayoutDirection::TopDown),
            Just(LayoutDirection::LeftRight)
        ],
        0.0f32..40.0,
        0.0f32..60.0,
        0.0f32..90.0,
        0.0f32..120.0,
        any::<bool>(),
    )
        .prop_map(
            |(direction, sibling_gap, subtree_gap, level_gap, forest_gap, layered)| LayoutParams {
                direction,
                sibling_gap,
                subtree_gap,
                level_gap,
                forest_gap,
                layered,
            },
        )
}

/// The parent of each node and the tree (root position) of each node.
fn parents_and_trees(tree: &LayoutTree) -> (Vec<Option<usize>>, Vec<usize>) {
    let n = tree.len();
    let mut parent = vec![None; n];
    let mut owner = vec![usize::MAX; n];
    for (t, &root) in tree.roots.iter().enumerate() {
        let mut stack = vec![root];
        while let Some(v) = stack.pop() {
            owner[v] = t;
            for &c in &tree.nodes[v].children {
                parent[c] = Some(v);
                stack.push(c);
            }
        }
    }
    (parent, owner)
}

/// The interval of a rectangle on the breadth axis.
fn breadth(rect: Rect, direction: LayoutDirection) -> (f32, f32) {
    match direction {
        LayoutDirection::TopDown => (rect.min.x, rect.max.x),
        LayoutDirection::LeftRight => (rect.min.y, rect.max.y),
    }
}

/// The interval of a rectangle on the depth axis.
fn depth(rect: Rect, direction: LayoutDirection) -> (f32, f32) {
    match direction {
        LayoutDirection::TopDown => (rect.min.y, rect.max.y),
        LayoutDirection::LeftRight => (rect.min.x, rect.max.x),
    }
}

fn centre(interval: (f32, f32)) -> f32 {
    (interval.0 + interval.1) * 0.5
}

/// The tolerance for a comparison of coordinates near `scale`.
fn eps(scale: f32) -> f32 {
    0.02 + scale.abs() * 2e-5
}

fn gap(params: &LayoutParams, a: f32, b: f32) -> f32 {
    let _ = (a, b);
    params.sibling_gap.min(params.subtree_gap)
}

/// The depth interval of a node, extended by the level gap (the band in
/// which no other node can be next to it without a gap).
fn band(result: &Layout, node: usize, params: &LayoutParams) -> (f32, f32) {
    let rect = result.rect(node).unwrap();
    let (top, bottom) = depth(rect, params.direction);
    (top, bottom + params.level_gap)
}

/// Checks all rules of the drawing. Returns a message for the first
/// broken rule.
fn check_rules(tree: &LayoutTree, params: &LayoutParams, result: &Layout) -> Result<(), String> {
    let n = tree.len();
    let dir = params.direction;
    let (parent, owner) = parents_and_trees(tree);
    if result.len() != n {
        return Err(format!("{} positions for {n} nodes", result.len()));
    }
    for (i, node) in tree.nodes.iter().enumerate() {
        let rect = result.rect(i).unwrap();
        if !rect.min.is_finite() || !rect.max.is_finite() {
            return Err(format!("node {i} has a position that is not finite"));
        }
        if result.sizes[i] != node.size {
            return Err(format!("node {i} changed its size"));
        }
        if !result.bounds.contains_rect(rect) {
            return Err(format!("node {i} is outside the bounds"));
        }
        if !result.tree_bounds[owner[i]].contains_rect(rect) {
            return Err(format!("node {i} is outside the bounds of its tree"));
        }
    }

    // No overlaps, with the gaps.
    for a in 0..n {
        let ra = result.rect(a).unwrap();
        let (al, ar) = breadth(ra, dir);
        for b in a + 1..n {
            if owner[a] != owner[b] {
                continue;
            }
            let rb = result.rect(b).unwrap();
            let (bl, br) = breadth(rb, dir);
            let required = if parent[a] == parent[b] {
                params.sibling_gap
            } else {
                params.subtree_gap
            };
            let separated_breadth = |e: f32| bl - ar >= required - e || al - br >= required - e;
            let e = eps(ar.max(br));
            if params.layered {
                let (da, db) = (result.depths[a], result.depths[b]);
                if da == db {
                    if !separated_breadth(e) {
                        return Err(format!(
                            "nodes {a} and {b} on one level are too near: {ra:?} {rb:?}"
                        ));
                    }
                } else {
                    let (upper, lower) = if da < db { (ra, rb) } else { (rb, ra) };
                    let (_, upper_end) = depth(upper, dir);
                    let (lower_start, _) = depth(lower, dir);
                    if lower_start - upper_end < params.level_gap - eps(lower_start) {
                        return Err(format!(
                            "nodes {a} and {b} on different levels are too near: {ra:?} {rb:?}"
                        ));
                    }
                }
            } else {
                let (at, ab) = band(result, a, params);
                let (bt, bb) = band(result, b, params);
                let e_depth = eps(ab.max(bb));
                let bands_overlap = at < bb - e_depth && bt < ab - e_depth;
                if bands_overlap && !separated_breadth(e) {
                    return Err(format!(
                        "nodes {a} and {b} overlap: {ra:?} {rb:?} (bands {at}..{ab}, {bt}..{bb})"
                    ));
                }
            }
        }
    }

    // The trees of the forest follow each other.
    for pair in result.tree_bounds.windows(2) {
        let (_, first_end) = breadth(pair[0], dir);
        let (second_start, _) = breadth(pair[1], dir);
        if second_start - first_end < params.forest_gap - eps(second_start) {
            return Err(format!("trees too near: {:?} {:?}", pair[0], pair[1]));
        }
    }

    for (p, node) in tree.nodes.iter().enumerate() {
        let rp = result.rect(p).unwrap();
        let (_, parent_end) = depth(rp, dir);
        // Children after their parent, in order.
        for &c in &node.children {
            let (child_start, _) = depth(result.rect(c).unwrap(), dir);
            if child_start - parent_end < params.level_gap - eps(child_start) {
                return Err(format!("child {c} is not after its parent {p}"));
            }
            if params.layered {
                // All nodes of a level of one tree start together.
                let level = result.depths[c];
                let same_level = (0..n)
                    .filter(|&o| owner[o] == owner[c] && result.depths[o] == level)
                    .all(|o| (depth(result.rect(o).unwrap(), dir).0 - child_start).abs() < 1e-3);
                if !same_level {
                    return Err(format!("level {level} does not start at one position"));
                }
            } else if (child_start - (parent_end + params.level_gap)).abs() > eps(child_start) {
                return Err(format!("child {c} does not start after its parent {p}"));
            }
        }
        for pair in node.children.windows(2) {
            let (_, first_end) = breadth(result.rect(pair[0]).unwrap(), dir);
            let (second_start, _) = breadth(result.rect(pair[1]).unwrap(), dir);
            if second_start - first_end < params.sibling_gap - eps(second_start) {
                return Err(format!("children {} and {} not in order", pair[0], pair[1]));
            }
        }
        // The parent is centred between its first and last child.
        if let (Some(&first), Some(&last)) = (node.children.first(), node.children.last()) {
            let mid = (centre(breadth(result.rect(first).unwrap(), dir))
                + centre(breadth(result.rect(last).unwrap(), dir)))
                * 0.5;
            let own = centre(breadth(rp, dir));
            if (own - mid).abs() > eps(mid) {
                return Err(format!("parent {p} at {own} is not centred at {mid}"));
            }
        }
    }
    let _ = gap;
    Ok(())
}

/// The nodes of the subtree of `root`.
fn subtree(tree: &LayoutTree, root: usize) -> Vec<usize> {
    let mut out = Vec::new();
    let mut stack = vec![root];
    while let Some(v) = stack.pop() {
        out.push(v);
        stack.extend(tree.nodes[v].children.iter().rev());
    }
    out
}

/// The smallest gap between the subtree of `block` and the subtrees of the
/// other children of `parent`, minus the required gap, over all node pairs
/// that must be apart.
fn slack(
    tree: &LayoutTree,
    params: &LayoutParams,
    result: &Layout,
    parent: usize,
    block: usize,
) -> f32 {
    let dir = params.direction;
    let (parents, _) = parents_and_trees(tree);
    let children = &tree.nodes[parent].children;
    let position = children.iter().position(|&c| c == block).unwrap();
    let inside = subtree(tree, block);
    let mut min = f32::INFINITY;
    for (k, &other) in children.iter().enumerate() {
        if k == position {
            continue;
        }
        for a in subtree(tree, other) {
            for &b in &inside {
                let must_be_apart = if params.layered {
                    result.depths[a] == result.depths[b]
                } else {
                    // The same tolerance as `check_rules`: bands that only
                    // touch (up to rounding) do not need a gap.
                    let (at, ab) = band(result, a, params);
                    let (bt, bb) = band(result, b, params);
                    let e_depth = eps(ab.max(bb));
                    at < bb - e_depth && bt < ab - e_depth
                };
                if !must_be_apart {
                    continue;
                }
                let required = if parents[a] == parents[b] {
                    params.sibling_gap
                } else {
                    params.subtree_gap
                };
                let (al, ar) = breadth(result.rect(a).unwrap(), dir);
                let (bl, br) = breadth(result.rect(b).unwrap(), dir);
                let free = if k < position {
                    bl - ar - required
                } else {
                    al - br - required
                };
                min = min.min(free);
            }
        }
    }
    min
}

proptest! {
    #![proptest_config(ProptestConfig::with_cases(300))]

    #[test]
    fn rules_hold_on_random_forests(raw in raw_nodes(120), params in params_strategy()) {
        let tree = build(&raw);
        let result = layout(&tree, &params).unwrap();
        let checked = check_rules(&tree, &params, &result);
        prop_assert!(checked.is_ok(), "{}", checked.unwrap_err());
        // Deterministic.
        prop_assert_eq!(layout(&tree, &params).unwrap(), result);
    }

    #[test]
    fn mirrored_input_gives_the_mirror_image(raw in raw_nodes(120), params in params_strategy()) {
        let tree = build(&raw);
        let mirrored = tree.mirrored();
        let a = layout(&tree, &params).unwrap();
        let b = layout(&mirrored, &params).unwrap();
        let dir = params.direction;
        let (lo, hi) = breadth(a.bounds, dir);
        prop_assert!(lo.abs() < 1e-3);
        let (lo_b, hi_b) = breadth(b.bounds, dir);
        prop_assert!(lo_b.abs() < 1e-3);
        prop_assert!((hi - hi_b).abs() <= eps(hi), "widths {} {}", hi, hi_b);
        for i in 0..tree.len() {
            let ra = a.rect(i).unwrap();
            let rb = b.rect(i).unwrap();
            let ca = centre(breadth(ra, dir));
            let cb = centre(breadth(rb, dir));
            prop_assert!(
                (cb - (hi - ca)).abs() <= eps(hi),
                "node {}: {} is not the mirror of {} (width {})", i, cb, ca, hi
            );
            prop_assert_eq!(depth(ra, dir), depth(rb, dir));
        }
    }

    // The symmetric mean of two passes can leave extra space next to the
    // first and the last subtree (see the docs of `layout`), so this test
    // checks only that they do not overlap. The unit tests in
    // src/layout/tests.rs check that one pass moves them against a sibling.
    #[test]
    fn first_and_last_subtrees_do_not_overlap_their_siblings(
        raw in raw_nodes(60),
        params in params_strategy(),
    ) {
        let tree = build(&raw);
        let result = layout(&tree, &params).unwrap();
        for (p, node) in tree.nodes.iter().enumerate() {
            if node.children.len() < 2 {
                continue;
            }
            for block in [node.children[0], *node.children.last().unwrap()] {
                let free = slack(&tree, &params, &result, p, block);
                let scale = breadth(result.bounds, params.direction).1;
                prop_assert!(free >= -eps(scale), "subtree {} of {} overlaps: {}", block, p, free);
            }
        }
    }

    #[test]
    fn equal_subtrees_get_equal_drawings(
        sub in raw_nodes(25),
        context in raw_nodes(40),
        attach in any::<u32>(),
        params in params_strategy(),
    ) {
        // One copy below a random node of a context tree, one copy below
        // the root.
        let sub_tree = build(&sub);
        let mut tree = LayoutTree::new();
        let root = tree.add_root((80.0, 30.0));
        let context_tree = build(&context);
        let context_root = copy_into(&mut tree, root, &context_tree, context_tree.roots[0]);
        let context_nodes = subtree(&tree, context_root);
        let host = context_nodes[attach as usize % context_nodes.len()];
        let first = copy_into(&mut tree, host, &sub_tree, sub_tree.roots[0]);
        let second = copy_into(&mut tree, root, &sub_tree, sub_tree.roots[0]);
        let result = layout(&tree, &params).unwrap();
        let checked = check_rules(&tree, &params, &result);
        prop_assert!(checked.is_ok(), "{}", checked.unwrap_err());

        let dir = params.direction;
        let one = subtree(&tree, first);
        let two = subtree(&tree, second);
        prop_assert_eq!(one.len(), two.len());
        let base1 = result.rect(first).unwrap();
        let base2 = result.rect(second).unwrap();
        for (&u, &v) in one.iter().zip(&two) {
            let (ru, rv) = (result.rect(u).unwrap(), result.rect(v).unwrap());
            let du = breadth(ru, dir).0 - breadth(base1, dir).0;
            let dv = breadth(rv, dir).0 - breadth(base2, dir).0;
            prop_assert!((du - dv).abs() <= eps(du), "breadth offsets {} {}", du, dv);
            if !params.layered {
                let du = depth(ru, dir).0 - depth(base1, dir).0;
                let dv = depth(rv, dir).0 - depth(base2, dir).0;
                prop_assert!((du - dv).abs() <= eps(du), "depth offsets {} {}", du, dv);
            }
        }
    }
}

/// Copies the subtree of `root` in `source` below `parent` in `target`.
/// Returns the index of the copy of `root`.
fn copy_into(target: &mut LayoutTree, parent: usize, source: &LayoutTree, root: usize) -> usize {
    let copy = target
        .add_child(parent, source.nodes[root].size)
        .expect("valid parent");
    let mut stack = vec![(root, copy)];
    while let Some((from, to)) = stack.pop() {
        for &child in &source.nodes[from].children {
            let new = target
                .add_child(to, source.nodes[child].size)
                .expect("valid parent");
            stack.push((child, new));
        }
    }
    copy
}

#[test]
fn equal_subtrees_in_a_fixed_tree() {
    // root: [A, B, A'] where A' is a copy of A and B is wide.
    let spec: Vec<RawNode> = vec![
        (0, 50.0, 20.0, 0x10),
        (0, 30.0, 20.0, 0x04),
        (0, 120.0, 20.0, 0x04),
        (1, 10.0, 60.0, 0x04),
    ];
    let a = build(&spec);
    let mut tree = LayoutTree::new();
    let root = tree.add_root((10.0, 10.0));
    let first = copy_into(&mut tree, root, &a, 0);
    let wide = tree.add_child(root, (500.0, 10.0)).unwrap();
    tree.add_child(wide, (700.0, 10.0)).unwrap();
    let second = copy_into(&mut tree, root, &a, 0);
    for layered in [true, false] {
        let params = LayoutParams {
            layered,
            ..LayoutParams::default()
        };
        let result = layout(&tree, &params).unwrap();
        check_rules(&tree, &params, &result).unwrap();
        let one = subtree(&tree, first);
        let two = subtree(&tree, second);
        for (&u, &v) in one.iter().zip(&two) {
            let du = result.positions[u] - result.positions[first];
            let dv = result.positions[v] - result.positions[second];
            assert!((du.x - dv.x).abs() < 1e-3 && (du.y - dv.y).abs() < 1e-3);
        }
    }
}
