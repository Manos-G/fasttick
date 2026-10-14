//! A blocking client for the publisher protocol.
//!
//! [`Client`] has one method for each request type. Each method sends one
//! request on the REQ socket and waits for the reply, up to the request
//! timeout. Use the client on one background thread, for example through
//! [`crate::session::LiveSession`].
//!
//! The client uses the "lazy pirate" pattern: after a timeout or a bad
//! reply, a REQ socket cannot send again, so the client closes it and opens
//! a new one for the next request.
//!
//! The client sends only JSON that serde makes from typed structs. Bad JSON
//! causes an exception in the publisher thread of BT.CPP 4.9.0
//! (`nlohmann::json::parse` in `serverLoop` of the publisher), which can
//! stop the executor.

use std::fmt;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{Duration, Instant};

use crate::messages::{
    self, BreakpointNotification, DecodeError, EncodeError, Hook, HookPosition, RecordedTransition,
    RecordingCommand, RemoveRequest, StatusSnapshot, UnlockRequest,
};
use crate::wire::{self, DEFAULT_PORT, ProtocolError, Reply, RequestHeader, RequestType, TreeUuid};

/// The settings of a [`Client`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ClientConfig {
    /// The host name or IP address of the executor. An IPv6 address can be
    /// given with or without brackets.
    pub host: String,
    /// The server port of the publisher. The PUB socket is on `port + 1`.
    pub port: u16,
    /// The longest time to wait for a reply.
    pub request_timeout: Duration,
    /// The longest time to wait for a TCP connection
    /// (`ZMQ_CONNECT_TIMEOUT`).
    pub connect_timeout: Duration,
}

impl Default for ClientConfig {
    fn default() -> Self {
        ClientConfig {
            host: "127.0.0.1".to_owned(),
            port: DEFAULT_PORT,
            request_timeout: Duration::from_millis(1000),
            connect_timeout: Duration::from_millis(1000),
        }
    }
}

impl ClientConfig {
    /// The default settings for `host` and `port`.
    pub fn new(host: impl Into<String>, port: u16) -> ClientConfig {
        ClientConfig {
            host: host.into(),
            port,
            ..ClientConfig::default()
        }
    }

    /// The endpoint of the REQ socket, for example `tcp://127.0.0.1:1667`.
    pub fn request_endpoint(&self) -> Result<String, ClientError> {
        self.endpoint(self.port)
    }

    /// The endpoint of the SUB socket, for example `tcp://127.0.0.1:1668`.
    pub fn notification_endpoint(&self) -> Result<String, ClientError> {
        let port = self.port.checked_add(1).ok_or_else(|| {
            ClientError::InvalidConfig(format!(
                "port {} has no next port for the notification socket",
                self.port
            ))
        })?;
        self.endpoint(port)
    }

    fn host_for_endpoint(&self) -> Result<(String, bool), ClientError> {
        let host = self.host.trim();
        if host.is_empty() {
            return Err(ClientError::InvalidConfig("the host is empty".to_owned()));
        }
        if host.contains(|c: char| c.is_whitespace() || c == '/') {
            return Err(ClientError::InvalidConfig(format!(
                "the host {host:?} is not valid"
            )));
        }
        if host.starts_with('[') && host.ends_with(']') {
            Ok((host.to_owned(), true))
        } else if host.contains(':') {
            Ok((format!("[{host}]"), true))
        } else {
            Ok((host.to_owned(), false))
        }
    }

    fn endpoint(&self, port: u16) -> Result<String, ClientError> {
        let (host, _) = self.host_for_endpoint()?;
        Ok(format!("tcp://{host}:{port}"))
    }

    fn is_ipv6(&self) -> bool {
        self.host_for_endpoint().is_ok_and(|(_, ipv6)| ipv6)
    }
}

/// An error from a [`Client`] request.
#[derive(Debug, thiserror::Error)]
pub enum ClientError {
    /// No reply came before the request timeout. The client closed the REQ
    /// socket. The next request uses a new socket.
    #[error("the {request} request timed out")]
    Timeout {
        /// The type of the request.
        request: RequestType,
    },
    /// A ZeroMQ call failed.
    #[error("ZeroMQ error: {0}")]
    Zmq(#[from] zmq::Error),
    /// The reply has a wrong header or the wrong number of parts.
    #[error("protocol error: {0}")]
    Protocol(#[from] ProtocolError),
    /// The publisher sent an error reply.
    #[error("the executor replied with an error: {0}")]
    Server(String),
    /// The payload of the reply is not valid.
    #[error("cannot decode the reply: {0}")]
    Decode(#[from] DecodeError),
    /// The request payload cannot be made.
    #[error("cannot encode the request: {0}")]
    Encode(#[from] EncodeError),
    /// The configuration is not valid.
    #[error("invalid client configuration: {0}")]
    InvalidConfig(String),
}

impl ClientError {
    /// True for errors after which the connection is probably lost: a
    /// timeout or a ZeroMQ error.
    pub fn is_connection_error(&self) -> bool {
        matches!(self, ClientError::Timeout { .. } | ClientError::Zmq(_))
    }
}

/// The events that the client reads from the monitor of the notification
/// socket (`zmq_socket_monitor`).
const MONITOR_EVENTS: i32 = zmq::SocketEvent::HANDSHAKE_SUCCEEDED as i32
    | zmq::SocketEvent::DISCONNECTED as i32
    | zmq::SocketEvent::CLOSED as i32;

/// The SUB socket for notifications, with a socket monitor that tells if it
/// is connected.
struct NotificationSocket {
    socket: zmq::Socket,
    /// A PAIR socket that receives the monitor events of `socket`.
    monitor: zmq::Socket,
    /// True after a complete ZeroMQ handshake, false after a disconnect.
    connected: bool,
}

impl NotificationSocket {
    /// Reads the monitor events that arrived and updates `connected`.
    ///
    /// Each event has two parts. Part 1 has the event number (`u16`) and a
    /// value (`u32`), in the byte order of the host. Part 2 is the
    /// endpoint (see `zmq_socket_monitor`).
    fn update(&mut self) {
        // A SUB socket sends its subscription again after a reconnect only
        // when the thread that uses it handles its pending commands
        // (`session_base_t::reconnect` and `xsub_t::xhiccuped` in libzmq).
        // Reading ZMQ_EVENTS handles them. Without the subscription, the
        // PUB socket of the publisher drops all notifications.
        let _ = self.socket.get_events();
        loop {
            let parts = match self.monitor.recv_multipart(zmq::DONTWAIT) {
                Ok(parts) => parts,
                Err(_) => return,
            };
            let Some(event) = parts
                .first()
                .and_then(|part| part.get(..2))
                .map(|bytes| u16::from_ne_bytes([bytes[0], bytes[1]]))
            else {
                continue;
            };
            if event == zmq::SocketEvent::HANDSHAKE_SUCCEEDED.to_raw() {
                self.connected = true;
            } else if event == zmq::SocketEvent::DISCONNECTED.to_raw()
                || event == zmq::SocketEvent::CLOSED.to_raw()
            {
                self.connected = false;
            }
        }
    }
}

/// A blocking client for the publisher protocol. See the module
/// documentation.
pub struct Client {
    config: ClientConfig,
    context: zmq::Context,
    request_socket: Option<zmq::Socket>,
    notification_socket: Option<NotificationSocket>,
    last_tree_uuid: Option<TreeUuid>,
}

impl fmt::Debug for Client {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Client")
            .field("config", &self.config)
            .field("request_socket_open", &self.request_socket.is_some())
            .field(
                "notification_socket_open",
                &self.notification_socket.is_some(),
            )
            .field("last_tree_uuid", &self.last_tree_uuid)
            .finish()
    }
}

impl Client {
    /// Opens the sockets and connects them to the publisher.
    ///
    /// ZeroMQ connects in the background, so this function does not wait
    /// for the executor. The first request times out if the executor is not
    /// there.
    pub fn connect(config: ClientConfig) -> Result<Client, ClientError> {
        // Check the configuration before sockets are made.
        config.request_endpoint()?;
        config.notification_endpoint()?;
        let mut client = Client {
            config,
            context: zmq::Context::new(),
            request_socket: None,
            notification_socket: None,
            last_tree_uuid: None,
        };
        client.request_socket = Some(client.open_request_socket()?);
        client.notification_socket = Some(client.open_notification_socket()?);
        Ok(client)
    }

    /// The configuration.
    pub fn config(&self) -> &ClientConfig {
        &self.config
    }

    /// Changes the request timeout for the next requests.
    pub fn set_request_timeout(&mut self, timeout: Duration) {
        if self.config.request_timeout != timeout {
            self.config.request_timeout = timeout;
            // The send timeout is a socket option. Apply it to a new socket.
            self.request_socket = None;
        }
    }

    /// The tree UUID of the last valid reply, if any.
    pub fn last_tree_uuid(&self) -> Option<TreeUuid> {
        self.last_tree_uuid
    }

    /// Closes both sockets and opens new ones.
    pub fn reset(&mut self) -> Result<(), ClientError> {
        self.request_socket = None;
        self.notification_socket = None;
        self.request_socket = Some(self.open_request_socket()?);
        self.notification_socket = Some(self.open_notification_socket()?);
        Ok(())
    }

    /// Closes the notification socket and opens a new one. A new socket
    /// tries to connect at once. An old socket that lost its connection
    /// tries again only after the ZeroMQ reconnect interval (100 ms to
    /// 200 ms). Notifications that were not read are lost.
    pub fn reset_notifications(&mut self) -> Result<(), ClientError> {
        self.notification_socket = None;
        self.notification_socket = Some(self.open_notification_socket()?);
        Ok(())
    }

    /// True if the notification socket (SUB, `port + 1`) is connected to
    /// the publisher: the ZeroMQ handshake is complete. This call does not
    /// wait. It reads the events of a ZeroMQ socket monitor.
    ///
    /// Check it before a request that can make a node stop at a
    /// breakpoint. The publisher sends BREAKPOINT_REACHED only once for
    /// each stop, and its PUB socket drops the message if no subscriber is
    /// connected. ZeroMQ connects the REQ and the SUB socket independently,
    /// so after a restart of the executor the REQ socket can be connected
    /// some time before the SUB socket.
    pub fn notifications_connected(&mut self) -> bool {
        match self.notification_socket.as_mut() {
            Some(notifications) => {
                notifications.update();
                notifications.connected
            }
            None => false,
        }
    }

    /// FULLTREE: gets the tree as XML (`WriteTreeToXML(tree, true, true)`).
    pub fn full_tree(&mut self) -> Result<(TreeUuid, String), ClientError> {
        let (uuid, mut payload) = self.request(RequestType::FullTree, None, 1)?;
        let xml = payload.pop().unwrap_or_default();
        let xml = String::from_utf8(xml).map_err(|_| DecodeError::Utf8 { what: "tree XML" })?;
        Ok((uuid, xml))
    }

    /// STATUS: gets the status of all nodes.
    pub fn status(&mut self) -> Result<StatusSnapshot, ClientError> {
        let (_, payload) = self.request(RequestType::Status, None, 1)?;
        Ok(StatusSnapshot::decode(first(&payload))?)
    }

    /// BLACKBOARD: gets the content of the named blackboards.
    ///
    /// A name is the instance path of a SubTree, or the tree ID for the
    /// root tree (see `SubtreeInstance::blackboard_name` in
    /// `fasttick-core`). The result is a JSON object with one key for each
    /// name that the publisher found. A name that contains `;` gives
    /// [`ClientError::Encode`] and sends nothing.
    pub fn blackboards<S: AsRef<str>>(
        &mut self,
        names: &[S],
    ) -> Result<serde_json::Value, ClientError> {
        let text = messages::encode_blackboard_names(names)?;
        let (_, payload) = self.request(RequestType::Blackboard, Some(text.as_bytes()), 1)?;
        Ok(messages::decode_blackboard_reply(first(&payload))?)
    }

    /// HOOK_INSERT with one hook. If a hook for the node exists, the
    /// publisher changes it.
    ///
    /// The publisher replies with success also when the node does not
    /// exist (`insertHook` of the publisher returns false, and the reply
    /// ignores it).
    pub fn insert_hook(&mut self, hook: &Hook) -> Result<(), ClientError> {
        let text = hook.to_json()?;
        self.request(RequestType::HookInsert, Some(text.as_bytes()), 0)?;
        Ok(())
    }

    /// HOOK_INSERT with several hooks (a JSON array).
    pub fn insert_hooks(&mut self, hooks: &[Hook]) -> Result<(), ClientError> {
        let text = messages::encode_hooks(hooks)?;
        self.request(RequestType::HookInsert, Some(text.as_bytes()), 0)?;
        Ok(())
    }

    /// HOOK_REMOVE. The publisher replies with an error ("Node ID not
    /// found") if it has no hook for the node at this position.
    pub fn remove_hook(&mut self, uid: u16, position: HookPosition) -> Result<(), ClientError> {
        let text = RemoveRequest { position, uid }.to_json()?;
        self.request(RequestType::HookRemove, Some(text.as_bytes()), 0)?;
        Ok(())
    }

    /// BREAKPOINT_UNLOCK: releases a node that waits at a breakpoint. The
    /// publisher replies with an error if it has no hook for the node at
    /// this position.
    pub fn unlock_breakpoint(&mut self, request: &UnlockRequest) -> Result<(), ClientError> {
        let text = request.to_json()?;
        self.request(RequestType::BreakpointUnlock, Some(text.as_bytes()), 0)?;
        Ok(())
    }

    /// HOOKS_DUMP: gets the installed hooks. BT.CPP 4.9.0 lists only the
    /// PRE hooks.
    pub fn hooks_dump(&mut self) -> Result<Vec<Hook>, ClientError> {
        let (_, payload) = self.request(RequestType::HooksDump, None, 1)?;
        Ok(messages::decode_hooks_dump(first(&payload))?)
    }

    /// REMOVE_ALL_HOOKS. Blocked nodes continue.
    pub fn remove_all_hooks(&mut self) -> Result<(), ClientError> {
        self.request(RequestType::RemoveAllHooks, None, 0)?;
        Ok(())
    }

    /// DISABLE_ALL_HOOKS. Blocked nodes continue.
    pub fn disable_all_hooks(&mut self) -> Result<(), ClientError> {
        self.request(RequestType::DisableAllHooks, None, 0)?;
        Ok(())
    }

    /// TOGGLE_RECORDING `"start"`: starts a recording and clears the
    /// transition buffer. Returns the system time of the executor in
    /// microseconds since the Unix epoch.
    pub fn start_recording(&mut self) -> Result<u64, ClientError> {
        let command = RecordingCommand::Start.as_str().as_bytes();
        let (_, payload) = self.request(RequestType::ToggleRecording, Some(command), 1)?;
        Ok(messages::decode_recording_start(first(&payload))?)
    }

    /// TOGGLE_RECORDING `"stop"`: stops the recording. The transitions that
    /// are in the buffer stay available for [`Client::transitions`].
    pub fn stop_recording(&mut self) -> Result<(), ClientError> {
        let command = RecordingCommand::Stop.as_str().as_bytes();
        self.request(RequestType::ToggleRecording, Some(command), 0)?;
        Ok(())
    }

    /// GET_TRANSITIONS: gets and clears the recorded transitions. The
    /// publisher keeps only the last
    /// [`messages::RECORDING_BUFFER_CAPACITY`] transitions.
    pub fn transitions(&mut self) -> Result<Vec<RecordedTransition>, ClientError> {
        let (_, payload) = self.request(RequestType::GetTransitions, None, 1)?;
        Ok(messages::decode_transitions(first(&payload))?)
    }

    /// Waits up to `timeout` for a BREAKPOINT_REACHED notification on the
    /// SUB socket. Returns `Ok(None)` if no notification came.
    ///
    /// A malformed notification gives [`ClientError::Decode`]. The message
    /// is consumed, so the next call reads the next message.
    pub fn poll_notification(
        &mut self,
        timeout: Duration,
    ) -> Result<Option<BreakpointNotification>, ClientError> {
        if self.notification_socket.is_none() {
            self.notification_socket = Some(self.open_notification_socket()?);
        }
        let Some(socket) = self.notification_socket.as_ref().map(|n| &n.socket) else {
            return Ok(None);
        };
        if !wait_readable(socket, Instant::now() + timeout)? {
            return Ok(None);
        }
        let parts = match socket.recv_multipart(zmq::DONTWAIT) {
            Ok(parts) => parts,
            Err(zmq::Error::EAGAIN) => return Ok(None),
            Err(err) => {
                self.notification_socket = None;
                return Err(err.into());
            }
        };
        Ok(Some(messages::decode_breakpoint_notification(&parts)?))
    }

    /// Sends one request and returns the tree UUID and the payload parts of
    /// the reply. `payload_parts` is the number of parts after the header
    /// that the reply must have.
    fn request(
        &mut self,
        request_type: RequestType,
        payload: Option<&[u8]>,
        payload_parts: usize,
    ) -> Result<(TreeUuid, Vec<Vec<u8>>), ClientError> {
        let header = RequestHeader::new(request_type);
        let result = self.exchange(&header, payload);
        let parts = match result {
            Ok(parts) => parts,
            Err(err) => {
                // The REQ socket waits for a reply that will not come.
                self.request_socket = None;
                return Err(err);
            }
        };
        match wire::parse_reply(&header, parts) {
            Ok(Reply::Ok { header, payload }) => {
                self.last_tree_uuid = Some(header.tree_uuid);
                if payload.len() != payload_parts {
                    return Err(ProtocolError::PartCount {
                        request: request_type,
                        expected: payload_parts + 1,
                        actual: payload.len() + 1,
                    }
                    .into());
                }
                Ok((header.tree_uuid, payload))
            }
            Ok(Reply::Error { message }) => Err(ClientError::Server(message)),
            Err(err) => {
                // The reply is not for this request. Start again with a
                // new socket, so that no other old reply can arrive.
                self.request_socket = None;
                Err(err.into())
            }
        }
    }

    fn exchange(
        &mut self,
        header: &RequestHeader,
        payload: Option<&[u8]>,
    ) -> Result<Vec<Vec<u8>>, ClientError> {
        if self.request_socket.is_none() {
            self.request_socket = Some(self.open_request_socket()?);
        }
        let Some(socket) = self.request_socket.as_ref() else {
            return Err(ClientError::Zmq(zmq::Error::ENOTSOCK));
        };
        let deadline = Instant::now() + self.config.request_timeout;
        let timeout = || ClientError::Timeout {
            request: header.request_type,
        };
        let encoded = header.encode();
        let sent = match payload {
            Some(payload) => socket
                .send(&encoded[..], zmq::SNDMORE)
                .and_then(|()| socket.send(payload, 0)),
            None => socket.send(&encoded[..], 0),
        };
        match sent {
            Ok(()) => {}
            Err(zmq::Error::EAGAIN) => return Err(timeout()),
            Err(err) => return Err(err.into()),
        }
        if !wait_readable(socket, deadline)? {
            return Err(timeout());
        }
        match socket.recv_multipart(zmq::DONTWAIT) {
            Ok(parts) => Ok(parts),
            Err(zmq::Error::EAGAIN) => Err(timeout()),
            Err(err) => Err(err.into()),
        }
    }

    fn open_request_socket(&self) -> Result<zmq::Socket, ClientError> {
        let endpoint = self.config.request_endpoint()?;
        let socket = self.context.socket(zmq::REQ)?;
        self.configure(&socket)?;
        socket.set_sndtimeo(millis(self.config.request_timeout))?;
        socket.connect(&endpoint)?;
        Ok(socket)
    }

    fn open_notification_socket(&self) -> Result<NotificationSocket, ClientError> {
        static MONITOR_COUNT: AtomicU64 = AtomicU64::new(0);
        let endpoint = self.config.notification_endpoint()?;
        let socket = self.context.socket(zmq::SUB)?;
        self.configure(&socket)?;
        socket.set_subscribe(b"")?;
        // The monitor must be ready before the connect, so that no event is
        // lost. The name must be unique in the context.
        let monitor_endpoint = format!(
            "inproc://fasttick-notification-monitor-{}",
            MONITOR_COUNT.fetch_add(1, Ordering::Relaxed)
        );
        socket.monitor(&monitor_endpoint, MONITOR_EVENTS)?;
        let monitor = self.context.socket(zmq::PAIR)?;
        monitor.set_linger(0)?;
        monitor.connect(&monitor_endpoint)?;
        socket.connect(&endpoint)?;
        Ok(NotificationSocket {
            socket,
            monitor,
            connected: false,
        })
    }

    fn configure(&self, socket: &zmq::Socket) -> Result<(), ClientError> {
        socket.set_linger(0)?;
        socket.set_connect_timeout(millis(self.config.connect_timeout))?;
        if self.config.is_ipv6() {
            socket.set_ipv6(true)?;
        }
        Ok(())
    }
}

/// The first payload part. [`Client::request`] checks the number of parts,
/// so the part exists; an empty slice is a safe fallback.
fn first(payload: &[Vec<u8>]) -> &[u8] {
    payload.first().map(Vec::as_slice).unwrap_or_default()
}

/// Converts a duration to whole milliseconds for a socket option. A
/// duration that is not zero gives at least 1 ms.
fn millis(duration: Duration) -> i32 {
    let ms = duration.as_millis();
    if ms == 0 && !duration.is_zero() {
        1
    } else {
        i32::try_from(ms).unwrap_or(i32::MAX)
    }
}

/// Waits until the socket has a message to read, or until the deadline.
fn wait_readable(socket: &zmq::Socket, deadline: Instant) -> Result<bool, ClientError> {
    loop {
        let remaining = deadline.saturating_duration_since(Instant::now());
        match socket.poll(zmq::POLLIN, i64::from(millis(remaining))) {
            Ok(0) if remaining.is_zero() => return Ok(false),
            Ok(0) => {}
            Ok(_) => return Ok(true),
            Err(zmq::Error::EINTR) => {}
            Err(err) => return Err(err.into()),
        }
        if Instant::now() >= deadline {
            // One last check without waiting.
            return match socket.poll(zmq::POLLIN, 0) {
                Ok(count) => Ok(count > 0),
                Err(zmq::Error::EINTR) => Ok(false),
                Err(err) => Err(err.into()),
            };
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn endpoints() {
        let config = ClientConfig::new("localhost", 1667);
        assert_eq!(config.request_endpoint().unwrap(), "tcp://localhost:1667");
        assert_eq!(
            config.notification_endpoint().unwrap(),
            "tcp://localhost:1668"
        );
        let config = ClientConfig::new("::1", 2000);
        assert_eq!(config.request_endpoint().unwrap(), "tcp://[::1]:2000");
        assert!(config.is_ipv6());
        let config = ClientConfig::new("[fe80::1]", 2000);
        assert_eq!(
            config.notification_endpoint().unwrap(),
            "tcp://[fe80::1]:2001"
        );
        let config = ClientConfig::new(" 10.0.0.2 ", 5);
        assert_eq!(config.request_endpoint().unwrap(), "tcp://10.0.0.2:5");
        assert!(!config.is_ipv6());
    }

    #[test]
    fn invalid_configs() {
        let config = ClientConfig::new("localhost", u16::MAX);
        assert!(config.request_endpoint().is_ok());
        assert!(matches!(
            config.notification_endpoint(),
            Err(ClientError::InvalidConfig(_))
        ));
        assert!(matches!(
            Client::connect(config),
            Err(ClientError::InvalidConfig(_))
        ));
        for host in ["", "   ", "a b", "x/y"] {
            assert!(matches!(
                Client::connect(ClientConfig::new(host, 1667)),
                Err(ClientError::InvalidConfig(_))
            ));
        }
    }

    #[test]
    fn millis_rounding() {
        assert_eq!(millis(Duration::ZERO), 0);
        assert_eq!(millis(Duration::from_micros(10)), 1);
        assert_eq!(millis(Duration::from_millis(1500)), 1500);
        assert_eq!(millis(Duration::from_secs(u64::MAX)), i32::MAX);
    }

    #[test]
    fn timeout_without_server() {
        // Nothing listens on this port (connect is lazy), so the request
        // times out and the client stays usable.
        let config = ClientConfig {
            request_timeout: Duration::from_millis(50),
            ..ClientConfig::new("127.0.0.1", 9)
        };
        let mut client = Client::connect(config).unwrap();
        let started = Instant::now();
        assert!(matches!(
            client.status(),
            Err(ClientError::Timeout {
                request: RequestType::Status
            })
        ));
        assert!(started.elapsed() < Duration::from_secs(2));
        assert!(matches!(
            client.full_tree(),
            Err(ClientError::Timeout { .. })
        ));
        assert!(client.last_tree_uuid().is_none());
        assert!(
            client
                .poll_notification(Duration::from_millis(5))
                .unwrap()
                .is_none()
        );
        client.reset().unwrap();
        assert!(format!("{client:?}").contains("Client"));
    }

    #[test]
    fn encode_error_sends_nothing() {
        let config = ClientConfig {
            request_timeout: Duration::from_millis(50),
            ..ClientConfig::new("127.0.0.1", 9)
        };
        let mut client = Client::connect(config).unwrap();
        assert!(matches!(
            client.blackboards(&["a;b"]),
            Err(ClientError::Encode(EncodeError::BlackboardNameSeparator(_)))
        ));
    }

    #[test]
    fn connection_error_classification() {
        assert!(
            ClientError::Timeout {
                request: RequestType::Status
            }
            .is_connection_error()
        );
        assert!(ClientError::Zmq(zmq::Error::ETERM).is_connection_error());
        assert!(!ClientError::Server("x".to_owned()).is_connection_error());
    }
}
