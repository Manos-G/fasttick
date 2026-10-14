//! Speed of the BT view on a large file. The time limits apply only to
//! optimized builds (`cargo test --release`).

#[cfg(not(debug_assertions))]
use std::time::Duration;
use std::time::Instant;

use fasttick_core::bt::{BtFile, NodeView};
use fasttick_core::project::Project;
use fasttick_core::xml::NodeId;

/// A file with 100 trees of 101 nodes each (10,100 tree nodes) and a model
/// section with 100 models.
fn large_file() -> String {
    let mut text = String::from("<root BTCPP_format=\"4\" main_tree_to_execute=\"Tree0\">\n");
    for t in 0..100 {
        text.push_str(&format!(
            "  <BehaviorTree ID=\"Tree{t}\">\n    <Sequence name=\"s{t}\">\n"
        ));
        for i in 0..98 {
            if i % 10 == 0 {
                text.push_str(&format!(
                    "      <SubTree ID=\"Tree{}\" value=\"{{bb_{i}}}\"/>\n",
                    (t + 1) % 100
                ));
            } else {
                text.push_str(&format!(
                    "      <Action{i} name=\"n{i}\" value=\"{{bb_{i}}}\" _skipIf=\"x == {i}\"/>\n"
                ));
            }
        }
        text.push_str("      <Inverter><AlwaysSuccess/></Inverter>\n");
        text.push_str("    </Sequence>\n  </BehaviorTree>\n");
    }
    text.push_str("  <TreeNodesModel>\n");
    for i in 0..100 {
        text.push_str(&format!(
            "    <Action ID=\"Action{i}\"><input_port name=\"value\" type=\"int\">v</input_port></Action>\n"
        ));
    }
    text.push_str("  </TreeNodesModel>\n</root>\n");
    text
}

fn all_nodes(file: &BtFile) -> Vec<NodeId> {
    let doc = file.doc();
    file.trees()
        .iter()
        .flat_map(|t| {
            doc.descendants(t.element)
                .skip(1)
                .filter(|&n| doc.is_element(n))
        })
        .collect()
}

#[test]
fn reindex_paths_hashes_and_lookups_are_fast() {
    let text = large_file();
    let mut file = BtFile::parse(&text, None).unwrap();
    let nodes = all_nodes(&file);
    assert_eq!(nodes.len(), 100 * 101);
    assert_eq!(file.index().models.models.len(), 100);

    // Reindex after a change.
    let start = Instant::now();
    for _ in 0..100 {
        file.reindex();
    }
    let reindex = start.elapsed() / 100;
    assert_eq!(file.trees().len(), 100);

    // Child-index paths and structure hashes of all nodes and trees.
    let start = Instant::now();
    for &node in &nodes {
        let (tree, path) = file.node_index_path(node).unwrap();
        assert_eq!(file.node_at_index_path(tree, &path), Some(node));
    }
    let paths = start.elapsed();
    let start = Instant::now();
    for t in 0..file.trees().len() {
        file.structure_hash(t).unwrap();
    }
    let hashes = start.elapsed();

    // Model look-up for every node, as the validator does it.
    let (project, id) = Project::with_file("perf", file);
    let file = project.file(id).unwrap();
    let start = Instant::now();
    let mut found = 0;
    for &node in &nodes {
        let view = NodeView::new(file.doc(), node).unwrap();
        if project.lookup_model(id, view.type_id().unwrap()).is_some() {
            found += 1;
        }
    }
    let lookups = start.elapsed();
    assert_eq!(found, nodes.len());

    // Project refresh (include resolution and registry rebuild).
    let mut project = project;
    let start = Instant::now();
    for _ in 0..10 {
        project.refresh();
    }
    let refresh = start.elapsed() / 10;

    eprintln!(
        "reindex: {reindex:?}, 10,100 index paths: {paths:?}, 100 hashes: {hashes:?}, \
         10,100 look-ups: {lookups:?}, refresh: {refresh:?}"
    );
    #[cfg(not(debug_assertions))]
    {
        assert!(reindex < Duration::from_millis(2), "reindex {reindex:?}");
        assert!(paths < Duration::from_millis(100), "paths {paths:?}");
        assert!(hashes < Duration::from_millis(20), "hashes {hashes:?}");
        assert!(lookups < Duration::from_millis(20), "look-ups {lookups:?}");
        assert!(refresh < Duration::from_millis(5), "refresh {refresh:?}");
    }
}
