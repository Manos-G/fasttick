//! Speed of the layout on large trees. The time limits apply only to
//! release builds (`cargo test --release`); debug builds only run the code.

use std::time::{Duration, Instant};

use fasttick_core::layout::{LayoutDirection, LayoutParams, LayoutTree, layout};

/// A deterministic tree: each node has 0 to 5 children, sizes vary.
fn big_tree(nodes: usize) -> LayoutTree {
    let mut tree = LayoutTree::with_capacity(nodes);
    let root = tree.add_root((120.0, 40.0));
    let mut open = std::collections::VecDeque::from([root]);
    let mut seed: u64 = 0x9E37_79B9_7F4A_7C15;
    let mut next = || {
        seed ^= seed << 13;
        seed ^= seed >> 7;
        seed ^= seed << 17;
        seed
    };
    while tree.len() < nodes {
        let Some(parent) = open.pop_front() else {
            break;
        };
        let children = 1 + (next() % 5) as usize;
        for _ in 0..children {
            if tree.len() >= nodes {
                break;
            }
            let width = 60.0 + (next() % 140) as f32;
            let height = 30.0 + (next() % 30) as f32;
            if let Some(child) = tree.add_child(parent, (width, height)) {
                open.push_back(child);
            }
        }
    }
    tree
}

fn time_layout(tree: &LayoutTree, params: &LayoutParams) -> Duration {
    // Take the best of a few runs to reduce noise.
    (0..5)
        .map(|_| {
            let start = Instant::now();
            let result = layout(tree, params).expect("valid tree");
            let elapsed = start.elapsed();
            assert_eq!(result.len(), tree.len());
            elapsed
        })
        .min()
        .unwrap_or_default()
}

#[test]
fn ten_thousand_nodes_are_fast() {
    let tree = big_tree(10_000);
    assert_eq!(tree.len(), 10_000);
    for direction in [LayoutDirection::TopDown, LayoutDirection::LeftRight] {
        for layered in [true, false] {
            let params = LayoutParams {
                layered,
                ..LayoutParams::new(direction)
            };
            let elapsed = time_layout(&tree, &params);
            eprintln!("layout of 10,000 nodes ({direction:?}, layered {layered}): {elapsed:?}");
            if !cfg!(debug_assertions) {
                assert!(
                    elapsed < Duration::from_millis(20),
                    "layout of 10,000 nodes took {elapsed:?}"
                );
            }
        }
    }
}

#[test]
fn a_deep_chain_of_ten_thousand_nodes_is_fast() {
    let mut tree = LayoutTree::with_capacity(10_000);
    let mut parent = tree.add_root((80.0, 30.0));
    for _ in 1..10_000 {
        parent = tree.add_child(parent, (80.0, 30.0)).expect("parent exists");
    }
    let elapsed = time_layout(&tree, &LayoutParams::default());
    eprintln!("layout of a 10,000-node chain: {elapsed:?}");
    if !cfg!(debug_assertions) {
        assert!(elapsed < Duration::from_millis(20), "took {elapsed:?}");
    }
}
