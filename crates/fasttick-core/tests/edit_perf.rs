//! Speed of the edit commands and of undo on a tree with 5,000 nodes. The
//! time limits apply only to optimized builds (`cargo test --release`).

use std::time::Instant;

use fasttick_core::bt::BtFile;
use fasttick_core::edit::History;
use fasttick_core::project::Project;
use fasttick_core::xml::NodeId;

/// A tree with 5,001 nodes: a root Sequence with 50 Sequences of 99
/// actions each.
fn large_tree() -> String {
    let mut text = String::from(
        "<root BTCPP_format=\"4\" main_tree_to_execute=\"Main\">\n  <BehaviorTree ID=\"Main\">\n    <Sequence name=\"root\">\n",
    );
    for g in 0..50 {
        text.push_str(&format!("      <Sequence name=\"g{g}\">\n"));
        for i in 0..99 {
            text.push_str(&format!(
                "        <Action{} name=\"n{g}_{i}\" value=\"{{bb_{i}}}\" _skipIf=\"x == {i}\"/>\n",
                i % 7
            ));
        }
        text.push_str("      </Sequence>\n");
    }
    text.push_str("    </Sequence>\n  </BehaviorTree>\n  <TreeNodesModel>\n");
    for i in 0..7 {
        text.push_str(&format!(
            "    <Action ID=\"Action{i}\">\n      <input_port name=\"value\"/>\n    </Action>\n"
        ));
    }
    text.push_str("  </TreeNodesModel>\n</root>\n");
    text
}

/// A small deterministic random number generator.
struct Lcg(u64);

impl Lcg {
    fn next(&mut self, n: usize) -> usize {
        self.0 = self
            .0
            .wrapping_mul(6_364_136_223_846_793_005)
            .wrapping_add(1_442_695_040_888_963_407);
        ((self.0 >> 33) as usize) % n
    }
}

#[test]
fn thousand_commands_and_undo_on_5000_nodes() {
    let text = large_tree();
    let (mut p, f) = Project::with_file("perf", BtFile::parse(&text, None).unwrap());
    let doc = p.file(f).unwrap().doc();
    let root = p.file(f).unwrap().tree("Main").unwrap().root.unwrap();
    let groups: Vec<NodeId> = doc.element_children(root).collect();
    let mut leaves: Vec<NodeId> = groups
        .iter()
        .flat_map(|&g| doc.element_children(g).collect::<Vec<_>>())
        .collect();
    let count = doc.descendants(root).filter(|&n| doc.is_element(n)).count();
    assert_eq!(count, 5001);

    let mut history = History::new();
    history.set_max_depth(2000);
    let mut rng = Lcg(7);
    let start = Instant::now();
    for i in 0..1000 {
        let leaf = leaves[rng.next(leaves.len())];
        let group = groups[rng.next(groups.len())];
        match i % 6 {
            0 => {
                let node = history
                    .run(&mut p, |ed| ed.add_node(f, group, 0, "AlwaysSuccess", None))
                    .unwrap();
                leaves.push(node);
            }
            1 => history
                .run(&mut p, |ed| {
                    ed.set_attribute(f, leaf, "value", &i.to_string())
                })
                .unwrap(),
            2 => history
                .run(&mut p, |ed| ed.move_node(f, leaf, group, 1))
                .unwrap(),
            3 => {
                history
                    .run(&mut p, |ed| ed.delete_nodes(f, &[leaf]))
                    .unwrap();
                leaves.retain(|&n| n != leaf);
            }
            4 => {
                let copies = history.run(&mut p, |ed| ed.duplicate(f, &[leaf])).unwrap();
                leaves.extend(copies);
            }
            _ => history
                .run(&mut p, |ed| {
                    ed.set_instance_name(f, leaf, Some(&format!("renamed{i}")))?;
                    ed.set_port_value(f, leaf, "other", Some("{x}"))
                })
                .unwrap(),
        }
    }
    let commands = start.elapsed();
    // A few moves go to the place where the node is: no step.
    assert!(history.undo_len() > 990, "{}", history.undo_len());
    let undo_start = Instant::now();
    while history.undo(&mut p).unwrap().is_some() {}
    let undo = undo_start.elapsed();
    assert_eq!(p.file(f).unwrap().to_xml_string(), text);
    assert!(!p.file(f).unwrap().is_dirty());
    let redo_start = Instant::now();
    while history.redo(&mut p).unwrap().is_some() {}
    let redo = redo_start.elapsed();
    let total = start.elapsed() - redo;
    eprintln!("1000 commands: {commands:?}, 1000 undos: {undo:?}, 1000 redos: {redo:?}");
    #[cfg(not(debug_assertions))]
    assert!(
        total < std::time::Duration::from_millis(250),
        "1000 commands and 1000 undos took {total:?}"
    );
    #[cfg(debug_assertions)]
    let _ = total;
}

/// A flat tree: one Sequence with 5,000 children. Each insert and each
/// removal scans the siblings.
#[test]
fn commands_on_a_wide_parent() {
    let mut text =
        String::from("<root BTCPP_format=\"4\">\n  <BehaviorTree ID=\"Main\">\n    <Sequence>\n");
    for i in 0..5000 {
        text.push_str(&format!("      <A{} x=\"{i}\"/>\n", i % 10));
    }
    text.push_str("    </Sequence>\n  </BehaviorTree>\n</root>\n");
    let (mut p, f) = Project::with_file("wide", BtFile::parse(&text, None).unwrap());
    let root = p.file(f).unwrap().tree("Main").unwrap().root.unwrap();
    let kids: Vec<NodeId> = p.file(f).unwrap().doc().element_children(root).collect();
    let mut history = History::with_max_depth(2000);
    let mut rng = Lcg(11);
    let start = Instant::now();
    for i in 0..1000 {
        let node = kids[rng.next(kids.len())];
        let index = rng.next(4999);
        match i % 3 {
            0 => {
                history
                    .run(&mut p, |ed| {
                        ed.add_node(f, root, index, "AlwaysSuccess", None)
                    })
                    .unwrap();
            }
            1 => {
                let _ = history.run(&mut p, |ed| ed.move_node(f, node, root, index));
            }
            _ => {
                let _ = history.run(&mut p, |ed| ed.duplicate(f, &[node]));
            }
        }
    }
    while history.undo(&mut p).unwrap().is_some() {}
    let total = start.elapsed();
    assert_eq!(p.file(f).unwrap().to_xml_string(), text);
    eprintln!("1000 commands and undos on a parent with 5,000 children: {total:?}");
    #[cfg(not(debug_assertions))]
    assert!(
        total < std::time::Duration::from_millis(500),
        "took {total:?}"
    );
}

/// Tree commands refresh the project (trees, models): 100 trees with
/// 10,000 nodes and 100 models.
#[test]
fn tree_commands_on_a_large_project() {
    let mut text = String::from("<root BTCPP_format=\"4\">\n");
    for t in 0..100 {
        text.push_str(&format!("  <BehaviorTree ID=\"T{t}\">\n    <Sequence>\n"));
        for i in 0..99 {
            text.push_str(&format!(
                "      <SubTree ID=\"T{}\" k=\"{{v{i}}}\"/>\n",
                (t + 1) % 100
            ));
        }
        text.push_str("    </Sequence>\n  </BehaviorTree>\n");
    }
    text.push_str("  <TreeNodesModel>\n");
    for i in 0..100 {
        text.push_str(&format!("    <Action ID=\"M{i}\"/>\n"));
    }
    text.push_str("  </TreeNodesModel>\n</root>\n");
    let (mut p, f) = Project::with_file("large", BtFile::parse(&text, None).unwrap());
    let mut history = History::new();
    let start = Instant::now();
    for i in 0..50 {
        history
            .run(&mut p, |ed| ed.add_tree(f, &format!("New{i}")))
            .unwrap();
        // 99 references in one tree.
        let count = history
            .run(&mut p, |ed| {
                ed.rename_tree(f, &format!("T{i}"), &format!("R{i}"))
            })
            .unwrap();
        assert_eq!(count, 99);
    }
    while history.undo(&mut p).unwrap().is_some() {}
    let total = start.elapsed();
    assert_eq!(p.file(f).unwrap().to_xml_string(), text);
    eprintln!("100 tree commands and undos on 100 trees: {total:?}");
    #[cfg(not(debug_assertions))]
    assert!(
        total < std::time::Duration::from_millis(1000),
        "took {total:?}"
    );
}
