//! The BT.CPP view on all BT.CPP 4.9.0 XML fixtures.
//!
//! Every fixture in `tests/fixtures/btcpp` (except the three malformed ones,
//! see `xml_roundtrip.rs`) must parse as a `BtFile`, and its index must be
//! consistent with the document.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use fasttick_core::bt::{
    AttributeClass, BtFile, MAX_NESTING_DEPTH, NodeView, PortValue, find_too_deep,
    format_index_path, parse_index_path, structure_hash,
};
use fasttick_core::models::NodeKind;
use fasttick_core::xml::{Document, NodeId};

const MALFORMED_FIXTURES: &[&str] = &[
    "inline/gtest_factory_16.xml",
    "inline/gtest_factory_17.xml",
    "inline/gtest_switch_1.xml",
];

fn fixture_root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../../tests/fixtures/btcpp")
}

fn fixtures() -> Vec<(String, PathBuf)> {
    let root = fixture_root();
    let mut stack = vec![root.clone()];
    let mut files = Vec::new();
    while let Some(dir) = stack.pop() {
        for entry in std::fs::read_dir(&dir).unwrap() {
            let path = entry.unwrap().path();
            if path.is_dir() {
                stack.push(path);
            } else if path.extension().is_some_and(|e| e == "xml") {
                let rel = path
                    .strip_prefix(&root)
                    .unwrap()
                    .to_string_lossy()
                    .replace('\\', "/");
                files.push((rel, path));
            }
        }
    }
    files.sort();
    files
}

/// All elements below a tree element, in document order.
fn tree_nodes(doc: &Document, tree: NodeId) -> Vec<NodeId> {
    doc.descendants(tree)
        .skip(1)
        .filter(|&n| doc.is_element(n))
        .collect()
}

#[derive(Default, Debug)]
struct Stats {
    files: usize,
    files_with_trees: usize,
    trees: usize,
    nodes: usize,
    explicit_form: usize,
    subtrees: usize,
    ports: usize,
    blackboard_ports: usize,
    conditions: usize,
    includes: usize,
    models: usize,
    unnamed_trees: usize,
}

#[test]
fn all_fixtures_parse_with_a_sane_index() {
    let mut stats = Stats::default();
    let mut tags: BTreeMap<String, usize> = BTreeMap::new();
    for (rel, path) in fixtures() {
        let result = BtFile::load(&path);
        if MALFORMED_FIXTURES.contains(&rel.as_str()) {
            assert!(result.is_err(), "{rel} must not parse");
            continue;
        }
        let file = result.unwrap_or_else(|e| panic!("{rel}: {e}"));
        stats.files += 1;
        let doc = file.doc();
        let index = file.index();
        let root = index
            .root
            .unwrap_or_else(|| panic!("{rel}: no root element"));
        assert_eq!(doc.document_element(), Some(root));
        assert_eq!(
            file.to_xml_string().as_bytes(),
            std::fs::read(&path).unwrap(),
            "{rel}"
        );

        // Every child element of the root is in exactly one list.
        let listed = index.trees.len()
            + index.includes.len()
            + index.model_sections.len()
            + index.other_elements.len();
        assert_eq!(listed, doc.element_children(root).count(), "{rel}");
        if !index.trees.is_empty() {
            stats.files_with_trees += 1;
        }
        stats.includes += index.includes.len();
        stats.models += index.models.models.len();
        for include in &index.includes {
            assert_eq!(doc.name(include.element), Some("include"));
            assert!(include.path.is_some(), "{rel}: include without path");
        }
        for section in &index.model_sections {
            assert_eq!(doc.name(*section), Some("TreeNodesModel"));
        }
        for model in &index.models.models {
            assert_eq!(doc.parent(model.element), Some(model.section), "{rel}");
            assert_eq!(
                doc.attribute(model.element, "ID"),
                Some(model.model.id.as_str())
            );
        }

        for (t, tree) in index.trees.iter().enumerate() {
            stats.trees += 1;
            assert_eq!(doc.name(tree.element), Some("BehaviorTree"), "{rel}");
            assert_eq!(doc.parent(tree.element), Some(root));
            if tree.has_explicit_id {
                assert_eq!(doc.attribute(tree.element, "ID"), Some(tree.id.as_str()));
            } else {
                stats.unnamed_trees += 1;
                assert!(tree.id.starts_with("BehaviorTree_"), "{rel}");
            }
            let children: Vec<NodeId> = doc.element_children(tree.element).collect();
            assert_eq!(tree.root, children.first().copied(), "{rel}");
            assert_eq!(tree.extra_roots, children.get(1..).unwrap_or(&[]), "{rel}");
            assert_eq!(find_too_deep(doc, tree.element), None, "{rel}");

            // The hash is stable and does not depend on the tree ID.
            let hash = structure_hash(doc, tree.element);
            assert_eq!(file.structure_hash(t), Some(hash));

            for node in tree_nodes(doc, tree.element) {
                stats.nodes += 1;
                let view = NodeView::new(doc, node).unwrap();
                *tags.entry(view.tag().to_owned()).or_default() += 1;
                if view.is_explicit_form() {
                    stats.explicit_form += 1;
                }
                if view.is_subtree() {
                    stats.subtrees += 1;
                    assert_eq!(view.category_tag(), Some(NodeKind::SubTree));
                    assert_eq!(view.registration_id(), Some("SubTree"));
                    assert_eq!(view.type_id(), view.subtree_id());
                }
                // The instance name defaults to the type ID.
                if view.name_attribute().is_none() {
                    assert_eq!(view.instance_name(), view.type_id());
                }
                for attribute in view.attributes() {
                    match attribute.class {
                        AttributeClass::Port => {
                            stats.ports += 1;
                            let value = view.port_value(attribute.name).unwrap();
                            if value.is_blackboard() {
                                stats.blackboard_ports += 1;
                                assert!(value.blackboard_key(attribute.name).is_some());
                            } else {
                                assert_eq!(value, PortValue::Literal(attribute.value));
                            }
                        }
                        AttributeClass::PreCondition(_) | AttributeClass::PostCondition(_) => {
                            stats.conditions += 1;
                        }
                        _ => {}
                    }
                }
                assert_eq!(
                    view.pre_conditions().count() + view.post_conditions().count(),
                    view.attributes()
                        .filter(|a| matches!(
                            a.class,
                            AttributeClass::PreCondition(_) | AttributeClass::PostCondition(_)
                        ))
                        .count()
                );

                // Child-index paths go both ways.
                let (tree_index, index_path) = file.node_index_path(node).unwrap();
                assert_eq!(tree_index, t, "{rel}");
                assert_eq!(file.node_at_index_path(t, &index_path), Some(node), "{rel}");
                let text = format_index_path(&index_path);
                assert_eq!(parse_index_path(&text), Some(index_path));
            }
        }

        // A second parse of the text gives the same index.
        let again = BtFile::parse(&file.to_xml_string(), None).unwrap();
        assert_eq!(again.index(), file.index(), "{rel}");
    }
    eprintln!("{stats:#?}");
    eprintln!("tags: {tags:?}");
    assert_eq!(stats.files, 317 - MALFORMED_FIXTURES.len());
    // Sanity numbers: most fixtures have trees; the forms all occur.
    assert!(stats.files_with_trees > 250, "{stats:?}");
    assert!(stats.explicit_form > 10, "{stats:?}");
    assert!(stats.subtrees > 50, "{stats:?}");
    assert!(stats.blackboard_ports > 100, "{stats:?}");
    assert!(stats.conditions > 10, "{stats:?}");
    // The fixtures are fixed (BT.CPP 4.9.0), so these numbers are exact.
    assert_eq!(stats.includes, 9, "{stats:?}");
    assert_eq!(stats.models, 12, "{stats:?}");
}

#[test]
fn known_fixture_details() {
    let root = fixture_root();
    let file = BtFile::load(root.join("examples/test_files/subtree_test.xml")).unwrap();
    let index = file.index();
    assert_eq!(index.format.as_deref(), Some("4"));
    assert_eq!(index.main_tree_to_execute, None);
    let paths: Vec<&str> = index
        .includes
        .iter()
        .map(|i| i.path.as_deref().unwrap())
        .collect();
    assert_eq!(paths, vec!["Check.xml", "subtrees/Talk.xml"]);
    assert_eq!(index.main_tree().unwrap().id, "BehaviorTree");

    let talk = BtFile::load(root.join("examples/test_files/subtrees/Talk.xml")).unwrap();
    let say_stuff = talk.tree("SayStuff").unwrap();
    let seq = talk.node(say_stuff.root.unwrap()).unwrap();
    assert_eq!(seq.type_id(), Some("SequenceWithMemory"));
    let actions: Vec<NodeView> = seq.children().collect();
    assert!(actions[0].is_explicit_form());
    assert_eq!(actions[0].type_id(), Some("SaySomething"));
    assert_eq!(
        actions[0].port_value("message"),
        Some(PortValue::Literal("Hello World"))
    );
    assert_eq!(actions[1].instance_name(), Some("SayHello"));

    let parent = BtFile::load(root.join("tests/trees/parent_include_child.xml")).unwrap();
    assert_eq!(
        parent.index().main_tree_to_execute.as_deref(),
        Some("ParentIncludeChild")
    );
    assert_eq!(parent.index().main_tree().unwrap().id, "ParentIncludeChild");
}

#[test]
fn hash_survives_reformatting() {
    // The same trees with other white space, comments and attributes give
    // the same hashes.
    for (rel, path) in fixtures() {
        if MALFORMED_FIXTURES.contains(&rel.as_str()) {
            continue;
        }
        let file = BtFile::load(&path).unwrap();
        let mut doc = file.doc().clone();
        // Add an attribute to each element of each tree.
        let elements: Vec<NodeId> = file
            .index()
            .trees
            .iter()
            .flat_map(|t| tree_nodes(&doc, t.element))
            .collect();
        for e in elements {
            doc.set_attribute(e, "_fasttick_test", "1").unwrap();
        }
        let changed = BtFile::parse(&doc.to_string(), None).unwrap();
        for t in 0..file.trees().len() {
            assert_eq!(file.structure_hash(t), changed.structure_hash(t), "{rel}");
        }
    }
}

#[test]
fn nesting_limit_matches_verify_xml() {
    // MAX_NESTING_DEPTH levels below <BehaviorTree> are accepted; one more
    // level is not.
    for (depth, too_deep) in [(MAX_NESTING_DEPTH, false), (MAX_NESTING_DEPTH + 1, true)] {
        let mut text = String::from("<root BTCPP_format=\"4\"><BehaviorTree ID=\"T\">");
        text.push_str(&"<Inverter>".repeat(depth - 1));
        text.push_str("<AlwaysSuccess/>");
        text.push_str(&"</Inverter>".repeat(depth - 1));
        text.push_str("</BehaviorTree></root>");
        let file = BtFile::parse(&text, None).unwrap();
        let found = find_too_deep(file.doc(), file.trees()[0].element);
        assert_eq!(found.is_some(), too_deep, "depth {depth}");
    }
}
