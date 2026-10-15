//! Recording of transitions (TOGGLE_RECORDING and GET_TRANSITIONS) with
//! [`Client`](fasttick_protocol::Client) against the real executor
//! (`test_harness`, BT.CPP 4.x), and an end-to-end check: a live recording
//! written with `fasttick_log::BtLogWriter`, read back with `BtLog` and
//! replayed.
//!
//! The recording logic of the publisher is in its `callback` and
//! `serverLoop`.
//! The publisher and `FileLogger2` get the same timestamp for a status
//! change (`TreeNode::setStatus` in `src/tree_node.cpp` calls the
//! subscribers with one `high_resolution_clock::now()` value).

mod real_support;

use std::collections::{BTreeMap, BTreeSet};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use fasttick_core::runtime::{RuntimeTree, Uid};
use fasttick_core::status::NodeStatus;
use fasttick_log::{BtLog, BtLogWriter, NodeState, ReplayIndex, Transition};
use fasttick_protocol::messages::{
    Hook, RECORDING_BUFFER_CAPACITY, RecordedTransition, StatusSnapshot, UnlockRequest,
};
use fasttick_testkit::TempDir;
use real_support::{Live, QUIET, options, wait_until};

const LONG_RUNNING: &str = "long_running.xml";

fn now_us() -> u64 {
    let now = SystemTime::now().duration_since(UNIX_EPOCH).unwrap();
    u64::try_from(now.as_micros()).unwrap()
}

/// Checks the transitions of one recording: known UIDs, timestamps that do
/// not go back, and a real change for each node (the publisher records a
/// change only when the status changes, `TreeNode::setStatus`).
fn check_transitions(tree: &RuntimeTree, transitions: &[RecordedTransition]) {
    let mut last: BTreeMap<Uid, NodeStatus> = BTreeMap::new();
    for pair in transitions.windows(2) {
        assert!(
            pair[0].timestamp_us <= pair[1].timestamp_us,
            "the timestamps go back: {pair:?}"
        );
    }
    for transition in transitions {
        assert!(
            tree.node_by_uid(transition.uid).is_some(),
            "UID {} is not in the tree",
            transition.uid
        );
        if let Some(previous) = last.insert(transition.uid, transition.status) {
            assert_ne!(previous, transition.status, "no change: {transition:?}");
        }
    }
}

/// Reads GET_TRANSITIONS and checks that the batch is not full.
fn read_batch(live: &mut Live, all: &mut Vec<RecordedTransition>) {
    let batch = live.client.transitions().unwrap();
    assert!(
        batch.len() < RECORDING_BUFFER_CAPACITY,
        "a full batch: transitions can be lost"
    );
    all.extend(batch);
}

#[test]
fn recording_start_stop_and_transitions() {
    let Some(harness) = real_support::harness() else {
        return;
    };
    let mut live = Live::start(options(harness, LONG_RUNNING));

    // The reply to "start" is the system time of the executor, in
    // microseconds since the Unix epoch.
    let before = now_us();
    let start_us = live.client.start_recording().unwrap();
    let started = Instant::now();
    let after = now_us();
    assert!(
        start_us + 5_000_000 >= before && start_us <= after + 5_000_000,
        "start time {start_us} is not near {before}..{after}"
    );

    // Collect transitions for some cycles of the tree.
    let mut all = Vec::new();
    wait_until("100 transitions", || {
        read_batch(&mut live, &mut all);
        (all.len() >= 100).then_some(())
    });
    live.client.stop_recording().unwrap();
    let stopped = started.elapsed();
    read_batch(&mut live, &mut all);
    check_transitions(&live.tree, &all);
    // The timestamps count from the start of the recording.
    let last = all.last().unwrap().timestamp_us;
    assert!(
        u128::from(last) <= stopped.as_micros() + 1_000_000,
        "last timestamp {last} µs, but the recording took {stopped:?}"
    );
    // Each node of the cycle changed status. The root, "setup" and
    // "forever" do not change while the tree runs.
    let changed: BTreeSet<&str> = all
        .iter()
        .map(|transition| live.tree.node_by_uid(transition.uid).unwrap().name.as_str())
        .collect();
    for name in [
        "cycle",
        "check_keep_going",
        "say_cycle",
        "write_cycle_values",
        "slow_action",
        "count_cycle",
        "worker",
        "worker_sequence",
        "read_worker_pose",
        "worker_wait",
    ] {
        assert!(
            changed.contains(name),
            "no transition of {name}: {changed:?}"
        );
    }
    for name in ["long_sequence", "setup", "forever"] {
        assert!(!changed.contains(name), "a transition of {name}");
    }
    // The async action goes RUNNING, SUCCESS, then IDLE when its parent
    // completes (the publisher also records the changes to IDLE).
    let slow = Uid(live.uid("slow_action"));
    let slow_statuses: Vec<NodeStatus> = all
        .iter()
        .filter(|transition| transition.uid == slow)
        .map(|transition| transition.status)
        .collect();
    let first_running = slow_statuses
        .iter()
        .position(|status| *status == NodeStatus::Running)
        .unwrap();
    assert_eq!(
        slow_statuses[first_running..first_running + 3],
        [NodeStatus::Running, NodeStatus::Success, NodeStatus::Idle]
    );

    // After the stop, no new transitions come.
    let quiet_end = Instant::now() + QUIET;
    while Instant::now() < quiet_end {
        std::thread::sleep(Duration::from_millis(50));
        assert_eq!(live.client.transitions().unwrap(), []);
    }

    // A new start clears the buffer and starts the time again.
    let second_start = live.client.start_recording().unwrap();
    let restarted = Instant::now();
    assert!(second_start > start_us, "{second_start} <= {start_us}");
    let mut second = Vec::new();
    wait_until("transitions of the second recording", || {
        read_batch(&mut live, &mut second);
        (second.len() >= 10).then_some(())
    });
    live.client.stop_recording().unwrap();
    read_batch(&mut live, &mut second);
    check_transitions(&live.tree, &second);
    let last = second.last().unwrap().timestamp_us;
    assert!(u128::from(last) <= restarted.elapsed().as_micros() + 1_000_000);
    assert!(live.executor.is_running().unwrap());
}

#[test]
fn the_buffer_keeps_only_the_last_1000_transitions() {
    let Some(harness) = real_support::harness() else {
        return;
    };
    let mut options = options(harness, LONG_RUNNING);
    options.tick_ms = 1;
    let mut live = Live::start(options);
    let cycles = |live: &mut Live| -> u64 {
        let value = live.client.blackboards(&["LongRunning"]).unwrap();
        value["LongRunning"]["cycles"].as_u64().unwrap_or(0)
    };

    // Notifications must arrive before the tree is stopped below.
    live.sync_notifications(live.uid("check_keep_going"));
    live.client.start_recording().unwrap();
    let first = cycles(&mut live);
    // A cycle has more than 20 transitions, so 60 cycles have more than
    // 1000.
    wait_until("60 cycles", || {
        (cycles(&mut live) >= first + 60).then_some(())
    });
    // Stop the tree before "say_cycle", so that the newest transitions are
    // known: the last change is "check_keep_going" to SUCCESS.
    let say = live.uid("say_cycle");
    live.client.insert_hook(&Hook::breakpoint(say)).unwrap();
    live.wait_hit(say);
    let status = live.client.status().unwrap();
    let batch = live.client.transitions().unwrap();
    assert_eq!(batch.len(), RECORDING_BUFFER_CAPACITY);
    check_transitions(&live.tree, &batch);

    // The publisher dropped the oldest transitions and kept the newest
    // ones (callback of the publisher pops the front of the queue). The
    // batch therefore ends with the changes just before the stop: the last
    // status of each node in the batch is its status now.
    let check = Uid(live.uid("check_keep_going"));
    let last = batch.last().unwrap();
    assert_eq!((last.uid, last.status), (check, NodeStatus::Success));
    let mut last_status: BTreeMap<Uid, NodeStatus> = BTreeMap::new();
    for transition in &batch {
        last_status.insert(transition.uid, transition.status);
    }
    for (uid, recorded) in &last_status {
        assert_eq!(
            status.get(*uid).unwrap().current,
            *recorded,
            "node {uid}: the batch does not end with its newest change"
        );
    }
    // The oldest changes are gone: the batch holds fewer cycles than the
    // tree completed during the recording.
    let count_cycle = Uid(live.uid("count_cycle"));
    let cycles_in_batch = batch
        .iter()
        .filter(|transition| {
            transition.uid == count_cycle && transition.status == NodeStatus::Success
        })
        .count() as u64;
    let completed = cycles(&mut live) - first;
    assert!(
        cycles_in_batch < completed,
        "{cycles_in_batch} cycles in the batch, {completed} cycles completed"
    );
    // GET_TRANSITIONS cleared the buffer, and the stopped tree adds nothing.
    assert_eq!(live.client.transitions().unwrap(), []);
    live.client.remove_all_hooks().unwrap();
    live.client.stop_recording().unwrap();
}

/// The state of each node before and after a recording, from STATUS.
fn node_states(snapshot: &StatusSnapshot) -> BTreeMap<Uid, NodeState> {
    snapshot
        .entries
        .iter()
        .map(|entry| {
            let last_non_idle = if entry.current == NodeStatus::Idle {
                entry.previous
            } else {
                Some(entry.current)
            };
            let state = NodeState {
                status: entry.current,
                last_non_idle,
            };
            (entry.uid, state)
        })
        .collect()
}

#[test]
fn recording_round_trip_through_a_btlog_file() {
    let Some(harness) = real_support::harness() else {
        return;
    };
    let temp = TempDir::new("fasttick-real-recording").unwrap();
    let executor_log = temp.join("executor.btlog");
    let live_log = temp.join("live.btlog");
    let mut options = options(harness, LONG_RUNNING);
    options.btlog = Some(executor_log.clone());
    let mut live = Live::start(options);

    // Stop the tree at "say_cycle". Then the recording starts with a known
    // state and no change can happen at the start.
    live.sync_notifications(live.uid("check_keep_going"));
    let say = live.uid("say_cycle");
    live.client.insert_hook(&Hook::breakpoint(say)).unwrap();
    live.wait_hit(say);
    let first_state = node_states(&live.client.status().unwrap());

    let start_us = live.client.start_recording().unwrap();
    let started = Instant::now();
    let mut transitions = Vec::new();
    read_batch(&mut live, &mut transitions);
    assert!(
        transitions.is_empty(),
        "the tree is stopped: {transitions:?}"
    );
    // Stay stopped for a moment: a clear gap in the executor log.
    live.assert_frozen();

    // One full cycle, until the tree stops at "say_cycle" again.
    let unlocked = started.elapsed();
    live.client
        .unlock_breakpoint(&UnlockRequest::resume(say))
        .unwrap();
    let deadline = Instant::now() + real_support::WAIT;
    loop {
        assert!(Instant::now() < deadline, "no second stop at say_cycle");
        read_batch(&mut live, &mut transitions);
        if live
            .client
            .poll_notification(Duration::from_millis(20))
            .unwrap()
            .is_some_and(|notification| notification.uid == Uid(say))
        {
            break;
        }
    }
    let stopped_again = started.elapsed();
    live.assert_frozen();
    live.client.stop_recording().unwrap();
    read_batch(&mut live, &mut transitions);
    let last_state = node_states(&live.client.status().unwrap());

    // Plausible times: all transitions happened between the unlock and
    // the second stop.
    check_transitions(&live.tree, &transitions);
    assert!(transitions.len() > 20, "{} transitions", transitions.len());
    let first_time = transitions.first().unwrap().timestamp_us;
    let last_time = transitions.last().unwrap().timestamp_us;
    assert!(
        u128::from(first_time) >= unlocked.as_micros(),
        "first change at {first_time} µs, before the unlock at {unlocked:?}"
    );
    assert!(
        u128::from(last_time) <= stopped_again.as_micros() + 1_000_000,
        "last change at {last_time} µs, after the stop at {stopped_again:?}"
    );

    // The first state plus the transitions gives the last state.
    let mut replayed = first_state.clone();
    for transition in &transitions {
        replayed
            .get_mut(&transition.uid)
            .unwrap()
            .apply(transition.status);
    }
    assert_eq!(replayed, last_state);

    // Write the recording as a .btlog file, as a live recording in the
    // GUI does: the XML from FULLTREE and the start time from the reply.
    let records: Vec<Transition> = transitions
        .iter()
        .map(|transition| {
            Transition::new(transition.timestamp_us, transition.uid, transition.status)
        })
        .collect();
    let start_time = i64::try_from(start_us).unwrap();
    let mut writer = BtLogWriter::create(&live_log, &live.xml, start_time).unwrap();
    writer.push_batch(&records).unwrap();
    writer.flush().unwrap();
    assert_eq!(writer.records_written(), records.len() as u64);
    drop(writer);

    let log = BtLog::open(&live_log).unwrap();
    assert_eq!(log.xml, live.xml);
    assert_eq!(log.start_time_us, start_time);
    assert_eq!(log.transitions, records);
    assert_eq!(log.truncated_bytes, 0);
    assert_eq!(log.invalid_records, 0);
    assert!(log.unknown_uids().is_empty());
    assert_eq!(log.tree.len(), live.tree.len());

    // The replay of the file agrees with STATUS for each node that changed.
    let index = ReplayIndex::from_log(&log);
    assert_eq!(index.len(), records.len());
    assert_eq!(index.uids().len(), live.tree.len());
    let end = index.state_after(index.len());
    assert_eq!(end, index.state_at(last_time));
    let changed: BTreeSet<Uid> = records.iter().map(|record| record.uid).collect();
    for uid in &changed {
        assert_eq!(end.status(*uid), last_state[uid].status, "node {uid}");
    }
    for uid in live.tree.nodes.iter().filter_map(|node| node.uid) {
        if !changed.contains(&uid) {
            assert_eq!(end.status(uid), NodeStatus::Idle);
            assert_eq!(first_state[&uid], last_state[&uid], "node {uid}");
        }
    }

    // The executor wrote its own log with FileLogger2. Stop it, so that the
    // log is complete, and compare: the same tree XML, and the same changes
    // at the same absolute times.
    live.client.remove_all_hooks().unwrap();
    let status = live.executor.stop().unwrap();
    assert!(status.success(), "{status}");
    let file_log = BtLog::open(&executor_log).unwrap();
    assert_eq!(file_log.xml, live.xml);
    let absolute = |start: i64, offset: u64| start + i64::try_from(offset).unwrap();
    let ours: Vec<(i64, Uid, NodeStatus)> = records
        .iter()
        .map(|record| {
            (
                absolute(start_time, record.offset_us),
                record.uid,
                record.status,
            )
        })
        .collect();
    // The tree was stopped for some time before and after the recording, so
    // a margin of 50 ms finds exactly the same changes.
    let margin = 50_000;
    let window = ours.first().unwrap().0 - margin..=ours.last().unwrap().0 + margin;
    let theirs: Vec<(i64, Uid, NodeStatus)> = file_log
        .transitions
        .iter()
        .map(|record| {
            (
                absolute(file_log.start_time_us, record.offset_us),
                record.uid,
                record.status,
            )
        })
        .filter(|(time, _, _)| window.contains(time))
        .collect();
    assert_eq!(
        ours.len(),
        theirs.len(),
        "ours: {ours:?}\ntheirs: {theirs:?}"
    );
    for (mine, logged) in ours.iter().zip(&theirs) {
        assert_eq!((mine.1, mine.2), (logged.1, logged.2));
        // The publisher reads the clock twice at the start (once for the
        // offsets, once for the reply), so the times can differ a little.
        assert!(
            (mine.0 - logged.0).abs() < 10_000,
            "times differ: {mine:?} {logged:?}"
        );
    }
}
