//! Edit commands on all BT.CPP 4.9.0 XML fixtures.
//!
//! The fixtures have many formatting styles (indentation units, nodes on
//! one line, comments, CR LF). Each command must keep the files well
//! formed, and undo must give back the exact original bytes.

mod edit_support;

use std::path::{Path, PathBuf};

use edit_support::try_check;
use fasttick_core::bt::BtFile;
use fasttick_core::edit::{StructurePolicy, copy};
use fasttick_core::project::{FileId, Project};
use fasttick_core::xml::NodeId;

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

fn children(p: &Project, f: FileId, node: NodeId) -> Vec<NodeId> {
    p.file(f).unwrap().doc().element_children(node).collect()
}

/// The last element in document order below `node`.
fn last_descendant(p: &Project, f: FileId, node: NodeId) -> NodeId {
    let doc = p.file(f).unwrap().doc();
    doc.descendants(node)
        .filter(|&n| doc.is_element(n))
        .last()
        .unwrap_or(node)
}

#[derive(Default, Debug)]
struct Counts {
    files: usize,
    trees: usize,
    ok: usize,
    failed: usize,
}

impl Counts {
    fn add<T>(&mut self, result: Option<T>) -> Option<T> {
        if result.is_some() {
            self.ok += 1;
        } else {
            self.failed += 1;
        }
        result
    }
}

/// Runs the commands on one tree. Each command starts from the original
/// file and is undone at the end, so the trees do not interfere.
fn edit_tree(p: &mut Project, f: FileId, tree_id: &str, counts: &mut Counts) {
    let tree = p.file(f).unwrap().tree(tree_id).unwrap().clone();
    let Some(root) = tree.root else {
        // An empty tree: add a root node.
        counts.add(try_check(p, false, |ed| {
            ed.add_node(f, tree.element, 0, "Sequence", None)
        }));
        return;
    };
    let kids = children(p, f, root);
    let last = last_descendant(p, f, root);
    let invalid = |ed: &mut fasttick_core::edit::Editor<'_>| {
        ed.set_structure_policy(StructurePolicy::AllowInvalid)
    };

    counts.add(try_check(p, false, |ed| {
        invalid(ed);
        ed.add_node(f, root, kids.len(), "AlwaysSuccess", None)
    }));
    counts.add(try_check(p, false, |ed| {
        invalid(ed);
        ed.add_node(f, last, 0, "AlwaysFailure", None)
    }));
    if last != root {
        counts.add(try_check(p, false, |ed| {
            invalid(ed);
            ed.move_node(f, last, root, 0)
        }));
    }
    counts.add(try_check(p, false, |ed| {
        ed.set_instance_name(f, root, Some("edited & <named>"))
    }));
    counts.add(try_check(p, false, |ed| {
        ed.set_attribute(f, last, "extra", "{value}")
    }));
    if let Some(name) = p
        .file(f)
        .unwrap()
        .doc()
        .attributes(last)
        .first()
        .map(|a| a.name().to_owned())
    {
        counts.add(try_check(p, false, |ed| {
            ed.remove_attribute(f, last, &name)
        }));
    }
    let clip = copy(p, f, &[root]).unwrap();
    counts.add(try_check(p, false, |ed| {
        invalid(ed);
        ed.paste(f, &clip, root, 0)
    }));
    counts.add(try_check(p, false, |ed| {
        invalid(ed);
        ed.paste(f, &clip, tree.element, 1)
    }));
    if !kids.is_empty() {
        counts.add(try_check(p, false, |ed| {
            invalid(ed);
            ed.duplicate(f, &kids)
        }));
        let first = kids[0];
        counts.add(try_check(p, false, |ed| {
            ed.disconnect(f, first, [0.0, 0.0])?;
            ed.connect(f, first, root, 0)
        }));
        counts.add(try_check(p, false, |ed| {
            ed.disconnect(f, first, [1.0, 1.0])
        }));
        counts.add(try_check(p, false, |ed| ed.delete_nodes(f, &[first])));
        counts.add(try_check(p, false, |ed| {
            ed.create_subtree_from_selection(f, &kids, &format!("{tree_id}_Part"))
        }));
        if kids.len() > 1 {
            let last_kid = *kids.last().unwrap();
            counts.add(try_check(p, false, |ed| ed.move_node(f, last_kid, root, 0)));
        }
    }
    counts.add(try_check(p, false, |ed| {
        invalid(ed);
        ed.change_node_type(f, root, "ReactiveSequence", None)
    }));
    counts.add(try_check(p, false, |ed| {
        ed.create_subtree_from_selection(f, &[root], &format!("{tree_id}_Root"))
    }));
    counts.add(try_check(p, false, |ed| {
        ed.rename_tree(f, tree_id, &format!("{tree_id}_Renamed"))
    }));
    counts.add(try_check(p, false, |ed| ed.delete_tree(f, tree_id)));
    counts.add(try_check(p, false, |ed| ed.set_main_tree(f, Some(tree_id))));
}

#[test]
fn commands_on_all_fixtures_undo_exactly() {
    let mut counts = Counts::default();
    for (rel, path) in fixtures() {
        if MALFORMED_FIXTURES.contains(&rel.as_str()) {
            continue;
        }
        let file = BtFile::load(&path).unwrap();
        let original = file.to_xml_string();
        let (mut p, f) = Project::with_file(rel.clone(), file);
        counts.files += 1;
        let ids: Vec<String> = p
            .file(f)
            .unwrap()
            .trees()
            .iter()
            .map(|t| t.id.clone())
            .take(4)
            .collect();
        for id in ids {
            counts.trees += 1;
            edit_tree(&mut p, f, &id, &mut counts);
            // Each command was undone: the file is as loaded.
            assert_eq!(
                p.file(f).unwrap().to_xml_string().len(),
                original.len(),
                "{rel}"
            );
        }
        // Everything is back: the exact bytes and no dirty state.
        assert_eq!(p.file(f).unwrap().to_xml_string(), original, "{rel}");
        assert!(!p.file(f).unwrap().is_dirty(), "{rel}");
        counts.add(try_check(&mut p, false, |ed| {
            ed.add_tree(f, "FastTickNewTree")
        }));
    }
    eprintln!("{counts:?}");
    assert!(counts.files > 300, "{counts:?}");
    assert!(counts.trees > 300, "{counts:?}");
    assert!(counts.ok > 4000, "{counts:?}");
}
