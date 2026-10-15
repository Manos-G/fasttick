//! A fake executor for the integration tests.
//!
//! This is a copy of `crates/fasttick-protocol/tests/support/fake_publisher.rs`
//! (FastTick, MIT): integration tests of one crate cannot use the test
//! support of another crate. Keep the two copies equal.
//!
//! It binds a REP socket on a port and a PUB socket on the port plus one,
//! and it follows the server logic of `serverLoop` in the publisher of
//! BT.CPP 4.9.0:
//!
//! - a status buffer with 3 bytes for each node,
//! - one hook map (the PRE hooks; a hook with position POST is stored there
//!   too, and `getHook` with POST finds nothing),
//! - unlock, remove, dump, remove all, disable all,
//! - a recording buffer capped at 1000 transitions,
//! - a MessagePack BLACKBOARD reply,
//! - error replies and a random tree UUID.
//!
//! It parses the JSON of the requests strictly, like `from_json` in
//! the BT.CPP protocol header. Where BT.CPP would throw (and stop the executor),
//! the fake sets [`FakeState::crashed`] and stops replying.
//!
//! Test controls: breakpoint notifications, a new UUID (restart), silence
//! (timeouts), stop and start of the sockets, and malformed replies.

#![allow(dead_code)]

use std::collections::{BTreeMap, VecDeque};
use std::sync::atomic::{AtomicBool, AtomicU32, Ordering};
use std::sync::{Arc, Mutex, MutexGuard};
use std::thread::{self, JoinHandle};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use fasttick_core::runtime::Uid;
use fasttick_core::status::NodeStatus;
use fasttick_protocol::messages::{self, Hook, HookMode, HookPosition, RecordedTransition};
use fasttick_protocol::wire::{self, RequestHeader, RequestType, TreeUuid};
use serde_json::{Value, json};

/// The runtime XML of the fake tree (`WriteTreeToXML(tree, true, true)`
/// form).
pub const TREE_XML: &str = r#"<root BTCPP_format="4">
    <BehaviorTree ID="MainTree" _fullpath="">
        <Sequence name="main" _uid="1">
            <AlwaysSuccess name="first" _uid="2"/>
            <SubTree ID="Sub" _fullpath="sub" _uid="3"/>
            <Script name="script" _uid="4" code="x := 1"/>
        </Sequence>
    </BehaviorTree>
    <BehaviorTree ID="Sub" _fullpath="sub">
        <Fallback name="fb" _uid="5">
            <AlwaysFailure name="fail" _uid="6"/>
        </Fallback>
    </BehaviorTree>
    <TreeNodesModel/>
</root>"#;

/// The node UIDs, in the order of the status buffer.
pub const NODE_UIDS: [u16; 6] = [1, 2, 3, 4, 5, 6];

/// [`TREE_XML`] with one more node at the start: the UIDs of the old nodes
/// move up by one.
pub const CHANGED_TREE_XML: &str = r#"<root BTCPP_format="4">
    <BehaviorTree ID="MainTree" _fullpath="">
        <Sequence name="main" _uid="1">
            <AlwaysSuccess name="new_node" _uid="2"/>
            <AlwaysSuccess name="first" _uid="3"/>
            <SubTree ID="Sub" _fullpath="sub" _uid="4"/>
            <Script name="script" _uid="5" code="x := 1"/>
        </Sequence>
    </BehaviorTree>
    <BehaviorTree ID="Sub" _fullpath="sub">
        <Fallback name="fb" _uid="6">
            <AlwaysFailure name="fail" _uid="7"/>
        </Fallback>
    </BehaviorTree>
    <TreeNodesModel/>
</root>"#;

/// The node UIDs of [`CHANGED_TREE_XML`].
pub const CHANGED_NODE_UIDS: [u16; 7] = [1, 2, 3, 4, 5, 6, 7];

/// One way to corrupt the next reply.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Malformed {
    /// The reply header has 10 bytes.
    WrongHeaderSize,
    /// The reply has another request ID (an out-of-date reply).
    WrongId,
    /// The reply has protocol ID 3.
    WrongProtocol,
    /// The reply has another request type.
    WrongType,
    /// The reply is one part of random bytes.
    Garbage,
    /// The payload part is 4 bytes of 0xff (or an extra part is added if
    /// the reply has no payload).
    BadPayload,
    /// The reply has an extra part.
    ExtraPart,
    /// An error reply with this message.
    ErrorReply(String),
}

/// A hook in the fake hook map.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct FakeHook {
    pub hook: Hook,
    /// True while a node waits at this breakpoint.
    pub blocked: bool,
}

/// One received request.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LoggedRequest {
    pub at: Instant,
    /// The first part (the request header) as received.
    pub header: Vec<u8>,
    /// The type byte (also for unknown types).
    pub type_byte: u8,
    /// The second part, if any.
    pub payload: Option<Vec<u8>>,
    pub parts: usize,
}

impl LoggedRequest {
    pub fn request_type(&self) -> Option<RequestType> {
        RequestType::from_byte(self.type_byte)
    }

    pub fn payload_text(&self) -> String {
        self.payload
            .as_deref()
            .map(|bytes| String::from_utf8_lossy(bytes).into_owned())
            .unwrap_or_default()
    }
}

/// The state of the fake executor.
#[derive(Debug)]
pub struct FakeState {
    pub tree_xml: String,
    pub uuid: TreeUuid,
    pub status: Vec<(u16, u8)>,
    pub hooks: BTreeMap<u16, FakeHook>,
    pub recording: bool,
    pub transitions: VecDeque<RecordedTransition>,
    pub blackboards: BTreeMap<String, Value>,
    pub requests: Vec<LoggedRequest>,
    pub malformed: VecDeque<Malformed>,
    /// True: the server does not read requests (the client times out).
    pub silent: bool,
    /// Set when BT.CPP would throw in the server thread.
    pub crashed: Option<String>,
    /// Messages for the PUB socket.
    pub outbox: VecDeque<Vec<Vec<u8>>>,
    /// The time of the next recorded transition, in microseconds.
    pub clock_us: u64,
    /// The time when the PUB socket was bound the last time.
    pub publisher_bound_at: Option<Instant>,
}

pub fn random_uuid() -> TreeUuid {
    let mut bytes = [0u8; 16];
    for chunk in bytes.chunks_exact_mut(4) {
        chunk.copy_from_slice(&wire::random_request_id().to_le_bytes());
    }
    // Version 4 and variant bits, as `CreateRandomUUID` sets them.
    bytes[6] = (bytes[6] & 0x4f) | 0x40;
    bytes[8] = (bytes[8] & 0xbf) | 0x80;
    TreeUuid(bytes)
}

impl FakeState {
    fn new() -> FakeState {
        let mut blackboards = BTreeMap::new();
        blackboards.insert(
            "MainTree".to_owned(),
            json!({"x": 1, "name": "fast", "ratio": 0.5}),
        );
        blackboards.insert("sub".to_owned(), json!({"list": [1.5, 2.25], "flag": true}));
        FakeState {
            tree_xml: TREE_XML.to_owned(),
            uuid: random_uuid(),
            status: NODE_UIDS.iter().map(|uid| (*uid, 0)).collect(),
            hooks: BTreeMap::new(),
            recording: false,
            transitions: VecDeque::new(),
            blackboards,
            requests: Vec::new(),
            malformed: VecDeque::new(),
            silent: false,
            crashed: None,
            outbox: VecDeque::new(),
            clock_us: 0,
            publisher_bound_at: None,
        }
    }

    fn has_node(&self, uid: u16) -> bool {
        self.status.iter().any(|(node, _)| *node == uid)
    }

    pub fn status_bytes(&self) -> Vec<u8> {
        let mut out = Vec::new();
        for (uid, status) in &self.status {
            out.extend_from_slice(&uid.to_le_bytes());
            out.push(*status);
        }
        out
    }

    pub fn set_status(&mut self, uid: u16, byte: u8) {
        for (node, status) in &mut self.status {
            if *node == uid {
                *status = byte;
            }
        }
    }

    /// Handles one request. Returns `None` if the executor "crashed".
    fn handle(&mut self, parts: Vec<Vec<u8>>) -> Option<Vec<Vec<u8>>> {
        let first = parts.first().cloned().unwrap_or_default();
        self.requests.push(LoggedRequest {
            at: Instant::now(),
            header: first.clone(),
            type_byte: first.get(1).copied().unwrap_or(0),
            payload: parts.get(1).cloned(),
            parts: parts.len(),
        });
        if first.len() != wire::REQUEST_HEADER_SIZE {
            return Some(wire::encode_error_reply("wrong request header"));
        }
        // The reply copies the request header and sets the protocol ID.
        let mut header = first.clone();
        header[0] = wire::PROTOCOL_ID;
        header.extend_from_slice(&self.uuid.0);
        let mut reply = vec![header];

        let two_parts = parts.len() == 2;
        let body = parts.get(1).cloned().unwrap_or_default();
        match first[1] {
            b'T' => reply.push(self.tree_xml.clone().into_bytes()),
            b'S' => reply.push(self.status_bytes()),
            b'B' => {
                if !two_parts {
                    return Some(wire::encode_error_reply("must be 2 parts message"));
                }
                reply.push(self.blackboard_dump(&body));
            }
            b'I' => {
                if !two_parts {
                    return Some(wire::encode_error_reply("must be 2 parts message"));
                }
                if let Err(reason) = self.hook_insert(&body) {
                    self.crashed = Some(reason);
                    return None;
                }
            }
            b'U' => {
                if !two_parts {
                    return Some(wire::encode_error_reply("must be 2 parts message"));
                }
                match self.unlock(&body) {
                    Err(reason) => {
                        self.crashed = Some(reason);
                        return None;
                    }
                    Ok(false) => return Some(wire::encode_error_reply("Node ID not found")),
                    Ok(true) => {}
                }
            }
            b'A' => {
                // `removeAllHooks`: blocked nodes wake up.
                self.hooks.clear();
            }
            b'X' => {
                // `enableAllHooks(false)`.
                for hook in self.hooks.values_mut() {
                    hook.hook.enabled = false;
                    if hook.hook.mode == HookMode::Breakpoint {
                        hook.blocked = false;
                    }
                }
            }
            b'R' => {
                if !two_parts {
                    return Some(wire::encode_error_reply("must be 2 parts message"));
                }
                match self.remove(&body) {
                    Err(reason) => {
                        self.crashed = Some(reason);
                        return None;
                    }
                    Ok(false) => return Some(wire::encode_error_reply("Node ID not found")),
                    Ok(true) => {}
                }
            }
            b'D' => {
                let list: Vec<Value> = self
                    .hooks
                    .values()
                    .map(|hook| hook_to_json(&hook.hook))
                    .collect();
                reply.push(Value::Array(list).to_string().into_bytes());
            }
            b'r' => {
                if !two_parts {
                    return Some(wire::encode_error_reply("must be 2 parts message"));
                }
                if body == b"start" {
                    self.recording = true;
                    self.clock_us = 0;
                    let now = SystemTime::now()
                        .duration_since(UNIX_EPOCH)
                        .map(|elapsed| elapsed.as_micros() as u64)
                        .unwrap_or_default();
                    reply.push(now.to_string().into_bytes());
                    self.transitions.clear();
                } else if body == b"stop" {
                    self.recording = false;
                }
            }
            b't' => {
                let records: Vec<RecordedTransition> = self.transitions.drain(..).collect();
                reply.push(messages::encode_transitions(&records));
            }
            _ => return Some(wire::encode_error_reply("Request not recognized")),
        }
        if let Some(malformed) = self.malformed.pop_front() {
            corrupt(&mut reply, malformed);
        }
        Some(reply)
    }

    /// `generateBlackboardsDump`: split at ';', keep the known names,
    /// `to_msgpack` of the result (null if no name is known).
    fn blackboard_dump(&self, body: &[u8]) -> Vec<u8> {
        let text = String::from_utf8_lossy(body);
        let mut found = serde_json::Map::new();
        for name in split_string(&text, ';') {
            if let Some(value) = self.blackboards.get(name) {
                found.insert(name.to_owned(), value.clone());
            }
        }
        let value = if found.is_empty() {
            Value::Null
        } else {
            Value::Object(found)
        };
        let mut out = Vec::new();
        rmpv::encode::write_value(&mut out, &json_to_msgpack(&value)).expect("write to a Vec");
        out
    }

    fn hook_insert(&mut self, body: &[u8]) -> Result<(), String> {
        let json: Value =
            serde_json::from_slice(body).map_err(|err| format!("json parse error: {err}"))?;
        match json {
            Value::Array(items) => {
                for item in items {
                    self.insert_one(&item)?;
                }
            }
            other => self.insert_one(&other)?,
        }
        Ok(())
    }

    fn insert_one(&mut self, json: &Value) -> Result<(), String> {
        let uid = get_u16(json, "uid")?;
        let position = get_int(json, "position")?;
        // `getHook(pos, uid)`: only PRE hooks exist.
        if position == 0
            && let Some(existing) = self.hooks.get_mut(&uid)
        {
            let was_interactive = existing.hook.mode == HookMode::Breakpoint;
            existing.hook = hook_from_json(json)?;
            if was_interactive && existing.hook.mode == HookMode::Replace {
                existing.blocked = false;
            }
            return Ok(());
        }
        let hook = hook_from_json(json)?;
        // `insertHook` returns false for an unknown node; the reply is OK.
        if self.has_node(uid) {
            self.hooks.insert(
                uid,
                FakeHook {
                    hook,
                    blocked: false,
                },
            );
        }
        Ok(())
    }

    /// Returns Ok(false) for "Node ID not found".
    fn unlock(&mut self, body: &[u8]) -> Result<bool, String> {
        let json: Value =
            serde_json::from_slice(body).map_err(|err| format!("json parse error: {err}"))?;
        let uid = get_u16(&json, "uid")?;
        let status = get_string(&json, "desired_status")?;
        let position = get_int(&json, "position")?;
        let remove = get_bool(&json, "remove_when_done")?;
        let desired = match status.as_str() {
            "SUCCESS" => NodeStatus::Success,
            "FAILURE" => NodeStatus::Failure,
            _ => NodeStatus::Skipped,
        };
        if !self.has_node(uid) || position != 0 {
            return Ok(false);
        }
        let Some(hook) = self.hooks.get_mut(&uid) else {
            return Ok(false);
        };
        hook.hook.desired_status = desired;
        hook.hook.once |= remove;
        if hook.hook.mode == HookMode::Breakpoint && hook.blocked {
            hook.blocked = false;
            if hook.hook.once {
                self.hooks.remove(&uid);
            }
        }
        Ok(true)
    }

    fn remove(&mut self, body: &[u8]) -> Result<bool, String> {
        let json: Value =
            serde_json::from_slice(body).map_err(|err| format!("json parse error: {err}"))?;
        let uid = get_u16(&json, "uid")?;
        let position = get_int(&json, "position")?;
        if !self.has_node(uid) || position != 0 || !self.hooks.contains_key(&uid) {
            return Ok(false);
        }
        self.hooks.remove(&uid);
        Ok(true)
    }
}

/// `BT::splitString(str, ';')`: no empty part at the end, empty parts in
/// the middle are kept.
fn split_string(text: &str, delimiter: char) -> Vec<&str> {
    let mut out = Vec::new();
    let mut rest = text;
    while !rest.is_empty() {
        match rest.find(delimiter) {
            Some(position) => {
                out.push(&rest[..position]);
                rest = &rest[position + delimiter.len_utf8()..];
            }
            None => {
                out.push(rest);
                rest = "";
            }
        }
    }
    out
}

fn corrupt(reply: &mut Vec<Vec<u8>>, malformed: Malformed) {
    match malformed {
        Malformed::WrongHeaderSize => reply[0].truncate(10),
        Malformed::WrongId => reply[0][2] ^= 0xff,
        Malformed::WrongProtocol => reply[0][0] = 3,
        Malformed::WrongType => reply[0][1] = if reply[0][1] == b'S' { b'T' } else { b'S' },
        Malformed::Garbage => *reply = vec![vec![0xde, 0xad, 0xbe, 0xef, 0x00, 0x13]],
        Malformed::BadPayload => {
            if reply.len() > 1 {
                reply[1] = vec![0xff; 4];
            } else {
                reply.push(vec![0xff; 4]);
            }
        }
        Malformed::ExtraPart => reply.push(b"extra".to_vec()),
        Malformed::ErrorReply(message) => *reply = wire::encode_error_reply(&message),
    }
}

/// `to_json(Hook)`: nlohmann writes the keys in sorted order.
pub fn hook_to_json(hook: &Hook) -> Value {
    json!({
        "desired_status": hook.desired_status.as_str(),
        "enabled": hook.enabled,
        "mode": hook.mode.as_i64(),
        "once": hook.once,
        "position": hook.position.as_i64(),
        "uid": hook.uid,
    })
}

/// `from_json(json, Hook)`, with the strict type checks of nlohmann's
/// `get<T>()` and `at()`.
fn hook_from_json(json: &Value) -> Result<Hook, String> {
    let enabled = get_bool(json, "enabled")?;
    let uid = get_u16(json, "uid")?;
    let once = get_bool(json, "once")?;
    let mode = get_int(json, "mode")?;
    let position = get_int(json, "position")?;
    let status = get_string(json, "desired_status")?;
    let desired_status: NodeStatus = status
        .parse()
        .map_err(|_| format!("Cannot convert this to NodeStatus: {status}"))?;
    Ok(Hook {
        desired_status,
        enabled,
        // BT.CPP casts any integer; the fake keeps only valid values.
        mode: HookMode::from_i64(mode).ok_or(format!("bad mode {mode}"))?,
        once,
        position: HookPosition::from_i64(position).ok_or(format!("bad position {position}"))?,
        uid,
    })
}

fn field<'a>(json: &'a Value, key: &str) -> Result<&'a Value, String> {
    json.get(key)
        .ok_or_else(|| format!("key '{key}' not found"))
}

fn get_bool(json: &Value, key: &str) -> Result<bool, String> {
    field(json, key)?
        .as_bool()
        .ok_or_else(|| format!("type must be boolean: {key}"))
}

fn get_int(json: &Value, key: &str) -> Result<i64, String> {
    field(json, key)?
        .as_i64()
        .ok_or_else(|| format!("type must be number: {key}"))
}

fn get_u16(json: &Value, key: &str) -> Result<u16, String> {
    field(json, key)?
        .as_u64()
        .and_then(|value| u16::try_from(value).ok())
        .ok_or_else(|| format!("type must be uint16: {key}"))
}

fn get_string(json: &Value, key: &str) -> Result<String, String> {
    field(json, key)?
        .as_str()
        .map(str::to_owned)
        .ok_or_else(|| format!("type must be string: {key}"))
}

/// Converts JSON to MessagePack as `nlohmann::json::to_msgpack` does.
pub fn json_to_msgpack(value: &Value) -> rmpv::Value {
    match value {
        Value::Null => rmpv::Value::Nil,
        Value::Bool(flag) => rmpv::Value::Boolean(*flag),
        Value::Number(number) => {
            if let Some(unsigned) = number.as_u64() {
                rmpv::Value::from(unsigned)
            } else if let Some(signed) = number.as_i64() {
                rmpv::Value::from(signed)
            } else {
                let float = number.as_f64().unwrap_or_default();
                // `write_compact_float`: float32 when it is exact.
                if f64::from(float as f32) == float {
                    rmpv::Value::F32(float as f32)
                } else {
                    rmpv::Value::F64(float)
                }
            }
        }
        Value::String(text) => rmpv::Value::from(text.as_str()),
        Value::Array(items) => rmpv::Value::Array(items.iter().map(json_to_msgpack).collect()),
        Value::Object(map) => rmpv::Value::Map(
            map.iter()
                .map(|(key, item)| (rmpv::Value::from(key.as_str()), json_to_msgpack(item)))
                .collect(),
        ),
    }
}

/// A running fake executor.
pub struct FakePublisher {
    /// The server port. The PUB socket is on `port + 1`.
    pub port: u16,
    state: Arc<Mutex<FakeState>>,
    context: zmq::Context,
    running: Option<(Arc<AtomicBool>, JoinHandle<()>)>,
}

impl std::fmt::Debug for FakePublisher {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("FakePublisher")
            .field("port", &self.port)
            .finish()
    }
}

static NEXT_CANDIDATE: AtomicU32 = AtomicU32::new(0);

fn bind_pair(context: &zmq::Context, port: u16) -> Option<(zmq::Socket, zmq::Socket)> {
    let server = bind_server(context, port)?;
    let publisher = bind_publisher(context, port)?;
    Some((server, publisher))
}

fn bind_server(context: &zmq::Context, port: u16) -> Option<zmq::Socket> {
    let server = context.socket(zmq::REP).ok()?;
    server.set_linger(0).ok()?;
    server.bind(&format!("tcp://127.0.0.1:{port}")).ok()?;
    Some(server)
}

/// Binds the PUB socket on `port + 1`.
fn bind_publisher(context: &zmq::Context, port: u16) -> Option<zmq::Socket> {
    let publisher = context.socket(zmq::PUB).ok()?;
    publisher.set_linger(0).ok()?;
    publisher
        .bind(&format!("tcp://127.0.0.1:{}", port.checked_add(1)?))
        .ok()?;
    Some(publisher)
}

/// A PUB socket that the server thread binds later.
struct PendingPublisher {
    context: zmq::Context,
    port: u16,
    at: Instant,
}

impl FakePublisher {
    /// Starts a fake executor on a free pair of ports.
    pub fn start() -> FakePublisher {
        let context = zmq::Context::new();
        let process = std::process::id();
        for _ in 0..500 {
            let candidate = NEXT_CANDIDATE.fetch_add(1, Ordering::Relaxed);
            // Even ports from 20000 to 31998, spread by process ID. The
            // range stays below the ephemeral ports of Linux (32768 and
            // up), so that an outgoing connection cannot take the port
            // while the fake is stopped (for example a TCP self-connect of
            // a client that tries to reconnect).
            let slot = (process
                .wrapping_mul(7919)
                .wrapping_add(candidate.wrapping_mul(2)))
                % 6000;
            let port = 20000 + (slot * 2) as u16;
            if let Some((server, publisher)) = bind_pair(&context, port) {
                let mut fake = FakePublisher {
                    port,
                    state: Arc::new(Mutex::new(FakeState::new())),
                    context,
                    running: None,
                };
                fake.spawn(server, publisher);
                return fake;
            }
        }
        panic!("no free port pair found");
    }

    fn spawn(&mut self, server: zmq::Socket, publisher: zmq::Socket) {
        self.state().publisher_bound_at = Some(Instant::now());
        self.spawn_with(server, Some(publisher), None);
    }

    fn spawn_with(
        &mut self,
        server: zmq::Socket,
        publisher: Option<zmq::Socket>,
        pending: Option<PendingPublisher>,
    ) {
        let stop = Arc::new(AtomicBool::new(false));
        let state = Arc::clone(&self.state);
        let flag = Arc::clone(&stop);
        let thread = thread::spawn(move || serve(server, publisher, pending, state, flag));
        self.running = Some((stop, thread));
    }

    /// The state, locked.
    pub fn state(&self) -> MutexGuard<'_, FakeState> {
        self.state
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
    }

    pub fn uuid(&self) -> TreeUuid {
        self.state().uuid
    }

    /// Closes both sockets (the executor goes away).
    pub fn stop(&mut self) {
        if let Some((stop, thread)) = self.running.take() {
            stop.store(true, Ordering::SeqCst);
            let _ = thread.join();
        }
    }

    /// Binds the same ports again after [`FakePublisher::stop`].
    pub fn start_again(&mut self) {
        let deadline = Instant::now() + Duration::from_secs(5);
        loop {
            if let Some((server, publisher)) = bind_pair(&self.context, self.port) {
                self.spawn(server, publisher);
                return;
            }
            assert!(
                Instant::now() < deadline,
                "cannot bind port {} again",
                self.port
            );
            thread::sleep(Duration::from_millis(10));
        }
    }

    /// Binds the server port again after [`FakePublisher::stop`], and the
    /// PUB socket only after `delay`. Messages for the PUB socket before
    /// that time are lost, as with a PUB socket without subscribers.
    pub fn start_again_with_publisher_delay(&mut self, delay: Duration) {
        let deadline = Instant::now() + Duration::from_secs(5);
        let server = loop {
            if let Some(server) = bind_server(&self.context, self.port) {
                break server;
            }
            assert!(
                Instant::now() < deadline,
                "cannot bind port {} again",
                self.port
            );
            thread::sleep(Duration::from_millis(10));
        };
        self.state().publisher_bound_at = None;
        let pending = PendingPublisher {
            context: self.context.clone(),
            port: self.port,
            at: Instant::now() + delay,
        };
        self.spawn_with(server, None, Some(pending));
    }

    /// Simulates a new executor process with another tree.
    pub fn restart_executor_with(&self, xml: &str, uids: &[u16]) -> TreeUuid {
        {
            let mut state = self.state();
            state.tree_xml = xml.to_owned();
            state.status = uids.iter().map(|uid| (*uid, 0)).collect();
        }
        self.restart_executor()
    }

    /// Simulates a new executor process: a new UUID, no hooks, no
    /// recording, all nodes IDLE. Returns the new UUID.
    pub fn restart_executor(&self) -> TreeUuid {
        let mut state = self.state();
        state.uuid = random_uuid();
        state.hooks.clear();
        state.recording = false;
        state.transitions.clear();
        for (_, status) in &mut state.status {
            *status = 0;
        }
        state.uuid
    }

    pub fn set_silent(&self, silent: bool) {
        self.state().silent = silent;
    }

    pub fn push_malformed(&self, malformed: Malformed) {
        self.state().malformed.push_back(malformed);
    }

    pub fn set_status(&self, uid: u16, byte: u8) {
        self.state().set_status(uid, byte);
    }

    /// Sends a raw message on the PUB socket.
    pub fn publish_raw(&self, parts: Vec<Vec<u8>>) {
        self.state().outbox.push_back(parts);
    }

    /// Sends a BREAKPOINT_REACHED notification for `uid`.
    pub fn notify(&self, uid: u16) {
        let header = RequestHeader::new(RequestType::BreakpointReached);
        self.publish_raw(messages::encode_breakpoint_notification(&header, Uid(uid)));
    }

    /// Simulates the pre-tick callback of the hook at `uid`
    /// (`insertHook` of the publisher): if the hook is enabled, it sends a
    /// notification, and in breakpoint mode the node blocks. Returns the
    /// mode of the hook that ran.
    pub fn hit(&self, uid: u16) -> Option<HookMode> {
        let mut state = self.state();
        let hook = *state.hooks.get(&uid)?;
        if !hook.hook.enabled {
            return None;
        }
        let header = RequestHeader::new(RequestType::BreakpointReached);
        state
            .outbox
            .push_back(messages::encode_breakpoint_notification(&header, Uid(uid)));
        match hook.hook.mode {
            HookMode::Breakpoint => {
                if let Some(entry) = state.hooks.get_mut(&uid) {
                    entry.blocked = true;
                }
            }
            HookMode::Replace => {
                if hook.hook.once {
                    state.hooks.remove(&uid);
                }
            }
        }
        Some(hook.hook.mode)
    }

    /// Records `count` transitions (as `callback` of the publisher), if a
    /// recording runs. The buffer keeps the last 1000.
    pub fn push_transitions(&self, count: usize) {
        let mut state = self.state();
        if !state.recording {
            return;
        }
        for index in 0..count {
            state.clock_us += 10;
            let transition = RecordedTransition {
                timestamp_us: state.clock_us,
                uid: Uid(NODE_UIDS[index % NODE_UIDS.len()]),
                status: NodeStatus::ALL[index % 4 + 1],
            };
            state.transitions.push_back(transition);
            while state.transitions.len() > messages::RECORDING_BUFFER_CAPACITY {
                state.transitions.pop_front();
            }
        }
    }

    /// All logged requests of one type.
    pub fn requests_of(&self, request_type: RequestType) -> Vec<LoggedRequest> {
        self.state()
            .requests
            .iter()
            .filter(|request| request.type_byte == request_type.as_byte())
            .cloned()
            .collect()
    }

    pub fn count(&self, request_type: RequestType) -> usize {
        self.requests_of(request_type).len()
    }

    /// Panics if the executor would have crashed.
    pub fn assert_not_crashed(&self) {
        let crashed = self.state().crashed.clone();
        assert!(crashed.is_none(), "the executor crashed: {crashed:?}");
    }

    /// Waits until `check` is true for the state, up to `timeout`.
    pub fn wait_until(&self, timeout: Duration, mut check: impl FnMut(&FakeState) -> bool) -> bool {
        let deadline = Instant::now() + timeout;
        loop {
            if check(&self.state()) {
                return true;
            }
            if Instant::now() >= deadline {
                return false;
            }
            thread::sleep(Duration::from_millis(5));
        }
    }
}

impl Drop for FakePublisher {
    fn drop(&mut self) {
        self.stop();
    }
}

fn serve(
    server: zmq::Socket,
    mut publisher: Option<zmq::Socket>,
    mut pending: Option<PendingPublisher>,
    state: Arc<Mutex<FakeState>>,
    stop: Arc<AtomicBool>,
) {
    let lock = || {
        state
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
    };
    while !stop.load(Ordering::SeqCst) {
        if let Some(later) = &pending
            && Instant::now() >= later.at
            && let Some(socket) = bind_publisher(&later.context, later.port)
        {
            publisher = Some(socket);
            pending = None;
            lock().publisher_bound_at = Some(Instant::now());
        }
        let outbox: Vec<Vec<Vec<u8>>> = lock().outbox.drain(..).collect();
        for message in outbox {
            if let Some(publisher) = &publisher {
                let _ = publisher.send_multipart(message, 0);
            }
        }
        match server.poll(zmq::POLLIN, 5) {
            Ok(count) if count > 0 => {}
            _ => continue,
        }
        // Check after the poll: a request that arrives while the server is
        // silent stays in the queue.
        let paused = {
            let state = lock();
            state.silent || state.crashed.is_some()
        };
        if paused {
            thread::sleep(Duration::from_millis(2));
            continue;
        }
        let Ok(parts) = server.recv_multipart(0) else {
            continue;
        };
        let reply = lock().handle(parts);
        if let Some(reply) = reply {
            let _ = server.send_multipart(reply, 0);
        }
    }
}
