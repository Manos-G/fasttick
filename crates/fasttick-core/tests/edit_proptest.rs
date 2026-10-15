//! Random sequences of edit commands, undo and redo.
//!
//! After each command, the project must be well formed (the text parses
//! and writes back unchanged). A command that fails must not change the
//! project. Each undo and each redo must give back the exact earlier state
//! (texts, unconnected nodes, layouts). At the end, the test undoes all
//! steps (the original bytes must come back, and no file is dirty) and
//! redoes all steps.

mod edit_support;

use std::time::{Duration, Instant};

use edit_support::{Snapshot, check_well_formed, project, snapshot};
use fasttick_core::edit::{
    Coalesce, CoalesceKey, CommandError, Editor, History, StructurePolicy, copy,
};
use fasttick_core::models::{NodeKind, NodeModel, PortDirection, PortModel};
use fasttick_core::project::{FileId, Project};
use fasttick_core::xml::NodeId;
use proptest::prelude::*;

const TEXTS: &[&str] = &[
    r#"<?xml version="1.0" encoding="UTF-8"?>
<root BTCPP_format="4" main_tree_to_execute="Main">
    <BehaviorTree ID="Main">
        <Sequence name="root">
            <SaySomething message="{greeting}"/>
            <Fallback name="door">
                <IsDoorOpen/>
                <OpenDoor door="{door}"/>
            </Fallback>
            <SubTree ID="Walk" target="{goal}"/>
        </Sequence>
    </BehaviorTree>
    <BehaviorTree ID="Walk">
        <MoveTo goal='{target}'/>
    </BehaviorTree>
    <!-- Models -->
    <TreeNodesModel>
        <Action ID="SaySomething">
            <input_port name="message"/>
        </Action>
        <SubTree ID="Walk">
            <input_port name="target"/>
        </SubTree>
    </TreeNodesModel>
</root>
"#,
    "<root BTCPP_format=\"4\">\r\n\t<BehaviorTree ID=\"Main\">\r\n\t\t<Sequence>\r\n\t\t\t<!-- first -->\r\n\t\t\t<A x=\"1\"/>\r\n\t\t\t<Inverter>\r\n\t\t\t\t<B/>\r\n\t\t\t</Inverter>\r\n\t\t\t<C/>\r\n\t\t</Sequence>\r\n\t</BehaviorTree>\r\n</root>\r\n",
    "<root BTCPP_format=\"4\"><BehaviorTree ID=\"Main\"><Sequence><A/><B x=\"1\"/><Fallback><C/><D/></Fallback></Sequence></BehaviorTree><BehaviorTree ID=\"Walk\"><E/></BehaviorTree></root>",
    "<root>\n  <BehaviorTree ID=\"Main\">\n    <Control ID=\"Sequence\" name=\"s\">\n      <Action ID=\"Act\" p=\"{k}\"/>  <Condition ID=\"Cond\"/>\n      <SubTree ID=\"Walk\" _autoremap=\"true\"/>\n    </Control>\n  </BehaviorTree>\n  <BehaviorTree ID=\"Walk\">\n    <Sleep msec=\"{t}\"/>\n  </BehaviorTree>\n</root>\n",
];

const TYPES: &[&str] = &[
    "Sequence",
    "Fallback",
    "AlwaysSuccess",
    "Inverter",
    "Sleep",
    "Walk",
    "Main",
    "Custom",
    "Switch2",
    "2Bad",
];
const ATTRS: &[&str] = &["msec", "name", "x", "_skipIf", "ID", "value", "1bad"];
const VALUES: &[&str] = &[
    "1", "{key}", "{=}", "a<b", "q\"'&", "", "Walk", "{@root}", "x\ty",
];
const TREES: &[&str] = &["New", "Walk", "Main", "Extra", "Sub2", "bad id"];
const MODELS: &[&str] = &["Beep", "Walk", "Sleep2", "SaySomething"];

#[derive(Debug, Clone)]
enum Op {
    Add {
        parent: usize,
        index: usize,
        ty: usize,
    },
    AddUnconnected {
        tree: usize,
        ty: usize,
    },
    Delete {
        nodes: Vec<usize>,
    },
    Move {
        node: usize,
        parent: usize,
        index: usize,
    },
    SetAttr {
        node: usize,
        name: usize,
        value: usize,
        coalesce: bool,
    },
    RemoveAttr {
        node: usize,
        name: usize,
    },
    ChangeType {
        node: usize,
        ty: usize,
    },
    CopyPaste {
        nodes: Vec<usize>,
        parent: usize,
        index: usize,
    },
    PasteUnconnected {
        nodes: Vec<usize>,
        tree: usize,
    },
    Duplicate {
        nodes: Vec<usize>,
    },
    Disconnect {
        node: usize,
    },
    Connect {
        unconnected: usize,
        parent: usize,
        index: usize,
    },
    AddTree {
        name: usize,
    },
    RenameTree {
        tree: usize,
        name: usize,
    },
    DeleteTree {
        tree: usize,
    },
    SetMain {
        tree: Option<usize>,
    },
    CreateSubtree {
        node: usize,
        count: usize,
        name: usize,
    },
    AddModel {
        name: usize,
        kind: usize,
    },
    RemoveModel {
        name: usize,
    },
    Undo,
    Redo,
}

fn op() -> impl Strategy<Value = Op> {
    let n = 0..64usize;
    let nodes = prop::collection::vec(0..64usize, 1..4);
    prop_oneof![
        3 => (n.clone(), 0..5usize, 0..TYPES.len())
            .prop_map(|(parent, index, ty)| Op::Add { parent, index, ty }),
        1 => (0..4usize, 0..TYPES.len()).prop_map(|(tree, ty)| Op::AddUnconnected { tree, ty }),
        2 => nodes.clone().prop_map(|nodes| Op::Delete { nodes }),
        3 => (n.clone(), n.clone(), 0..5usize)
            .prop_map(|(node, parent, index)| Op::Move { node, parent, index }),
        3 => (n.clone(), 0..ATTRS.len(), 0..VALUES.len(), any::<bool>())
            .prop_map(|(node, name, value, coalesce)| Op::SetAttr { node, name, value, coalesce }),
        1 => (n.clone(), 0..ATTRS.len()).prop_map(|(node, name)| Op::RemoveAttr { node, name }),
        1 => (n.clone(), 0..TYPES.len()).prop_map(|(node, ty)| Op::ChangeType { node, ty }),
        2 => (nodes.clone(), n.clone(), 0..5usize)
            .prop_map(|(nodes, parent, index)| Op::CopyPaste { nodes, parent, index }),
        1 => (nodes.clone(), 0..4usize).prop_map(|(nodes, tree)| Op::PasteUnconnected { nodes, tree }),
        1 => nodes.prop_map(|nodes| Op::Duplicate { nodes }),
        2 => n.clone().prop_map(|node| Op::Disconnect { node }),
        2 => (0..4usize, n.clone(), 0..5usize)
            .prop_map(|(unconnected, parent, index)| Op::Connect { unconnected, parent, index }),
        1 => (0..TREES.len()).prop_map(|name| Op::AddTree { name }),
        1 => (0..4usize, 0..TREES.len()).prop_map(|(tree, name)| Op::RenameTree { tree, name }),
        1 => (0..4usize).prop_map(|tree| Op::DeleteTree { tree }),
        1 => prop::option::of(0..4usize).prop_map(|tree| Op::SetMain { tree }),
        1 => (n, 1..3usize, 0..TREES.len())
            .prop_map(|(node, count, name)| Op::CreateSubtree { node, count, name }),
        1 => (0..MODELS.len(), 0..5usize).prop_map(|(name, kind)| Op::AddModel { name, kind }),
        1 => (0..MODELS.len()).prop_map(|name| Op::RemoveModel { name }),
        2 => Just(Op::Undo),
        2 => Just(Op::Redo),
    ]
}

/// The tree nodes of a file: the nodes of the trees and of the
/// unconnected area, in document order.
fn tree_nodes(p: &Project, f: FileId) -> Vec<NodeId> {
    let bt = p.file(f).unwrap();
    let doc = bt.doc();
    let mut out: Vec<NodeId> = Vec::new();
    for tree in bt.trees() {
        out.extend(
            doc.descendants(tree.element)
                .skip(1)
                .filter(|&n| doc.is_element(n)),
        );
    }
    for (file, _, n) in p.all_unconnected() {
        if file == f {
            out.extend(doc.descendants(n.node).filter(|&n| doc.is_element(n)));
        }
    }
    out
}

/// Possible parents: the tree elements and the tree nodes.
fn parents(p: &Project, f: FileId) -> Vec<NodeId> {
    let mut out: Vec<NodeId> = p
        .file(f)
        .unwrap()
        .trees()
        .iter()
        .map(|t| t.element)
        .collect();
    out.extend(tree_nodes(p, f));
    out
}

/// An insertion index from 0 to the number of element children of
/// `parent` (one more than the end sometimes, to test the error).
fn index_in(p: &Project, f: FileId, parent: NodeId, i: usize) -> usize {
    let count = p.file(f).unwrap().doc().element_children(parent).count();
    i % (count + 2)
}

fn pick<T: Copy>(items: &[T], i: usize) -> Option<T> {
    if items.is_empty() {
        None
    } else {
        Some(items[i % items.len()])
    }
}

fn tree_id(p: &Project, f: FileId, i: usize) -> Option<String> {
    let trees = p.file(f).unwrap().trees();
    pick(&(0..trees.len()).collect::<Vec<_>>(), i).map(|t| trees[t].id.clone())
}

fn model(name: &str, kind: usize) -> NodeModel {
    let kind = NodeKind::ALL[kind % NodeKind::ALL.len()];
    let mut m = NodeModel::new(name, kind);
    m.ports.push(PortModel::new("p", PortDirection::Input));
    m
}

/// Runs one command in a session. `Ok(None)` means that the command was
/// skipped (no target).
fn command(ed: &mut Editor<'_>, f: FileId, op: &Op) -> Result<Option<()>, CommandError> {
    let p = ed.project();
    let nodes = tree_nodes(p, f);
    let parents = parents(p, f);
    let unconnected: Vec<NodeId> = p
        .all_unconnected()
        .filter(|(file, _, _)| *file == f)
        .map(|(_, _, n)| n.node)
        .collect();
    let pick_all =
        |list: &[usize]| -> Vec<NodeId> { list.iter().filter_map(|&i| pick(&nodes, i)).collect() };
    match op {
        Op::Add { parent, index, ty } => {
            let Some(parent) = pick(&parents, *parent) else {
                return Ok(None);
            };
            let index = index_in(p, f, parent, *index);
            ed.add_node(f, parent, index, TYPES[*ty], None)?;
        }
        Op::AddUnconnected { tree, ty } => {
            let Some(tree) = tree_id(p, f, *tree) else {
                return Ok(None);
            };
            ed.add_unconnected_node(f, &tree, TYPES[*ty], None, [*ty as f32, 1.0])?;
        }
        Op::Delete { nodes: list } => {
            let list = pick_all(list);
            if list.is_empty() {
                return Ok(None);
            }
            ed.delete_nodes(f, &list)?;
        }
        Op::Move {
            node,
            parent,
            index,
        } => {
            let (Some(node), Some(parent)) = (pick(&nodes, *node), pick(&parents, *parent)) else {
                return Ok(None);
            };
            let index = index_in(p, f, parent, *index);
            ed.move_node(f, node, parent, index)?;
        }
        Op::SetAttr {
            node, name, value, ..
        } => {
            let Some(node) = pick(&nodes, *node) else {
                return Ok(None);
            };
            ed.set_attribute(f, node, ATTRS[*name], VALUES[*value])?;
        }
        Op::RemoveAttr { node, name } => {
            let Some(node) = pick(&nodes, *node) else {
                return Ok(None);
            };
            ed.remove_attribute(f, node, ATTRS[*name])?;
        }
        Op::ChangeType { node, ty } => {
            let Some(node) = pick(&nodes, *node) else {
                return Ok(None);
            };
            ed.change_node_type(f, node, TYPES[*ty], None)?;
        }
        Op::CopyPaste {
            nodes: list,
            parent,
            index,
        } => {
            let list = pick_all(list);
            let Some(parent) = pick(&parents, *parent) else {
                return Ok(None);
            };
            if list.is_empty() {
                return Ok(None);
            }
            let index = index_in(p, f, parent, *index);
            let text = copy(ed.project(), f, &list)?;
            ed.paste(f, &text, parent, index)?;
        }
        Op::PasteUnconnected { nodes: list, tree } => {
            let list = pick_all(list);
            let Some(tree) = tree_id(p, f, *tree) else {
                return Ok(None);
            };
            if list.is_empty() {
                return Ok(None);
            }
            let text = copy(ed.project(), f, &list)?;
            ed.paste_unconnected(f, &tree, &text, [5.0, 5.0])?;
        }
        Op::Duplicate { nodes: list } => {
            let list = pick_all(list);
            if list.is_empty() {
                return Ok(None);
            }
            ed.duplicate(f, &list)?;
        }
        Op::Disconnect { node } => {
            let Some(node) = pick(&nodes, *node) else {
                return Ok(None);
            };
            ed.disconnect(f, node, [10.0, 10.0])?;
        }
        Op::Connect {
            unconnected: u,
            parent,
            index,
        } => {
            let (Some(node), Some(parent)) = (pick(&unconnected, *u), pick(&parents, *parent))
            else {
                return Ok(None);
            };
            let index = index_in(p, f, parent, *index);
            ed.connect(f, node, parent, index)?;
        }
        Op::AddTree { name } => {
            ed.add_tree(f, TREES[*name])?;
        }
        Op::RenameTree { tree, name } => {
            let Some(tree) = tree_id(p, f, *tree) else {
                return Ok(None);
            };
            ed.rename_tree(f, &tree, TREES[*name])?;
        }
        Op::DeleteTree { tree } => {
            let Some(tree) = tree_id(p, f, *tree) else {
                return Ok(None);
            };
            ed.delete_tree(f, &tree)?;
        }
        Op::SetMain { tree } => {
            let tree = match tree {
                Some(t) => match tree_id(p, f, *t) {
                    Some(id) => Some(id),
                    None => return Ok(None),
                },
                None => None,
            };
            ed.set_main_tree(f, tree.as_deref())?;
        }
        Op::CreateSubtree { node, count, name } => {
            let Some(first) = pick(&nodes, *node) else {
                return Ok(None);
            };
            let doc = ed.project().file(f).unwrap().doc();
            let mut list = vec![first];
            let mut next = first;
            for _ in 1..*count {
                match doc.next_sibling(next) {
                    Some(s) => {
                        next = s;
                        if doc.is_element(s) {
                            list.push(s);
                        }
                    }
                    None => break,
                }
            }
            ed.create_subtree_from_selection(f, &list, TREES[*name])?;
        }
        Op::AddModel { name, kind } => {
            ed.add_model(f, &model(MODELS[*name], *kind))?;
        }
        Op::RemoveModel { name } => {
            ed.remove_model(f, MODELS[*name])?;
        }
        Op::Undo | Op::Redo => return Ok(None),
    }
    Ok(Some(()))
}

fn no_file_dirty(p: &Project) -> bool {
    p.files().all(|(_, _, f)| !f.is_dirty())
}

fn run_sequence(text_index: usize, ops: Vec<(Op, bool)>) -> Result<(), TestCaseError> {
    let (mut p, f) = project(TEXTS[text_index]);
    let original = snapshot(&p);
    let mut history = History::new();
    // states[i]: the state after step i of the undo stack (0: the start).
    let mut states: Vec<Snapshot> = vec![original.clone()];
    let mut redo_states: Vec<Snapshot> = Vec::new();
    let t0 = Instant::now();
    for (i, (op, allow_invalid)) in ops.iter().enumerate() {
        match op {
            Op::Undo => {
                if history.undo(&mut p).unwrap().is_some() {
                    redo_states.push(states.pop().unwrap());
                    prop_assert_eq!(&snapshot(&p), states.last().unwrap(), "undo");
                    if states.len() == 1 {
                        prop_assert!(no_file_dirty(&p));
                    }
                } else {
                    prop_assert_eq!(states.len(), 1);
                }
                continue;
            }
            Op::Redo => {
                if history.redo(&mut p).unwrap().is_some() {
                    states.push(redo_states.pop().unwrap());
                    prop_assert_eq!(&snapshot(&p), states.last().unwrap(), "redo");
                } else {
                    prop_assert!(redo_states.is_empty());
                }
                continue;
            }
            _ => {}
        }
        history.set_structure_policy(if *allow_invalid {
            StructurePolicy::AllowInvalid
        } else {
            StructurePolicy::Enforce
        });
        let before = snapshot(&p);
        let depth = history.undo_len();
        let result = match op {
            Op::SetAttr {
                node,
                name,
                coalesce: true,
                ..
            } => {
                let key = CoalesceKey::new(format!("{node}:{name}"));
                let coalesce = Coalesce {
                    key,
                    now: t0 + Duration::from_millis(100 * i as u64),
                    window: Duration::from_millis(250),
                };
                history.run_coalesced(&mut p, coalesce, |ed| command(ed, f, op))
            }
            _ => history.run(&mut p, |ed| command(ed, f, op)),
        };
        let after = snapshot(&p);
        match result {
            Err(_) | Ok(None) => {
                prop_assert_eq!(
                    &after,
                    &before,
                    "a failed or skipped command changed the project"
                );
                prop_assert_eq!(history.undo_len(), depth);
            }
            Ok(Some(())) => {
                check_well_formed(&p);
                if history.undo_len() == depth + 1 {
                    redo_states.clear();
                    states.push(after);
                } else {
                    prop_assert_eq!(history.undo_len(), depth);
                    if after != before {
                        // Merged into the previous step.
                        *states.last_mut().unwrap() = after;
                        prop_assert!(redo_states.is_empty());
                    }
                }
            }
        }
    }
    prop_assert_eq!(history.undo_len(), states.len() - 1);
    prop_assert_eq!(history.redo_len(), redo_states.len());
    // Undo everything.
    while history.undo(&mut p).unwrap().is_some() {
        redo_states.push(states.pop().unwrap());
        prop_assert_eq!(&snapshot(&p), states.last().unwrap(), "final undo");
    }
    prop_assert_eq!(&snapshot(&p), &original);
    prop_assert!(no_file_dirty(&p));
    // Redo everything.
    while history.redo(&mut p).unwrap().is_some() {
        states.push(redo_states.pop().unwrap());
        prop_assert_eq!(&snapshot(&p), states.last().unwrap(), "final redo");
        check_well_formed(&p);
    }
    Ok(())
}

proptest! {
    #![proptest_config(ProptestConfig { cases: 200, ..ProptestConfig::default() })]

    #[test]
    fn random_commands_undo_and_redo(
        text_index in 0..TEXTS.len(),
        ops in prop::collection::vec((op(), prop::bool::weighted(0.3)), 1..40),
    ) {
        run_sequence(text_index, ops)?;
    }
}

/// A long fixed sequence on each text, so that each command runs on each
/// formatting style.
#[test]
fn every_command_on_every_text() {
    let ops: Vec<Op> = vec![
        Op::Add {
            parent: 1,
            index: 0,
            ty: 2,
        },
        Op::Add {
            parent: 1,
            index: 1,
            ty: 0,
        },
        Op::SetAttr {
            node: 2,
            name: 0,
            value: 4,
            coalesce: false,
        },
        Op::Move {
            node: 3,
            parent: 1,
            index: 0,
        },
        Op::CopyPaste {
            nodes: vec![1],
            parent: 0,
            index: 0,
        },
        Op::Duplicate { nodes: vec![2, 3] },
        Op::Disconnect { node: 2 },
        Op::Connect {
            unconnected: 0,
            parent: 1,
            index: 1,
        },
        Op::ChangeType { node: 1, ty: 1 },
        Op::RemoveAttr { node: 2, name: 0 },
        Op::AddTree { name: 4 },
        Op::Add {
            parent: 0,
            index: 0,
            ty: 0,
        },
        Op::CreateSubtree {
            node: 2,
            count: 2,
            name: 3,
        },
        Op::RenameTree { tree: 0, name: 0 },
        Op::SetMain { tree: Some(1) },
        Op::AddModel { name: 0, kind: 1 },
        Op::PasteUnconnected {
            nodes: vec![1],
            tree: 0,
        },
        Op::AddUnconnected { tree: 1, ty: 3 },
        Op::Delete { nodes: vec![1, 4] },
        Op::DeleteTree { tree: 1 },
        Op::RemoveModel { name: 0 },
        Op::Undo,
        Op::Undo,
        Op::Redo,
    ];
    for text_index in 0..TEXTS.len() {
        let ops = ops.iter().cloned().map(|op| (op, true)).collect();
        if let Err(e) = run_sequence(text_index, ops) {
            panic!("text {text_index}: {e}");
        }
    }
}
