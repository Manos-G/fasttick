//! Synthetic trees for benchmarks, performance tests and demos.

use fasttick_core::runtime::{NodeIndex, Uid};

use super::model::{CanvasNode, CanvasTree, CanvasTreeBuilder, NodeCategory, NodeSource, PortLine};

/// A tree with about `nodes` nodes: controls with `fanout` children down
/// to a fixed depth, then leaves with two ports. The node at index `i` has
/// the UID `i + 1`. The result is the same for the same arguments.
pub fn synthetic_tree(nodes: usize, fanout: usize) -> CanvasTree {
    let fanout = fanout.max(2);
    let nodes = nodes.max(1);
    // The depth at which a full tree reaches `nodes`.
    let mut depth = 0;
    let mut total = 1usize;
    let mut level = 1usize;
    while total < nodes {
        level = level.saturating_mul(fanout);
        total = total.saturating_add(level);
        depth += 1;
    }
    let mut builder = CanvasTreeBuilder::new();
    let mut stack: Vec<(Option<super::model::CanvasNodeId>, usize)> = vec![(None, 0)];
    while let Some((parent, node_depth)) = stack.pop() {
        if builder.len() >= nodes {
            break;
        }
        let index = builder.len();
        let leaf = node_depth >= depth;
        let (type_id, category) = if leaf {
            if index % 3 == 0 {
                ("CheckBattery", NodeCategory::Condition)
            } else {
                ("MoveBase", NodeCategory::Action)
            }
        } else if node_depth % 2 == 0 {
            ("Sequence", NodeCategory::Control)
        } else {
            ("Fallback", NodeCategory::Control)
        };
        let mut node = CanvasNode::new(
            NodeSource::Runtime(NodeIndex(index)),
            format!("{}_{index}", type_id.to_ascii_lowercase()),
            type_id,
        );
        node.category = category;
        node.uid = u16::try_from(index + 1).ok().map(Uid);
        if leaf {
            node.ports = vec![
                PortLine {
                    name: "goal".into(),
                    value: format!("{{target_{}}}", index % 17),
                },
                PortLine {
                    name: "timeout_ms".into(),
                    value: (index % 500).to_string(),
                },
            ];
        }
        let id = builder.push(parent, node);
        if !leaf {
            for _ in 0..fanout {
                stack.push((Some(id), node_depth + 1));
            }
        }
    }
    builder.finish("Synthetic")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sizes_and_structure() {
        let tree = synthetic_tree(5000, 6);
        assert_eq!(tree.len(), 5000);
        assert_eq!(tree.nodes()[0].depth, 0);
        assert!(tree.find_uid(Uid(5000)).is_some());
        for node in tree.nodes() {
            if let Some(parent) = node.parent {
                assert_eq!(tree.get(parent).unwrap().depth + 1, node.depth);
            }
        }
        assert_eq!(synthetic_tree(1, 3).len(), 1);
        assert_eq!(synthetic_tree(10, 0).len(), 10);
    }
}
