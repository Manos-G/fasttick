//! Client for the live-debugging protocol served by the publisher of
//! BehaviorTree.CPP 4.x.
//!
//! The publisher binds two ZeroMQ sockets in the executor process:
//!
//! - a REP socket on the server port (default 1667) for requests and
//!   replies, and
//! - a PUB socket on the server port plus one for notifications.
//!
//! The crate has these modules:
//!
//! - [`wire`]: request types, request and reply headers, the tree UUID.
//! - [`messages`]: the payload of each request and reply (JSON, binary
//!   records and MessagePack).
//! - [`client`]: a blocking [`Client`] with one method for each request.
//! - [`session`]: a [`LiveSession`] that runs on its own thread. It polls,
//!   sends heartbeats, reconnects, detects a restart of the executor and
//!   cleans up the hooks.
//! - [`breakpoints`]: breakpoint files (`.ftbp.json`).
//!
//! The wire format comes from the protocol header and the publisher source
//! of BT.CPP 4.9.0.

pub mod breakpoints;
pub mod client;
pub mod messages;
pub mod session;
pub mod wire;

pub use breakpoints::{
    Breakpoint, BreakpointFileError, BreakpointSet, Resolution, ResolvedBreakpoint,
};
pub use client::{Client, ClientConfig, ClientError};
pub use messages::{
    BreakpointNotification, DecodeError, EncodeError, Hook, HookMode, HookPosition,
    NodeStatusEntry, RecordedTransition, RemoveRequest, StatusSnapshot, UnlockRequest,
};
pub use session::{
    ConnectionState, LiveSession, PollConfig, SessionCommand, SessionConfig, SessionEvent,
    SessionHandle,
};
pub use wire::{ProtocolError, ReplyHeader, RequestHeader, RequestType, TreeUuid};
