//! Tests of [`LiveSession`] against the real executor (`test_harness`,
//! BT.CPP 4.x): the full flow through commands and events, the cleanup on
//! disconnect, the heartbeat and the detection of an executor restart.
//!
//! The tests use `test_harness/trees/probe.xml` (see `real_breakpoints.rs`).
//! A separate [`Client`](fasttick_protocol::Client) checks the real effects
//! on the executor.

mod real_support;

use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::mpsc::{Receiver, RecvTimeoutError};
use std::time::{Duration, Instant};

use fasttick_core::runtime::{RuntimeTree, Uid};
use fasttick_core::status::NodeStatus;
use fasttick_protocol::messages::{Hook, HookPosition, RecordedTransition, UnlockRequest};
use fasttick_protocol::session::{
    ConnectionState, LiveSession, PollConfig, SessionCommand, SessionConfig, SessionEvent,
    SessionHandle,
};
use fasttick_protocol::wire::TreeUuid;
use fasttick_testkit::{Executor, require_harness};
use real_support::{Live, ProbeCounters, WAIT, client, options, wait_until};

/// The event receiver, with a log of the events seen so far.
struct Events {
    receiver: Receiver<SessionEvent>,
    seen: Vec<SessionEvent>,
}

impl Events {
    fn new(receiver: Receiver<SessionEvent>) -> Events {
        Events {
            receiver,
            seen: Vec::new(),
        }
    }

    /// Waits for an event that matches `check`. Other events go to the
    /// log. An `Error` event fails the test.
    fn wait_for<T>(&mut self, what: &str, mut check: impl FnMut(&SessionEvent) -> Option<T>) -> T {
        let deadline = Instant::now() + WAIT;
        loop {
            let remaining = deadline.saturating_duration_since(Instant::now());
            match self.receiver.recv_timeout(remaining) {
                Ok(event) => {
                    if let SessionEvent::Error(message) = &event {
                        panic!(
                            "error event while waiting for {what}: {message}; events: {:#?}",
                            self.seen
                        );
                    }
                    let found = check(&event);
                    self.seen.push(event);
                    if let Some(found) = found {
                        return found;
                    }
                }
                Err(RecvTimeoutError::Timeout) => {
                    panic!("timeout waiting for {what}; events: {:#?}", self.seen)
                }
                Err(RecvTimeoutError::Disconnected) => {
                    panic!(
                        "the session stopped while waiting for {what}; events: {:#?}",
                        self.seen
                    )
                }
            }
        }
    }

    fn connected(&mut self) -> TreeUuid {
        self.wait_for("Connected", |event| match event {
            SessionEvent::State(ConnectionState::Connected { uuid }) => Some(*uuid),
            _ => None,
        })
    }

    fn tree(&mut self) -> (TreeUuid, Arc<RuntimeTree>) {
        self.wait_for("Tree", |event| match event {
            SessionEvent::Tree { uuid, tree } => Some((*uuid, Arc::clone(tree))),
            _ => None,
        })
    }

    fn breakpoint(&mut self, uid: u16) {
        self.wait_for(&format!("BreakpointReached for node {uid}"), |event| {
            matches!(event, SessionEvent::BreakpointReached { uid: hit } if *hit == Uid(uid))
                .then_some(())
        });
    }

    fn disconnected(&mut self) {
        self.wait_for("Disconnected", |event| {
            matches!(event, SessionEvent::State(ConnectionState::Disconnected)).then_some(())
        });
    }

    /// The events that arrived up to now, without a wait.
    fn drain(&mut self) -> Vec<SessionEvent> {
        let new: Vec<SessionEvent> = self.receiver.try_iter().collect();
        self.seen.extend(new.iter().cloned());
        new
    }
}

fn fast_config() -> SessionConfig {
    SessionConfig {
        poll: PollConfig {
            status_interval: Duration::from_millis(30),
            heartbeat_interval: Duration::from_millis(200),
            transitions_interval: Duration::from_millis(50),
        },
        request_timeout: Duration::from_secs(2),
        connect_timeout: Duration::from_secs(1),
        reconnect_initial_delay: Duration::from_millis(50),
        reconnect_max_delay: Duration::from_millis(200),
        reinsert_hooks: true,
        cleanup_timeout: Duration::from_secs(2),
    }
}

/// Starts a session, connects it to the executor of `live` and waits for
/// the tree and two status events.
fn connect(live: &Live, config: SessionConfig) -> (SessionHandle, Events) {
    let (handle, receiver) = LiveSession::spawn(config, None);
    let mut events = Events::new(receiver);
    assert!(handle.connect("127.0.0.1", live.port));
    let uuid = events.connected();
    assert_eq!(uuid, live.uuid);
    let (tree_uuid, tree) = events.tree();
    assert_eq!(tree_uuid, live.uuid);
    assert_eq!(*tree, live.tree);
    for _ in 0..2 {
        events.wait_for("Status", |event| {
            matches!(event, SessionEvent::Status { .. }).then_some(())
        });
    }
    (handle, events)
}

fn send(handle: &SessionHandle, command: SessionCommand) {
    assert!(handle.send(command), "the session thread stopped");
}

#[test]
fn session_end_to_end() {
    let harness = require_harness!();
    let mut live = Live::probe(harness);
    let probe = live.uid("probe");

    let wakes = Arc::new(AtomicUsize::new(0));
    let counter = Arc::clone(&wakes);
    let (handle, receiver) = LiveSession::spawn(
        fast_config(),
        Some(Box::new(move || {
            counter.fetch_add(1, Ordering::SeqCst);
        })),
    );
    let mut events = Events::new(receiver);
    assert!(handle.connect("127.0.0.1", live.port));
    events.wait_for("Connecting", |event| {
        matches!(event, SessionEvent::State(ConnectionState::Connecting)).then_some(())
    });
    assert_eq!(events.connected(), live.uuid);
    let (uuid, tree) = events.tree();
    assert_eq!(uuid, live.uuid);
    assert_eq!(tree.xml, live.xml);

    // Status events come only when the statuses change. probe.xml changes
    // on each tick.
    let mut snapshots = Vec::new();
    while snapshots.len() < 4 {
        let snapshot = events.wait_for("Status", |event| match event {
            SessionEvent::Status { snapshot } => Some(snapshot.clone()),
            _ => None,
        });
        assert_eq!(snapshot.len(), tree.len());
        for node in &tree.nodes {
            assert!(snapshot.get(node.uid.unwrap()).is_some());
        }
        if let Some(previous) = snapshots.last() {
            assert_ne!(previous, &snapshot, "a Status event without a change");
        }
        snapshots.push(snapshot);
    }

    // Blackboards: read again and again until the watch stops.
    send(
        &handle,
        SessionCommand::WatchBlackboards {
            names: vec!["Probe".to_owned(), "nothing".to_owned()],
            interval: Duration::from_millis(50),
        },
    );
    let mut runs = Vec::new();
    while runs.len() < 2 || runs.first() == runs.last() {
        let value = events.wait_for("Blackboards", |event| match event {
            SessionEvent::Blackboards(value) => Some(value.clone()),
            _ => None,
        });
        let keys: Vec<&String> = value.as_object().unwrap().keys().collect();
        assert_eq!(keys, ["Probe"]);
        runs.push(ProbeCounters::from_json(&value["Probe"]).runs);
    }
    send(
        &handle,
        SessionCommand::WatchBlackboards {
            names: Vec::new(),
            interval: Duration::ZERO,
        },
    );

    // A breakpoint through the session.
    send(&handle, SessionCommand::InsertHook(Hook::breakpoint(probe)));
    events.breakpoint(probe);
    let before = live.assert_blocked();
    send(&handle, SessionCommand::DumpHooks);
    let hooks = events.wait_for("Hooks", |event| match event {
        SessionEvent::Hooks(hooks) => Some(hooks.clone()),
        _ => None,
    });
    assert_eq!(hooks, [Hook::breakpoint(probe)]);

    // Unlock with FAILURE: the run fails without a tick of "probe".
    send(
        &handle,
        SessionCommand::Unlock(UnlockRequest::with_status(probe, NodeStatus::Failure)),
    );
    assert_eq!(live.wait_result(before.runs), "FAILURE");
    events.breakpoint(probe);
    let after = live.probe_counters();
    assert_eq!(after.runs, before.runs + 1);
    assert_eq!(after.probe_ticks, before.probe_ticks);

    // A recording of one run, from one stop at the breakpoint to the next.
    send(&handle, SessionCommand::StartRecording);
    let start_time_us = events.wait_for("RecordingStarted", |event| match event {
        SessionEvent::RecordingStarted { start_time_us } => Some(*start_time_us),
        _ => None,
    });
    assert!(start_time_us > 0);
    send(
        &handle,
        SessionCommand::Unlock(UnlockRequest::resume(probe)),
    );
    let mut recorded: Vec<RecordedTransition> = Vec::new();
    events.wait_for(
        "BreakpointReached after the recorded run",
        |event| match event {
            SessionEvent::Transitions {
                batch,
                possible_loss,
            } => {
                assert!(!possible_loss);
                recorded.extend(batch);
                None
            }
            SessionEvent::BreakpointReached { uid } if *uid == Uid(probe) => Some(()),
            _ => None,
        },
    );
    send(&handle, SessionCommand::StopRecording);
    events.wait_for("RecordingStopped", |event| match event {
        SessionEvent::Transitions { batch, .. } => {
            recorded.extend(batch);
            None
        }
        SessionEvent::RecordingStopped => Some(()),
        _ => None,
    });
    assert_eq!(live.wait_result(after.runs), "SUCCESS");
    // The run ticked "probe" for real: RUNNING is not used by a sync
    // action, so its only changes are SUCCESS and back to IDLE.
    let probe_changes: Vec<NodeStatus> = recorded
        .iter()
        .filter(|transition| transition.uid == Uid(probe))
        .map(|transition| transition.status)
        .collect();
    assert_eq!(
        probe_changes,
        [NodeStatus::Success, NodeStatus::Idle],
        "{recorded:?}"
    );
    for transition in &recorded {
        assert!(tree.node_by_uid(transition.uid).is_some());
    }
    for pair in recorded.windows(2) {
        assert!(pair[0].timestamp_us <= pair[1].timestamp_us);
    }
    let wait = Uid(live.uid("probe_wait"));
    assert!(
        recorded
            .iter()
            .any(|transition| transition.uid == wait && transition.status == NodeStatus::Running)
    );

    // Remove the hook: the waiting node continues.
    let stopped = live.probe_counters();
    send(
        &handle,
        SessionCommand::RemoveHook {
            uid: probe,
            position: HookPosition::Pre,
        },
    );
    assert_eq!(live.wait_result(stopped.runs), "SUCCESS");
    assert!(live.hooks().is_empty());

    send(&handle, SessionCommand::Disconnect);
    events.disconnected();
    assert!(handle.join());
    assert!(wakes.load(Ordering::SeqCst) >= events.seen.len());
    assert!(live.executor.is_running().unwrap());
}

#[test]
fn disconnect_removes_the_hooks_and_the_executor_keeps_running() {
    let harness = require_harness!();
    let mut live = Live::probe(harness);
    let probe = live.uid("probe");
    let count_done = live.uid("count_done");
    let (handle, mut events) = connect(&live, fast_config());

    // A breakpoint that stops the tree, a replace hook and a recording.
    send(
        &handle,
        SessionCommand::InsertHook(Hook::replace(count_done, NodeStatus::Success)),
    );
    send(&handle, SessionCommand::InsertHook(Hook::breakpoint(probe)));
    events.breakpoint(probe);
    let before = live.assert_blocked();
    assert_eq!(live.hooks().len(), 2);
    send(&handle, SessionCommand::StartRecording);
    events.wait_for("RecordingStarted", |event| {
        matches!(event, SessionEvent::RecordingStarted { .. }).then_some(())
    });

    send(&handle, SessionCommand::Disconnect);
    events.wait_for("RecordingStopped", |event| {
        matches!(event, SessionEvent::RecordingStopped).then_some(())
    });
    events.disconnected();
    assert!(
        !events
            .drain()
            .iter()
            .any(|event| matches!(event, SessionEvent::Warning(_))),
        "{:#?}",
        events.seen
    );

    // The executor has no hooks: the tree continues, "probe" and
    // "count_done" tick again.
    assert!(live.hooks().is_empty());
    assert_eq!(live.wait_result(before.runs), "SUCCESS");
    let runs = live.wait_more_runs(3);
    assert!(
        runs.iter().all(|(_, result)| result == "SUCCESS"),
        "{runs:?}"
    );
    let after = live.probe_counters();
    assert!(after.probe_ticks >= before.probe_ticks + 4, "{after:?}");
    assert!(after.done >= before.done + 3, "{after:?}");
    // The recording stopped: no new transitions are recorded.
    wait_until("an empty transition buffer", || {
        live.client.transitions().unwrap().is_empty().then_some(())
    });
    live.wait_more_runs(1);
    assert_eq!(live.client.transitions().unwrap(), []);
    assert!(live.executor.is_running().unwrap());

    // The session can connect again after a disconnect.
    assert!(handle.connect("127.0.0.1", live.port));
    assert_eq!(events.connected(), live.uuid);
    events.tree();
    assert!(handle.join());
}

#[test]
fn disabling_a_hit_breakpoint_through_the_session_releases_the_node() {
    let harness = require_harness!();
    let mut live = Live::probe(harness);
    let probe = live.uid("probe");
    let (handle, mut events) = connect(&live, fast_config());

    send(&handle, SessionCommand::InsertHook(Hook::breakpoint(probe)));
    events.breakpoint(probe);
    let before = live.assert_blocked();

    // The session unlocks the node before it disables the hook (protocol
    // fact 4: a disable through HOOK_INSERT alone does not wake the node).
    let disabled = Hook {
        enabled: false,
        ..Hook::breakpoint(probe)
    };
    send(&handle, SessionCommand::InsertHook(disabled));
    assert_eq!(live.wait_result(before.runs), "SUCCESS");
    let runs = live.wait_more_runs(2);
    assert!(
        runs.iter().all(|(_, result)| result == "SUCCESS"),
        "{runs:?}"
    );
    assert_eq!(live.hooks(), [disabled]);

    // Enable it again, then DISABLE_ALL_HOOKS through the session.
    send(&handle, SessionCommand::InsertHook(Hook::breakpoint(probe)));
    events.breakpoint(probe);
    let before = live.assert_blocked();
    send(&handle, SessionCommand::DisableAllHooks);
    assert_eq!(live.wait_result(before.runs), "SUCCESS");
    assert_eq!(live.hooks(), [disabled]);

    // And REMOVE_ALL_HOOKS.
    send(&handle, SessionCommand::InsertHook(Hook::breakpoint(probe)));
    events.breakpoint(probe);
    let before = live.assert_blocked();
    send(&handle, SessionCommand::RemoveAllHooks);
    assert_eq!(live.wait_result(before.runs), "SUCCESS");
    assert!(live.hooks().is_empty());

    for event in events.drain() {
        assert!(
            !matches!(event, SessionEvent::Error(_) | SessionEvent::Warning(_)),
            "{event:?}"
        );
    }
    assert!(handle.join());
}

#[test]
fn session_heartbeat_keeps_a_breakpoint_active() {
    let harness = require_harness!();
    let heartbeat = Duration::from_millis(300);
    let mut options = options(harness, "probe.xml");
    options.heartbeat_ms = Some(heartbeat.as_millis() as u64);
    let mut live = Live::start(options);
    let probe = live.uid("probe");
    // Few STATUS polls: only the heartbeat of the session keeps the
    // publisher active.
    let mut config = fast_config();
    config.poll.status_interval = Duration::from_secs(60);
    config.poll.heartbeat_interval = Duration::from_millis(100);
    let (handle, receiver) = LiveSession::spawn(config, None);
    let mut events = Events::new(receiver);
    assert!(handle.connect("127.0.0.1", live.port));
    events.tree();
    // No wait here: the session sends its first request only after the
    // notification socket is connected, so the hit is reported.
    send(&handle, SessionCommand::InsertHook(Hook::breakpoint(probe)));
    events.breakpoint(probe);
    let stopped = live.probe_counters();
    // Five heartbeat timeouts without a request from the test: the node
    // stays blocked, so the publisher did not disable the hook.
    let result = live.executor.wait_for_stdout(5 * heartbeat, |line| {
        line.starts_with(&format!("RESULT loop={} ", stopped.runs))
    });
    assert!(result.is_err(), "the node continued: {result:?}");

    send(&handle, SessionCommand::Disconnect);
    events.disconnected();
    assert_eq!(live.wait_result(stopped.runs), "SUCCESS");
    assert!(handle.join());
}

#[test]
fn executor_restart_is_detected_and_the_hooks_come_back() {
    let harness = require_harness!();
    let options_a = options(harness, "probe.xml");
    let port = options_a.port;
    let mut live = Live::start(options_a.clone());
    let probe = live.uid("probe");
    let count_done = live.uid("count_done");
    let mut config = fast_config();
    // Detect the stop of the executor fast.
    config.request_timeout = Duration::from_millis(500);
    let (handle, mut events) = connect(&live, config);

    let disabled = Hook {
        enabled: false,
        ..Hook::breakpoint(count_done)
    };
    send(&handle, SessionCommand::InsertHook(disabled));
    send(&handle, SessionCommand::InsertHook(Hook::breakpoint(probe)));
    events.breakpoint(probe);
    live.assert_blocked();
    let old_uuid = live.uuid;

    // Stop the executor (it releases its own blocked tick) and start a new
    // one on the same port.
    let status = live.executor.stop().unwrap();
    assert!(status.success(), "{status}");
    events.wait_for("Reconnecting", |event| {
        matches!(
            event,
            SessionEvent::State(ConnectionState::Reconnecting { .. })
        )
        .then_some(())
    });
    let executor_b = Executor::spawn(options_a).expect("the second executor starts");
    let mut client_b = client(port);
    let (uuid_b, xml_b) = client_b.full_tree().unwrap();
    assert_ne!(uuid_b, old_uuid);

    let (old, new) = events.wait_for("ExecutorRestarted", |event| match event {
        SessionEvent::ExecutorRestarted { old, new } => Some((*old, *new)),
        _ => None,
    });
    assert_eq!((old, new), (old_uuid, uuid_b));
    let (tree_uuid, tree) = events.tree();
    assert_eq!(tree_uuid, uuid_b);
    assert_eq!(tree.xml, xml_b);
    assert!(events.seen.iter().any(|event| matches!(
        event,
        SessionEvent::State(ConnectionState::Connected { uuid }) if *uuid == uuid_b
    )));

    // The session inserts its hooks again, on the same nodes.
    let mut expected = vec![Hook::breakpoint(probe), disabled];
    expected.sort_by_key(|hook| hook.uid);
    wait_until("the hooks in the new executor", || {
        let mut hooks = client_b.hooks_dump().unwrap();
        hooks.sort_by_key(|hook| hook.uid);
        (hooks == expected).then_some(())
    });
    // The new executor stops at the breakpoint, and the session can
    // release it.
    events.breakpoint(probe);
    let mut live_b = Live {
        executor: executor_b,
        port,
        client: client_b,
        uuid: uuid_b,
        xml: xml_b,
        tree: (*tree).clone(),
    };
    let before = live_b.assert_blocked();
    send(
        &handle,
        SessionCommand::Unlock(UnlockRequest::resume(probe)),
    );
    assert_eq!(live_b.wait_result(before.runs), "SUCCESS");
    events.breakpoint(probe);
    assert!(handle.join());
    // The cleanup on shutdown removed the hooks from the new executor.
    assert!(live_b.hooks().is_empty());
    assert!(live_b.executor.is_running().unwrap());
}
