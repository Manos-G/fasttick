//! Wire format of the publisher protocol: request types, request headers,
//! reply headers and the tree UUID.
//!
//! All functions in this module are pure. They do not open sockets.
//!
//! The format comes from the protocol header of BT.CPP 4.9.0
//! (`RequestType`, `RequestHeader`, `ReplyHeader`, `SerializeHeader`,
//! `DeserializeRequestHeader`, `DeserializeReplyHeader`) and from
//! `serverLoop` of the publisher.
//!
//! Each request and each reply is a multipart ZeroMQ message:
//!
//! - Part 1 of a request is a 6-byte [`RequestHeader`]: the protocol ID
//!   (`u8`, value 2), the request type (`u8`, an ASCII letter) and a unique
//!   ID (`u32`, little-endian).
//! - Part 1 of a normal reply is a 22-byte [`ReplyHeader`]: the request
//!   header (with the same type and ID) and a 16-byte [`TreeUuid`].
//! - An error reply has two parts: the text `error` and a message. It has no
//!   reply header.

use std::collections::hash_map::RandomState;
use std::fmt;
use std::hash::{BuildHasher, Hasher};
use std::str::FromStr;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{SystemTime, UNIX_EPOCH};

/// The protocol ID that BT.CPP 4.x uses (`kProtocolID` in the protocol
/// header).
pub const PROTOCOL_ID: u8 = 2;

/// The size of a serialized [`RequestHeader`] in bytes
/// (`RequestHeader::size()` in the protocol header).
pub const REQUEST_HEADER_SIZE: usize = 6;

/// The size of a [`TreeUuid`] in bytes (`TreeUniqueUUID` in the protocol
/// header).
pub const TREE_UUID_SIZE: usize = 16;

/// The size of a serialized [`ReplyHeader`] in bytes
/// (`ReplyHeader::size()` in the protocol header).
pub const REPLY_HEADER_SIZE: usize = REQUEST_HEADER_SIZE + TREE_UUID_SIZE;

/// The default port of the publisher (its constructor).
/// The publisher also binds a PUB socket on this port plus one.
pub const DEFAULT_PORT: u16 = 1667;

/// The first part of an error reply (`sendErrorReply` in
/// `serverLoop` of the publisher).
pub const ERROR_REPLY_TAG: &[u8] = b"error";

/// The type of a request (`BT::Monitor::RequestType`).
///
/// The numeric value of each variant is the byte on the wire.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
#[repr(u8)]
pub enum RequestType {
    /// Get the tree as XML (`FULLTREE`, `'T'`).
    FullTree = b'T',
    /// Get the status of all nodes (`STATUS`, `'S'`).
    Status = b'S',
    /// Get the values of a set of blackboards (`BLACKBOARD`, `'B'`).
    Blackboard = b'B',
    /// Insert or change a hook (`HOOK_INSERT`, `'I'`).
    HookInsert = b'I',
    /// Remove a hook (`HOOK_REMOVE`, `'R'`).
    HookRemove = b'R',
    /// Notification that a node reached a breakpoint
    /// (`BREAKPOINT_REACHED`, `'N'`). The publisher sends it on the PUB
    /// socket. It is not a request.
    BreakpointReached = b'N',
    /// Release a node that waits at a breakpoint (`BREAKPOINT_UNLOCK`, `'U'`).
    BreakpointUnlock = b'U',
    /// Get the installed hooks as JSON (`HOOKS_DUMP`, `'D'`).
    HooksDump = b'D',
    /// Remove all hooks (`REMOVE_ALL_HOOKS`, `'A'`).
    RemoveAllHooks = b'A',
    /// Disable all hooks (`DISABLE_ALL_HOOKS`, `'X'`).
    DisableAllHooks = b'X',
    /// Start or stop the recording of transitions (`TOGGLE_RECORDING`, `'r'`).
    ToggleRecording = b'r',
    /// Get the recorded transitions (`GET_TRANSITIONS`, `'t'`).
    GetTransitions = b't',
}

impl RequestType {
    /// All request types, in the order of the protocol header.
    pub const ALL: [RequestType; 12] = [
        RequestType::FullTree,
        RequestType::Status,
        RequestType::Blackboard,
        RequestType::HookInsert,
        RequestType::HookRemove,
        RequestType::BreakpointReached,
        RequestType::BreakpointUnlock,
        RequestType::HooksDump,
        RequestType::RemoveAllHooks,
        RequestType::DisableAllHooks,
        RequestType::ToggleRecording,
        RequestType::GetTransitions,
    ];

    /// Converts a wire byte to a request type. Returns `None` for a byte
    /// that is not a known type.
    pub fn from_byte(byte: u8) -> Option<RequestType> {
        RequestType::ALL
            .into_iter()
            .find(|request_type| request_type.as_byte() == byte)
    }

    /// The wire byte of this request type.
    pub fn as_byte(self) -> u8 {
        self as u8
    }

    /// The name that BT.CPP gives to this type (`ToString` in
    /// the protocol header), for example `"full_tree"`.
    pub fn name(self) -> &'static str {
        match self {
            RequestType::FullTree => "full_tree",
            RequestType::Status => "status",
            RequestType::Blackboard => "blackboard",
            RequestType::HookInsert => "hook_insert",
            RequestType::HookRemove => "hook_remove",
            RequestType::BreakpointReached => "breakpoint_reached",
            RequestType::BreakpointUnlock => "breakpoint_unlock",
            RequestType::RemoveAllHooks => "hooks_remove_all",
            RequestType::HooksDump => "hooks_dump",
            RequestType::DisableAllHooks => "disable_hooks",
            RequestType::ToggleRecording => "toggle_recording",
            RequestType::GetTransitions => "get_transitions",
        }
    }
}

impl fmt::Display for RequestType {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.name())
    }
}

/// An error in the framing of a message: a header, the number of parts, or
/// a reply that does not match its request.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum ProtocolError {
    /// A header has the wrong size.
    #[error("the {what} has {actual} bytes, expected {expected}")]
    HeaderSize {
        /// The kind of header.
        what: &'static str,
        /// The correct size.
        expected: usize,
        /// The size that was received.
        actual: usize,
    },
    /// The type byte is not a known request type.
    #[error("unknown request type byte 0x{0:02x}")]
    UnknownRequestType(u8),
    /// The protocol ID is not the expected value.
    #[error("protocol ID {actual}, expected {expected}")]
    ProtocolMismatch {
        /// The expected protocol ID.
        expected: u8,
        /// The received protocol ID.
        actual: u8,
    },
    /// The reply has a different type than the request.
    #[error("reply type {actual}, expected {expected}")]
    TypeMismatch {
        /// The type of the request.
        expected: RequestType,
        /// The type in the reply.
        actual: RequestType,
    },
    /// The reply has a different ID than the request. This is usually an
    /// out-of-date reply.
    #[error("reply ID {actual}, expected {expected}")]
    IdMismatch {
        /// The ID of the request.
        expected: u32,
        /// The ID in the reply.
        actual: u32,
    },
    /// The message has no parts.
    #[error("the message has no parts")]
    EmptyMessage,
    /// The message has the wrong number of parts.
    #[error("the {request} message has {actual} parts, expected {expected}")]
    PartCount {
        /// The type of the message.
        request: RequestType,
        /// The correct number of parts.
        expected: usize,
        /// The number of parts that was received.
        actual: usize,
    },
}

/// The header of a request (part 1 of each request message).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct RequestHeader {
    /// The protocol ID. It is [`PROTOCOL_ID`] for BT.CPP 4.x.
    pub protocol: u8,
    /// The request type.
    pub request_type: RequestType,
    /// A random ID. The reply must contain the same ID.
    pub unique_id: u32,
}

impl RequestHeader {
    /// Makes a header with the current protocol ID and a random ID.
    pub fn new(request_type: RequestType) -> RequestHeader {
        RequestHeader::with_id(request_type, random_request_id())
    }

    /// Makes a header with the current protocol ID and the given ID.
    pub fn with_id(request_type: RequestType, unique_id: u32) -> RequestHeader {
        RequestHeader {
            protocol: PROTOCOL_ID,
            request_type,
            unique_id,
        }
    }

    /// Encodes the header in 6 bytes.
    ///
    /// The order is: protocol (`u8`), type (`u8`), unique ID (`u32`
    /// little-endian). This is the order of `SerializeHeader` in
    /// the protocol header. BT.CPP copies the `u32` with `memcpy` in the
    /// byte order of the host, which is little-endian on all supported
    /// targets.
    pub fn encode(&self) -> [u8; REQUEST_HEADER_SIZE] {
        let id = self.unique_id.to_le_bytes();
        [
            self.protocol,
            self.request_type.as_byte(),
            id[0],
            id[1],
            id[2],
            id[3],
        ]
    }

    /// Decodes a 6-byte header (`DeserializeRequestHeader`).
    ///
    /// The function does not check the protocol ID. Use
    /// [`ReplyHeader::validate`] or [`RequestHeader::check_protocol`] for
    /// that.
    pub fn decode(bytes: &[u8]) -> Result<RequestHeader, ProtocolError> {
        let bytes: &[u8; REQUEST_HEADER_SIZE] =
            bytes.try_into().map_err(|_| ProtocolError::HeaderSize {
                what: "request header",
                expected: REQUEST_HEADER_SIZE,
                actual: bytes.len(),
            })?;
        Self::decode_array(bytes)
    }

    fn decode_array(bytes: &[u8; REQUEST_HEADER_SIZE]) -> Result<RequestHeader, ProtocolError> {
        let [protocol, type_byte, a, b, c, d] = *bytes;
        let request_type = RequestType::from_byte(type_byte)
            .ok_or(ProtocolError::UnknownRequestType(type_byte))?;
        Ok(RequestHeader {
            protocol,
            request_type,
            unique_id: u32::from_le_bytes([a, b, c, d]),
        })
    }

    /// Returns an error if the protocol ID is not [`PROTOCOL_ID`].
    pub fn check_protocol(&self) -> Result<(), ProtocolError> {
        if self.protocol == PROTOCOL_ID {
            Ok(())
        } else {
            Err(ProtocolError::ProtocolMismatch {
                expected: PROTOCOL_ID,
                actual: self.protocol,
            })
        }
    }
}

/// The 16-byte random ID that the publisher makes when its server thread
/// starts (`CreateRandomUUID` in the publisher source).
///
/// Each reply contains it. A new value means that the executor restarted.
/// [`fmt::Display`] gives the standard `8-4-4-4-12` hexadecimal form, with
/// the bytes in wire order.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord, Default)]
pub struct TreeUuid(pub [u8; TREE_UUID_SIZE]);

impl TreeUuid {
    /// The bytes in wire order.
    pub fn as_bytes(&self) -> &[u8; TREE_UUID_SIZE] {
        &self.0
    }
}

impl fmt::Display for TreeUuid {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        for (index, byte) in self.0.iter().enumerate() {
            if matches!(index, 4 | 6 | 8 | 10) {
                f.write_str("-")?;
            }
            write!(f, "{byte:02x}")?;
        }
        Ok(())
    }
}

/// Error for a text that is not a UUID in the `8-4-4-4-12` form.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
#[error("invalid tree UUID text: {0:?}")]
pub struct ParseTreeUuidError(pub String);

impl FromStr for TreeUuid {
    type Err = ParseTreeUuidError;

    /// Parses the `8-4-4-4-12` hexadecimal form. Upper and lower case are
    /// accepted.
    fn from_str(text: &str) -> Result<TreeUuid, ParseTreeUuidError> {
        let error = || ParseTreeUuidError(text.to_owned());
        let bytes = text.as_bytes();
        if bytes.len() != 36 {
            return Err(error());
        }
        let mut out = [0u8; TREE_UUID_SIZE];
        let mut digits = Vec::with_capacity(32);
        for (index, &byte) in bytes.iter().enumerate() {
            if matches!(index, 8 | 13 | 18 | 23) {
                if byte != b'-' {
                    return Err(error());
                }
                continue;
            }
            let digit = char::from(byte).to_digit(16).ok_or_else(error)?;
            digits.push(digit as u8);
        }
        for (slot, pair) in out.iter_mut().zip(digits.as_chunks::<2>().0) {
            *slot = (pair[0] << 4) | pair[1];
        }
        Ok(TreeUuid(out))
    }
}

impl serde::Serialize for TreeUuid {
    fn serialize<S: serde::Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        serializer.collect_str(self)
    }
}

impl<'de> serde::Deserialize<'de> for TreeUuid {
    fn deserialize<D: serde::Deserializer<'de>>(deserializer: D) -> Result<TreeUuid, D::Error> {
        let text = String::deserialize(deserializer)?;
        text.parse().map_err(serde::de::Error::custom)
    }
}

/// The header of a normal reply (part 1 of each reply message that is not
/// an error reply).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct ReplyHeader {
    /// A copy of the request header. The publisher sets the protocol ID to
    /// its own value.
    pub request: RequestHeader,
    /// The ID of the running tree.
    pub tree_uuid: TreeUuid,
}

impl ReplyHeader {
    /// Encodes the header in 22 bytes: the 6-byte request header, then the
    /// 16 UUID bytes (`SerializeHeader(const ReplyHeader&)`).
    pub fn encode(&self) -> [u8; REPLY_HEADER_SIZE] {
        let mut out = [0u8; REPLY_HEADER_SIZE];
        out[..REQUEST_HEADER_SIZE].copy_from_slice(&self.request.encode());
        out[REQUEST_HEADER_SIZE..].copy_from_slice(&self.tree_uuid.0);
        out
    }

    /// Decodes a 22-byte header (`DeserializeReplyHeader`).
    pub fn decode(bytes: &[u8]) -> Result<ReplyHeader, ProtocolError> {
        let bytes: &[u8; REPLY_HEADER_SIZE] =
            bytes.try_into().map_err(|_| ProtocolError::HeaderSize {
                what: "reply header",
                expected: REPLY_HEADER_SIZE,
                actual: bytes.len(),
            })?;
        let mut request = [0u8; REQUEST_HEADER_SIZE];
        request.copy_from_slice(&bytes[..REQUEST_HEADER_SIZE]);
        let mut uuid = [0u8; TREE_UUID_SIZE];
        uuid.copy_from_slice(&bytes[REQUEST_HEADER_SIZE..]);
        Ok(ReplyHeader {
            request: RequestHeader::decode_array(&request)?,
            tree_uuid: TreeUuid(uuid),
        })
    }

    /// Checks that this reply belongs to `request`: the protocol ID must be
    /// [`PROTOCOL_ID`], and the type and the ID must be equal to those of
    /// the request.
    pub fn validate(&self, request: &RequestHeader) -> Result<(), ProtocolError> {
        self.request.check_protocol()?;
        if self.request.request_type != request.request_type {
            return Err(ProtocolError::TypeMismatch {
                expected: request.request_type,
                actual: self.request.request_type,
            });
        }
        if self.request.unique_id != request.unique_id {
            return Err(ProtocolError::IdMismatch {
                expected: request.unique_id,
                actual: self.request.unique_id,
            });
        }
        Ok(())
    }
}

/// A reply, after the framing checks.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Reply {
    /// A normal reply.
    Ok {
        /// The validated reply header.
        header: ReplyHeader,
        /// The parts after the header.
        payload: Vec<Vec<u8>>,
    },
    /// An error reply. The publisher did not do the request.
    Error {
        /// The message of the publisher, for example `"Node ID not found"`.
        /// Bytes that are not UTF-8 are replaced.
        message: String,
    },
}

/// True if the parts are an error reply: the first part is the text
/// `error`.
///
/// A normal reply header has 22 bytes, so it cannot be equal to `error`.
pub fn is_error_reply(parts: &[Vec<u8>]) -> bool {
    parts
        .first()
        .is_some_and(|first| first.as_slice() == ERROR_REPLY_TAG)
}

/// Checks the framing of a reply to `request` and splits it into the header
/// and the payload.
///
/// An error reply gives [`Reply::Error`]. The message is the second part,
/// or an empty text if the part is missing.
pub fn parse_reply(request: &RequestHeader, parts: Vec<Vec<u8>>) -> Result<Reply, ProtocolError> {
    if is_error_reply(&parts) {
        let message = parts
            .get(1)
            .map(|bytes| String::from_utf8_lossy(bytes).into_owned())
            .unwrap_or_default();
        return Ok(Reply::Error { message });
    }
    let mut parts = parts.into_iter();
    let first = parts.next().ok_or(ProtocolError::EmptyMessage)?;
    let header = ReplyHeader::decode(&first)?;
    header.validate(request)?;
    Ok(Reply::Ok {
        header,
        payload: parts.collect(),
    })
}

/// Makes the two parts of an error reply (`sendErrorReply` in
/// `serverLoop` of the publisher).
pub fn encode_error_reply(message: &str) -> Vec<Vec<u8>> {
    vec![ERROR_REPLY_TAG.to_vec(), message.as_bytes().to_vec()]
}

/// Makes a random request ID.
///
/// It hashes a process-wide counter and the current time with the random
/// keys of [`RandomState`]. The IDs do not need to be secret. They must only
/// be different for consecutive requests.
pub fn random_request_id() -> u32 {
    static COUNTER: AtomicU64 = AtomicU64::new(0);
    let count = COUNTER.fetch_add(1, Ordering::Relaxed);
    let nanos = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|elapsed| elapsed.as_nanos())
        .unwrap_or_default();
    let mut hasher = RandomState::new().build_hasher();
    hasher.write_u64(count);
    hasher.write_u128(nanos);
    let hash = hasher.finish();
    // Fold the 64-bit hash into 32 bits.
    (hash ^ (hash >> 32)) as u32
}

#[cfg(test)]
mod tests {
    use super::*;
    use proptest::prelude::*;

    #[test]
    fn request_type_bytes_match_btcpp() {
        let expected: [(RequestType, u8); 12] = [
            (RequestType::FullTree, b'T'),
            (RequestType::Status, b'S'),
            (RequestType::Blackboard, b'B'),
            (RequestType::HookInsert, b'I'),
            (RequestType::HookRemove, b'R'),
            (RequestType::BreakpointReached, b'N'),
            (RequestType::BreakpointUnlock, b'U'),
            (RequestType::HooksDump, b'D'),
            (RequestType::RemoveAllHooks, b'A'),
            (RequestType::DisableAllHooks, b'X'),
            (RequestType::ToggleRecording, b'r'),
            (RequestType::GetTransitions, b't'),
        ];
        for (request_type, byte) in expected {
            assert_eq!(request_type.as_byte(), byte);
            assert_eq!(RequestType::from_byte(byte), Some(request_type));
        }
        assert_eq!(RequestType::from_byte(0), None);
        assert_eq!(RequestType::from_byte(b'Z'), None);
        assert_eq!(RequestType::RemoveAllHooks.to_string(), "hooks_remove_all");
        assert_eq!(RequestType::DisableAllHooks.name(), "disable_hooks");
    }

    #[test]
    fn request_header_fixed_bytes() {
        let header = RequestHeader::with_id(RequestType::Status, 0x1234_5678);
        assert_eq!(header.encode(), [2, b'S', 0x78, 0x56, 0x34, 0x12]);
        assert_eq!(
            RequestHeader::decode(&[2, b'S', 0x78, 0x56, 0x34, 0x12]),
            Ok(header)
        );
        let header = RequestHeader::with_id(RequestType::GetTransitions, u32::MAX);
        assert_eq!(header.encode(), [2, b't', 0xff, 0xff, 0xff, 0xff]);
    }

    #[test]
    fn request_header_errors() {
        assert_eq!(
            RequestHeader::decode(&[2, b'S', 0, 0, 0]),
            Err(ProtocolError::HeaderSize {
                what: "request header",
                expected: 6,
                actual: 5
            })
        );
        assert!(matches!(
            RequestHeader::decode(&[2, b'S', 0, 0, 0, 0, 0]),
            Err(ProtocolError::HeaderSize { actual: 7, .. })
        ));
        assert!(matches!(
            RequestHeader::decode(&[]),
            Err(ProtocolError::HeaderSize { actual: 0, .. })
        ));
        assert_eq!(
            RequestHeader::decode(&[2, b'?', 0, 0, 0, 0]),
            Err(ProtocolError::UnknownRequestType(b'?'))
        );
        // The protocol ID is decoded but not checked.
        let header = RequestHeader::decode(&[3, b'T', 1, 0, 0, 0]).unwrap();
        assert_eq!(header.protocol, 3);
        assert_eq!(
            header.check_protocol(),
            Err(ProtocolError::ProtocolMismatch {
                expected: 2,
                actual: 3
            })
        );
    }

    const SAMPLE_UUID: [u8; 16] = [
        0x12, 0x34, 0x56, 0x78, 0x9a, 0xbc, 0x4d, 0xef, 0x80, 0x01, 0x02, 0x03, 0x04, 0x05, 0x06,
        0x07,
    ];

    #[test]
    fn reply_header_fixed_bytes() {
        let header = ReplyHeader {
            request: RequestHeader::with_id(RequestType::FullTree, 7),
            tree_uuid: TreeUuid(SAMPLE_UUID),
        };
        let bytes = header.encode();
        let mut expected = vec![2, b'T', 7, 0, 0, 0];
        expected.extend_from_slice(&SAMPLE_UUID);
        assert_eq!(bytes.as_slice(), expected.as_slice());
        assert_eq!(ReplyHeader::decode(&bytes), Ok(header));
        assert!(matches!(
            ReplyHeader::decode(&bytes[..21]),
            Err(ProtocolError::HeaderSize {
                expected: 22,
                actual: 21,
                ..
            })
        ));
        assert!(matches!(
            ReplyHeader::decode(&bytes[..6]),
            Err(ProtocolError::HeaderSize { actual: 6, .. })
        ));
    }

    #[test]
    fn uuid_display_and_parse() {
        let uuid = TreeUuid(SAMPLE_UUID);
        let text = uuid.to_string();
        assert_eq!(text, "12345678-9abc-4def-8001-020304050607");
        assert_eq!(text.parse::<TreeUuid>(), Ok(uuid));
        assert_eq!(
            "12345678-9ABC-4DEF-8001-020304050607".parse::<TreeUuid>(),
            Ok(uuid)
        );
        assert_eq!(
            TreeUuid::default().to_string(),
            "00000000-0000-0000-0000-000000000000"
        );
        for bad in [
            "",
            "12345678-9abc-4def-8001-02030405060",
            "12345678-9abc-4def-8001-0203040506070",
            "12345678x9abc-4def-8001-020304050607",
            "1234567g-9abc-4def-8001-020304050607",
            "123456789abc4def8001020304050607",
            "12345678-9abc-4def-8001-02030405060\u{e9}",
        ] {
            assert!(bad.parse::<TreeUuid>().is_err(), "{bad:?}");
        }
        let json = serde_json::to_string(&uuid).unwrap();
        assert_eq!(json, "\"12345678-9abc-4def-8001-020304050607\"");
        assert_eq!(serde_json::from_str::<TreeUuid>(&json).unwrap(), uuid);
        assert!(serde_json::from_str::<TreeUuid>("\"nope\"").is_err());
    }

    fn reply_parts(header: &ReplyHeader, payload: &[&[u8]]) -> Vec<Vec<u8>> {
        let mut parts = vec![header.encode().to_vec()];
        parts.extend(payload.iter().map(|part| part.to_vec()));
        parts
    }

    #[test]
    fn parse_valid_reply() {
        let request = RequestHeader::with_id(RequestType::FullTree, 42);
        let header = ReplyHeader {
            request,
            tree_uuid: TreeUuid(SAMPLE_UUID),
        };
        let reply = parse_reply(&request, reply_parts(&header, &[b"<root/>"])).unwrap();
        assert_eq!(
            reply,
            Reply::Ok {
                header,
                payload: vec![b"<root/>".to_vec()]
            }
        );
    }

    #[test]
    fn parse_reply_checks_protocol_type_and_id() {
        let request = RequestHeader::with_id(RequestType::Status, 42);
        let good = ReplyHeader {
            request,
            tree_uuid: TreeUuid(SAMPLE_UUID),
        };

        let mut wrong_protocol = good;
        wrong_protocol.request.protocol = 1;
        assert_eq!(
            parse_reply(&request, reply_parts(&wrong_protocol, &[b""])),
            Err(ProtocolError::ProtocolMismatch {
                expected: 2,
                actual: 1
            })
        );

        let mut wrong_type = good;
        wrong_type.request.request_type = RequestType::FullTree;
        assert_eq!(
            parse_reply(&request, reply_parts(&wrong_type, &[b""])),
            Err(ProtocolError::TypeMismatch {
                expected: RequestType::Status,
                actual: RequestType::FullTree
            })
        );

        let mut wrong_id = good;
        wrong_id.request.unique_id = 41;
        assert_eq!(
            parse_reply(&request, reply_parts(&wrong_id, &[b""])),
            Err(ProtocolError::IdMismatch {
                expected: 42,
                actual: 41
            })
        );

        assert_eq!(
            parse_reply(&request, Vec::new()),
            Err(ProtocolError::EmptyMessage)
        );
        assert!(matches!(
            parse_reply(&request, vec![vec![1, 2, 3]]),
            Err(ProtocolError::HeaderSize { actual: 3, .. })
        ));
        let mut unknown = good.encode();
        unknown[1] = b'?';
        assert_eq!(
            parse_reply(&request, vec![unknown.to_vec()]),
            Err(ProtocolError::UnknownRequestType(b'?'))
        );
    }

    #[test]
    fn parse_error_reply() {
        let request = RequestHeader::with_id(RequestType::HookRemove, 1);
        assert_eq!(
            parse_reply(&request, encode_error_reply("Node ID not found")),
            Ok(Reply::Error {
                message: "Node ID not found".to_owned()
            })
        );
        assert_eq!(
            parse_reply(&request, vec![b"error".to_vec()]),
            Ok(Reply::Error {
                message: String::new()
            })
        );
        assert_eq!(
            parse_reply(&request, vec![b"error".to_vec(), vec![0xff, b'x']]),
            Ok(Reply::Error {
                message: "\u{fffd}x".to_owned()
            })
        );
        assert!(!is_error_reply(&[]));
        assert!(!is_error_reply(&[b"errors".to_vec()]));
    }

    #[test]
    fn random_ids_differ() {
        let ids: std::collections::HashSet<u32> = (0..1000).map(|_| random_request_id()).collect();
        // Collisions in 1000 random 32-bit values are very unlikely.
        assert!(ids.len() >= 999);
        let a = RequestHeader::new(RequestType::Status);
        assert_eq!(a.protocol, PROTOCOL_ID);
        assert_eq!(a.request_type, RequestType::Status);
    }

    proptest! {
        #[test]
        fn request_header_round_trip(index in 0usize..12, id in any::<u32>(), protocol in any::<u8>()) {
            let header = RequestHeader {
                protocol,
                request_type: RequestType::ALL[index],
                unique_id: id,
            };
            prop_assert_eq!(RequestHeader::decode(&header.encode()), Ok(header));
        }

        #[test]
        fn reply_header_round_trip(index in 0usize..12, id in any::<u32>(), uuid in any::<[u8; 16]>()) {
            let header = ReplyHeader {
                request: RequestHeader::with_id(RequestType::ALL[index], id),
                tree_uuid: TreeUuid(uuid),
            };
            prop_assert_eq!(ReplyHeader::decode(&header.encode()), Ok(header));
            prop_assert_eq!(header.tree_uuid.to_string().parse::<TreeUuid>(), Ok(header.tree_uuid));
        }

        #[test]
        fn decode_never_panics(bytes in proptest::collection::vec(any::<u8>(), 0..40)) {
            let _ = RequestHeader::decode(&bytes);
            let _ = ReplyHeader::decode(&bytes);
            let request = RequestHeader::with_id(RequestType::Status, 1);
            let _ = parse_reply(&request, vec![bytes.clone(), bytes]);
        }

        #[test]
        fn uuid_parse_never_panics(text in ".{0,40}") {
            let _ = text.parse::<TreeUuid>();
        }
    }
}
