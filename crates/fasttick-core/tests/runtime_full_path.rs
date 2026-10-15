//! `RuntimeTree::full_path` against BT.CPP.
//!
//! The test executor writes a `SqliteLogger` database. Its `Nodes` table
//! holds `TreeNode::fullPath()` and the UID of each node, and its
//! `Definitions` table holds the runtime XML. `full_path` must give the
//! same path for each UID.
//!
//! The tests skip (and pass) when BT.CPP 4.x is not available (see
//! `fasttick_testkit::require_harness!`) or when the BT.CPP build has no
//! `SqliteLogger`.

use std::collections::BTreeMap;
use std::path::Path;
use std::time::Duration;

use fasttick_core::runtime::{RuntimeTree, Uid};
use fasttick_testkit::{Executor, ExecutorOptions, Harness, require_harness};

/// A tree with the cases that matter for the path: nodes without a name,
/// a name equal to the type, an empty name, the explicit form, SubTrees
/// with and without a name (also a name equal to the tree ID), and a nested
/// SubTree.
const PATHS_XML: &str = r#"<?xml version="1.0" encoding="UTF-8"?>
<root BTCPP_format="4" main_tree_to_execute="Main">
  <BehaviorTree ID="Main">
    <Sequence>
      <Script code="flag:=true"/>
      <Script name="Script" code="x:=1"/>
      <Script name="" code="y:=2"/>
      <AlwaysSuccess name="all good. 1"/>
      <Action ID="SayMessage" message="explicit"/>
      <Condition ID="CheckFlag" name="check" flag="{flag}"/>
      <Inverter>
        <AlwaysFailure/>
      </Inverter>
      <SubTree ID="Sub"/>
      <SubTree ID="Sub" name="named"/>
      <SubTree ID="Sub" name="Sub"/>
    </Sequence>
  </BehaviorTree>

  <BehaviorTree ID="Sub">
    <Sequence name="sub_sequence">
      <CheckFlag flag="true"/>
      <SubTree ID="Leaf" _autoremap="true"/>
    </Sequence>
  </BehaviorTree>

  <BehaviorTree ID="Leaf">
    <ForceSuccess name="ForceSuccess">
      <SayMessage/>
    </ForceSuccess>
  </BehaviorTree>
</root>
"#;

/// Runs the executor once with a SQLite log. Returns `None` if the BT.CPP
/// build has no `SqliteLogger`.
fn run(harness: &Harness, xml: &Path, db: &Path) -> Option<()> {
    let mut options = ExecutorOptions::new(xml);
    options.sqlite = Some(db.to_owned());
    options.loops = 1;
    options.tick_ms = 1;
    match Executor::spawn_with(harness, options) {
        Ok(mut executor) => {
            executor.wait_done(Duration::from_secs(60)).unwrap();
            assert_eq!(executor.results(), ["SUCCESS"]);
            Some(())
        }
        Err(err) if err.to_string().contains("no SqliteLogger") => {
            eprintln!("SKIPPED: this BT.CPP build has no SqliteLogger");
            None
        }
        Err(err) => panic!("{err}"),
    }
}

/// Reads the runtime XML and the `Nodes` rows (UID to full path).
fn read_db(db: &Path) -> (String, BTreeMap<u16, String>) {
    let conn = rusqlite::Connection::open(db).unwrap();
    let xml: String = conn
        .query_row(
            "SELECT xml_tree FROM Definitions ORDER BY session_id DESC LIMIT 1",
            [],
            |row| row.get(0),
        )
        .unwrap();
    let mut stmt = conn
        .prepare("SELECT fullpath, node_uid FROM Nodes")
        .unwrap();
    let rows = stmt
        .query_map([], |row| {
            Ok((row.get::<_, u16>(1)?, row.get::<_, String>(0)?))
        })
        .unwrap();
    let mut out = BTreeMap::new();
    for row in rows {
        let (uid, path) = row.unwrap();
        assert!(out.insert(uid, path).is_none(), "duplicate UID {uid}");
    }
    (xml, out)
}

/// Checks `full_path` for each node of the database. Returns the paths.
fn check(xml_path: &Path, db: &Path) -> BTreeMap<u16, String> {
    let (xml, nodes) = read_db(db);
    let tree = RuntimeTree::parse(&xml).unwrap();
    assert_eq!(nodes.len(), tree.len(), "{xml_path:?}");
    for (uid, expected) in &nodes {
        let index = tree.index_of_uid(Uid(*uid)).unwrap();
        assert_eq!(
            &tree.full_path(index),
            expected,
            "UID {uid} in {xml_path:?}"
        );
    }
    nodes
}

#[test]
fn full_path_matches_sqlite_nodes_table() {
    let harness = require_harness!();
    let dir = tempfile::tempdir().unwrap();
    let xml = dir.path().join("paths.xml");
    std::fs::write(&xml, PATHS_XML).unwrap();
    let db = dir.path().join("paths.db3");
    if run(harness, &xml, &db).is_none() {
        return;
    }
    let nodes = check(&xml, &db);
    let paths: Vec<&str> = nodes.values().map(String::as_str).collect();
    // The cases of the tree, as BT.CPP writes them.
    for expected in [
        "Sequence::1",
        "Script::2",
        "Script::3",
        "",
        "all good. 1",
        "SayMessage::6",
        "check",
        "Inverter::8",
        "AlwaysFailure::9",
        "Sub::10",
        "Sub::10/sub_sequence",
        "Sub::10/CheckFlag::12",
        "Sub::10/Leaf::13",
        "Sub::10/Leaf::13/ForceSuccess::14",
        "Sub::10/Leaf::13/SayMessage::15",
        "named",
        "named/CheckFlag::18",
        // A SubTree named like its tree ID: the node path gets "::<uid>",
        // but the instance path (the prefix of its nodes) does not.
        "Sub::22",
        "Sub/sub_sequence",
    ] {
        assert!(
            paths.contains(&expected),
            "missing {expected:?} in {paths:?}"
        );
    }
}

#[test]
fn full_path_matches_sqlite_for_harness_trees() {
    let harness = require_harness!();
    let dir = tempfile::tempdir().unwrap();
    for tree in ["main.xml", "include_main.xml"] {
        let db = dir.path().join(tree.replace(".xml", ".db3"));
        let xml = harness.tree(tree);
        if run(harness, &xml, &db).is_none() {
            return;
        }
        let nodes = check(&xml, &db);
        assert!(nodes.len() > 5);
    }
}
