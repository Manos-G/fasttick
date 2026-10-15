//! Edited trees run in BT.CPP 4.9.0 (the test executor of `test_harness/`).
//!
//! The test skips cleanly when BT.CPP is not installed; set
//! `FASTTICK_BTCPP_PREFIX` to run it.

use std::fs;
use std::path::Path;
use std::time::Duration;

use fasttick_core::edit::History;
use fasttick_core::project::{FileId, Project};
use fasttick_core::xml::NodeId;
use fasttick_testkit::{Executor, ExecutorOptions, require_harness};

/// The result depends on the blackboard: the condition is true only if the
/// three counters write the same entries as before an edit.
const TREE: &str = r#"<?xml version="1.0" encoding="UTF-8"?>
<root BTCPP_format="4" main_tree_to_execute="Main">
  <BehaviorTree ID="Main">
    <Sequence name="main">
      <Script code="counter := 0; total := 0"/>
      <IncrementCounter name="two" counter="{counter}" step="2"/>
      <IncrementCounter name="three" counter="{=}" step="3"/>
      <IncrementCounter name="seven" counter="{@total}" step="7"/>
      <ScriptCondition code="counter == 5 &amp;&amp; total == 7"/>
    </Sequence>
  </BehaviorTree>
</root>
"#;

/// Runs the main tree once. Gives the root status, or the error text if
/// BT.CPP cannot load the file.
fn run(path: &Path) -> Result<String, String> {
    let mut options = ExecutorOptions::new(path);
    options.loops = 1;
    options.tick_ms = 1;
    let mut executor = Executor::spawn(options).map_err(|e| e.to_string())?;
    executor
        .wait_done(Duration::from_secs(30))
        .map_err(|e| e.to_string())?;
    Ok(executor.results().join(","))
}

fn named(p: &Project, f: FileId, name: &str) -> NodeId {
    let doc = p.file(f).unwrap().doc();
    doc.descendants(doc.root())
        .find(|&n| doc.attribute(n, "name") == Some(name))
        .unwrap()
}

fn tag(p: &Project, f: FileId, name: &str) -> NodeId {
    let doc = p.file(f).unwrap().doc();
    doc.descendants(doc.root())
        .find(|&n| doc.name(n) == Some(name))
        .unwrap()
}

#[test]
fn a_new_subtree_keeps_the_blackboard_flow() {
    let _harness = require_harness!();
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("tree.xml");
    fs::write(&path, TREE).unwrap();
    assert_eq!(run(&path).unwrap(), "SUCCESS");

    let mut p = Project::open_xml(&path).unwrap();
    let f = p.file_id_by_path(&path).unwrap();
    let nodes = [
        named(&p, f, "two"),
        named(&p, f, "three"),
        named(&p, f, "seven"),
    ];
    let mut history = History::new();
    let result = history
        .run(&mut p, |ed| {
            ed.create_subtree_from_selection(f, &nodes, "Inc")
        })
        .unwrap();
    // "{@total}" needs no remapping; "{=}" is the key "counter".
    assert_eq!(result.remapped_keys, vec!["counter"]);
    p.save_file(f).unwrap();
    let text = fs::read_to_string(&path).unwrap();
    assert!(
        text.contains("<SubTree ID=\"Inc\" counter=\"{counter}\"/>"),
        "{text}"
    );
    // BT.CPP loads the new tree and the SubTree model, and the result is
    // the same.
    assert_eq!(run(&path).unwrap(), "SUCCESS", "{text}");

    // Rename the new tree: the reference and the model follow.
    history
        .run(&mut p, |ed| ed.rename_tree(f, "Inc", "Increment"))
        .unwrap();
    p.save_file(f).unwrap();
    assert_eq!(run(&path).unwrap(), "SUCCESS");

    // Control: without the remapping, the counter of the new tree is a
    // different entry, and the condition fails. (The model must go too:
    // BT.CPP requires a remapping for each port of a SubTree model.)
    let sub = tag(&p, f, "SubTree");
    history
        .run(&mut p, |ed| {
            ed.set_port_value(f, sub, "counter", None)?;
            ed.remove_model(f, "Increment")
        })
        .unwrap();
    p.save_file(f).unwrap();
    assert_eq!(run(&path).unwrap(), "FAILURE");
    // Only the model removed: BT.CPP refuses the missing remapping.
    history.undo(&mut p).unwrap();
    history
        .run(&mut p, |ed| ed.set_port_value(f, sub, "counter", None))
        .unwrap();
    p.save_file(f).unwrap();
    let error = run(&path).unwrap_err();
    assert!(error.contains("mandatory port"), "{error}");

    // Undo everything: the original bytes, and the original result.
    while history.undo(&mut p).unwrap().is_some() {}
    p.save_file(f).unwrap();
    assert_eq!(fs::read_to_string(&path).unwrap(), TREE);
    assert_eq!(run(&path).unwrap(), "SUCCESS");
}

#[test]
fn moved_pasted_and_unconnected_nodes_run() {
    let _harness = require_harness!();
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("tree.xml");
    fs::write(&path, TREE).unwrap();
    let mut p = Project::open_xml(&path).unwrap();
    let f = p.file_id_by_path(&path).unwrap();
    let main = named(&p, f, "main");
    let two = named(&p, f, "two");
    let seven = named(&p, f, "seven");
    let mut history = History::new();

    // An unconnected node is not written to the XML.
    history
        .run(&mut p, |ed| {
            ed.add_unconnected_node(f, "Main", "AlwaysFailure", None, [0.0, 0.0])?;
            ed.move_node(f, seven, main, 1)
        })
        .unwrap();
    p.save_file(f).unwrap();
    assert!(!fs::read_to_string(&path).unwrap().contains("AlwaysFailure"));
    assert_eq!(run(&path).unwrap(), "SUCCESS");

    // A copy of "two" adds 2 more: the condition fails.
    let clip = fasttick_core::edit::copy(&p, f, &[two]).unwrap();
    history
        .run(&mut p, |ed| ed.paste(f, &clip, main, 2))
        .unwrap();
    p.save_file(f).unwrap();
    assert_eq!(run(&path).unwrap(), "FAILURE");
    history.undo(&mut p).unwrap();

    // A disconnected node does not run: the counter stays 3.
    history
        .run(&mut p, |ed| ed.disconnect(f, two, [1.0, 1.0]))
        .unwrap();
    p.save_file(f).unwrap();
    assert_eq!(run(&path).unwrap(), "FAILURE");
    // Connected again at another place: the same sum.
    history
        .run(&mut p, |ed| ed.connect(f, two, main, 3))
        .unwrap();
    p.save_file(f).unwrap();
    assert_eq!(run(&path).unwrap(), "SUCCESS");
}
