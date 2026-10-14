//! End-to-end tests of `Client` against the fake executor.

mod support;

use std::time::{Duration, Instant};

use fasttick_core::runtime::{RuntimeTree, Uid};
use fasttick_core::status::NodeStatus;
use fasttick_protocol::client::{Client, ClientConfig, ClientError};
use fasttick_protocol::messages::{Hook, HookMode, HookPosition, UnlockRequest};
use fasttick_protocol::wire::{ProtocolError, RequestType};
use fasttick_protocol::{DecodeError, EncodeError};
use serde_json::json;
use support::fake_publisher::{FakePublisher, Malformed, NODE_UIDS, TREE_XML};

fn client_for(fake: &FakePublisher, timeout_ms: u64) -> Client {
    Client::connect(ClientConfig {
        request_timeout: Duration::from_millis(timeout_ms),
        ..ClientConfig::new("127.0.0.1", fake.port)
    })
    .unwrap()
}

#[test]
fn full_tree_and_status() {
    let fake = FakePublisher::start();
    let mut client = client_for(&fake, 2000);
    let (uuid, xml) = client.full_tree().unwrap();
    assert_eq!(uuid, fake.uuid());
    assert_eq!(xml, TREE_XML);
    assert_eq!(client.last_tree_uuid(), Some(uuid));
    let tree = RuntimeTree::parse(&xml).unwrap();
    assert_eq!(tree.len(), NODE_UIDS.len());

    fake.set_status(2, 2);
    fake.set_status(5, 13);
    let snapshot = client.status().unwrap();
    assert_eq!(snapshot.len(), NODE_UIDS.len());
    assert_eq!(snapshot.get(Uid(2)).unwrap().current, NodeStatus::Success);
    let idle = snapshot.get(Uid(5)).unwrap();
    assert_eq!(idle.current, NodeStatus::Idle);
    assert_eq!(idle.previous, Some(NodeStatus::Failure));
    assert_eq!(snapshot.get(Uid(1)).unwrap().current, NodeStatus::Idle);
    fake.assert_not_crashed();
}

#[test]
fn blackboards() {
    let fake = FakePublisher::start();
    let mut client = client_for(&fake, 2000);
    let value = client
        .blackboards(&["MainTree".to_owned(), "missing".to_owned()])
        .unwrap();
    assert_eq!(
        value,
        json!({"MainTree": {"x": 1, "name": "fast", "ratio": 0.5}})
    );
    let value = client.blackboards(&["sub", "MainTree"]).unwrap();
    assert_eq!(value["sub"], json!({"list": [1.5, 2.25], "flag": true}));
    assert_eq!(value["MainTree"]["x"], json!(1));
    // No name found: the publisher sends nil, the client gives {}.
    assert_eq!(client.blackboards(&["nope"]).unwrap(), json!({}));
    assert_eq!(client.blackboards::<&str>(&[]).unwrap(), json!({}));
    // The request text is the names joined with ';'.
    let sent = fake.requests_of(RequestType::Blackboard);
    assert_eq!(sent[0].payload_text(), "MainTree;missing");
    assert_eq!(sent[1].payload_text(), "sub;MainTree");
    // A name with ';' is rejected before anything is sent.
    let before = fake.count(RequestType::Blackboard);
    assert!(matches!(
        client.blackboards(&["a;b"]),
        Err(ClientError::Encode(EncodeError::BlackboardNameSeparator(_)))
    ));
    assert_eq!(fake.count(RequestType::Blackboard), before);
    fake.assert_not_crashed();
}

#[test]
fn hooks_end_to_end() {
    let fake = FakePublisher::start();
    let mut client = client_for(&fake, 2000);

    client.insert_hook(&Hook::breakpoint(2)).unwrap();
    let sent = fake.requests_of(RequestType::HookInsert);
    assert_eq!(
        sent[0].payload_text(),
        r#"{"desired_status":"SKIPPED","enabled":true,"mode":0,"once":false,"position":0,"uid":2}"#
    );
    assert_eq!(client.hooks_dump().unwrap(), vec![Hook::breakpoint(2)]);

    // Several hooks in one request; an unknown node is ignored.
    client
        .insert_hooks(&[
            Hook::replace(4, NodeStatus::Failure),
            Hook::breakpoint(6),
            Hook::breakpoint(999),
        ])
        .unwrap();
    let dump = client.hooks_dump().unwrap();
    assert_eq!(
        dump,
        vec![
            Hook::breakpoint(2),
            Hook::replace(4, NodeStatus::Failure),
            Hook::breakpoint(6)
        ]
    );
    // An empty array is a valid request and changes nothing.
    client.insert_hooks(&[]).unwrap();
    assert_eq!(
        fake.requests_of(RequestType::HookInsert)
            .last()
            .unwrap()
            .payload_text(),
        "[]"
    );
    assert_eq!(client.hooks_dump().unwrap(), dump);

    // Change an existing hook.
    let disabled = Hook {
        enabled: false,
        ..Hook::breakpoint(6)
    };
    client.insert_hook(&disabled).unwrap();
    assert!(!fake.state().hooks[&6].hook.enabled);

    // Unlock a blocked breakpoint.
    assert_eq!(fake.hit(2), Some(HookMode::Breakpoint));
    assert!(fake.state().hooks[&2].blocked);
    client
        .unlock_breakpoint(&UnlockRequest::with_status(2, NodeStatus::Success))
        .unwrap();
    assert!(!fake.state().hooks[&2].blocked);
    let sent = fake.requests_of(RequestType::BreakpointUnlock);
    assert_eq!(
        sent[0].payload_text(),
        r#"{"desired_status":"SUCCESS","position":0,"remove_when_done":false,"uid":2}"#
    );

    // Unlock and remove errors.
    let error = client
        .unlock_breakpoint(&UnlockRequest::resume(3))
        .unwrap_err();
    assert!(matches!(error, ClientError::Server(ref text) if text == "Node ID not found"));
    let post = UnlockRequest {
        position: HookPosition::Post,
        ..UnlockRequest::resume(2)
    };
    assert!(matches!(
        client.unlock_breakpoint(&post),
        Err(ClientError::Server(_))
    ));
    assert!(matches!(
        client.remove_hook(2, HookPosition::Post),
        Err(ClientError::Server(_))
    ));
    assert!(matches!(
        client.remove_hook(999, HookPosition::Pre),
        Err(ClientError::Server(_))
    ));

    // Remove one hook.
    client.remove_hook(4, HookPosition::Pre).unwrap();
    assert_eq!(
        fake.requests_of(RequestType::HookRemove)
            .last()
            .unwrap()
            .payload_text(),
        r#"{"position":0,"uid":4}"#
    );
    assert_eq!(client.hooks_dump().unwrap().len(), 2);

    // Disable all, then remove all.
    client.disable_all_hooks().unwrap();
    assert!(
        client
            .hooks_dump()
            .unwrap()
            .iter()
            .all(|hook| !hook.enabled)
    );
    client.remove_all_hooks().unwrap();
    assert!(client.hooks_dump().unwrap().is_empty());
    assert_eq!(fake.count(RequestType::RemoveAllHooks), 1);
    assert_eq!(fake.count(RequestType::DisableAllHooks), 1);

    // A POST hook is stored with the PRE hooks (BT.CPP 4.9.0).
    let post_hook = Hook {
        position: HookPosition::Post,
        ..Hook::breakpoint(5)
    };
    client.insert_hook(&post_hook).unwrap();
    assert_eq!(client.hooks_dump().unwrap(), vec![post_hook]);
    fake.assert_not_crashed();
}

#[test]
fn recording() {
    let fake = FakePublisher::start();
    let mut client = client_for(&fake, 2000);
    let before = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_micros() as u64;
    let start = client.start_recording().unwrap();
    assert!(start >= before);
    assert!(start < before + 60_000_000);
    assert_eq!(
        fake.requests_of(RequestType::ToggleRecording)[0].payload_text(),
        "start"
    );
    assert!(client.transitions().unwrap().is_empty());

    fake.push_transitions(5);
    let batch = client.transitions().unwrap();
    assert_eq!(batch.len(), 5);
    assert_eq!(batch[0].timestamp_us, 10);
    assert_eq!(batch[0].uid, Uid(NODE_UIDS[0]));
    assert_eq!(batch[4].timestamp_us, 50);
    assert!(client.transitions().unwrap().is_empty());

    // The buffer keeps the last 1000.
    fake.push_transitions(1500);
    let batch = client.transitions().unwrap();
    assert_eq!(batch.len(), 1000);
    assert_eq!(batch[0].timestamp_us, 50 + 501 * 10);

    client.stop_recording().unwrap();
    assert_eq!(
        fake.requests_of(RequestType::ToggleRecording)[1].payload_text(),
        "stop"
    );
    assert!(!fake.state().recording);
    fake.assert_not_crashed();
}

#[test]
fn timeout_and_recovery() {
    let fake = FakePublisher::start();
    let mut client = client_for(&fake, 200);
    client.status().unwrap();

    fake.set_silent(true);
    let started = Instant::now();
    assert!(matches!(
        client.status(),
        Err(ClientError::Timeout {
            request: RequestType::Status
        })
    ));
    let elapsed = started.elapsed();
    assert!(elapsed >= Duration::from_millis(150), "{elapsed:?}");
    assert!(elapsed < Duration::from_secs(2), "{elapsed:?}");
    assert!(matches!(
        client.full_tree(),
        Err(ClientError::Timeout { .. })
    ));

    // The server answers again. The old requests get replies that nobody
    // reads; the new socket gets the right reply.
    fake.set_silent(false);
    let (uuid, _) = client.full_tree().unwrap();
    assert_eq!(uuid, fake.uuid());
    client.status().unwrap();
    fake.assert_not_crashed();
}

#[test]
fn server_stop_and_start() {
    let mut fake = FakePublisher::start();
    let mut client = client_for(&fake, 200);
    let first_uuid = client.full_tree().unwrap().0;
    fake.stop();
    assert!(client.status().unwrap_err().is_connection_error());
    fake.start_again();
    // ZeroMQ reconnects in the background. Retry until it works.
    let deadline = Instant::now() + Duration::from_secs(5);
    let snapshot = loop {
        match client.status() {
            Ok(snapshot) => break snapshot,
            Err(err) if err.is_connection_error() && Instant::now() < deadline => continue,
            Err(err) => panic!("{err}"),
        }
    };
    assert_eq!(snapshot.len(), NODE_UIDS.len());
    assert_eq!(client.last_tree_uuid(), Some(first_uuid));

    // A restart of the executor gives a new UUID in the next reply.
    let new_uuid = fake.restart_executor();
    assert_ne!(new_uuid, first_uuid);
    client.status().unwrap();
    assert_eq!(client.last_tree_uuid(), Some(new_uuid));
}

#[test]
fn malformed_replies_give_errors() {
    let fake = FakePublisher::start();
    let mut client = client_for(&fake, 1000);

    fake.push_malformed(Malformed::WrongHeaderSize);
    assert!(matches!(
        client.status(),
        Err(ClientError::Protocol(ProtocolError::HeaderSize {
            expected: 22,
            actual: 10,
            ..
        }))
    ));
    fake.push_malformed(Malformed::WrongId);
    assert!(matches!(
        client.status(),
        Err(ClientError::Protocol(ProtocolError::IdMismatch { .. }))
    ));
    fake.push_malformed(Malformed::WrongProtocol);
    assert!(matches!(
        client.status(),
        Err(ClientError::Protocol(ProtocolError::ProtocolMismatch {
            expected: 2,
            actual: 3
        }))
    ));
    fake.push_malformed(Malformed::WrongType);
    assert!(matches!(
        client.status(),
        Err(ClientError::Protocol(ProtocolError::TypeMismatch {
            expected: RequestType::Status,
            actual: RequestType::FullTree
        }))
    ));
    fake.push_malformed(Malformed::Garbage);
    assert!(matches!(
        client.status(),
        Err(ClientError::Protocol(ProtocolError::HeaderSize { .. }))
    ));
    fake.push_malformed(Malformed::ExtraPart);
    assert!(matches!(
        client.status(),
        Err(ClientError::Protocol(ProtocolError::PartCount {
            request: RequestType::Status,
            expected: 2,
            actual: 3
        }))
    ));
    fake.push_malformed(Malformed::ErrorReply("boom".to_owned()));
    assert!(matches!(client.status(), Err(ClientError::Server(ref text)) if text == "boom"));

    // Bad payloads for each kind of reply.
    fake.push_malformed(Malformed::BadPayload);
    assert!(matches!(
        client.status(),
        Err(ClientError::Decode(DecodeError::RecordSize { .. }))
    ));
    fake.push_malformed(Malformed::BadPayload);
    assert!(matches!(
        client.full_tree(),
        Err(ClientError::Decode(DecodeError::Utf8 { .. }))
    ));
    fake.push_malformed(Malformed::BadPayload);
    assert!(matches!(
        client.transitions(),
        Err(ClientError::Decode(DecodeError::RecordSize { .. }))
    ));
    fake.push_malformed(Malformed::BadPayload);
    assert!(matches!(
        client.hooks_dump(),
        Err(ClientError::Decode(DecodeError::Json { .. }))
    ));
    fake.push_malformed(Malformed::BadPayload);
    assert!(matches!(
        client.blackboards(&["MainTree"]),
        Err(ClientError::Decode(_))
    ));
    fake.push_malformed(Malformed::BadPayload);
    assert!(matches!(
        client.start_recording(),
        Err(ClientError::Decode(_))
    ));
    // A reply without payload gets an extra part.
    fake.push_malformed(Malformed::BadPayload);
    assert!(matches!(
        client.remove_all_hooks(),
        Err(ClientError::Protocol(ProtocolError::PartCount { .. }))
    ));

    // After all this, the client still works.
    let snapshot = client.status().unwrap();
    assert_eq!(snapshot.len(), NODE_UIDS.len());
    assert_eq!(client.full_tree().unwrap().1, TREE_XML);
    fake.assert_not_crashed();
}

/// Sends notifications until the SUB socket receives one (ZeroMQ drops
/// messages until the subscription reaches the publisher).
fn receive_notification(fake: &FakePublisher, client: &mut Client, uid: u16) -> Uid {
    let deadline = Instant::now() + Duration::from_secs(5);
    while Instant::now() < deadline {
        fake.notify(uid);
        if let Some(notification) = client.poll_notification(Duration::from_millis(50)).unwrap() {
            return notification.uid;
        }
    }
    panic!("no notification received");
}

#[test]
fn breakpoint_notifications() {
    let fake = FakePublisher::start();
    let mut client = client_for(&fake, 1000);
    assert_eq!(receive_notification(&fake, &mut client, 4), Uid(4));
    // Drain the extra copies.
    while client
        .poll_notification(Duration::from_millis(50))
        .unwrap()
        .is_some()
    {}

    // A malformed notification gives a decode error; the next one works.
    fake.publish_raw(vec![vec![2, b'N', 0, 0, 0, 0], b"x1".to_vec()]);
    let deadline = Instant::now() + Duration::from_secs(5);
    loop {
        match client.poll_notification(Duration::from_millis(50)) {
            Err(ClientError::Decode(_)) => break,
            // A late copy from `receive_notification`.
            Ok(Some(notification)) if notification.uid == Uid(4) => continue,
            Ok(None) if Instant::now() < deadline => continue,
            other => panic!("unexpected: {other:?}"),
        }
    }
    fake.publish_raw(vec![vec![2, b'S', 0, 0, 0, 0], b"1".to_vec()]);
    fake.publish_raw(vec![vec![2, b'N', 0, 0, 0, 0]]);
    fake.notify(6);
    let mut errors = 0;
    let deadline = Instant::now() + Duration::from_secs(5);
    let uid = loop {
        match client.poll_notification(Duration::from_millis(50)) {
            Ok(Some(notification)) if notification.uid == Uid(4) => continue,
            Ok(Some(notification)) => break notification.uid,
            Ok(None) => assert!(Instant::now() < deadline, "no notification"),
            Err(ClientError::Decode(_)) => errors += 1,
            Err(other) => panic!("{other}"),
        }
    };
    assert_eq!(uid, Uid(6));
    assert_eq!(errors, 2);
}

#[test]
fn request_timeout_can_change() {
    let fake = FakePublisher::start();
    let mut client = client_for(&fake, 2000);
    client.status().unwrap();
    client.set_request_timeout(Duration::from_millis(100));
    assert_eq!(client.config().request_timeout, Duration::from_millis(100));
    fake.set_silent(true);
    let started = Instant::now();
    assert!(client.status().is_err());
    assert!(started.elapsed() < Duration::from_secs(1));
    fake.set_silent(false);
    client.status().unwrap();
    client.reset().unwrap();
    client.status().unwrap();
}

#[test]
fn requests_are_well_formed() {
    let fake = FakePublisher::start();
    let mut client = client_for(&fake, 2000);
    client.full_tree().unwrap();
    client.status().unwrap();
    client.blackboards(&["MainTree"]).unwrap();
    client.insert_hook(&Hook::breakpoint(2)).unwrap();
    client.insert_hooks(&[Hook::breakpoint(3)]).unwrap();
    client.unlock_breakpoint(&UnlockRequest::resume(2)).unwrap();
    client.hooks_dump().unwrap();
    client.remove_hook(3, HookPosition::Pre).unwrap();
    client.disable_all_hooks().unwrap();
    client.remove_all_hooks().unwrap();
    client.start_recording().unwrap();
    client.transitions().unwrap();
    client.stop_recording().unwrap();

    let requests = fake.state().requests.clone();
    let expected = [
        (RequestType::FullTree, 1),
        (RequestType::Status, 1),
        (RequestType::Blackboard, 2),
        (RequestType::HookInsert, 2),
        (RequestType::HookInsert, 2),
        (RequestType::BreakpointUnlock, 2),
        (RequestType::HooksDump, 1),
        (RequestType::HookRemove, 2),
        (RequestType::DisableAllHooks, 1),
        (RequestType::RemoveAllHooks, 1),
        (RequestType::ToggleRecording, 2),
        (RequestType::GetTransitions, 1),
        (RequestType::ToggleRecording, 2),
    ];
    assert_eq!(requests.len(), expected.len());
    let mut ids = std::collections::HashSet::new();
    for (request, (request_type, parts)) in requests.iter().zip(expected) {
        // `SerializeHeader`: protocol 2, type byte, u32 ID (6 bytes).
        assert_eq!(request.header.len(), 6, "{request:?}");
        assert_eq!(request.header[0], 2, "{request:?}");
        assert_eq!(request.header[1], request_type.as_byte(), "{request:?}");
        assert_eq!(request.parts, parts, "{request:?}");
        let id = u32::from_le_bytes([
            request.header[2],
            request.header[3],
            request.header[4],
            request.header[5],
        ]);
        assert!(ids.insert(id), "request ID {id} was used twice");
    }
    fake.assert_not_crashed();
}
