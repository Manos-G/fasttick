//! Payloads of the publisher protocol: hooks, unlock and remove requests,
//! the STATUS reply, the recorded transitions, the recording start time,
//! the BLACKBOARD reply and the breakpoint notification.
//!
//! All functions in this module are pure. They do not open sockets.
//! Decode functions return a [`DecodeError`] for malformed input. They never
//! panic.

use fasttick_core::runtime::Uid;
use fasttick_core::status::{NodeStatus, RawStatus};
use serde::{Deserialize, Deserializer, Serialize, Serializer};

use crate::wire::{ProtocolError, RequestHeader, RequestType};

/// An error in the payload of a reply or of a notification.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum DecodeError {
    /// The framing of the message is wrong.
    #[error(transparent)]
    Protocol(#[from] ProtocolError),
    /// The length of a buffer is not a multiple of the record size.
    #[error("the {what} has {len} bytes, which is not a multiple of {record_size}")]
    RecordSize {
        /// The kind of buffer.
        what: &'static str,
        /// The length of the buffer.
        len: usize,
        /// The size of one record.
        record_size: usize,
    },
    /// A status byte has a value that BT.CPP does not send.
    #[error("node {uid} has the unknown status byte {byte}")]
    UnknownStatus {
        /// The UID of the node.
        uid: u16,
        /// The status byte.
        byte: u8,
    },
    /// A text is not valid UTF-8.
    #[error("the {what} is not valid UTF-8")]
    Utf8 {
        /// The kind of text.
        what: &'static str,
    },
    /// A text is not a valid decimal number.
    #[error("the {what} is not a valid number: {text:?}")]
    Number {
        /// The kind of number.
        what: &'static str,
        /// The text (with replacement characters for bytes that are not
        /// UTF-8).
        text: String,
    },
    /// A JSON text is not valid or does not have the expected fields.
    #[error("invalid JSON in the {what}: {message}")]
    Json {
        /// The kind of JSON document.
        what: &'static str,
        /// The message of the JSON parser.
        message: String,
    },
    /// The BLACKBOARD reply is not valid MessagePack.
    #[error("invalid MessagePack in the blackboard reply: {0}")]
    MsgPack(String),
    /// The BLACKBOARD reply has bytes after the MessagePack value.
    #[error("the blackboard reply has {0} bytes after the MessagePack value")]
    TrailingBytes(usize),
    /// The BLACKBOARD reply is not a map.
    #[error("the blackboard reply is {0}, expected a map")]
    BlackboardShape(&'static str),
}

/// An error when a request payload is made.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum EncodeError {
    /// A blackboard name contains `;`. The publisher splits the request at
    /// each `;`, so such a name cannot be requested.
    #[error("the blackboard name {0:?} contains ';'")]
    BlackboardNameSeparator(String),
    /// Serialization to JSON failed.
    #[error("cannot serialize the {what} to JSON: {message}")]
    Json {
        /// The kind of payload.
        what: &'static str,
        /// The message of the serializer.
        message: String,
    },
}

// ---------------------------------------------------------------------------
// Hooks

/// What a hook does when a node is about to tick (`Hook::Mode` in the
/// BT.CPP protocol header). JSON uses the integer value.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord, Default)]
pub enum HookMode {
    /// The node stops and waits for BREAKPOINT_UNLOCK (`BREAKPOINT`, 0).
    #[default]
    Breakpoint = 0,
    /// The node does not tick. It returns `desired_status` at once
    /// (`REPLACE`, 1). Use SUCCESS or FAILURE: BT.CPP ignores other values
    /// and ticks the node normally (`TreeNode::executeTick`).
    Replace = 1,
}

impl HookMode {
    /// The integer value on the wire.
    pub fn as_i64(self) -> i64 {
        self as i64
    }

    /// Converts the integer value on the wire.
    pub fn from_i64(value: i64) -> Option<HookMode> {
        match value {
            0 => Some(HookMode::Breakpoint),
            1 => Some(HookMode::Replace),
            _ => None,
        }
    }
}

/// When a hook runs (`Hook::Position` in the BT.CPP protocol header). JSON
/// uses the integer value.
///
/// BT.CPP 4.9.0 runs every hook before the tick, also a hook with position
/// [`HookPosition::Post`]. It stores such a hook with the PRE hooks, so
/// unlock and remove with position POST then fail with
/// "Node ID not found" (`insertHook` and `getHook` of the publisher).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord, Default)]
pub enum HookPosition {
    /// Before the tick (`PRE`, 0).
    #[default]
    Pre = 0,
    /// After the tick (`POST`, 1).
    Post = 1,
}

impl HookPosition {
    /// The integer value on the wire.
    pub fn as_i64(self) -> i64 {
        self as i64
    }

    /// Converts the integer value on the wire.
    pub fn from_i64(value: i64) -> Option<HookPosition> {
        match value {
            0 => Some(HookPosition::Pre),
            1 => Some(HookPosition::Post),
            _ => None,
        }
    }
}

macro_rules! integer_serde {
    ($type:ident, $what:literal) => {
        impl Serialize for $type {
            fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
                serializer.serialize_i64(self.as_i64())
            }
        }

        impl<'de> Deserialize<'de> for $type {
            fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<$type, D::Error> {
                let value = i64::deserialize(deserializer)?;
                $type::from_i64(value).ok_or_else(|| {
                    serde::de::Error::custom(format_args!("invalid {} value {}", $what, value))
                })
            }
        }
    };
}

integer_serde!(HookMode, "hook mode");
integer_serde!(HookPosition, "hook position");

/// A hook on one node (`BT::Monitor::Hook`).
///
/// The JSON form is the form of `to_json` and `from_json` in the BT.CPP
/// protocol header:
///
/// ```json
/// {"desired_status":"SKIPPED","enabled":true,"mode":0,"once":false,"position":0,"uid":3}
/// ```
///
/// The fields are in alphabetical order. This is the order in which
/// `nlohmann::json` writes the keys, so the text is equal to the HOOKS_DUMP
/// output of the publisher.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct Hook {
    /// The status that the node returns when the hook replaces the tick.
    /// JSON uses the BT.CPP name, for example `"SUCCESS"`.
    pub desired_status: NodeStatus,
    /// False to keep the hook installed but inactive.
    pub enabled: bool,
    /// Breakpoint or replace.
    pub mode: HookMode,
    /// Remove the hook after it runs once (`remove_when_done` in BT.CPP,
    /// JSON key `"once"`).
    pub once: bool,
    /// Before or after the tick.
    pub position: HookPosition,
    /// The UID of the node.
    pub uid: u16,
}

impl Hook {
    /// An enabled breakpoint before the tick of the node `uid`.
    pub fn breakpoint(uid: u16) -> Hook {
        Hook {
            desired_status: NodeStatus::Skipped,
            enabled: true,
            mode: HookMode::Breakpoint,
            once: false,
            position: HookPosition::Pre,
            uid,
        }
    }

    /// An enabled hook that replaces the tick of the node `uid`: the node
    /// returns `status` and does not tick. Use SUCCESS or FAILURE.
    pub fn replace(uid: u16, status: NodeStatus) -> Hook {
        Hook {
            desired_status: status,
            mode: HookMode::Replace,
            ..Hook::breakpoint(uid)
        }
    }

    /// The UID of the node as a core [`Uid`].
    pub fn node_uid(&self) -> Uid {
        Uid(self.uid)
    }

    /// The JSON text for HOOK_INSERT.
    pub fn to_json(&self) -> Result<String, EncodeError> {
        to_json_text(self, "hook")
    }
}

/// The JSON text for HOOK_INSERT with several hooks: a JSON array.
pub fn encode_hooks(hooks: &[Hook]) -> Result<String, EncodeError> {
    to_json_text(hooks, "hook list")
}

/// Decodes the HOOKS_DUMP reply: a JSON array of hooks.
pub fn decode_hooks_dump(bytes: &[u8]) -> Result<Vec<Hook>, DecodeError> {
    serde_json::from_slice(bytes).map_err(|err| DecodeError::Json {
        what: "hooks dump",
        message: err.to_string(),
    })
}

/// The payload of BREAKPOINT_UNLOCK.
///
/// JSON keys: `uid`, `desired_status` (a status name), `position` (an
/// integer) and `remove_when_done` (`serverLoop` of the publisher, case
/// `BREAKPOINT_UNLOCK`).
///
/// The publisher uses `desired_status` only if it is `"SUCCESS"` or
/// `"FAILURE"`: the node then returns that status and does not tick. Any
/// other value lets the node tick normally.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct UnlockRequest {
    /// The status to return, or SKIPPED to tick the node normally.
    pub desired_status: NodeStatus,
    /// The position of the hook.
    pub position: HookPosition,
    /// Remove the hook after the node continues.
    pub remove_when_done: bool,
    /// The UID of the node.
    pub uid: u16,
}

impl UnlockRequest {
    /// Releases the node at a PRE breakpoint. The node then ticks normally.
    pub fn resume(uid: u16) -> UnlockRequest {
        UnlockRequest {
            desired_status: NodeStatus::Skipped,
            position: HookPosition::Pre,
            remove_when_done: false,
            uid,
        }
    }

    /// Releases the node at a PRE breakpoint. The node returns `status`
    /// without a tick if `status` is SUCCESS or FAILURE.
    pub fn with_status(uid: u16, status: NodeStatus) -> UnlockRequest {
        UnlockRequest {
            desired_status: status,
            ..UnlockRequest::resume(uid)
        }
    }

    /// The JSON text for BREAKPOINT_UNLOCK.
    pub fn to_json(&self) -> Result<String, EncodeError> {
        to_json_text(self, "unlock request")
    }
}

/// The payload of HOOK_REMOVE: JSON keys `uid` and `position`
/// (`serverLoop` of the publisher, case `HOOK_REMOVE`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct RemoveRequest {
    /// The position of the hook.
    pub position: HookPosition,
    /// The UID of the node.
    pub uid: u16,
}

impl RemoveRequest {
    /// The JSON text for HOOK_REMOVE.
    pub fn to_json(&self) -> Result<String, EncodeError> {
        to_json_text(self, "remove request")
    }
}

fn to_json_text<T: Serialize + ?Sized>(
    value: &T,
    what: &'static str,
) -> Result<String, EncodeError> {
    serde_json::to_string(value).map_err(|err| EncodeError::Json {
        what,
        message: err.to_string(),
    })
}

// ---------------------------------------------------------------------------
// STATUS

/// The size of one node record in the STATUS reply: UID (`u16` LE) and one
/// status byte (the constructor of the publisher).
pub const STATUS_RECORD_SIZE: usize = 3;

/// The status of one node in a [`StatusSnapshot`].
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct NodeStatusEntry {
    /// The UID of the node.
    pub uid: Uid,
    /// The byte on the wire.
    pub raw: RawStatus,
    /// The status now.
    pub current: NodeStatus,
    /// The status before the node went back to IDLE, if the byte tells it.
    pub previous: Option<NodeStatus>,
}

/// The decoded STATUS reply: the status of each node, in the order of the
/// publisher.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct StatusSnapshot {
    /// One entry for each node.
    pub entries: Vec<NodeStatusEntry>,
}

impl StatusSnapshot {
    /// Decodes the STATUS reply: 3 bytes for each node.
    ///
    /// The status byte is decoded with [`RawStatus::decode`]. A byte that
    /// BT.CPP does not send gives [`DecodeError::UnknownStatus`].
    pub fn decode(bytes: &[u8]) -> Result<StatusSnapshot, DecodeError> {
        if !bytes.len().is_multiple_of(STATUS_RECORD_SIZE) {
            return Err(DecodeError::RecordSize {
                what: "status reply",
                len: bytes.len(),
                record_size: STATUS_RECORD_SIZE,
            });
        }
        let entries = bytes
            .as_chunks::<STATUS_RECORD_SIZE>()
            .0
            .iter()
            .map(|record| {
                let uid = u16::from_le_bytes([record[0], record[1]]);
                let raw = RawStatus(record[2]);
                let decoded = raw
                    .decode()
                    .ok_or(DecodeError::UnknownStatus { uid, byte: raw.0 })?;
                Ok(NodeStatusEntry {
                    uid: Uid(uid),
                    raw,
                    current: decoded.current,
                    previous: decoded.previous,
                })
            })
            .collect::<Result<Vec<_>, DecodeError>>()?;
        Ok(StatusSnapshot { entries })
    }

    /// Encodes the snapshot in the format of the STATUS reply.
    pub fn encode(&self) -> Vec<u8> {
        let mut out = Vec::with_capacity(self.entries.len() * STATUS_RECORD_SIZE);
        for entry in &self.entries {
            out.extend_from_slice(&entry.uid.0.to_le_bytes());
            out.push(entry.raw.0);
        }
        out
    }

    /// The entry of the node `uid`.
    pub fn get(&self, uid: Uid) -> Option<&NodeStatusEntry> {
        self.entries.iter().find(|entry| entry.uid == uid)
    }

    /// The number of nodes.
    pub fn len(&self) -> usize {
        self.entries.len()
    }

    /// True if the snapshot has no nodes.
    pub fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }
}

// ---------------------------------------------------------------------------
// Recording

/// The size of one record in the GET_TRANSITIONS reply
/// (`serverLoop` of the publisher, case `GET_TRANSITIONS`).
pub const TRANSITION_RECORD_SIZE: usize = 9;

/// The publisher keeps only this number of transitions between two
/// GET_TRANSITIONS requests (`callback` of the publisher). A batch of this
/// size can mean that older transitions are lost.
pub const RECORDING_BUFFER_CAPACITY: usize = 1000;

/// The largest timestamp that 6 bytes can hold.
pub const MAX_TRANSITION_TIMESTAMP_US: u64 = (1 << 48) - 1;

/// One status change that the publisher recorded.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct RecordedTransition {
    /// Microseconds since the start of the recording.
    pub timestamp_us: u64,
    /// The UID of the node.
    pub uid: Uid,
    /// The new status.
    pub status: NodeStatus,
}

/// Decodes the GET_TRANSITIONS reply: 9-byte records with a 6-byte
/// timestamp (LE, microseconds since the start of the recording), the UID
/// (`u16` LE) and the new status (`u8`, 0 to 4).
pub fn decode_transitions(bytes: &[u8]) -> Result<Vec<RecordedTransition>, DecodeError> {
    if !bytes.len().is_multiple_of(TRANSITION_RECORD_SIZE) {
        return Err(DecodeError::RecordSize {
            what: "transitions reply",
            len: bytes.len(),
            record_size: TRANSITION_RECORD_SIZE,
        });
    }
    bytes
        .as_chunks::<TRANSITION_RECORD_SIZE>()
        .0
        .iter()
        .map(|record| {
            let mut time = [0u8; 8];
            time[..6].copy_from_slice(&record[..6]);
            let uid = u16::from_le_bytes([record[6], record[7]]);
            let status = NodeStatus::from_u8(record[8]).ok_or(DecodeError::UnknownStatus {
                uid,
                byte: record[8],
            })?;
            Ok(RecordedTransition {
                timestamp_us: u64::from_le_bytes(time),
                uid: Uid(uid),
                status,
            })
        })
        .collect()
}

/// Encodes transitions in the format of the GET_TRANSITIONS reply.
///
/// Only the low 48 bits of each timestamp are kept, as in BT.CPP.
pub fn encode_transitions(transitions: &[RecordedTransition]) -> Vec<u8> {
    let mut out = Vec::with_capacity(transitions.len() * TRANSITION_RECORD_SIZE);
    for transition in transitions {
        out.extend_from_slice(&transition.timestamp_us.to_le_bytes()[..6]);
        out.extend_from_slice(&transition.uid.0.to_le_bytes());
        out.push(transition.status.as_u8());
    }
    out
}

/// The second part of a TOGGLE_RECORDING request.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum RecordingCommand {
    /// Start a recording (`"start"`). The reply has the start time.
    Start,
    /// Stop the recording (`"stop"`).
    Stop,
}

impl RecordingCommand {
    /// The text on the wire.
    pub fn as_str(self) -> &'static str {
        match self {
            RecordingCommand::Start => "start",
            RecordingCommand::Stop => "stop",
        }
    }
}

/// Decodes the second part of the reply to TOGGLE_RECORDING `"start"`: the
/// system time in microseconds since the Unix epoch, as decimal text.
pub fn decode_recording_start(bytes: &[u8]) -> Result<u64, DecodeError> {
    let text = std::str::from_utf8(bytes).map_err(|_| DecodeError::Utf8 {
        what: "recording start time",
    })?;
    parse_decimal(text, "recording start time")
}

fn parse_decimal<T: std::str::FromStr>(text: &str, what: &'static str) -> Result<T, DecodeError> {
    let valid = !text.is_empty() && text.bytes().all(|byte| byte.is_ascii_digit());
    valid
        .then(|| text.parse().ok())
        .flatten()
        .ok_or_else(|| DecodeError::Number {
            what,
            text: text.to_owned(),
        })
}

// ---------------------------------------------------------------------------
// BLACKBOARD

/// The separator of the blackboard names in a BLACKBOARD request
/// (`generateBlackboardsDump` of the publisher uses `splitString(bb_list, ';')`).
pub const BLACKBOARD_NAME_SEPARATOR: char = ';';

/// Makes the second part of a BLACKBOARD request: the names joined with
/// `;`.
///
/// A blackboard name is the instance path of a SubTree, or the tree ID for
/// the root tree. A name that contains `;` gives an error.
pub fn encode_blackboard_names<S: AsRef<str>>(names: &[S]) -> Result<String, EncodeError> {
    let mut out = String::new();
    for (index, name) in names.iter().enumerate() {
        let name = name.as_ref();
        if name.contains(BLACKBOARD_NAME_SEPARATOR) {
            return Err(EncodeError::BlackboardNameSeparator(name.to_owned()));
        }
        if index > 0 {
            out.push(BLACKBOARD_NAME_SEPARATOR);
        }
        out.push_str(name);
    }
    Ok(out)
}

/// The deepest nesting of arrays and maps that [`decode_blackboard_reply`]
/// accepts (the top-level map counts as 1). The decoder is recursive, so a
/// limit protects the stack of the calling thread from malformed replies.
pub const BLACKBOARD_MAX_NESTING: usize = 128;

/// The depth budget of `rmpv` (`read_value_with_max_depth`): each nesting
/// level uses 2 units (the value and its content), and a leaf string or
/// binary value uses up to 3 more.
const RMPV_MAX_DEPTH: usize = 2 * BLACKBOARD_MAX_NESTING + 4;

/// Decodes the BLACKBOARD reply.
///
/// The reply is `nlohmann::json::to_msgpack` of an object that maps each
/// blackboard name that the publisher found to the output of
/// `ExportBlackboardToJSON` (`generateBlackboardsDump` of the publisher).
/// If no name was found, the publisher sends MessagePack `nil`; this
/// function then returns an empty object. The result is always a JSON
/// object. See [`msgpack_to_json`] for the conversion rules.
pub fn decode_blackboard_reply(bytes: &[u8]) -> Result<serde_json::Value, DecodeError> {
    let mut reader = bytes;
    let value = rmpv::decode::read_value_with_max_depth(&mut reader, RMPV_MAX_DEPTH)
        .map_err(|err| DecodeError::MsgPack(err.to_string()))?;
    if !reader.is_empty() {
        return Err(DecodeError::TrailingBytes(reader.len()));
    }
    match value {
        rmpv::Value::Nil => Ok(serde_json::Value::Object(serde_json::Map::new())),
        rmpv::Value::Map(_) => Ok(msgpack_to_json(&value)),
        other => Err(DecodeError::BlackboardShape(msgpack_kind(&other))),
    }
}

fn msgpack_kind(value: &rmpv::Value) -> &'static str {
    match value {
        rmpv::Value::Nil => "nil",
        rmpv::Value::Boolean(_) => "a boolean",
        rmpv::Value::Integer(_) => "an integer",
        rmpv::Value::F32(_) | rmpv::Value::F64(_) => "a float",
        rmpv::Value::String(_) => "a string",
        rmpv::Value::Binary(_) => "binary data",
        rmpv::Value::Array(_) => "an array",
        rmpv::Value::Map(_) => "a map",
        rmpv::Value::Ext(..) => "an extension value",
    }
}

/// Converts a MessagePack value to a JSON value. The conversion never fails:
///
/// - `nil` becomes `null`, booleans and integers stay the same.
/// - Floats become JSON numbers. NaN and infinity become `null`.
/// - Strings that are not UTF-8 are converted with replacement characters.
/// - Binary data becomes an array of byte values.
/// - An extension value becomes `{"ext_type": <type>, "data": [<bytes>]}`.
/// - A map key that is not a string becomes its JSON text (for example the
///   integer key `5` becomes `"5"`). If a key occurs again, the last value
///   is kept.
pub fn msgpack_to_json(value: &rmpv::Value) -> serde_json::Value {
    use serde_json::Value as Json;
    match value {
        rmpv::Value::Nil => Json::Null,
        rmpv::Value::Boolean(flag) => Json::Bool(*flag),
        rmpv::Value::Integer(integer) => {
            if let Some(unsigned) = integer.as_u64() {
                Json::from(unsigned)
            } else if let Some(signed) = integer.as_i64() {
                Json::from(signed)
            } else {
                Json::Null
            }
        }
        rmpv::Value::F32(number) => float_to_json(f64::from(*number)),
        rmpv::Value::F64(number) => float_to_json(*number),
        rmpv::Value::String(text) => Json::String(utf8_lossy(text)),
        rmpv::Value::Binary(bytes) => bytes_to_json(bytes),
        rmpv::Value::Array(items) => Json::Array(items.iter().map(msgpack_to_json).collect()),
        rmpv::Value::Map(pairs) => {
            let mut map = serde_json::Map::with_capacity(pairs.len());
            for (key, item) in pairs {
                map.insert(map_key(key), msgpack_to_json(item));
            }
            Json::Object(map)
        }
        rmpv::Value::Ext(ext_type, data) => {
            let mut map = serde_json::Map::new();
            map.insert("ext_type".to_owned(), Json::from(*ext_type));
            map.insert("data".to_owned(), bytes_to_json(data));
            Json::Object(map)
        }
    }
}

fn float_to_json(number: f64) -> serde_json::Value {
    serde_json::Number::from_f64(number)
        .map(serde_json::Value::Number)
        .unwrap_or(serde_json::Value::Null)
}

fn bytes_to_json(bytes: &[u8]) -> serde_json::Value {
    serde_json::Value::Array(
        bytes
            .iter()
            .map(|byte| serde_json::Value::from(*byte))
            .collect(),
    )
}

fn utf8_lossy(text: &rmpv::Utf8String) -> String {
    match text.as_str() {
        Some(valid) => valid.to_owned(),
        None => String::from_utf8_lossy(text.as_bytes()).into_owned(),
    }
}

fn map_key(key: &rmpv::Value) -> String {
    match key {
        rmpv::Value::String(text) => utf8_lossy(text),
        other => msgpack_to_json(other).to_string(),
    }
}

// ---------------------------------------------------------------------------
// BREAKPOINT_REACHED

/// A BREAKPOINT_REACHED notification from the PUB socket of the publisher.
///
/// The publisher sends it each time an enabled hook runs, in breakpoint mode
/// and in replace mode (`insertHook` of the publisher).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct BreakpointNotification {
    /// The header. Its type is [`RequestType::BreakpointReached`].
    pub header: RequestHeader,
    /// The node that reached the hook.
    pub uid: Uid,
}

/// Decodes a BREAKPOINT_REACHED notification: part 1 is a 6-byte request
/// header with type `'N'`, part 2 is the node UID as decimal text.
pub fn decode_breakpoint_notification(
    parts: &[Vec<u8>],
) -> Result<BreakpointNotification, DecodeError> {
    let [header, uid] = parts else {
        return Err(ProtocolError::PartCount {
            request: RequestType::BreakpointReached,
            expected: 2,
            actual: parts.len(),
        }
        .into());
    };
    let header = RequestHeader::decode(header)?;
    header.check_protocol()?;
    if header.request_type != RequestType::BreakpointReached {
        return Err(ProtocolError::TypeMismatch {
            expected: RequestType::BreakpointReached,
            actual: header.request_type,
        }
        .into());
    }
    let text = std::str::from_utf8(uid).map_err(|_| DecodeError::Utf8 {
        what: "breakpoint node UID",
    })?;
    let uid: u16 = parse_decimal(text, "breakpoint node UID")?;
    Ok(BreakpointNotification {
        header,
        uid: Uid(uid),
    })
}

/// Encodes a BREAKPOINT_REACHED notification (as the publisher does).
pub fn encode_breakpoint_notification(header: &RequestHeader, uid: Uid) -> Vec<Vec<u8>> {
    vec![header.encode().to_vec(), uid.0.to_string().into_bytes()]
}

#[cfg(test)]
mod tests {
    use super::*;
    use proptest::prelude::*;
    use serde_json::json;

    #[test]
    fn hook_json_matches_btcpp_keys() {
        let hook = Hook::breakpoint(3);
        assert_eq!(
            hook.to_json().unwrap(),
            r#"{"desired_status":"SKIPPED","enabled":true,"mode":0,"once":false,"position":0,"uid":3}"#
        );
        let hook = Hook {
            desired_status: NodeStatus::Failure,
            enabled: false,
            mode: HookMode::Replace,
            once: true,
            position: HookPosition::Post,
            uid: 65535,
        };
        assert_eq!(
            hook.to_json().unwrap(),
            r#"{"desired_status":"FAILURE","enabled":false,"mode":1,"once":true,"position":1,"uid":65535}"#
        );
        assert_eq!(
            encode_hooks(&[Hook::breakpoint(1), Hook::replace(2, NodeStatus::Success)]).unwrap(),
            concat!(
                r#"[{"desired_status":"SKIPPED","enabled":true,"mode":0,"once":false,"position":0,"uid":1},"#,
                r#"{"desired_status":"SUCCESS","enabled":true,"mode":1,"once":false,"position":0,"uid":2}]"#
            )
        );
        assert_eq!(encode_hooks(&[]).unwrap(), "[]");
    }

    #[test]
    fn hook_json_from_btcpp_dump() {
        // The text that `json_out.dump()` makes for one hook.
        let text = br#"[{"desired_status":"SUCCESS","enabled":true,"mode":1,"once":false,"position":0,"uid":7}]"#;
        let hooks = decode_hooks_dump(text).unwrap();
        assert_eq!(hooks, vec![Hook::replace(7, NodeStatus::Success)]);
        assert_eq!(hooks[0].node_uid(), Uid(7));
        assert_eq!(decode_hooks_dump(b"[]").unwrap(), Vec::<Hook>::new());
        // Key order does not matter for decoding.
        let text = br#"[{"uid":7,"position":0,"once":false,"mode":1,"enabled":true,"desired_status":"SUCCESS"}]"#;
        assert_eq!(decode_hooks_dump(text).unwrap().len(), 1);
    }

    #[test]
    fn hooks_dump_errors() {
        for bad in [
            &b""[..],
            b"{",
            b"{}",
            b"[1]",
            b"\xff\xfe",
            br#"[{"desired_status":"SUCCESS","enabled":true,"mode":2,"once":false,"position":0,"uid":7}]"#,
            br#"[{"desired_status":"SUCCESS","enabled":true,"mode":0,"once":false,"position":5,"uid":7}]"#,
            br#"[{"desired_status":"success","enabled":true,"mode":0,"once":false,"position":0,"uid":7}]"#,
            br#"[{"desired_status":"SUCCESS","enabled":true,"mode":0,"once":false,"position":0,"uid":70000}]"#,
            br#"[{"desired_status":"SUCCESS","enabled":1,"mode":0,"once":false,"position":0,"uid":7}]"#,
            br#"[{"desired_status":"SUCCESS","enabled":true,"mode":0,"position":0,"uid":7}]"#,
            br#"[{"desired_status":"SUCCESS","enabled":true,"mode":"0","once":false,"position":0,"uid":7}]"#,
        ] {
            assert!(
                matches!(
                    decode_hooks_dump(bad),
                    Err(DecodeError::Json {
                        what: "hooks dump",
                        ..
                    })
                ),
                "{}",
                String::from_utf8_lossy(bad)
            );
        }
    }

    #[test]
    fn unlock_and_remove_json() {
        assert_eq!(
            UnlockRequest::resume(4).to_json().unwrap(),
            r#"{"desired_status":"SKIPPED","position":0,"remove_when_done":false,"uid":4}"#
        );
        let request = UnlockRequest {
            remove_when_done: true,
            ..UnlockRequest::with_status(9, NodeStatus::Failure)
        };
        assert_eq!(
            request.to_json().unwrap(),
            r#"{"desired_status":"FAILURE","position":0,"remove_when_done":true,"uid":9}"#
        );
        let back: UnlockRequest = serde_json::from_str(&request.to_json().unwrap()).unwrap();
        assert_eq!(back, request);
        let remove = RemoveRequest {
            position: HookPosition::Post,
            uid: 12,
        };
        assert_eq!(remove.to_json().unwrap(), r#"{"position":1,"uid":12}"#);
    }

    #[test]
    fn mode_and_position_integers() {
        assert_eq!(serde_json::to_string(&HookMode::Breakpoint).unwrap(), "0");
        assert_eq!(serde_json::to_string(&HookMode::Replace).unwrap(), "1");
        assert_eq!(serde_json::to_string(&HookPosition::Pre).unwrap(), "0");
        assert_eq!(serde_json::to_string(&HookPosition::Post).unwrap(), "1");
        assert_eq!(HookMode::from_i64(-1), None);
        assert_eq!(HookPosition::from_i64(2), None);
        assert!(serde_json::from_str::<HookMode>("1.5").is_err());
        assert!(serde_json::from_str::<HookPosition>("true").is_err());
    }

    #[test]
    fn status_fixed_bytes() {
        let bytes = [1, 0, 1, 2, 0, 12, 0x34, 0x12, 4];
        let snapshot = StatusSnapshot::decode(&bytes).unwrap();
        assert_eq!(snapshot.len(), 3);
        assert_eq!(snapshot.entries[0].uid, Uid(1));
        assert_eq!(snapshot.entries[0].current, NodeStatus::Running);
        assert_eq!(snapshot.entries[0].previous, None);
        assert_eq!(snapshot.entries[1].current, NodeStatus::Idle);
        assert_eq!(snapshot.entries[1].previous, Some(NodeStatus::Success));
        assert_eq!(snapshot.entries[1].raw, RawStatus(12));
        let last = snapshot.get(Uid(0x1234)).unwrap();
        assert_eq!(last.current, NodeStatus::Skipped);
        assert!(snapshot.get(Uid(99)).is_none());
        assert_eq!(snapshot.encode(), bytes);
        assert!(StatusSnapshot::decode(&[]).unwrap().is_empty());
    }

    #[test]
    fn status_errors() {
        assert_eq!(
            StatusSnapshot::decode(&[1, 0]),
            Err(DecodeError::RecordSize {
                what: "status reply",
                len: 2,
                record_size: 3
            })
        );
        assert!(matches!(
            StatusSnapshot::decode(&[1, 0, 0, 2]),
            Err(DecodeError::RecordSize { len: 4, .. })
        ));
        assert_eq!(
            StatusSnapshot::decode(&[1, 0, 0, 5, 0, 10]),
            Err(DecodeError::UnknownStatus { uid: 5, byte: 10 })
        );
        assert_eq!(
            StatusSnapshot::decode(&[7, 0, 5]),
            Err(DecodeError::UnknownStatus { uid: 7, byte: 5 })
        );
        assert!(StatusSnapshot::decode(&[7, 0, 255]).is_err());
    }

    #[test]
    fn transitions_fixed_bytes() {
        let bytes = [
            0x01, 0x02, 0x03, 0x04, 0x05, 0x06, 0x0a, 0x00, 0x02, //
            0xff, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff, 0x03,
        ];
        let transitions = decode_transitions(&bytes).unwrap();
        assert_eq!(
            transitions,
            vec![
                RecordedTransition {
                    timestamp_us: 0x0605_0403_0201,
                    uid: Uid(10),
                    status: NodeStatus::Success,
                },
                RecordedTransition {
                    timestamp_us: MAX_TRANSITION_TIMESTAMP_US,
                    uid: Uid(65535),
                    status: NodeStatus::Failure,
                },
            ]
        );
        assert_eq!(encode_transitions(&transitions), bytes);
        assert!(decode_transitions(&[]).unwrap().is_empty());
        // Only 48 bits are kept.
        let wide = RecordedTransition {
            timestamp_us: (1 << 48) + 5,
            uid: Uid(1),
            status: NodeStatus::Idle,
        };
        assert_eq!(
            decode_transitions(&encode_transitions(&[wide])).unwrap()[0].timestamp_us,
            5
        );
    }

    #[test]
    fn transitions_errors() {
        assert_eq!(
            decode_transitions(&[0; 8]),
            Err(DecodeError::RecordSize {
                what: "transitions reply",
                len: 8,
                record_size: 9
            })
        );
        assert!(decode_transitions(&[0; 10]).is_err());
        assert_eq!(
            decode_transitions(&[0, 0, 0, 0, 0, 0, 3, 0, 12]),
            Err(DecodeError::UnknownStatus { uid: 3, byte: 12 })
        );
    }

    #[test]
    fn recording_start_time() {
        assert_eq!(
            decode_recording_start(b"1760000000123456"),
            Ok(1_760_000_000_123_456)
        );
        assert_eq!(decode_recording_start(b"0"), Ok(0));
        for bad in [
            &b""[..],
            b" 1",
            b"1 ",
            b"-5",
            b"+5",
            b"12a",
            b"99999999999999999999999",
        ] {
            assert!(
                matches!(decode_recording_start(bad), Err(DecodeError::Number { .. })),
                "{bad:?}"
            );
        }
        assert_eq!(
            decode_recording_start(b"\xff1"),
            Err(DecodeError::Utf8 {
                what: "recording start time"
            })
        );
        assert_eq!(RecordingCommand::Start.as_str(), "start");
        assert_eq!(RecordingCommand::Stop.as_str(), "stop");
    }

    #[test]
    fn blackboard_names() {
        assert_eq!(
            encode_blackboard_names(&["MainTree", "sub/inner", "x::3"]).unwrap(),
            "MainTree;sub/inner;x::3"
        );
        assert_eq!(encode_blackboard_names::<&str>(&[]).unwrap(), "");
        assert_eq!(
            encode_blackboard_names(&["a".to_owned(), "b;c".to_owned()]),
            Err(EncodeError::BlackboardNameSeparator("b;c".to_owned()))
        );
    }

    fn msgpack(value: &rmpv::Value) -> Vec<u8> {
        let mut out = Vec::new();
        rmpv::encode::write_value(&mut out, value).unwrap();
        out
    }

    #[test]
    fn blackboard_reply_fixed_bytes() {
        // nlohmann::json::to_msgpack of {"MainTree":{"answer":42,"name":"x"}}.
        let bytes = [
            0x81, 0xa8, b'M', b'a', b'i', b'n', b'T', b'r', b'e', b'e', 0x82, 0xa6, b'a', b'n',
            b's', b'w', b'e', b'r', 0x2a, 0xa4, b'n', b'a', b'm', b'e', 0xa1, b'x',
        ];
        let value = decode_blackboard_reply(&bytes).unwrap();
        assert_eq!(value, json!({"MainTree": {"answer": 42, "name": "x"}}));
        assert_eq!(
            value.to_string(),
            r#"{"MainTree":{"answer":42,"name":"x"}}"#
        );
        // No blackboard found: nlohmann writes nil.
        assert_eq!(decode_blackboard_reply(&[0xc0]).unwrap(), json!({}));
    }

    #[test]
    fn blackboard_reply_all_msgpack_variants() {
        use rmpv::Value as M;
        let inner = M::Map(vec![
            (M::from("nil"), M::Nil),
            (M::from("bool"), M::Boolean(true)),
            (M::from("neg"), M::from(-5i64)),
            (M::from("big"), M::from(u64::MAX)),
            (M::from("min"), M::from(i64::MIN)),
            (M::from("f32"), M::F32(0.5)),
            (M::from("f64"), M::F64(1.25)),
            (M::from("nan"), M::F64(f64::NAN)),
            (M::from("inf"), M::F32(f32::INFINITY)),
            (M::from("ninf"), M::F64(f64::NEG_INFINITY)),
            (M::from("bin"), M::Binary(vec![0, 255])),
            (M::from("ext"), M::Ext(5, vec![1, 2])),
            (M::from("arr"), M::Array(vec![M::from(1), M::from("a")])),
            (
                M::from("text"),
                M::String(rmpv::Utf8String::from(String::from("ok"))),
            ),
            (M::from(7), M::from("int key")),
            (M::Nil, M::from("nil key")),
            (M::Boolean(false), M::from("bool key")),
            (M::Array(vec![M::from(1)]), M::from("array key")),
            (M::F64(2.5), M::from("float key")),
        ]);
        let outer = M::Map(vec![(M::from("bb"), inner)]);
        let value = decode_blackboard_reply(&msgpack(&outer)).unwrap();
        assert_eq!(
            value,
            json!({"bb": {
                "nil": null,
                "bool": true,
                "neg": -5,
                "big": u64::MAX,
                "min": i64::MIN,
                "f32": 0.5,
                "f64": 1.25,
                "nan": null,
                "inf": null,
                "ninf": null,
                "bin": [0, 255],
                "ext": {"ext_type": 5, "data": [1, 2]},
                "arr": [1, "a"],
                "text": "ok",
                "7": "int key",
                "null": "nil key",
                "false": "bool key",
                "[1]": "array key",
                "2.5": "float key",
            }})
        );
    }

    #[test]
    fn blackboard_reply_invalid_utf8_and_duplicates() {
        // fixmap(2): "k" -> str(0xff 0x41), "k" -> 1. Keys: invalid UTF-8 key.
        let bytes = [
            0x83, 0xa1, b'k', 0xa2, 0xff, b'A', 0xa1, b'k', 0x01, 0xa1, 0xfe, 0x02,
        ];
        let value = decode_blackboard_reply(&bytes).unwrap();
        assert_eq!(value, json!({"k": 1, "\u{fffd}": 2}));
        // Invalid UTF-8 in a value.
        let bytes = [0x81, 0xa1, b'k', 0xa2, 0xff, b'A'];
        assert_eq!(
            decode_blackboard_reply(&bytes).unwrap(),
            json!({"k": "\u{fffd}A"})
        );
    }

    #[test]
    fn blackboard_reply_errors() {
        assert!(matches!(
            decode_blackboard_reply(&[]),
            Err(DecodeError::MsgPack(_))
        ));
        // A map with 1 pair and no content.
        assert!(matches!(
            decode_blackboard_reply(&[0x81]),
            Err(DecodeError::MsgPack(_))
        ));
        // A string that claims 10 bytes but has 1.
        assert!(matches!(
            decode_blackboard_reply(&[0x81, 0xaa, b'x']),
            Err(DecodeError::MsgPack(_))
        ));
        // A huge array length with no data must not allocate or panic.
        assert!(matches!(
            decode_blackboard_reply(&[0xdd, 0xff, 0xff, 0xff, 0xff]),
            Err(DecodeError::MsgPack(_)) | Err(DecodeError::BlackboardShape(_))
        ));
        // A str8 marker without its length byte.
        assert!(matches!(
            decode_blackboard_reply(&[0xd9]),
            Err(DecodeError::MsgPack(_))
        ));
        assert_eq!(
            decode_blackboard_reply(&[0xc0, 0x00]),
            Err(DecodeError::TrailingBytes(1))
        );
        assert_eq!(
            decode_blackboard_reply(&[0x05]),
            Err(DecodeError::BlackboardShape("an integer"))
        );
        assert_eq!(
            decode_blackboard_reply(&[0x90]),
            Err(DecodeError::BlackboardShape("an array"))
        );
        // Very deep nesting gives an error, not a stack overflow.
        let deep = vec![0x91u8; 100_000];
        assert!(matches!(
            decode_blackboard_reply(&deep),
            Err(DecodeError::MsgPack(_))
        ));
        // Nesting up to the limit is accepted: a map and arrays, with a
        // string at the end.
        let nested = |levels: usize| {
            let mut bytes = vec![0x81, 0xa1, b'k'];
            bytes.extend(std::iter::repeat_n(0x91u8, levels - 1));
            bytes.extend_from_slice(&[0xa1, b'v']);
            bytes
        };
        let value = decode_blackboard_reply(&nested(BLACKBOARD_MAX_NESTING)).unwrap();
        assert!(value.get("k").is_some_and(serde_json::Value::is_array));
        assert!(matches!(
            decode_blackboard_reply(&nested(BLACKBOARD_MAX_NESTING + 1)),
            Err(DecodeError::MsgPack(_))
        ));
    }

    #[test]
    fn notification_fixed_bytes() {
        let header = RequestHeader::with_id(RequestType::BreakpointReached, 0xaabbccdd);
        let parts = vec![vec![2, b'N', 0xdd, 0xcc, 0xbb, 0xaa], b"42".to_vec()];
        assert_eq!(encode_breakpoint_notification(&header, Uid(42)), parts);
        let notification = decode_breakpoint_notification(&parts).unwrap();
        assert_eq!(notification.uid, Uid(42));
        assert_eq!(notification.header, header);
    }

    #[test]
    fn notification_errors() {
        let good_header = vec![2, b'N', 0, 0, 0, 0];
        let cases: Vec<Vec<Vec<u8>>> = vec![
            vec![],
            vec![good_header.clone()],
            vec![good_header.clone(), b"1".to_vec(), b"2".to_vec()],
            vec![vec![2, b'N', 0, 0, 0], b"1".to_vec()],
            vec![vec![3, b'N', 0, 0, 0, 0], b"1".to_vec()],
            vec![vec![2, b'S', 0, 0, 0, 0], b"1".to_vec()],
            vec![vec![2, 0, 0, 0, 0, 0], b"1".to_vec()],
            vec![good_header.clone(), b"".to_vec()],
            vec![good_header.clone(), b"65536".to_vec()],
            vec![good_header.clone(), b"-1".to_vec()],
            vec![good_header.clone(), b"1x".to_vec()],
            vec![good_header.clone(), vec![0xff]],
        ];
        for parts in cases {
            assert!(decode_breakpoint_notification(&parts).is_err(), "{parts:?}");
        }
        assert_eq!(
            decode_breakpoint_notification(&[good_header]),
            Err(DecodeError::Protocol(ProtocolError::PartCount {
                request: RequestType::BreakpointReached,
                expected: 2,
                actual: 1
            }))
        );
    }

    fn any_status() -> impl Strategy<Value = NodeStatus> {
        (0u8..5).prop_map(|value| NodeStatus::from_u8(value).unwrap_or_default())
    }

    fn any_mode() -> impl Strategy<Value = HookMode> {
        prop_oneof![Just(HookMode::Breakpoint), Just(HookMode::Replace)]
    }

    fn any_position() -> impl Strategy<Value = HookPosition> {
        prop_oneof![Just(HookPosition::Pre), Just(HookPosition::Post)]
    }

    prop_compose! {
        fn any_hook()(
            desired_status in any_status(),
            enabled in any::<bool>(),
            mode in any_mode(),
            once in any::<bool>(),
            position in any_position(),
            uid in any::<u16>(),
        ) -> Hook {
            Hook { desired_status, enabled, mode, once, position, uid }
        }
    }

    proptest! {
        #[test]
        fn hook_json_round_trip(hooks in proptest::collection::vec(any_hook(), 0..8)) {
            let text = encode_hooks(&hooks).unwrap();
            prop_assert_eq!(decode_hooks_dump(text.as_bytes()).unwrap(), hooks.clone());
            for hook in &hooks {
                let single = hook.to_json().unwrap();
                let value: serde_json::Value = serde_json::from_str(&single).unwrap();
                // Every field that `from_json` reads is present, with the right type.
                prop_assert!(value["enabled"].is_boolean());
                prop_assert!(value["once"].is_boolean());
                prop_assert!(value["uid"].is_u64());
                prop_assert!(value["mode"].is_i64());
                prop_assert!(value["position"].is_i64());
                prop_assert!(value["desired_status"].is_string());
                prop_assert_eq!(value.as_object().map(|map| map.len()), Some(6));
            }
        }

        #[test]
        fn status_round_trip(records in proptest::collection::vec((any::<u16>(), prop_oneof![0u8..5, 11u8..15]), 0..50)) {
            let mut bytes = Vec::new();
            for (uid, byte) in &records {
                bytes.extend_from_slice(&uid.to_le_bytes());
                bytes.push(*byte);
            }
            let snapshot = StatusSnapshot::decode(&bytes).unwrap();
            prop_assert_eq!(snapshot.len(), records.len());
            prop_assert_eq!(snapshot.encode(), bytes);
        }

        #[test]
        fn transitions_round_trip(records in proptest::collection::vec((0u64..=MAX_TRANSITION_TIMESTAMP_US, any::<u16>(), any_status()), 0..50)) {
            let transitions: Vec<RecordedTransition> = records
                .into_iter()
                .map(|(timestamp_us, uid, status)| RecordedTransition { timestamp_us, uid: Uid(uid), status })
                .collect();
            let bytes = encode_transitions(&transitions);
            prop_assert_eq!(bytes.len(), transitions.len() * TRANSITION_RECORD_SIZE);
            prop_assert_eq!(decode_transitions(&bytes).unwrap(), transitions);
        }

        #[test]
        fn decoders_never_panic(bytes in proptest::collection::vec(any::<u8>(), 0..64)) {
            let _ = StatusSnapshot::decode(&bytes);
            let _ = decode_transitions(&bytes);
            let _ = decode_recording_start(&bytes);
            let _ = decode_blackboard_reply(&bytes);
            let _ = decode_hooks_dump(&bytes);
            let _ = decode_breakpoint_notification(&[bytes.clone(), bytes.clone()]);
        }

        #[test]
        fn blackboard_names_round_trip(names in proptest::collection::vec("[^;]{1,10}", 0..5)) {
            let text = encode_blackboard_names(&names).unwrap();
            let split: Vec<&str> = if text.is_empty() { Vec::new() } else { text.split(';').collect() };
            prop_assert_eq!(split, names.iter().map(String::as_str).collect::<Vec<_>>());
        }

        #[test]
        fn notification_round_trip(uid in any::<u16>(), id in any::<u32>()) {
            let header = RequestHeader::with_id(RequestType::BreakpointReached, id);
            let parts = encode_breakpoint_notification(&header, Uid(uid));
            prop_assert_eq!(decode_breakpoint_notification(&parts).unwrap().uid, Uid(uid));
        }
    }
}
