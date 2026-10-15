//! Undo and redo: dirty state, saves, coalescing, maximum depth, multi-file
//! transactions and rollback.

mod edit_support;

use std::fs;
use std::path::Path;
use std::time::{Duration, Instant};

use edit_support::{named, project, snapshot, tag, text};
use fasttick_core::bt::StructureHash;
use fasttick_core::edit::{
    Coalesce, CoalesceKey, CommandError, Edit, History, StructurePolicy, Transaction, run,
};
use fasttick_core::models::{NodeKind, NodeModel};
use fasttick_core::project::{FileId, LayoutDirection, LayoutState, Project, UnconnectedNode};
use fasttick_core::xml::{Attribute, Change, EditError};

const TEXT: &str = "<root BTCPP_format=\"4\">\n  <BehaviorTree ID=\"T\">\n    <Sequence name=\"s\">\n      <Sleep msec=\"10\"/>\n    </Sequence>\n  </BehaviorTree>\n</root>\n";

fn set_msec(history: &mut History, p: &mut Project, f: FileId, value: &str) {
    let sleep = tag(p, f, "Sleep");
    history
        .run(p, |ed| ed.set_port_value(f, sleep, "msec", Some(value)))
        .unwrap();
}

fn msec(p: &Project, f: FileId) -> String {
    let sleep = tag(p, f, "Sleep");
    p.file(f)
        .unwrap()
        .doc()
        .attribute(sleep, "msec")
        .unwrap()
        .to_owned()
}

#[test]
fn dirty_state_follows_undo_redo_and_saves() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("t.xml");
    fs::write(&path, TEXT).unwrap();
    let mut p = Project::open_xml(&path).unwrap();
    let f = p.file_id_by_path(&path).unwrap();
    let mut history = History::new();
    assert!(!p.file(f).unwrap().is_dirty());

    set_msec(&mut history, &mut p, f, "20");
    assert!(p.file(f).unwrap().is_dirty());
    p.save_file(f).unwrap();
    assert!(!p.file(f).unwrap().is_dirty());
    assert_eq!(fs::read_to_string(&path).unwrap(), TEXT.replace("10", "20"));

    set_msec(&mut history, &mut p, f, "30");
    assert!(p.file(f).unwrap().is_dirty());
    assert_eq!(history.undo(&mut p).unwrap().as_deref(), Some("Set port"));
    // Back at the saved state.
    assert_eq!(msec(&p, f), "20");
    assert!(!p.file(f).unwrap().is_dirty());
    history.undo(&mut p).unwrap();
    assert_eq!(text(&p, f), TEXT);
    assert!(p.file(f).unwrap().is_dirty(), "the file on disk has 20");
    history.redo(&mut p).unwrap();
    assert!(!p.file(f).unwrap().is_dirty());
    history.redo(&mut p).unwrap();
    assert!(p.file(f).unwrap().is_dirty());
    assert_eq!(history.redo(&mut p).unwrap(), None);

    // A new edit after an undo never looks clean by accident.
    history.undo(&mut p).unwrap();
    assert!(!p.file(f).unwrap().is_dirty());
    set_msec(&mut history, &mut p, f, "40");
    assert!(p.file(f).unwrap().is_dirty());
    assert!(!history.can_redo(), "a new step clears the redo stack");
}

#[test]
fn labels_and_counts() {
    let (mut p, f) = project(TEXT);
    let mut history = History::new();
    assert_eq!(history.undo_label(), None);
    assert_eq!(history.undo(&mut p).unwrap(), None);
    let sequence = named(&p, f, "s");
    history
        .run(&mut p, |ed| {
            ed.add_node(f, sequence, 0, "AlwaysSuccess", None)
        })
        .unwrap();
    set_msec(&mut history, &mut p, f, "5");
    assert_eq!(history.undo_label(), Some("Set port"));
    assert_eq!(history.undo_len(), 2);
    history.undo(&mut p).unwrap();
    assert_eq!(history.undo_label(), Some("Add node"));
    assert_eq!(history.redo_label(), Some("Set port"));
    assert_eq!(history.redo_len(), 1);
    history.clear();
    assert!(!history.can_undo() && !history.can_redo());
}

#[test]
fn coalescing_merges_quick_edits_of_the_same_key() {
    let (mut p, f) = project(TEXT);
    let sleep = tag(&p, f, "Sleep");
    let mut history = History::new();
    let t0 = Instant::now();
    let window = Duration::from_millis(500);
    let key = CoalesceKey::attribute(f, sleep, "msec");
    let edit = |history: &mut History, p: &mut Project, value: &str, ms: u64| {
        history
            .run_coalesced(
                p,
                Coalesce {
                    key: key.clone(),
                    now: t0 + Duration::from_millis(ms),
                    window,
                },
                |ed| ed.set_port_value(f, sleep, "msec", Some(value)),
            )
            .unwrap();
    };
    // Typing "1", "12", "123": one step. The window slides.
    edit(&mut history, &mut p, "1", 0);
    edit(&mut history, &mut p, "12", 400);
    edit(&mut history, &mut p, "123", 800);
    // The same value: no step, and the chain stays open.
    edit(&mut history, &mut p, "123", 900);
    assert_eq!(history.undo_len(), 1);
    assert_eq!(msec(&p, f), "123");
    history.undo(&mut p).unwrap();
    assert_eq!(text(&p, f), TEXT);
    assert!(!p.file(f).unwrap().is_dirty());
    history.redo(&mut p).unwrap();
    assert_eq!(msec(&p, f), "123");

    // After a redo, a new edit does not merge.
    edit(&mut history, &mut p, "1234", 1000);
    assert_eq!(history.undo_len(), 2);
    // Too late: a new step.
    edit(&mut history, &mut p, "12345", 2000);
    assert_eq!(history.undo_len(), 3);
    // Another key: a new step.
    history
        .run_coalesced(
            &mut p,
            Coalesce {
                key: CoalesceKey::new("other"),
                now: t0 + Duration::from_millis(2100),
                window,
            },
            |ed| ed.set_port_value(f, sleep, "msec", Some("7")),
        )
        .unwrap();
    assert_eq!(history.undo_len(), 4);
    // A normal step between two coalesced edits stops the merge.
    edit(&mut history, &mut p, "8", 2200);
    history
        .run(&mut p, |ed| ed.set_instance_name(f, sleep, Some("nap")))
        .unwrap();
    edit(&mut history, &mut p, "9", 2300);
    assert_eq!(history.undo_len(), 7);
    // An edit before the time of the last one does not merge.
    edit(&mut history, &mut p, "10", 2299);
    assert_eq!(history.undo_len(), 8);
    assert_eq!(CoalesceKey::new("x").as_str(), "x");
    assert_eq!(
        CoalesceKey::position(f, sleep).to_string(),
        format!("position:{}:{}", f.0, sleep.index())
    );
}

#[test]
fn coalescing_stops_at_a_save() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("t.xml");
    fs::write(&path, TEXT).unwrap();
    let mut p = Project::open_xml(&path).unwrap();
    let f = p.file_id_by_path(&path).unwrap();
    let sleep = tag(&p, f, "Sleep");
    let mut history = History::new();
    let t0 = Instant::now();
    let coalesce = |ms: u64| Coalesce {
        key: CoalesceKey::attribute(f, sleep, "msec"),
        now: t0 + Duration::from_millis(ms),
        window: Duration::from_secs(1),
    };
    history
        .run_coalesced(&mut p, coalesce(0), |ed| {
            ed.set_port_value(f, sleep, "msec", Some("1"))
        })
        .unwrap();
    p.save_file(f).unwrap();
    history
        .run_coalesced(&mut p, coalesce(10), |ed| {
            ed.set_port_value(f, sleep, "msec", Some("2"))
        })
        .unwrap();
    assert_eq!(history.undo_len(), 2);
    history.undo(&mut p).unwrap();
    assert!(!p.file(f).unwrap().is_dirty());
    assert_eq!(msec(&p, f), "1");
}

#[test]
fn coalesced_canvas_moves_of_an_unconnected_node() {
    let (mut p, f) = project(TEXT);
    let mut history = History::new();
    let node = history
        .run(&mut p, |ed| {
            ed.add_unconnected_node(f, "T", "AlwaysSuccess", None, [0.0, 0.0])
        })
        .unwrap();
    let t0 = Instant::now();
    for i in 1..=10 {
        history
            .run_coalesced(
                &mut p,
                Coalesce {
                    key: CoalesceKey::position(f, node),
                    now: t0 + Duration::from_millis(16 * i),
                    window: Duration::from_millis(100),
                },
                |ed| ed.set_unconnected_position(f, node, [i as f32, 2.0 * i as f32]),
            )
            .unwrap();
    }
    assert_eq!(history.undo_len(), 2);
    assert_eq!(p.unconnected(f, "T")[0].position, [10.0, 20.0]);
    history.undo(&mut p).unwrap();
    assert_eq!(p.unconnected(f, "T")[0].position, [0.0, 0.0]);
    // The XML did not change, so the file is not dirty.
    assert!(!p.file(f).unwrap().is_dirty());
}

#[test]
fn maximum_depth_drops_the_oldest_steps() {
    let (mut p, f) = project(TEXT);
    let mut history = History::with_max_depth(3);
    assert_eq!(history.max_depth(), 3);
    let mut states = vec![text(&p, f)];
    for i in 0..5 {
        set_msec(&mut history, &mut p, f, &i.to_string());
        states.push(text(&p, f));
    }
    assert_eq!(history.undo_len(), 3);
    for i in (2..5).rev() {
        history.undo(&mut p).unwrap();
        assert_eq!(text(&p, f), states[i]);
    }
    assert!(!history.can_undo());
    assert_eq!(history.redo_len(), 3);
    history.redo(&mut p).unwrap();
    history.redo(&mut p).unwrap();
    history.set_max_depth(1);
    assert_eq!(history.undo_len(), 1);
    history.set_max_depth(0);
    assert_eq!(history.max_depth(), 1);
    assert_eq!(
        History::new().max_depth(),
        fasttick_core::edit::DEFAULT_MAX_DEPTH
    );
}

/// Writes the files of a project with includes and returns the paths.
fn write_include_project(
    dir: &Path,
) -> (std::path::PathBuf, std::path::PathBuf, std::path::PathBuf) {
    let main = dir.join("main.xml");
    let sub = dir.join("sub.xml");
    let other = dir.join("other.xml");
    fs::write(
        &main,
        r#"<root BTCPP_format="4" main_tree_to_execute="Main">
  <include path="sub.xml"/>
  <BehaviorTree ID="Main">
    <SubTree ID="Sub" goal="{g}"/>
  </BehaviorTree>
  <TreeNodesModel>
    <SubTree ID="Sub">
      <input_port name="goal"/>
    </SubTree>
  </TreeNodesModel>
</root>
"#,
    )
    .unwrap();
    fs::write(
        &sub,
        r#"<root BTCPP_format="4" main_tree_to_execute="Sub">
  <BehaviorTree ID="Sub">
    <MoveTo goal="{goal}"/>
  </BehaviorTree>
</root>
"#,
    )
    .unwrap();
    fs::write(
        &other,
        r#"<root BTCPP_format="4">
  <BehaviorTree ID="Other">
    <Sequence>
      <SubTree ID="Sub"/>
      <SubTree ID="Subway"/>
    </Sequence>
  </BehaviorTree>
</root>
"#,
    )
    .unwrap();
    (main, sub, other)
}

#[test]
fn rename_tree_across_files_is_one_step() {
    let dir = tempfile::tempdir().unwrap();
    let (main_path, sub_path, other_path) = write_include_project(dir.path());
    let mut p = Project::open_xml(&main_path).unwrap();
    let other = p.add_file(&other_path).unwrap();
    let main = p.file_id_by_path(&main_path).unwrap();
    let sub = p.file_id_by_path(&sub_path).unwrap();
    let before = snapshot(&p);
    let mut history = History::new();
    let count = history
        .run(&mut p, |ed| ed.rename_tree(sub, "Sub", "Walk"))
        .unwrap();
    // main.xml: the SubTree node and the model; sub.xml:
    // main_tree_to_execute; other.xml: one SubTree node.
    assert_eq!(count, 4);
    assert!(text(&p, main).contains("<SubTree ID=\"Walk\" goal=\"{g}\"/>"));
    assert!(text(&p, main).contains("    <SubTree ID=\"Walk\">\n      <input_port"));
    assert!(text(&p, sub).contains("main_tree_to_execute=\"Walk\""));
    assert!(text(&p, sub).contains("<BehaviorTree ID=\"Walk\">"));
    assert!(text(&p, other).contains("<SubTree ID=\"Walk\"/>"));
    assert!(text(&p, other).contains("<SubTree ID=\"Subway\"/>"));
    assert_eq!(p.dirty_files().len(), 3);
    // The project sees the new ID at once.
    assert!(p.find_tree("Walk").is_some());
    assert!(p.find_tree("Sub").is_none());
    assert_eq!(
        p.lookup_model(main, "Walk").unwrap().0.kind,
        NodeKind::SubTree
    );

    history.undo(&mut p).unwrap();
    assert_eq!(snapshot(&p), before);
    assert!(p.dirty_files().is_empty());
    assert!(p.find_tree("Sub").is_some());
    history.redo(&mut p).unwrap();
    assert_eq!(p.dirty_files().len(), 3);
    p.save_dirty_files().unwrap();
    assert!(
        fs::read_to_string(&sub_path)
            .unwrap()
            .contains("ID=\"Walk\"")
    );
}

#[test]
fn a_failed_transaction_rolls_back_all_files() {
    let (mut p, f) = project(TEXT);
    let g = p.add_bt_file(
        fasttick_core::bt::BtFile::parse(TEXT, None).unwrap(),
        fasttick_core::project::FileRole::Tree,
    );
    p.refresh();
    let sleep = tag(&p, f, "Sleep");
    let before = snapshot(&p);
    let gens: Vec<u64> = [f, g]
        .iter()
        .map(|&id| p.file(id).unwrap().generation())
        .collect();
    let tx = Transaction::from_changes(
        "Bad",
        [
            (
                f,
                Change::InsertAttribute {
                    node: sleep,
                    index: 0,
                    attribute: Attribute::new("a", "1"),
                },
            ),
            (
                g,
                Change::InsertAttribute {
                    node: sleep,
                    index: 0,
                    attribute: Attribute::new("b", "2"),
                },
            ),
            // The attribute "a" exists now: this fails.
            (
                f,
                Change::InsertAttribute {
                    node: sleep,
                    index: 0,
                    attribute: Attribute::new("a", "3"),
                },
            ),
        ],
    );
    assert_eq!(tx.xml_files(), vec![f, g]);
    let error = tx.apply(&mut p).unwrap_err();
    assert_eq!(
        error,
        CommandError::Xml(EditError::DuplicateAttribute("a".into()))
    );
    assert_eq!(snapshot(&p), before);
    let after: Vec<u64> = [f, g]
        .iter()
        .map(|&id| p.file(id).unwrap().generation())
        .collect();
    assert_eq!(gens, after);

    // An unknown file fails before any change.
    let tx = Transaction::from_changes("Unknown", [(FileId(42), Change::Detach { node: sleep })]);
    assert_eq!(tx.apply(&mut p), Err(CommandError::UnknownFile(FileId(42))));
    assert_eq!(snapshot(&p), before);
}

#[test]
fn transactions_invert_and_record_in_the_history() {
    let (mut p, f) = project(TEXT);
    let sleep = tag(&p, f, "Sleep");
    let mut history = History::new();
    let tx = Transaction::from_changes(
        "Rename",
        [(
            f,
            Change::Rename {
                node: sleep,
                name: "Wait".into(),
            },
        )],
    );
    assert!(!tx.is_empty());
    history.apply(&mut p, tx).unwrap();
    assert!(text(&p, f).contains("<Wait msec=\"10\"/>"));
    assert!(p.file(f).unwrap().is_dirty());
    assert_eq!(history.undo_label(), Some("Rename"));
    history.undo(&mut p).unwrap();
    assert_eq!(text(&p, f), TEXT);
    assert!(!p.file(f).unwrap().is_dirty());

    // Project-state edits.
    let node = p
        .file_mut(f)
        .unwrap()
        .doc_mut()
        .create_element("Detached")
        .unwrap();
    let mut layout = LayoutState::new(LayoutDirection::LeftRight, StructureHash(9));
    layout.set_position(&[0], [1.0, 1.0]);
    let tx = Transaction {
        label: "State".into(),
        edits: vec![
            Edit::Layout {
                file: f,
                tree: "T".into(),
                state: Some(Box::new(layout.clone())),
            },
            Edit::InsertUnconnected {
                file: f,
                tree: "T".into(),
                index: 0,
                node: UnconnectedNode {
                    node,
                    position: [3.0, 4.0],
                },
            },
            Edit::UnconnectedPosition {
                file: f,
                node,
                position: [5.0, 6.0],
            },
        ],
    };
    assert_eq!(tx.edits[0].file(), f);
    let undo = tx.apply(&mut p).unwrap();
    assert_eq!(undo.label, "State");
    assert_eq!(p.layout(f, "T"), Some(&layout));
    assert_eq!(p.unconnected(f, "T")[0].position, [5.0, 6.0]);
    assert!(!p.file(f).unwrap().is_dirty(), "no XML change");
    let redo = undo.apply(&mut p).unwrap();
    assert!(p.layout(f, "T").is_none());
    assert!(p.unconnected(f, "T").is_empty());
    redo.apply(&mut p).unwrap();
    assert_eq!(p.unconnected(f, "T").len(), 1);

    // Bad state edits fail and roll back.
    let before = snapshot(&p);
    let bad = Transaction {
        label: "Bad".into(),
        edits: vec![
            Edit::Layout {
                file: f,
                tree: "T".into(),
                state: None,
            },
            Edit::RemoveUnconnected {
                file: f,
                tree: "T".into(),
                index: 5,
            },
        ],
    };
    assert!(matches!(
        bad.apply(&mut p),
        Err(CommandError::UnconnectedMismatch { .. })
    ));
    assert_eq!(snapshot(&p), before);
    let sleep_attached = Transaction {
        label: "Bad".into(),
        edits: vec![Edit::InsertUnconnected {
            file: f,
            tree: "T".into(),
            index: 0,
            node: UnconnectedNode {
                node: sleep,
                position: [0.0, 0.0],
            },
        }],
    };
    assert_eq!(
        sleep_attached.apply(&mut p),
        Err(CommandError::Xml(EditError::AlreadyAttached(sleep)))
    );
    let twice = Transaction {
        label: "Bad".into(),
        edits: vec![Edit::InsertUnconnected {
            file: f,
            tree: "Other".into(),
            index: 0,
            node: UnconnectedNode {
                node,
                position: [0.0, 0.0],
            },
        }],
    };
    assert_eq!(
        twice.apply(&mut p),
        Err(CommandError::AlreadyUnconnected(node))
    );
    let missing = Transaction {
        label: "Bad".into(),
        edits: vec![Edit::UnconnectedPosition {
            file: f,
            node: sleep,
            position: [0.0, 0.0],
        }],
    };
    assert_eq!(
        missing.apply(&mut p),
        Err(CommandError::NotUnconnected(sleep))
    );
    assert_eq!(snapshot(&p), before);
}

#[test]
fn sessions_outside_the_history() {
    let (mut p, f) = project(TEXT);
    let sequence = named(&p, f, "s");
    let (node, applied) = run(&mut p, StructurePolicy::Enforce, |ed| {
        ed.add_node(f, sequence, 1, "AlwaysSuccess", None)
    })
    .unwrap();
    assert_eq!(applied.label(), "Add node");
    assert_eq!(applied.generations.len(), 1);
    assert_eq!(applied.generations[0].before, 0);
    assert!(p.file(f).unwrap().doc().is_attached(node));
    let mut history = History::new();
    history.push(applied);
    history.undo(&mut p).unwrap();
    assert_eq!(text(&p, f), TEXT);

    // An empty session adds no step.
    let ((), applied) = run(&mut p, StructurePolicy::Enforce, |_| Ok(())).unwrap();
    assert!(applied.is_empty());
    history.push(applied);
    assert!(!history.can_undo());
    assert_eq!(history.redo_len(), 1);
}

#[test]
fn history_structure_policy() {
    let (mut p, f) = project(TEXT);
    let sleep = tag(&p, f, "Sleep");
    let mut history = History::new();
    assert_eq!(history.structure_policy(), StructurePolicy::Enforce);
    assert!(
        history
            .run(&mut p, |ed| ed.add_node(f, sleep, 0, "AlwaysSuccess", None))
            .is_err()
    );
    history.set_structure_policy(StructurePolicy::AllowInvalid);
    history
        .run(&mut p, |ed| {
            assert_eq!(ed.structure_policy(), StructurePolicy::AllowInvalid);
            ed.add_node(f, sleep, 0, "AlwaysSuccess", None)
        })
        .unwrap();
    assert!(text(&p, f).contains("<Sleep msec=\"10\">"));
}

#[test]
fn undo_fails_cleanly_when_a_file_is_gone() {
    let (mut p, f) = project(TEXT);
    let g = p.add_bt_file(
        fasttick_core::bt::BtFile::parse(TEXT, None).unwrap(),
        fasttick_core::project::FileRole::Tree,
    );
    p.refresh();
    let mut history = History::new();
    set_msec(&mut history, &mut p, f, "1");
    let sleep_g = tag(&p, g, "Sleep");
    history
        .run(&mut p, |ed| {
            ed.set_port_value(g, sleep_g, "msec", Some("2"))
        })
        .unwrap();
    set_msec(&mut history, &mut p, f, "3");
    history.undo(&mut p).unwrap();
    history.redo(&mut p).unwrap();
    history.undo(&mut p).unwrap();
    p.remove_file(g);
    let before = snapshot(&p);
    let error = history.undo(&mut p).unwrap_err();
    assert_eq!(error.action, "undo");
    assert_eq!(error.error, CommandError::UnknownFile(g));
    assert!(error.to_string().starts_with("cannot undo \"Set port\""));
    assert_eq!(snapshot(&p), before);
    assert!(!history.can_undo());
    // The redo stack still works.
    history.redo(&mut p).unwrap();
    assert_eq!(msec(&p, f), "3");
}

#[test]
fn model_changes_refresh_the_registry() {
    let (mut p, f) = project(TEXT);
    let mut history = History::new();
    let mut model = NodeModel::new("Beep", NodeKind::Condition);
    model.ports.push(fasttick_core::models::PortModel::new(
        "x",
        fasttick_core::models::PortDirection::Input,
    ));
    history.run(&mut p, |ed| ed.add_model(f, &model)).unwrap();
    assert_eq!(
        p.lookup_model(f, "Beep").unwrap().0.kind,
        NodeKind::Condition
    );
    // A session sees its own model change at once: the kind of the new
    // model decides the child rule of the new node.
    let sequence = named(&p, f, "s");
    let err = history
        .run(&mut p, |ed| {
            let mut other = NodeModel::new("Tick", NodeKind::Action);
            other.ports.clear();
            ed.add_model(f, &other)?;
            let tick = ed.add_node(f, sequence, 0, "Tick", None)?;
            // An action cannot have children.
            ed.add_node(f, tick, 0, "AlwaysSuccess", None)
        })
        .unwrap_err();
    assert!(matches!(err, CommandError::ChildLimit { .. }));
    assert!(p.lookup_model(f, "Tick").is_none());
    history.undo(&mut p).unwrap();
    assert!(p.lookup_model(f, "Beep").is_none());
    history.redo(&mut p).unwrap();
    assert!(p.lookup_model(f, "Beep").is_some());
}
