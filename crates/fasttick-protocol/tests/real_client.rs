//! Tests of [`Client`] against the real executor (`test_harness`, BT.CPP
//! 4.x): FULLTREE, STATUS, BLACKBOARD and the hook requests.
//!
//! The breakpoint, replace and heartbeat behaviour is in `real_breakpoints.rs`,
//! the recording requests are in `real_recording.rs`.
//!
//! Each test skips cleanly when BT.CPP 4.x is not found. Run them with
//! `FASTTICK_BTCPP_PREFIX=<BT.CPP install prefix>`.

mod real_support;

use std::collections::{BTreeMap, BTreeSet};
use std::time::Instant;

use fasttick_core::runtime::{RuntimeTree, Uid};
use fasttick_core::status::NodeStatus;
use fasttick_protocol::client::ClientError;
use fasttick_protocol::messages::{Hook, HookMode, HookPosition, UnlockRequest};
use real_support::{Live, WAIT, client, options, statuses_by_name, wait_until};
use serde_json::{Value, json};

const LONG_RUNNING: &str = "long_running.xml";

/// The nodes of `long_running.xml` in depth-first order of the expanded
/// tree: depth, type and name.
const LONG_RUNNING_NODES: [(usize, &str, &str); 13] = [
    (0, "Sequence", "long_sequence"),
    (1, "Script", "setup"),
    (1, "KeepRunningUntilFailure", "forever"),
    (2, "Sequence", "cycle"),
    (3, "CheckFlag", "check_keep_going"),
    (3, "SayMessage", "say_cycle"),
    (3, "WriteValues", "write_cycle_values"),
    (3, "WaitTicks", "slow_action"),
    (3, "IncrementCounter", "count_cycle"),
    (3, "SubTree", "worker"),
    (4, "Sequence", "worker_sequence"),
    (5, "ReadPose", "read_worker_pose"),
    (5, "WaitTicks", "worker_wait"),
];

fn all_uids(tree: &RuntimeTree) -> BTreeSet<u16> {
    tree.nodes
        .iter()
        .map(|node| node.uid.expect("runtime nodes have a UID").0)
        .collect()
}

#[test]
fn full_tree_matches_the_executed_xml() {
    let Some(harness) = real_support::harness() else {
        return;
    };
    let mut live = Live::start(options(harness, LONG_RUNNING));

    // The reply UUID is the random version-4 UUID of the publisher
    // (CreateRandomUUID in the publisher source).
    let uuid = live.uuid.as_bytes();
    assert_ne!(uuid, &[0_u8; 16]);
    assert_eq!(uuid[6] & 0xF0, 0x40, "UUID version: {}", live.uuid);
    assert_eq!(uuid[8] & 0xC0, 0x80, "UUID variant: {}", live.uuid);
    assert_eq!(live.client.last_tree_uuid(), Some(live.uuid));

    // The expanded tree has the structure of the source file.
    assert_eq!(live.tree.instances.len(), 2);
    assert_eq!(live.tree.root_instance().tree_id, "LongRunning");
    assert_eq!(live.tree.root_instance().fullpath, "");
    assert_eq!(live.tree.instances[1].tree_id, "Worker");
    assert_eq!(live.tree.instances[1].fullpath, "worker");
    assert_eq!(live.tree.instances[1].blackboard_name(), "worker");
    let walk: Vec<(usize, &str, &str)> = live
        .tree
        .walk_expanded()
        .into_iter()
        .map(|(index, depth)| {
            let node = live.tree.node(index);
            (depth, node.type_id.as_str(), node.name.as_str())
        })
        .collect();
    assert_eq!(walk, LONG_RUNNING_NODES);

    // Each node of the source file is in the executed tree, at the same
    // place, with the same attributes. The executor adds `_uid`,
    // `_fullpath` and the default values of ports (for example `step`),
    // and it drops `_autoremap` (WriteTreeToXML in xml_parsing.cpp).
    let source_xml = std::fs::read_to_string(harness.tree(LONG_RUNNING)).unwrap();
    let source = RuntimeTree::parse(&source_xml).unwrap();
    assert_eq!(source.instances.len(), live.tree.instances.len());
    for (source_instance, runtime_instance) in source.instances.iter().zip(&live.tree.instances) {
        assert_eq!(source_instance.tree_id, runtime_instance.tree_id);
        let source_nodes: Vec<_> = source
            .nodes
            .iter()
            .filter(|node| source.instances[node.instance.0].tree_id == source_instance.tree_id)
            .collect();
        let runtime_nodes: Vec<_> = live
            .tree
            .nodes
            .iter()
            .filter(|node| live.tree.instances[node.instance.0].tree_id == runtime_instance.tree_id)
            .collect();
        assert_eq!(source_nodes.len(), runtime_nodes.len());
        for (source_node, runtime_node) in source_nodes.iter().zip(&runtime_nodes) {
            assert_eq!(source_node.type_id, runtime_node.type_id);
            assert_eq!(source_node.name, runtime_node.name);
            for (key, value) in &source_node.attributes {
                if key == "_autoremap" || key == "name" {
                    continue;
                }
                assert_eq!(
                    runtime_node.attribute(key),
                    Some(value.as_str()),
                    "attribute {key} of node {}",
                    source_node.name
                );
            }
        }
    }
    let defaulted = live.tree.node_by_uid(Uid(live.uid("count_cycle"))).unwrap();
    assert_eq!(defaulted.attribute("step"), Some("1"));

    // The UIDs are unique and every node has one (RuntimeTree::parse
    // rejects duplicates; check the count too).
    assert_eq!(all_uids(&live.tree).len(), live.tree.len());
    assert!(live.tree.nodes.iter().all(|node| node.uid.is_some()));

    // FULLTREE is stable: a second request and a second client get the
    // same text and the same UUID.
    let (uuid_again, xml_again) = live.client.full_tree().unwrap();
    assert_eq!((uuid_again, &xml_again), (live.uuid, &live.xml));
    let mut other = client(live.port);
    let (other_uuid, other_xml) = other.full_tree().unwrap();
    assert_eq!((other_uuid, &other_xml), (live.uuid, &live.xml));
    assert!(live.executor.is_running().unwrap());
}

#[test]
fn status_covers_every_node_and_changes_over_time() {
    let Some(harness) = real_support::harness() else {
        return;
    };
    let mut live = Live::start(options(harness, LONG_RUNNING));
    let tree_uids = all_uids(&live.tree);
    let slow_action = Uid(live.uid("slow_action"));

    let mut distinct = BTreeSet::new();
    let mut slow_states = BTreeSet::new();
    let deadline = Instant::now() + WAIT;
    // Poll until the async action was seen RUNNING and also back in IDLE
    // after a SUCCESS, and the snapshots changed several times.
    while !(slow_states.contains(&(NodeStatus::Running, None))
        && slow_states.contains(&(NodeStatus::Idle, Some(NodeStatus::Success)))
        && distinct.len() >= 3)
    {
        assert!(
            Instant::now() < deadline,
            "STATUS did not change as expected; seen states of slow_action: {slow_states:?}"
        );
        let snapshot = live.client.status().unwrap();
        assert_eq!(snapshot.len(), live.tree.len());
        let uids: BTreeSet<u16> = snapshot.entries.iter().map(|entry| entry.uid.0).collect();
        assert_eq!(uids, tree_uids, "STATUS has each UID of the tree once");

        let by_name = statuses_by_name(&live.tree, &snapshot);
        // The root stays RUNNING, and the Script ran once at the start.
        assert_eq!(by_name["long_sequence"], NodeStatus::Running);
        assert_eq!(by_name["setup"], NodeStatus::Success);
        for entry in &snapshot.entries {
            // The byte 10 + status means IDLE after that status
            // (callback of the publisher).
            match entry.raw.0 {
                0..=4 => assert_eq!(entry.previous, None),
                11..=13 => {
                    assert_eq!(entry.current, NodeStatus::Idle);
                    assert_eq!(
                        entry.previous.map(NodeStatus::as_u8),
                        Some(entry.raw.0 - 10)
                    );
                }
                other => panic!("unexpected status byte {other}"),
            }
        }
        let slow = snapshot.get(slow_action).unwrap();
        slow_states.insert((slow.current, slow.previous));
        distinct.insert(snapshot.encode());
        std::thread::sleep(std::time::Duration::from_millis(5));
    }
    assert_eq!(live.client.last_tree_uuid(), Some(live.uuid));
}

#[test]
fn blackboards_of_the_root_tree_and_of_a_subtree_instance() {
    let Some(harness) = real_support::harness() else {
        return;
    };
    let mut live = Live::start(options(harness, LONG_RUNNING));
    let root_name = live.tree.root_instance().blackboard_name().to_owned();
    assert_eq!(root_name, "LongRunning");

    // Wait for one complete cycle, so that all entries have a value.
    wait_until("one complete cycle", || {
        let value = live.client.blackboards(&[&root_name]).unwrap();
        (value[&root_name]["cycles"].as_u64() >= Some(1)).then_some(())
    });

    // Stop the tree before "say_cycle": WriteValues and the SubTree of
    // the last cycle are complete, and nothing changes during the reads.
    let check = live.uid("check_keep_going");
    live.sync_notifications(check);
    let say = live.uid("say_cycle");
    live.client.insert_hook(&Hook::breakpoint(say)).unwrap();
    live.wait_hit(say);

    let value = live
        .client
        .blackboards(&[root_name.as_str(), "worker", "no_such_blackboard"])
        .unwrap();
    let names: Vec<&String> = value.as_object().unwrap().keys().collect();
    assert_eq!(names, ["LongRunning", "worker"], "{value:#}");

    // WriteValues writes the values of its tick number n (nodes.cpp).
    let root = &value["LongRunning"];
    let n = root["an_int"].as_i64().unwrap() - 42;
    assert!(n >= 0, "{root:#}");
    let pose = json!({"__type": "Pose2D", "x": 1.0 + n as f64, "y": 2.0, "theta": 0.5});
    assert_eq!(root["a_double"], json!(3.25 * (n + 1) as f64));
    assert_eq!(root["a_string"], json!(format!("tick {n}")));
    assert_eq!(root["a_vector"], json!([1, 2, 3, n]));
    // A type with a JSON converter is a nested object, and a vector of it
    // is an array of objects (JsonExporter in json_export.h).
    assert_eq!(root["a_pose"], pose);
    assert_eq!(
        root["pose_list"],
        json!([pose, {"__type": "Pose2D", "x": 0.0, "y": 0.0, "theta": 0.0}])
    );
    // The cycle counter runs after WriteValues: it shows the number of
    // complete cycles, one more than n.
    assert_eq!(root["cycles"], json!(n + 1));
    assert!(root.get("keep_going").is_some(), "{root:#}");

    // The SubTree instance has its own blackboard. Its "pose" port is
    // remapped to the parent entry, so only "distance" is local.
    let worker = value["worker"].as_object().unwrap();
    assert_eq!(worker.keys().collect::<Vec<_>>(), ["distance"]);
    let distance = worker["distance"].as_f64().unwrap();
    let expected = (1.0 + n as f64).hypot(2.0);
    assert!(
        (distance - expected).abs() < 1e-9,
        "{distance} != {expected}"
    );

    // Unknown names are left out. They are not an error.
    assert_eq!(
        live.client.blackboards(&["no_such_blackboard"]).unwrap(),
        json!({})
    );
    assert_eq!(live.client.blackboards::<&str>(&[]).unwrap(), json!({}));
    let only_worker = live.client.blackboards(&["worker"]).unwrap();
    assert_eq!(only_worker, json!({"worker": value["worker"].clone()}));

    // The tree stays at the breakpoint during the reads.
    let again = live.client.blackboards(&["LongRunning"]).unwrap();
    assert_eq!(again["LongRunning"], value["LongRunning"]);
    live.client.remove_all_hooks().unwrap();
}

#[test]
fn hook_requests_insert_dump_remove_and_report_errors() {
    let Some(harness) = real_support::harness() else {
        return;
    };
    let mut live = Live::start(options(harness, LONG_RUNNING));
    let say = live.uid("say_cycle");
    let write = live.uid("write_cycle_values");
    let count = live.uid("count_cycle");
    let check = live.uid("check_keep_going");
    assert!(live.hooks().is_empty());

    // HOOK_INSERT with one hook. A disabled breakpoint does not stop the
    // tree.
    let disabled = Hook {
        enabled: false,
        ..Hook::breakpoint(say)
    };
    live.client.insert_hook(&disabled).unwrap();
    assert_eq!(live.hooks(), [disabled]);

    // HOOK_INSERT with an array. Each field comes back in HOOKS_DUMP.
    let array = [
        Hook {
            enabled: false,
            once: true,
            ..Hook::breakpoint(write)
        },
        Hook {
            enabled: false,
            ..Hook::replace(count, NodeStatus::Failure)
        },
        // Enabled, but the condition returns SUCCESS anyway.
        Hook::replace(check, NodeStatus::Success),
    ];
    live.client.insert_hooks(&array).unwrap();
    let mut expected: BTreeMap<u16, Hook> = [disabled]
        .iter()
        .chain(&array)
        .map(|hook| (hook.uid, *hook))
        .collect();
    assert_eq!(live.hooks(), expected.values().copied().collect::<Vec<_>>());

    // HOOK_INSERT for a node that has a hook changes that hook.
    let changed = Hook {
        enabled: false,
        desired_status: NodeStatus::Failure,
        mode: HookMode::Replace,
        ..Hook::breakpoint(say)
    };
    live.client.insert_hook(&changed).unwrap();
    expected.insert(say, changed);
    assert_eq!(live.hooks(), expected.values().copied().collect::<Vec<_>>());

    // A UID that is not in the tree: the publisher replies with success
    // but installs nothing (insertHook of the publisher).
    let unknown = u16::try_from(live.tree.len() + 100).unwrap();
    live.client.insert_hook(&Hook::breakpoint(unknown)).unwrap();
    assert_eq!(live.hooks().len(), expected.len());

    // HOOK_REMOVE.
    live.client.remove_hook(write, HookPosition::Pre).unwrap();
    expected.remove(&write);
    assert_eq!(live.hooks(), expected.values().copied().collect::<Vec<_>>());

    // Error replies: the hook or the node does not exist.
    for result in [
        live.client.remove_hook(write, HookPosition::Pre),
        live.client.remove_hook(unknown, HookPosition::Pre),
        live.client.unlock_breakpoint(&UnlockRequest::resume(write)),
        live.client
            .unlock_breakpoint(&UnlockRequest::resume(unknown)),
    ] {
        match result {
            Err(ClientError::Server(message)) => assert_eq!(message, "Node ID not found"),
            other => panic!("expected an error reply, got {other:?}"),
        }
    }
    // The connection stays usable after error replies.
    assert_eq!(live.client.status().unwrap().len(), live.tree.len());

    // DISABLE_ALL_HOOKS keeps the hooks and clears "enabled".
    live.client.disable_all_hooks().unwrap();
    let after_disable = live.hooks();
    assert_eq!(after_disable.len(), expected.len());
    for hook in &after_disable {
        let before = expected[&hook.uid];
        assert_eq!(
            *hook,
            Hook {
                enabled: false,
                ..before
            }
        );
    }

    // REMOVE_ALL_HOOKS removes all hooks. A second call has nothing to do.
    live.client.remove_all_hooks().unwrap();
    assert!(live.hooks().is_empty());
    live.client.remove_all_hooks().unwrap();
    live.client.disable_all_hooks().unwrap();
    assert!(live.hooks().is_empty());

    // The executor ran all the time.
    assert!(live.executor.is_running().unwrap());
    assert_eq!(live.client.last_tree_uuid(), Some(live.uuid));
    let cycles = |live: &mut Live| -> u64 {
        let value: Value = live.client.blackboards(&["LongRunning"]).unwrap();
        value["LongRunning"]["cycles"].as_u64().unwrap_or(0)
    };
    let start = cycles(&mut live);
    wait_until("the tree to make progress", || {
        (cycles(&mut live) > start).then_some(())
    });
}
