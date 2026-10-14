//! End-to-end tests of `LiveSession` against the fake executor.

mod support;

use std::sync::mpsc::{Receiver, RecvTimeoutError};
use std::time::{Duration, Instant};

use fasttick_core::runtime::Uid;
use fasttick_core::status::NodeStatus;
use fasttick_protocol::messages::{Hook, HookMode, HookPosition, UnlockRequest};
use fasttick_protocol::session::{
    ConnectionState, LiveSession, PollConfig, SessionCommand, SessionConfig, SessionEvent,
    SessionHandle,
};
use fasttick_protocol::wire::{RequestType, TreeUuid};
use serde_json::json;
use support::fake_publisher::{
    CHANGED_NODE_UIDS, CHANGED_TREE_XML, FakePublisher, Malformed, NODE_UIDS,
};

const WAIT: Duration = Duration::from_secs(5);

/// The event receiver, with a log of the events seen so far.
struct Events {
    receiver: Receiver<SessionEvent>,
    seen: Vec<SessionEvent>,
}

impl Events {
    /// Waits for an event that matches `check`. Other events go to the log.
    fn wait_for(
        &mut self,
        what: &str,
        mut check: impl FnMut(&SessionEvent) -> bool,
    ) -> SessionEvent {
        let deadline = Instant::now() + WAIT;
        loop {
            let remaining = deadline.saturating_duration_since(Instant::now());
            match self.receiver.recv_timeout(remaining) {
                Ok(event) => {
                    self.seen.push(event.clone());
                    if check(&event) {
                        return event;
                    }
                }
                Err(RecvTimeoutError::Timeout) => {
                    panic!("timeout waiting for {what}; events: {:#?}", self.seen)
                }
                Err(RecvTimeoutError::Disconnected) => {
                    panic!(
                        "session stopped while waiting for {what}; events: {:#?}",
                        self.seen
                    )
                }
            }
        }
    }

    /// Reads the events that come within `duration`.
    fn collect_for(&mut self, duration: Duration) -> Vec<SessionEvent> {
        let deadline = Instant::now() + duration;
        let mut out = Vec::new();
        while let Ok(event) = self
            .receiver
            .recv_timeout(deadline.saturating_duration_since(Instant::now()))
        {
            self.seen.push(event.clone());
            out.push(event);
        }
        out
    }

    fn position(&self, check: impl Fn(&SessionEvent) -> bool) -> Option<usize> {
        self.seen.iter().position(check)
    }
}

fn fast_config() -> SessionConfig {
    SessionConfig {
        poll: PollConfig {
            status_interval: Duration::from_millis(20),
            heartbeat_interval: Duration::from_millis(200),
            transitions_interval: Duration::from_millis(30),
        },
        request_timeout: Duration::from_millis(300),
        connect_timeout: Duration::from_millis(300),
        reconnect_initial_delay: Duration::from_millis(20),
        reconnect_max_delay: Duration::from_millis(200),
        reinsert_hooks: true,
        cleanup_timeout: Duration::from_millis(200),
    }
}

/// Starts a session, connects it to `fake`, and waits for the tree.
fn connect(fake: &FakePublisher, config: SessionConfig) -> (SessionHandle, Events, TreeUuid) {
    let (handle, receiver) = LiveSession::spawn(config, None);
    let mut events = Events {
        receiver,
        seen: Vec::new(),
    };
    assert!(handle.connect("127.0.0.1", fake.port));
    events.wait_for("Connecting", |event| {
        matches!(event, SessionEvent::State(ConnectionState::Connecting))
    });
    let uuid = match events.wait_for("Connected", |event| {
        matches!(
            event,
            SessionEvent::State(ConnectionState::Connected { .. })
        )
    }) {
        SessionEvent::State(ConnectionState::Connected { uuid }) => uuid,
        _ => unreachable!(),
    };
    assert_eq!(uuid, fake.uuid());
    events.wait_for("Tree", |event| matches!(event, SessionEvent::Tree { .. }));
    (handle, events, uuid)
}

fn has_hook(fake: &FakePublisher, uid: u16) -> bool {
    fake.wait_until(WAIT, |state| state.hooks.contains_key(&uid))
}

#[test]
fn connect_gets_tree_and_status() {
    let fake = FakePublisher::start();
    let (handle, mut events, uuid) = connect(&fake, fast_config());
    let tree = events
        .seen
        .iter()
        .find_map(|event| match event {
            SessionEvent::Tree { uuid, tree } => Some((*uuid, tree.clone())),
            _ => None,
        })
        .unwrap();
    assert_eq!(tree.0, uuid);
    assert_eq!(tree.1.len(), NODE_UIDS.len());
    // The first status, then a change.
    events.wait_for("first Status", |event| {
        matches!(event, SessionEvent::Status { .. })
    });
    fake.set_status(2, NodeStatus::Running.as_u8());
    events.wait_for("Status with RUNNING", |event| match event {
        SessionEvent::Status { snapshot } => snapshot
            .get(Uid(2))
            .is_some_and(|entry| entry.current == NodeStatus::Running),
        _ => false,
    });
    // An unchanged status gives no new event.
    let later = events.collect_for(Duration::from_millis(150));
    assert!(
        !later
            .iter()
            .any(|event| matches!(event, SessionEvent::Status { .. })),
        "{later:#?}"
    );
    assert!(handle.join());
    fake.assert_not_crashed();
}

#[test]
fn poll_cadence_and_heartbeat() {
    let fake = FakePublisher::start();
    let config = SessionConfig {
        poll: PollConfig {
            status_interval: Duration::from_millis(50),
            heartbeat_interval: Duration::from_millis(150),
            transitions_interval: Duration::from_millis(100),
        },
        ..fast_config()
    };
    let (handle, mut events, _) = connect(&fake, config);
    let before = fake.count(RequestType::Status);
    events.collect_for(Duration::from_millis(600));
    let polls = fake.count(RequestType::Status) - before;
    // About 12 polls in 600 ms at 50 ms.
    assert!((6..=20).contains(&polls), "{polls} polls");

    // A long poll interval: the heartbeat still sends a request each 150 ms.
    handle.send(SessionCommand::SetPollInterval(Duration::from_secs(10)));
    events.collect_for(Duration::from_millis(100));
    let start = Instant::now();
    events.collect_for(Duration::from_millis(900));
    let times: Vec<Instant> = fake
        .state()
        .requests
        .iter()
        .map(|request| request.at)
        .filter(|at| *at >= start)
        .collect();
    assert!(times.len() >= 4, "{} requests", times.len());
    assert!(times.len() <= 12, "{} requests", times.len());
    let largest_gap = times
        .windows(2)
        .map(|pair| pair[1] - pair[0])
        .max()
        .unwrap_or_default();
    assert!(largest_gap < Duration::from_millis(400), "{largest_gap:?}");
    assert!(handle.join());
}

#[test]
fn reconnects_after_server_restart() {
    let mut fake = FakePublisher::start();
    let (handle, mut events, uuid) = connect(&fake, fast_config());
    fake.stop();
    events.wait_for("Reconnecting", |event| {
        matches!(
            event,
            SessionEvent::State(ConnectionState::Reconnecting { attempt: 1 })
        )
    });
    events.wait_for("second attempt", |event| {
        matches!(
            event,
            SessionEvent::State(ConnectionState::Reconnecting { attempt: 2 })
        )
    });
    fake.start_again();
    let event = events.wait_for("Connected again", |event| {
        matches!(
            event,
            SessionEvent::State(ConnectionState::Connected { .. })
        )
    });
    assert_eq!(
        event,
        SessionEvent::State(ConnectionState::Connected { uuid })
    );
    // Same executor: no restart event.
    assert!(
        events
            .position(|event| matches!(event, SessionEvent::ExecutorRestarted { .. }))
            .is_none()
    );
    events.wait_for("Status after reconnect", |event| {
        matches!(event, SessionEvent::Status { .. })
    });
    assert!(handle.join());
}

#[test]
fn executor_restart_reinserts_hooks() {
    let fake = FakePublisher::start();
    let (handle, mut events, old_uuid) = connect(&fake, fast_config());
    handle.send(SessionCommand::InsertHook(Hook::breakpoint(2)));
    handle.send(SessionCommand::InsertHook(Hook::replace(
        4,
        NodeStatus::Failure,
    )));
    assert!(has_hook(&fake, 2) && has_hook(&fake, 4));

    let new_uuid = fake.restart_executor();
    assert!(fake.state().hooks.is_empty());
    let event = events.wait_for("ExecutorRestarted", |event| {
        matches!(event, SessionEvent::ExecutorRestarted { .. })
    });
    assert_eq!(
        event,
        SessionEvent::ExecutorRestarted {
            old: old_uuid,
            new: new_uuid
        }
    );
    let restart_at = events.seen.len() - 1;
    let tree = events.wait_for("new Tree", |event| {
        matches!(event, SessionEvent::Tree { .. })
    });
    assert!(matches!(tree, SessionEvent::Tree { uuid, .. } if uuid == new_uuid));
    // No status of the new executor before its tree.
    assert!(
        !events.seen[restart_at..events.seen.len() - 1]
            .iter()
            .any(|event| matches!(event, SessionEvent::Status { .. }))
    );
    assert!(has_hook(&fake, 2) && has_hook(&fake, 4));
    let state = fake.state();
    assert_eq!(state.hooks[&2].hook, Hook::breakpoint(2));
    assert_eq!(state.hooks[&4].hook, Hook::replace(4, NodeStatus::Failure));
    drop(state);
    assert!(
        events
            .seen
            .contains(&SessionEvent::State(ConnectionState::Connected {
                uuid: new_uuid
            }))
    );
    assert!(handle.join());
    fake.assert_not_crashed();
}

#[test]
fn restart_with_changed_tree_moves_hooks_by_path() {
    let fake = FakePublisher::start();
    let (handle, mut events, _) = connect(&fake, fast_config());
    // "first" is UID 2 and "sub/fail" is UID 6 in the first tree.
    handle.send(SessionCommand::InsertHook(Hook::breakpoint(2)));
    handle.send(SessionCommand::InsertHook(Hook::replace(
        6,
        NodeStatus::Success,
    )));
    assert!(has_hook(&fake, 2) && has_hook(&fake, 6));

    // In the new tree, "first" is UID 3 and "sub/fail" is UID 7.
    let new_uuid = fake.restart_executor_with(CHANGED_TREE_XML, &CHANGED_NODE_UIDS);
    events.wait_for(
        "new Tree",
        |event| matches!(event, SessionEvent::Tree { uuid, .. } if *uuid == new_uuid),
    );
    assert!(fake.wait_until(WAIT, |state| state.hooks.len() == 2));
    events.collect_for(Duration::from_millis(100));
    let state = fake.state();
    assert_eq!(
        state.hooks.keys().copied().collect::<Vec<u16>>(),
        vec![3, 7]
    );
    assert_eq!(state.hooks[&3].hook, Hook::breakpoint(3));
    assert_eq!(state.hooks[&7].hook, Hook::replace(7, NodeStatus::Success));
    drop(state);

    // The session now knows the new UIDs: removing UID 3 works.
    handle.send(SessionCommand::RemoveHook {
        uid: 3,
        position: HookPosition::Pre,
    });
    assert!(fake.wait_until(WAIT, |state| state.hooks.len() == 1));
    // A node that is gone: its hook is dropped with a warning.
    fake.state().tree_xml = TREE_WITHOUT_FAIL.to_owned();
    fake.restart_executor();
    events.wait_for(
        "Warning for the dropped hook",
        |event| matches!(event, SessionEvent::Warning(text) if text.contains("not inserted again")),
    );
    events.collect_for(Duration::from_millis(150));
    assert!(fake.state().hooks.is_empty());
    assert!(handle.join());
    fake.assert_not_crashed();
}

/// A tree without the node "sub/fail" and without UID 7.
const TREE_WITHOUT_FAIL: &str = r#"<root BTCPP_format="4">
    <BehaviorTree ID="MainTree" _fullpath="">
        <Sequence name="main" _uid="1">
            <AlwaysSuccess name="first" _uid="2"/>
        </Sequence>
    </BehaviorTree>
</root>"#;

#[test]
fn no_reinsert_when_disabled() {
    let fake = FakePublisher::start();
    let config = SessionConfig {
        reinsert_hooks: false,
        ..fast_config()
    };
    let (handle, mut events, _) = connect(&fake, config);
    handle.send(SessionCommand::InsertHook(Hook::breakpoint(2)));
    assert!(has_hook(&fake, 2));
    fake.restart_executor();
    events.wait_for("ExecutorRestarted", |event| {
        matches!(event, SessionEvent::ExecutorRestarted { .. })
    });
    events.wait_for("new Tree", |event| {
        matches!(event, SessionEvent::Tree { .. })
    });
    events.collect_for(Duration::from_millis(150));
    assert!(fake.state().hooks.is_empty());
    assert!(handle.join());
}

#[test]
fn disconnect_removes_hooks_and_stops_recording() {
    let fake = FakePublisher::start();
    let (handle, mut events, _) = connect(&fake, fast_config());
    handle.send(SessionCommand::InsertHook(Hook::breakpoint(5)));
    handle.send(SessionCommand::StartRecording);
    events.wait_for("RecordingStarted", |event| {
        matches!(event, SessionEvent::RecordingStarted { .. })
    });
    assert!(has_hook(&fake, 5));
    handle.disconnect();
    events.wait_for("RecordingStopped", |event| {
        matches!(event, SessionEvent::RecordingStopped)
    });
    events.wait_for("Disconnected", |event| {
        matches!(event, SessionEvent::State(ConnectionState::Disconnected))
    });
    let state = fake.state();
    assert!(state.hooks.is_empty());
    assert!(!state.recording);
    let kinds: Vec<u8> = state
        .requests
        .iter()
        .map(|request| request.type_byte)
        .collect();
    let insert = kinds.iter().position(|kind| *kind == b'I').unwrap();
    let remove_all = kinds.iter().rposition(|kind| *kind == b'A').unwrap();
    assert!(remove_all > insert);
    let stop = state
        .requests
        .iter()
        .rposition(|request| request.type_byte == b'r' && request.payload_text() == "stop")
        .unwrap();
    assert!(stop < remove_all);
    drop(state);

    // Requests stop after the disconnect.
    let count = fake.state().requests.len();
    events.collect_for(Duration::from_millis(300));
    assert_eq!(fake.state().requests.len(), count);
    // Commands now give errors.
    handle.send(SessionCommand::DumpHooks);
    events.wait_for("Error", |event| matches!(event, SessionEvent::Error(_)));

    // Connect again.
    handle.connect("127.0.0.1", fake.port);
    events.wait_for("Connected again", |event| {
        matches!(
            event,
            SessionEvent::State(ConnectionState::Connected { .. })
        )
    });
    assert!(handle.join());
    fake.assert_not_crashed();
}

#[test]
fn shutdown_removes_hooks() {
    let fake = FakePublisher::start();
    let (handle, _events, _) = connect(&fake, fast_config());
    handle.send(SessionCommand::InsertHook(Hook::breakpoint(1)));
    assert!(has_hook(&fake, 1));
    let started = Instant::now();
    assert!(handle.join());
    assert!(started.elapsed() < Duration::from_secs(2));
    assert!(fake.state().hooks.is_empty());
    assert!(fake.count(RequestType::RemoveAllHooks) >= 1);
}

#[test]
fn dropping_the_handle_cleans_up() {
    let fake = FakePublisher::start();
    let (handle, mut events, _) = connect(&fake, fast_config());
    handle.send(SessionCommand::InsertHook(Hook::breakpoint(3)));
    assert!(has_hook(&fake, 3));
    drop(handle);
    events.wait_for("Disconnected", |event| {
        matches!(event, SessionEvent::State(ConnectionState::Disconnected))
    });
    assert!(fake.state().hooks.is_empty());
}

#[test]
fn recording_flow() {
    let fake = FakePublisher::start();
    let (handle, mut events, _) = connect(&fake, fast_config());
    handle.send(SessionCommand::StartRecording);
    let event = events.wait_for("RecordingStarted", |event| {
        matches!(event, SessionEvent::RecordingStarted { .. })
    });
    let SessionEvent::RecordingStarted { start_time_us } = event else {
        unreachable!()
    };
    assert!(start_time_us > 1_600_000_000_000_000);

    // More than the buffer holds between two reads.
    fake.push_transitions(1500);
    let event = events.wait_for("full batch", |event| {
        matches!(event, SessionEvent::Transitions { .. })
    });
    let SessionEvent::Transitions {
        batch,
        possible_loss,
    } = event
    else {
        unreachable!()
    };
    assert_eq!(batch.len(), 1000);
    assert!(possible_loss);

    fake.push_transitions(5);
    let event = events.wait_for("small batch", |event| {
        matches!(event, SessionEvent::Transitions { .. })
    });
    assert!(matches!(
        event,
        SessionEvent::Transitions { ref batch, possible_loss: false } if batch.len() == 5
    ));

    // Stop: the rest of the buffer comes before RecordingStopped.
    fake.push_transitions(3);
    handle.send(SessionCommand::StopRecording);
    events.wait_for("RecordingStopped", |event| {
        matches!(event, SessionEvent::RecordingStopped)
    });
    let total: usize = events
        .seen
        .iter()
        .filter_map(|event| match event {
            SessionEvent::Transitions { batch, .. } => Some(batch.len()),
            _ => None,
        })
        .sum();
    assert_eq!(total, 1008);
    assert!(!fake.state().recording);
    // No more GET_TRANSITIONS after the stop.
    events.collect_for(Duration::from_millis(50));
    let count = fake.count(RequestType::GetTransitions);
    events.collect_for(Duration::from_millis(200));
    assert_eq!(fake.count(RequestType::GetTransitions), count);
    assert!(handle.join());
}

#[test]
fn restart_stops_recording() {
    let fake = FakePublisher::start();
    let (handle, mut events, _) = connect(&fake, fast_config());
    handle.send(SessionCommand::StartRecording);
    events.wait_for("RecordingStarted", |event| {
        matches!(event, SessionEvent::RecordingStarted { .. })
    });
    fake.restart_executor();
    events.wait_for("ExecutorRestarted", |event| {
        matches!(event, SessionEvent::ExecutorRestarted { .. })
    });
    events.wait_for("RecordingStopped", |event| {
        matches!(event, SessionEvent::RecordingStopped)
    });
    assert!(handle.join());
}

#[test]
fn malformed_replies_give_warnings() {
    let fake = FakePublisher::start();
    let (handle, mut events, uuid) = connect(&fake, fast_config());
    events.wait_for("first Status", |event| {
        matches!(event, SessionEvent::Status { .. })
    });
    for malformed in [
        Malformed::WrongHeaderSize,
        Malformed::WrongId,
        Malformed::WrongProtocol,
        Malformed::WrongType,
        Malformed::Garbage,
        Malformed::BadPayload,
        Malformed::ExtraPart,
    ] {
        fake.push_malformed(malformed.clone());
        events.wait_for("Warning", |event| matches!(event, SessionEvent::Warning(_)));
    }
    fake.push_malformed(Malformed::ErrorReply("bad".to_owned()));
    events.wait_for(
        "Error",
        |event| matches!(event, SessionEvent::Error(text) if text.contains("bad")),
    );

    // The session still works and stays connected.
    fake.set_status(6, NodeStatus::Failure.as_u8());
    events.wait_for("Status after the bad replies", |event| match event {
        SessionEvent::Status { snapshot } => snapshot
            .get(Uid(6))
            .is_some_and(|entry| entry.current == NodeStatus::Failure),
        _ => false,
    });
    assert!(!events.seen.iter().any(|event| matches!(
        event,
        SessionEvent::State(ConnectionState::Reconnecting { .. })
    )));
    assert!(
        !events
            .seen
            .iter()
            .any(|event| matches!(event, SessionEvent::ExecutorRestarted { .. }))
    );
    assert_eq!(fake.uuid(), uuid);

    // Malformed notifications give warnings too.
    let deadline = Instant::now() + WAIT;
    loop {
        fake.publish_raw(vec![vec![2, b'N', 0, 0, 0, 0], b"not a number".to_vec()]);
        let found = events.collect_for(Duration::from_millis(50)).iter().any(
            |event| matches!(event, SessionEvent::Warning(text) if text.contains("notification")),
        );
        if found {
            break;
        }
        assert!(Instant::now() < deadline, "no warning for the notification");
    }
    assert!(handle.join());
    fake.assert_not_crashed();
}

#[test]
fn timeout_then_recovery() {
    let fake = FakePublisher::start();
    let (handle, mut events, uuid) = connect(&fake, fast_config());
    fake.set_silent(true);
    events.wait_for("Reconnecting", |event| {
        matches!(
            event,
            SessionEvent::State(ConnectionState::Reconnecting { .. })
        )
    });
    // The warning comes just before the state change.
    assert!(events.seen.iter().any(
        |event| matches!(event, SessionEvent::Warning(text) if text.contains("connection lost"))
    ));
    fake.set_silent(false);
    let event = events.wait_for("Connected again", |event| {
        matches!(
            event,
            SessionEvent::State(ConnectionState::Connected { .. })
        )
    });
    assert_eq!(
        event,
        SessionEvent::State(ConnectionState::Connected { uuid })
    );
    assert!(handle.join());
}

/// Hits the hook at `uid` until the session reports it (ZeroMQ drops
/// notifications until the subscription is active).
fn hit_until_reported(fake: &FakePublisher, events: &mut Events, uid: u16) {
    let deadline = Instant::now() + WAIT;
    loop {
        assert!(fake.hit(uid).is_some(), "no enabled hook at {uid}");
        let reported = events
            .collect_for(Duration::from_millis(50))
            .iter()
            .any(|event| *event == SessionEvent::BreakpointReached { uid: Uid(uid) });
        if reported {
            return;
        }
        assert!(Instant::now() < deadline, "BreakpointReached not reported");
    }
}

#[test]
fn disabling_a_hit_breakpoint_unlocks_first() {
    let fake = FakePublisher::start();
    let (handle, mut events, _) = connect(&fake, fast_config());
    handle.send(SessionCommand::InsertHook(Hook::breakpoint(4)));
    assert!(has_hook(&fake, 4));
    hit_until_reported(&fake, &mut events, 4);
    assert!(fake.state().hooks[&4].blocked);

    handle.send(SessionCommand::InsertHook(Hook {
        enabled: false,
        ..Hook::breakpoint(4)
    }));
    assert!(fake.wait_until(WAIT, |state| !state.hooks[&4].hook.enabled));
    let state = fake.state();
    assert!(!state.hooks[&4].blocked);
    let unlock = state
        .requests
        .iter()
        .rposition(|request| request.type_byte == b'U')
        .unwrap();
    let insert = state
        .requests
        .iter()
        .rposition(|request| request.type_byte == b'I')
        .unwrap();
    assert!(unlock < insert);
    assert_eq!(
        state.requests[unlock].payload_text(),
        r#"{"desired_status":"SKIPPED","position":0,"remove_when_done":false,"uid":4}"#
    );
    drop(state);
    assert!(handle.join());
    fake.assert_not_crashed();
}

#[test]
fn unlock_and_remove_commands() {
    let fake = FakePublisher::start();
    let (handle, mut events, _) = connect(&fake, fast_config());
    handle.send(SessionCommand::InsertHook(Hook::breakpoint(6)));
    assert!(has_hook(&fake, 6));
    hit_until_reported(&fake, &mut events, 6);
    handle.send(SessionCommand::Unlock(UnlockRequest::with_status(
        6,
        NodeStatus::Success,
    )));
    assert!(fake.wait_until(WAIT, |state| !state.hooks[&6].blocked));
    assert_eq!(
        fake.state().hooks[&6].hook.desired_status,
        NodeStatus::Success
    );

    // Disabling a hook that is not hit sends no unlock.
    let unlocks = fake.count(RequestType::BreakpointUnlock);
    handle.send(SessionCommand::InsertHook(Hook {
        enabled: false,
        ..Hook::breakpoint(6)
    }));
    assert!(fake.wait_until(WAIT, |state| !state.hooks[&6].hook.enabled));
    assert_eq!(fake.count(RequestType::BreakpointUnlock), unlocks);

    handle.send(SessionCommand::RemoveHook {
        uid: 6,
        position: HookPosition::Pre,
    });
    assert!(fake.wait_until(WAIT, |state| state.hooks.is_empty()));
    // Removing it again: the executor replies with an error.
    handle.send(SessionCommand::RemoveHook {
        uid: 6,
        position: HookPosition::Pre,
    });
    events.wait_for(
        "Error for an unknown hook",
        |event| matches!(event, SessionEvent::Error(text) if text.contains("Node ID not found")),
    );
    assert!(handle.join());
    fake.assert_not_crashed();
}

#[test]
fn post_hooks_give_a_warning() {
    let fake = FakePublisher::start();
    let (handle, mut events, _) = connect(&fake, fast_config());
    handle.send(SessionCommand::InsertHook(Hook {
        position: HookPosition::Post,
        ..Hook::breakpoint(2)
    }));
    events.wait_for(
        "POST warning",
        |event| matches!(event, SessionEvent::Warning(text) if text.contains("POST")),
    );
    assert!(has_hook(&fake, 2));
    assert!(handle.join());
}

#[test]
fn replace_hook_once_is_forgotten_after_it_runs() {
    let fake = FakePublisher::start();
    let (handle, mut events, _) = connect(&fake, fast_config());
    let once = Hook {
        once: true,
        ..Hook::replace(2, NodeStatus::Success)
    };
    handle.send(SessionCommand::InsertHook(once));
    assert!(has_hook(&fake, 2));
    // The hook runs once; the fake removes it, so stop the loop at once.
    let deadline = Instant::now() + WAIT;
    let mut reported = false;
    while !reported {
        if fake.hit(2).is_none() {
            // The hook ran and removed itself; insert it again to retry
            // until the subscription is active.
            handle.send(SessionCommand::InsertHook(once));
            assert!(has_hook(&fake, 2));
            continue;
        }
        reported = events
            .collect_for(Duration::from_millis(50))
            .contains(&SessionEvent::BreakpointReached { uid: Uid(2) });
        assert!(Instant::now() < deadline, "not reported");
    }
    assert_eq!(fake.state().hooks.get(&2).map(|hook| hook.hook.mode), None);
    // After a restart, the session does not insert the used hook again.
    fake.restart_executor();
    events.wait_for("new Tree", |event| {
        matches!(event, SessionEvent::Tree { .. })
    });
    events.collect_for(Duration::from_millis(150));
    assert!(fake.state().hooks.is_empty());
    assert_eq!(HookMode::Replace, once.mode);
    assert!(handle.join());
}

#[test]
fn blackboards_dump_and_tree_refresh() {
    let fake = FakePublisher::start();
    let (handle, mut events, _) = connect(&fake, fast_config());
    handle.send(SessionCommand::WatchBlackboards {
        names: vec!["MainTree".to_owned(), "sub".to_owned()],
        interval: Duration::from_millis(40),
    });
    let event = events.wait_for("Blackboards", |event| {
        matches!(event, SessionEvent::Blackboards(_))
    });
    let SessionEvent::Blackboards(value) = event else {
        unreachable!()
    };
    assert_eq!(value["MainTree"]["name"], json!("fast"));
    assert_eq!(value["sub"]["flag"], json!(true));
    // It repeats.
    events.wait_for("second Blackboards", |event| {
        matches!(event, SessionEvent::Blackboards(_))
    });
    // An empty list stops the watch.
    handle.send(SessionCommand::WatchBlackboards {
        names: Vec::new(),
        interval: Duration::from_millis(40),
    });
    events.collect_for(Duration::from_millis(60));
    let count = fake.count(RequestType::Blackboard);
    events.collect_for(Duration::from_millis(200));
    assert_eq!(fake.count(RequestType::Blackboard), count);
    // A zero interval reads once.
    handle.send(SessionCommand::WatchBlackboards {
        names: vec!["sub".to_owned()],
        interval: Duration::ZERO,
    });
    events.wait_for("one Blackboards", |event| {
        matches!(event, SessionEvent::Blackboards(_))
    });
    events.collect_for(Duration::from_millis(150));
    assert_eq!(fake.count(RequestType::Blackboard), count + 1);

    handle.send(SessionCommand::InsertHook(Hook::breakpoint(5)));
    handle.send(SessionCommand::DumpHooks);
    let event = events.wait_for("Hooks", |event| matches!(event, SessionEvent::Hooks(_)));
    assert_eq!(event, SessionEvent::Hooks(vec![Hook::breakpoint(5)]));

    handle.send(SessionCommand::DisableAllHooks);
    assert!(fake.wait_until(WAIT, |state| !state.hooks[&5].hook.enabled));
    handle.send(SessionCommand::RemoveAllHooks);
    assert!(fake.wait_until(WAIT, |state| state.hooks.is_empty()));

    let trees = fake.count(RequestType::FullTree);
    handle.send(SessionCommand::RefreshTree);
    events.wait_for("Tree", |event| matches!(event, SessionEvent::Tree { .. }));
    assert_eq!(fake.count(RequestType::FullTree), trees + 1);
    assert!(handle.join());
    fake.assert_not_crashed();
}

#[test]
fn invalid_tree_xml_gives_an_error_and_the_session_continues() {
    let fake = FakePublisher::start();
    fake.state().tree_xml = "<root><BehaviorTree".to_owned();
    let (handle, receiver) = LiveSession::spawn(fast_config(), None);
    let mut events = Events {
        receiver,
        seen: Vec::new(),
    };
    handle.connect("127.0.0.1", fake.port);
    events.wait_for("Connected", |event| {
        matches!(
            event,
            SessionEvent::State(ConnectionState::Connected { .. })
        )
    });
    events.wait_for(
        "Error for the XML",
        |event| matches!(event, SessionEvent::Error(text) if text.contains("tree XML")),
    );
    events.wait_for("Status", |event| {
        matches!(event, SessionEvent::Status { .. })
    });
    assert!(handle.join());
}

#[test]
fn connect_to_absent_executor_keeps_trying() {
    // Find a free port pair, then close it.
    let port = {
        let fake = FakePublisher::start();
        fake.port
    };
    let (handle, receiver) = LiveSession::spawn(fast_config(), None);
    let mut events = Events {
        receiver,
        seen: Vec::new(),
    };
    handle.connect("127.0.0.1", port);
    events.wait_for("attempt 3", |event| {
        matches!(
            event,
            SessionEvent::State(ConnectionState::Reconnecting { attempt: 3 })
        )
    });
    // Disconnect while reconnecting is quick.
    let started = Instant::now();
    handle.disconnect();
    events.wait_for("Disconnected", |event| {
        matches!(event, SessionEvent::State(ConnectionState::Disconnected))
    });
    assert!(started.elapsed() < Duration::from_secs(2));
    assert!(handle.join());
}

#[test]
fn hit_survives_a_short_connection_loss() {
    let fake = FakePublisher::start();
    let (handle, mut events, uuid) = connect(&fake, fast_config());
    handle.send(SessionCommand::InsertHook(Hook::breakpoint(4)));
    assert!(has_hook(&fake, 4));
    hit_until_reported(&fake, &mut events, 4);
    assert!(fake.state().hooks[&4].blocked);

    // A short loss of the connection: the node still waits at the
    // breakpoint (the publisher wakes it only after its heartbeat timeout).
    fake.set_silent(true);
    events.wait_for("Reconnecting", |event| {
        matches!(
            event,
            SessionEvent::State(ConnectionState::Reconnecting { .. })
        )
    });
    fake.set_silent(false);
    events.wait_for("Connected again", |event| {
        *event == SessionEvent::State(ConnectionState::Connected { uuid })
    });
    assert!(fake.state().hooks[&4].blocked);
    let unlocks = fake.count(RequestType::BreakpointUnlock);
    assert_eq!(unlocks, 0);

    // Disabling the hook must release the node first.
    handle.send(SessionCommand::InsertHook(Hook {
        enabled: false,
        ..Hook::breakpoint(4)
    }));
    assert!(fake.wait_until(WAIT, |state| !state.hooks[&4].hook.enabled));
    let state = fake.state();
    assert!(!state.hooks[&4].blocked, "the node is still blocked");
    let unlock = state
        .requests
        .iter()
        .rposition(|request| request.type_byte == b'U')
        .expect("no BREAKPOINT_UNLOCK was sent");
    let insert = state
        .requests
        .iter()
        .rposition(|request| request.type_byte == b'I')
        .unwrap();
    assert!(unlock < insert);
    drop(state);
    assert!(handle.join());
    fake.assert_not_crashed();
}

#[test]
fn restart_with_renamed_node_drops_its_hook() {
    let fake = FakePublisher::start();
    let (handle, mut events, _) = connect(&fake, fast_config());
    // UID 2 is "first", UID 6 is "sub/fail".
    handle.send(SessionCommand::InsertHook(Hook::replace(
        2,
        NodeStatus::Failure,
    )));
    handle.send(SessionCommand::InsertHook(Hook::breakpoint(6)));
    assert!(has_hook(&fake, 2) && has_hook(&fake, 6));

    // The new executor has the same UIDs, but the node "first" is renamed.
    // UID 2 is now another node: the replace hook must not go there.
    let renamed = support::fake_publisher::TREE_XML.replace(r#"name="first""#, r#"name="other""#);
    let new_uuid = fake.restart_executor_with(&renamed, &NODE_UIDS);
    // The warning comes before the new tree.
    events.wait_for(
        "Warning for the dropped hook",
        |event| matches!(event, SessionEvent::Warning(text) if text.contains("\"first\"")),
    );
    events.wait_for(
        "new Tree",
        |event| matches!(event, SessionEvent::Tree { uuid, .. } if *uuid == new_uuid),
    );
    assert!(has_hook(&fake, 6));
    events.collect_for(Duration::from_millis(150));
    let state = fake.state();
    assert_eq!(state.hooks.keys().copied().collect::<Vec<u16>>(), vec![6]);
    assert_eq!(state.hooks[&6].hook, Hook::breakpoint(6));
    drop(state);
    assert!(handle.join());
    fake.assert_not_crashed();
}

#[test]
fn unreadable_tree_after_restart_drops_the_hooks() {
    let fake = FakePublisher::start();
    let (handle, mut events, _) = connect(&fake, fast_config());
    handle.send(SessionCommand::InsertHook(Hook::breakpoint(2)));
    assert!(has_hook(&fake, 2));

    fake.restart_executor_with("<root><BehaviorTree", &NODE_UIDS);
    events.wait_for(
        "Error for the XML",
        |event| matches!(event, SessionEvent::Error(text) if text.contains("tree XML")),
    );
    events.wait_for(
        "Warning for the hooks",
        |event| matches!(event, SessionEvent::Warning(text) if text.contains("not inserted again")),
    );
    // The session keeps polling, but it inserts no hook on an unknown tree.
    fake.set_status(3, NodeStatus::Running.as_u8());
    events.wait_for("Status", |event| match event {
        SessionEvent::Status { snapshot } => snapshot
            .get(Uid(3))
            .is_some_and(|entry| entry.current == NodeStatus::Running),
        _ => false,
    });
    events.collect_for(Duration::from_millis(100));
    assert!(fake.state().hooks.is_empty());
    assert_eq!(fake.count(RequestType::HookInsert), 1);
    assert!(handle.join());
    fake.assert_not_crashed();
}

#[test]
fn disconnect_reads_the_last_transitions() {
    let fake = FakePublisher::start();
    let config = SessionConfig {
        poll: PollConfig {
            // No periodic read during this test.
            transitions_interval: Duration::from_secs(60),
            ..fast_config().poll
        },
        ..fast_config()
    };
    let (handle, mut events, _) = connect(&fake, config);
    handle.send(SessionCommand::StartRecording);
    events.wait_for("RecordingStarted", |event| {
        matches!(event, SessionEvent::RecordingStarted { .. })
    });
    fake.push_transitions(3);
    handle.disconnect();
    let event = events.wait_for("Transitions", |event| {
        matches!(
            event,
            SessionEvent::Transitions { .. } | SessionEvent::RecordingStopped
        )
    });
    assert!(
        matches!(
            event,
            SessionEvent::Transitions { ref batch, possible_loss: false } if batch.len() == 3
        ),
        "{event:?}"
    );
    events.wait_for("RecordingStopped", |event| {
        matches!(event, SessionEvent::RecordingStopped)
    });
    events.wait_for("Disconnected", |event| {
        matches!(event, SessionEvent::State(ConnectionState::Disconnected))
    });
    let state = fake.state();
    assert!(!state.recording);
    assert!(state.transitions.is_empty());
    drop(state);
    assert!(handle.join());
}

#[test]
fn unlock_with_remove_when_done_forgets_the_hook() {
    let fake = FakePublisher::start();
    let (handle, mut events, _) = connect(&fake, fast_config());
    handle.send(SessionCommand::InsertHook(Hook::breakpoint(5)));
    handle.send(SessionCommand::InsertHook(Hook::breakpoint(6)));
    assert!(has_hook(&fake, 5) && has_hook(&fake, 6));
    hit_until_reported(&fake, &mut events, 5);
    handle.send(SessionCommand::Unlock(UnlockRequest {
        remove_when_done: true,
        ..UnlockRequest::resume(5)
    }));
    assert!(fake.wait_until(WAIT, |state| !state.hooks.contains_key(&5)));

    // After a restart, only the hook that is still active comes back.
    fake.restart_executor();
    events.wait_for("new Tree", |event| {
        matches!(event, SessionEvent::Tree { .. })
    });
    assert!(has_hook(&fake, 6));
    events.collect_for(Duration::from_millis(150));
    assert_eq!(
        fake.state().hooks.keys().copied().collect::<Vec<u16>>(),
        vec![6]
    );
    assert!(handle.join());
    fake.assert_not_crashed();
}

#[test]
fn hooks_go_back_only_after_the_notification_socket_connects() {
    // Regression test. After a restart of the executor, ZeroMQ can connect
    // the REQ socket some time before the SUB socket. A hook that goes back
    // at once can stop a node at once, and the publisher sends the
    // notification only once: without a subscriber, the PUB socket drops
    // it, and the session never learns about the stopped node.
    let mut fake = FakePublisher::start();
    let (handle, mut events, old_uuid) = connect(&fake, fast_config());
    handle.send(SessionCommand::InsertHook(Hook::breakpoint(2)));
    assert!(has_hook(&fake, 2));

    fake.stop();
    events.wait_for("Reconnecting", |event| {
        matches!(
            event,
            SessionEvent::State(ConnectionState::Reconnecting { .. })
        )
    });
    // The new executor binds its server port at once and its PUB port
    // later (less than the connect timeout of the session).
    let new_uuid = fake.restart_executor();
    fake.start_again_with_publisher_delay(Duration::from_millis(150));
    let event = events.wait_for("ExecutorRestarted", |event| {
        matches!(event, SessionEvent::ExecutorRestarted { .. })
    });
    assert_eq!(
        event,
        SessionEvent::ExecutorRestarted {
            old: old_uuid,
            new: new_uuid
        }
    );
    assert!(has_hook(&fake, 2));
    let bound_at = fake
        .state()
        .publisher_bound_at
        .expect("the PUB socket is bound");
    let reinserted = fake.requests_of(RequestType::HookInsert);
    let reinserted_at = reinserted.last().unwrap().at;
    assert!(
        reinserted_at > bound_at,
        "the hooks went back {:?} before the PUB socket was bound",
        bound_at - reinserted_at
    );

    // One single hit is reported.
    assert_eq!(fake.hit(2), Some(HookMode::Breakpoint));
    events.wait_for("BreakpointReached", |event| {
        *event == SessionEvent::BreakpointReached { uid: Uid(2) }
    });
    assert!(handle.join());
    fake.assert_not_crashed();
}
