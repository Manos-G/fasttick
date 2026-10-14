//! A live debugging session that runs on its own thread.
//!
//! [`LiveSession::spawn`] starts the thread. The caller (usually the GUI)
//! sends [`SessionCommand`]s through a [`SessionHandle`] and reads
//! [`SessionEvent`]s from a channel. No call blocks the caller.
//!
//! The session does this work:
//!
//! - It polls STATUS at the poll interval. It sends a [`SessionEvent::Status`]
//!   only when the snapshot changes.
//! - It sends at least one request in each heartbeat interval. The publisher
//!   disables all hooks after 5 s without a request
//!   (`heartbeatLoop` of the publisher).
//! - It reads BREAKPOINT_REACHED notifications from the PUB socket.
//! - It detects a restart of the executor: each reply contains the tree
//!   UUID, and a new UUID means a new executor. The session then gets the
//!   tree again, sends [`SessionEvent::ExecutorRestarted`] and inserts its
//!   hooks again. If the tree changed, the hooks move to the new UIDs by
//!   node path. A hook whose node path is not in the new tree is dropped
//!   with a [`SessionEvent::Warning`]: its old UID can belong to another
//!   node now.
//! - After a timeout, it reconnects automatically with a backoff from
//!   100 ms to 2 s.
//! - During a recording, it reads GET_TRANSITIONS every 100 ms.
//! - On disconnect and on shutdown, it stops the recording (and reads the
//!   last transitions), then removes all hooks (best effort, with a short
//!   timeout).
//!
//! Malformed and out-of-date replies give a [`SessionEvent::Warning`]. The
//! session continues.

use std::collections::{BTreeMap, BTreeSet};
use std::fmt;
use std::panic::{self, AssertUnwindSafe};
use std::sync::Arc;
use std::sync::mpsc::{self, Receiver, RecvTimeoutError, Sender, TryRecvError};
use std::thread::{self, JoinHandle};
use std::time::{Duration, Instant};

use fasttick_core::runtime::{RuntimeTree, Uid};
use fasttick_core::status::NodeStatus;

use crate::breakpoints::{Breakpoint, BreakpointSet, Resolution};
use crate::client::{Client, ClientConfig, ClientError};
use crate::messages::{
    self, Hook, HookMode, HookPosition, RECORDING_BUFFER_CAPACITY, RecordedTransition,
    StatusSnapshot, UnlockRequest,
};
use crate::wire::TreeUuid;

/// The shortest poll interval that the session accepts.
pub const MIN_POLL_INTERVAL: Duration = Duration::from_millis(10);

/// The longest time that the session waits on the notification socket
/// before it looks for new commands.
const COMMAND_CHECK_INTERVAL: Duration = Duration::from_millis(5);

/// The most notifications that the session reads in one pass.
const MAX_NOTIFICATIONS_PER_PASS: usize = 256;

/// How often the session polls.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct PollConfig {
    /// The interval between two STATUS requests (at least
    /// [`MIN_POLL_INTERVAL`]). Default: 100 ms.
    pub status_interval: Duration,
    /// The longest time without a request. Default: 1 s. Keep it well
    /// below the heartbeat timeout of the publisher (5 s by default).
    pub heartbeat_interval: Duration,
    /// The interval between two GET_TRANSITIONS requests during a
    /// recording. Default: 100 ms.
    pub transitions_interval: Duration,
}

impl Default for PollConfig {
    fn default() -> Self {
        PollConfig {
            status_interval: Duration::from_millis(100),
            heartbeat_interval: Duration::from_secs(1),
            transitions_interval: Duration::from_millis(100),
        }
    }
}

/// The settings of a [`LiveSession`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SessionConfig {
    /// The poll intervals.
    pub poll: PollConfig,
    /// The longest time to wait for a reply. Default: 1 s.
    pub request_timeout: Duration,
    /// The longest time to wait for a TCP connection. Default: 1 s.
    pub connect_timeout: Duration,
    /// The delay before the first reconnect attempt. The delay doubles
    /// after each failed attempt. Default: 100 ms.
    pub reconnect_initial_delay: Duration,
    /// The longest delay between two reconnect attempts. Default: 2 s.
    pub reconnect_max_delay: Duration,
    /// Insert the hooks of this session again after a reconnect and after
    /// a restart of the executor. Default: true.
    pub reinsert_hooks: bool,
    /// The request timeout for the cleanup on disconnect and on shutdown.
    /// Default: 300 ms.
    pub cleanup_timeout: Duration,
}

impl Default for SessionConfig {
    fn default() -> Self {
        SessionConfig {
            poll: PollConfig::default(),
            request_timeout: Duration::from_secs(1),
            connect_timeout: Duration::from_secs(1),
            reconnect_initial_delay: Duration::from_millis(100),
            reconnect_max_delay: Duration::from_secs(2),
            reinsert_hooks: true,
            cleanup_timeout: Duration::from_millis(300),
        }
    }
}

/// A command for the session thread.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SessionCommand {
    /// Connect to the publisher at `host` and `port` (the server port; the
    /// notification port is `port + 1`). A current connection is closed
    /// first.
    Connect {
        /// The host name or IP address.
        host: String,
        /// The server port, usually [`crate::wire::DEFAULT_PORT`].
        port: u16,
    },
    /// Remove all hooks, stop the recording and close the connection.
    Disconnect,
    /// Change the STATUS poll interval (at least [`MIN_POLL_INTERVAL`]).
    SetPollInterval(Duration),
    /// Read these blackboards now and then at each `interval`. An empty
    /// list stops the reading. A zero interval reads them once.
    WatchBlackboards {
        /// The blackboard names (see `SubtreeInstance::blackboard_name`).
        names: Vec<String>,
        /// The interval between two reads.
        interval: Duration,
    },
    /// Get the tree again.
    RefreshTree,
    /// Insert a hook or change it. If the node waits at a breakpoint and
    /// the hook is disabled, the session releases the node first.
    InsertHook(Hook),
    /// Remove a hook.
    RemoveHook {
        /// The UID of the node.
        uid: u16,
        /// The position of the hook.
        position: HookPosition,
    },
    /// Release a node that waits at a breakpoint.
    Unlock(UnlockRequest),
    /// Get the installed hooks. The result is a [`SessionEvent::Hooks`].
    DumpHooks,
    /// Remove all hooks (also the hooks of other clients).
    RemoveAllHooks,
    /// Disable all hooks (also the hooks of other clients).
    DisableAllHooks,
    /// Start a recording of transitions.
    StartRecording,
    /// Stop the recording.
    StopRecording,
    /// Disconnect and stop the session thread.
    Shutdown,
}

/// The state of the connection.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum ConnectionState {
    /// No connection.
    Disconnected,
    /// The first attempt to reach the executor is in progress.
    Connecting,
    /// The executor replies.
    Connected {
        /// The UUID of the running tree.
        uuid: TreeUuid,
    },
    /// The executor did not reply. The session tries again.
    Reconnecting {
        /// The number of the next attempt, starting at 1.
        attempt: u32,
    },
}

/// An event from the session thread.
#[derive(Debug, Clone, PartialEq)]
pub enum SessionEvent {
    /// The connection state changed.
    State(ConnectionState),
    /// The tree of the executor.
    Tree {
        /// The UUID of the running tree.
        uuid: TreeUuid,
        /// The parsed tree.
        tree: Arc<RuntimeTree>,
    },
    /// The node statuses changed.
    Status {
        /// The new statuses.
        snapshot: StatusSnapshot,
    },
    /// The content of the watched blackboards (a JSON object with one key
    /// for each blackboard name that the executor knows).
    Blackboards(serde_json::Value),
    /// A node reached a hook. The publisher also sends this for hooks in
    /// replace mode.
    BreakpointReached {
        /// The UID of the node.
        uid: Uid,
    },
    /// The hooks that the executor has (reply to
    /// [`SessionCommand::DumpHooks`]).
    Hooks(Vec<Hook>),
    /// A recording started.
    RecordingStarted {
        /// The system time of the executor at the start, in microseconds
        /// since the Unix epoch.
        start_time_us: u64,
    },
    /// New recorded transitions.
    Transitions {
        /// The transitions, oldest first.
        batch: Vec<RecordedTransition>,
        /// True if the batch has [`RECORDING_BUFFER_CAPACITY`] or more
        /// transitions: the publisher keeps only the last 1000, so older
        /// ones can be lost.
        possible_loss: bool,
    },
    /// The recording stopped.
    RecordingStopped,
    /// The tree UUID changed: a new executor runs. A [`SessionEvent::Tree`]
    /// follows.
    ExecutorRestarted {
        /// The UUID before the restart.
        old: TreeUuid,
        /// The new UUID.
        new: TreeUuid,
    },
    /// A problem that the session survived, for example a malformed reply.
    Warning(String),
    /// A command failed.
    Error(String),
}

/// The control side of a [`LiveSession`].
///
/// Dropping the handle stops the session thread after the cleanup, but it
/// does not wait for the thread. Before the process exits, call
/// [`SessionHandle::join`] to make sure that the cleanup (stop the
/// recording, remove the hooks) is complete.
pub struct SessionHandle {
    commands: Sender<SessionCommand>,
    thread: Option<JoinHandle<()>>,
}

impl fmt::Debug for SessionHandle {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("SessionHandle")
            .field("finished", &self.is_finished())
            .finish()
    }
}

impl SessionHandle {
    /// Sends a command. Returns false if the session thread stopped.
    pub fn send(&self, command: SessionCommand) -> bool {
        self.commands.send(command).is_ok()
    }

    /// Sends [`SessionCommand::Connect`].
    pub fn connect(&self, host: impl Into<String>, port: u16) -> bool {
        self.send(SessionCommand::Connect {
            host: host.into(),
            port,
        })
    }

    /// Sends [`SessionCommand::Disconnect`].
    pub fn disconnect(&self) -> bool {
        self.send(SessionCommand::Disconnect)
    }

    /// Sends [`SessionCommand::Shutdown`]. The thread stops after the
    /// cleanup. This function does not wait.
    pub fn shutdown(&self) -> bool {
        self.send(SessionCommand::Shutdown)
    }

    /// True if the session thread stopped.
    pub fn is_finished(&self) -> bool {
        self.thread.as_ref().is_none_or(JoinHandle::is_finished)
    }

    /// Sends [`SessionCommand::Shutdown`] and waits for the thread to stop.
    /// The wait is short: the cleanup uses
    /// [`SessionConfig::cleanup_timeout`] for each request, and a running
    /// request ends after [`SessionConfig::request_timeout`]. Returns false
    /// if the thread stopped because of an internal error.
    pub fn join(mut self) -> bool {
        self.shutdown();
        match self.thread.take() {
            Some(thread) => thread.join().is_ok(),
            None => true,
        }
    }
}

impl Drop for SessionHandle {
    fn drop(&mut self) {
        let _ = self.commands.send(SessionCommand::Shutdown);
    }
}

/// The waker type: a function that the session calls after each event, for
/// example to request a repaint of the GUI.
pub type Waker = Box<dyn Fn() + Send>;

/// Blackboards to read again and again.
#[derive(Debug, Clone)]
struct BlackboardWatch {
    names: Vec<String>,
    interval: Duration,
    next_at: Instant,
}

/// Why a request failed.
enum Failure {
    /// The publisher sent an error reply. An event was sent already.
    Server,
    /// Any other failure. An event was sent already.
    Other,
}

/// The session thread. See the module documentation.
pub struct LiveSession {
    config: SessionConfig,
    commands: Receiver<SessionCommand>,
    events: Sender<SessionEvent>,
    waker: Option<Waker>,
    client: Option<Client>,
    state: ConnectionState,
    uuid: Option<TreeUuid>,
    tree_known: bool,
    tree: Option<Arc<RuntimeTree>>,
    /// The tree before a restart. The hooks of the session use its UIDs
    /// until the new tree arrives.
    tree_before_restart: Option<Arc<RuntimeTree>>,
    tree_refresh_at: Option<Instant>,
    rehook_pending: bool,
    hooks: BTreeMap<(u16, HookPosition), Hook>,
    hit: BTreeSet<u16>,
    recording: bool,
    watch: Option<BlackboardWatch>,
    last_status: Option<StatusSnapshot>,
    next_status_at: Instant,
    next_transitions_at: Instant,
    last_request_at: Instant,
    next_attempt_at: Instant,
    /// True while the notification socket fails. The session reports the
    /// failure once, not on each pass of the loop.
    notifications_failing: bool,
}

impl fmt::Debug for LiveSession {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("LiveSession")
            .field("config", &self.config)
            .field("state", &self.state)
            .field("hooks", &self.hooks)
            .field("recording", &self.recording)
            .finish_non_exhaustive()
    }
}

impl LiveSession {
    /// Starts a session thread.
    ///
    /// The session starts in the state [`ConnectionState::Disconnected`].
    /// Send [`SessionCommand::Connect`] to connect. The `waker` is called
    /// after each event.
    pub fn spawn(
        config: SessionConfig,
        waker: Option<Waker>,
    ) -> (SessionHandle, Receiver<SessionEvent>) {
        let (command_sender, command_receiver) = mpsc::channel();
        let (event_sender, event_receiver) = mpsc::channel();
        let fallback = event_sender.clone();
        let session = LiveSession::new(config, command_receiver, event_sender, waker);
        let thread = thread::Builder::new()
            .name("fasttick-live-session".to_owned())
            .spawn(move || session.run());
        let thread = match thread {
            Ok(thread) => Some(thread),
            Err(err) => {
                let _ = fallback.send(SessionEvent::Error(format!(
                    "cannot start the session thread: {err}"
                )));
                None
            }
        };
        (
            SessionHandle {
                commands: command_sender,
                thread,
            },
            event_receiver,
        )
    }

    fn new(
        config: SessionConfig,
        commands: Receiver<SessionCommand>,
        events: Sender<SessionEvent>,
        waker: Option<Waker>,
    ) -> LiveSession {
        let now = Instant::now();
        let mut config = config;
        config.poll.status_interval = config.poll.status_interval.max(MIN_POLL_INTERVAL);
        config.poll.heartbeat_interval = config.poll.heartbeat_interval.max(MIN_POLL_INTERVAL);
        config.poll.transitions_interval = config.poll.transitions_interval.max(MIN_POLL_INTERVAL);
        LiveSession {
            config,
            commands,
            events,
            waker,
            client: None,
            state: ConnectionState::Disconnected,
            uuid: None,
            tree_known: false,
            tree: None,
            tree_before_restart: None,
            tree_refresh_at: None,
            rehook_pending: false,
            hooks: BTreeMap::new(),
            hit: BTreeSet::new(),
            recording: false,
            watch: None,
            last_status: None,
            next_status_at: now,
            next_transitions_at: now,
            last_request_at: now,
            next_attempt_at: now,
            notifications_failing: false,
        }
    }

    fn run(mut self) {
        let outcome = panic::catch_unwind(AssertUnwindSafe(|| self.main_loop()));
        if outcome.is_err() {
            self.client = None;
            self.emit(SessionEvent::Error(
                "internal error: the live session stopped".to_owned(),
            ));
            self.emit(SessionEvent::State(ConnectionState::Disconnected));
        }
    }

    fn main_loop(&mut self) {
        loop {
            loop {
                match self.commands.try_recv() {
                    Ok(command) => {
                        if !self.handle_command(command) {
                            return;
                        }
                    }
                    Err(TryRecvError::Empty) => break,
                    Err(TryRecvError::Disconnected) => {
                        self.disconnect();
                        return;
                    }
                }
            }
            self.run_due_work();
            if !self.wait() {
                return;
            }
        }
    }

    /// Waits for the next event: a notification or a command while
    /// connected, a command or the next attempt otherwise. Returns false
    /// when the session must stop.
    fn wait(&mut self) -> bool {
        let now = Instant::now();
        let deadline = self.next_deadline();
        if self.is_connected() {
            let wait = deadline
                .map_or(COMMAND_CHECK_INTERVAL, |at| {
                    at.saturating_duration_since(now)
                })
                .min(COMMAND_CHECK_INTERVAL);
            self.poll_notifications(wait);
            return true;
        }
        let received = match deadline {
            Some(at) => self
                .commands
                .recv_timeout(at.saturating_duration_since(now)),
            None => self
                .commands
                .recv()
                .map_err(|_| RecvTimeoutError::Disconnected),
        };
        match received {
            Ok(command) => self.handle_command(command),
            Err(RecvTimeoutError::Timeout) => true,
            Err(RecvTimeoutError::Disconnected) => {
                self.disconnect();
                false
            }
        }
    }

    fn next_deadline(&self) -> Option<Instant> {
        match self.state {
            ConnectionState::Disconnected => None,
            ConnectionState::Connecting | ConnectionState::Reconnecting { .. } => {
                Some(self.next_attempt_at)
            }
            ConnectionState::Connected { .. } => {
                let mut deadline = self
                    .next_status_at
                    .min(self.last_request_at + self.config.poll.heartbeat_interval);
                if self.recording {
                    deadline = deadline.min(self.next_transitions_at);
                }
                if let Some(watch) = &self.watch {
                    deadline = deadline.min(watch.next_at);
                }
                if let Some(at) = self.tree_refresh_at {
                    deadline = deadline.min(at);
                }
                if self.rehook_pending && self.tree_refresh_at.is_none() {
                    deadline = Instant::now();
                }
                Some(deadline)
            }
        }
    }

    fn is_connected(&self) -> bool {
        matches!(self.state, ConnectionState::Connected { .. })
    }

    fn emit(&self, event: SessionEvent) {
        if self.events.send(event).is_ok()
            && let Some(waker) = &self.waker
        {
            waker();
        }
    }

    fn set_state(&mut self, state: ConnectionState) {
        if self.state != state {
            self.state = state;
            self.emit(SessionEvent::State(state));
        }
    }

    // -----------------------------------------------------------------------
    // Commands

    /// Handles one command. Returns false when the session must stop.
    fn handle_command(&mut self, command: SessionCommand) -> bool {
        match command {
            SessionCommand::Connect { host, port } => self.connect(host, port),
            SessionCommand::Disconnect => self.disconnect(),
            SessionCommand::SetPollInterval(interval) => {
                let interval = interval.max(MIN_POLL_INTERVAL);
                self.config.poll.status_interval = interval;
                self.next_status_at = self.next_status_at.min(Instant::now() + interval);
            }
            SessionCommand::WatchBlackboards { names, interval } => {
                self.watch_blackboards(names, interval)
            }
            SessionCommand::RefreshTree => {
                if self.require_connected("refresh tree")
                    && let Ok((uuid, xml)) = self.call("FULLTREE", Client::full_tree)
                {
                    self.publish_tree(uuid, &xml);
                }
            }
            SessionCommand::InsertHook(hook) => self.insert_hook(hook),
            SessionCommand::RemoveHook { uid, position } => self.remove_hook(uid, position),
            SessionCommand::Unlock(request) => self.unlock(request),
            SessionCommand::DumpHooks => {
                if self.require_connected("dump hooks")
                    && let Ok(hooks) = self.call("HOOKS_DUMP", Client::hooks_dump)
                {
                    self.emit(SessionEvent::Hooks(hooks));
                }
            }
            SessionCommand::RemoveAllHooks => {
                if self.require_connected("remove all hooks")
                    && self
                        .call("REMOVE_ALL_HOOKS", Client::remove_all_hooks)
                        .is_ok()
                {
                    self.hooks.clear();
                    self.hit.clear();
                }
            }
            SessionCommand::DisableAllHooks => {
                if self.require_connected("disable all hooks")
                    && self
                        .call("DISABLE_ALL_HOOKS", Client::disable_all_hooks)
                        .is_ok()
                {
                    for hook in self.hooks.values_mut() {
                        hook.enabled = false;
                    }
                    self.hit.clear();
                }
            }
            SessionCommand::StartRecording => self.start_recording(),
            SessionCommand::StopRecording => self.stop_recording(),
            SessionCommand::Shutdown => {
                self.disconnect();
                return false;
            }
        }
        true
    }

    fn require_connected(&mut self, what: &str) -> bool {
        if self.is_connected() {
            true
        } else {
            self.emit(SessionEvent::Error(format!(
                "cannot {what}: not connected to an executor"
            )));
            false
        }
    }

    fn connect(&mut self, host: String, port: u16) {
        if self.client.is_some() {
            self.disconnect();
        }
        let config = ClientConfig {
            host,
            port,
            request_timeout: self.config.request_timeout,
            connect_timeout: self.config.connect_timeout,
        };
        match Client::connect(config) {
            Ok(client) => {
                self.client = Some(client);
                self.reset_connection_data();
                self.next_attempt_at = Instant::now();
                self.set_state(ConnectionState::Connecting);
            }
            Err(err) => {
                self.emit(SessionEvent::Error(format!("cannot connect: {err}")));
                self.set_state(ConnectionState::Disconnected);
            }
        }
    }

    fn reset_connection_data(&mut self) {
        self.uuid = None;
        self.tree_known = false;
        self.tree = None;
        self.tree_before_restart = None;
        self.tree_refresh_at = None;
        self.rehook_pending = false;
        self.hooks.clear();
        self.hit.clear();
        self.recording = false;
        self.last_status = None;
        self.notifications_failing = false;
    }

    /// Removes all hooks and stops the recording (best effort), then closes
    /// the sockets.
    fn disconnect(&mut self) {
        if let Some(mut client) = self.client.take() {
            client.set_request_timeout(self.config.cleanup_timeout);
            let mut reachable = true;
            if self.recording {
                match client.stop_recording() {
                    Ok(()) => {
                        // Read the transitions that are still in the
                        // buffer, as `StopRecording` does.
                        match client.transitions() {
                            Ok(batch) if !batch.is_empty() => {
                                let possible_loss = batch.len() >= RECORDING_BUFFER_CAPACITY;
                                self.emit(SessionEvent::Transitions {
                                    batch,
                                    possible_loss,
                                });
                            }
                            Ok(_) => {}
                            Err(err) => {
                                reachable = !err.is_connection_error();
                                self.emit(SessionEvent::Warning(format!(
                                    "cannot read the last transitions: {err}"
                                )));
                            }
                        }
                    }
                    Err(err) => {
                        reachable = !err.is_connection_error();
                        self.emit(SessionEvent::Warning(format!(
                            "cannot stop the recording: {err}"
                        )));
                    }
                }
                self.recording = false;
                self.emit(SessionEvent::RecordingStopped);
            }
            if reachable && let Err(err) = client.remove_all_hooks() {
                self.emit(SessionEvent::Warning(format!(
                    "cannot remove the hooks: {err}"
                )));
            }
        }
        self.reset_connection_data();
        self.set_state(ConnectionState::Disconnected);
    }

    fn watch_blackboards(&mut self, names: Vec<String>, interval: Duration) {
        if names.is_empty() {
            self.watch = None;
            return;
        }
        if let Err(err) = messages::encode_blackboard_names(&names) {
            self.emit(SessionEvent::Error(format!(
                "cannot watch the blackboards: {err}"
            )));
            return;
        }
        let interval = if interval.is_zero() {
            Duration::ZERO
        } else {
            interval.max(MIN_POLL_INTERVAL)
        };
        self.watch = Some(BlackboardWatch {
            names,
            interval,
            next_at: Instant::now(),
        });
    }

    fn insert_hook(&mut self, hook: Hook) {
        if !self.require_connected("insert the hook") {
            return;
        }
        if hook.position == HookPosition::Post {
            self.emit(SessionEvent::Warning(format!(
                "the hook at node {} uses position POST; BT.CPP 4.9.0 runs every hook \
                 before the tick, and unlock or remove with position POST fails",
                hook.uid
            )));
        }
        if !hook.enabled && self.hit.contains(&hook.uid) {
            // The publisher does not wake a blocked node when its hook is
            // disabled through HOOK_INSERT. Release the node first.
            let unlock = UnlockRequest {
                desired_status: NodeStatus::Skipped,
                position: hook.position,
                remove_when_done: false,
                uid: hook.uid,
            };
            match self.call("BREAKPOINT_UNLOCK", |client| {
                client.unlock_breakpoint(&unlock)
            }) {
                Ok(()) => {
                    self.hit.remove(&hook.uid);
                }
                Err(Failure::Server) => {}
                Err(Failure::Other) => {
                    if !self.is_connected() {
                        return;
                    }
                }
            }
        }
        if self
            .call("HOOK_INSERT", |client| client.insert_hook(&hook))
            .is_ok()
        {
            if hook.mode == HookMode::Replace {
                // The publisher releases a blocked node when the mode
                // changes from breakpoint to replace.
                self.hit.remove(&hook.uid);
            }
            self.hooks.insert((hook.uid, hook.position), hook);
        }
    }

    fn remove_hook(&mut self, uid: u16, position: HookPosition) {
        if !self.require_connected("remove the hook") {
            return;
        }
        match self.call("HOOK_REMOVE", |client| client.remove_hook(uid, position)) {
            Ok(()) => {
                self.hooks.remove(&(uid, position));
                self.hit.remove(&uid);
            }
            Err(Failure::Server) => {
                // The executor does not have this hook; forget it too.
                self.hooks.remove(&(uid, position));
            }
            Err(Failure::Other) => {}
        }
    }

    fn unlock(&mut self, request: UnlockRequest) {
        if !self.require_connected("unlock the breakpoint") {
            return;
        }
        if self
            .call("BREAKPOINT_UNLOCK", |client| {
                client.unlock_breakpoint(&request)
            })
            .is_ok()
        {
            self.hit.remove(&request.uid);
            let key = (request.uid, request.position);
            let once = self.hooks.get(&key).is_some_and(|hook| hook.once);
            if request.remove_when_done || once {
                // The publisher removes the hook when the node continues.
                self.hooks.remove(&key);
            }
        }
    }

    fn start_recording(&mut self) {
        if !self.require_connected("start the recording") {
            return;
        }
        if let Ok(start_time_us) = self.call("TOGGLE_RECORDING start", Client::start_recording) {
            self.recording = true;
            self.next_transitions_at = Instant::now() + self.config.poll.transitions_interval;
            self.emit(SessionEvent::RecordingStarted { start_time_us });
        }
    }

    fn stop_recording(&mut self) {
        if !self.require_connected("stop the recording") {
            return;
        }
        if self
            .call("TOGGLE_RECORDING stop", Client::stop_recording)
            .is_ok()
        {
            if self.recording {
                // Read the transitions that are still in the buffer.
                self.fetch_transitions();
            }
            self.recording = false;
            self.emit(SessionEvent::RecordingStopped);
        }
    }

    // -----------------------------------------------------------------------
    // Periodic work

    fn run_due_work(&mut self) {
        let now = Instant::now();
        match self.state {
            ConnectionState::Disconnected => {}
            ConnectionState::Connecting | ConnectionState::Reconnecting { .. } => {
                if now >= self.next_attempt_at {
                    self.attempt_connection();
                }
            }
            ConnectionState::Connected { .. } => {
                if self.tree_refresh_at.is_some_and(|at| now >= at) {
                    self.refresh_tree_after_restart();
                }
                // After a restart, wait for the new tree: the hooks are
                // moved to the new UIDs first.
                if self.rehook_pending && self.is_connected() && self.tree_refresh_at.is_none() {
                    self.rehook();
                }
                if self.recording && self.is_connected() && now >= self.next_transitions_at {
                    self.fetch_transitions();
                }
                if self.is_connected()
                    && self
                        .watch
                        .as_ref()
                        .is_some_and(|watch| now >= watch.next_at)
                {
                    self.fetch_blackboards();
                }
                let heartbeat_due =
                    now >= self.last_request_at + self.config.poll.heartbeat_interval;
                if self.is_connected() && (now >= self.next_status_at || heartbeat_due) {
                    self.poll_status();
                }
            }
        }
    }

    fn attempt_connection(&mut self) {
        let result = match self.client.as_mut() {
            None => {
                self.set_state(ConnectionState::Disconnected);
                return;
            }
            Some(client) => {
                if self.tree_known {
                    client.status().map(Attempt::Status)
                } else {
                    client.full_tree().map(Attempt::Tree)
                }
            }
        };
        let uuid = self.client.as_ref().and_then(Client::last_tree_uuid);
        match result {
            Ok(attempt) => {
                self.last_request_at = Instant::now();
                let Some(uuid) = uuid else {
                    self.connection_lost("the reply has no tree UUID".to_owned());
                    return;
                };
                let restarted = self.uuid.is_some_and(|old| old != uuid);
                if restarted {
                    self.check_uuid(Some(uuid));
                } else {
                    self.uuid = Some(uuid);
                }
                self.set_state(ConnectionState::Connected { uuid });
                self.rehook_pending |= self.config.reinsert_hooks && !self.hooks.is_empty();
                self.next_status_at = Instant::now();
                match attempt {
                    Attempt::Tree((uuid, xml)) => self.publish_tree(uuid, &xml),
                    Attempt::Status(snapshot) => {
                        if !restarted {
                            self.publish_status(snapshot);
                        }
                    }
                }
            }
            Err(err) if err.is_connection_error() => self.connection_lost(err.to_string()),
            Err(err) => {
                // A malformed or error reply: the executor is there, but the
                // reply is not usable. Try again later.
                self.emit(SessionEvent::Warning(format!(
                    "unusable reply while connecting: {err}"
                )));
                self.connection_lost(err.to_string());
            }
        }
    }

    fn connection_lost(&mut self, reason: String) {
        let attempt = match self.state {
            ConnectionState::Reconnecting { attempt } => attempt.saturating_add(1),
            ConnectionState::Connected { .. } => {
                self.emit(SessionEvent::Warning(format!("connection lost: {reason}")));
                1
            }
            _ => 1,
        };
        // Keep `hit`: a short loss of the connection does not release a
        // node that waits at a breakpoint (the publisher wakes it only
        // after its heartbeat timeout, 5 s by default). If the session
        // forgot the hit, a later disable would leave the node blocked
        // (protocol fact 4). A restart of the executor clears `hit` in
        // `check_uuid`.
        // Send the first status after the reconnect, also if it is unchanged.
        self.last_status = None;
        self.next_attempt_at = Instant::now() + self.backoff(attempt);
        self.set_state(ConnectionState::Reconnecting { attempt });
    }

    fn backoff(&self, attempt: u32) -> Duration {
        let shift = attempt.saturating_sub(1).min(16);
        self.config
            .reconnect_initial_delay
            .saturating_mul(1u32 << shift)
            .min(self.config.reconnect_max_delay)
    }

    /// Runs a request on the client and handles the errors: a connection
    /// error starts the reconnect, the other errors give an event.
    fn call<T>(
        &mut self,
        what: &str,
        request: impl FnOnce(&mut Client) -> Result<T, ClientError>,
    ) -> Result<T, Failure> {
        let Some(client) = self.client.as_mut() else {
            return Err(Failure::Other);
        };
        let result = request(client);
        let uuid = client.last_tree_uuid();
        if let Err(err) = &result
            && err.is_connection_error()
        {
            let message = format!("{what}: {err}");
            self.connection_lost(message);
            return Err(Failure::Other);
        }
        self.last_request_at = Instant::now();
        self.check_uuid(uuid);
        match result {
            Ok(value) => Ok(value),
            Err(ClientError::Server(message)) => {
                self.emit(SessionEvent::Error(format!(
                    "{what}: the executor replied: {message}"
                )));
                Err(Failure::Server)
            }
            Err(err @ (ClientError::Encode(_) | ClientError::InvalidConfig(_))) => {
                self.emit(SessionEvent::Error(format!("{what}: {err}")));
                Err(Failure::Other)
            }
            Err(err) => {
                self.emit(SessionEvent::Warning(format!("{what}: {err}")));
                Err(Failure::Other)
            }
        }
    }

    /// Compares the UUID of the last reply with the known UUID. A new UUID
    /// means that the executor restarted.
    fn check_uuid(&mut self, uuid: Option<TreeUuid>) {
        let Some(new) = uuid else {
            return;
        };
        match self.uuid {
            Some(old) if old != new => {
                self.uuid = Some(new);
                self.hit.clear();
                self.last_status = None;
                self.tree_known = false;
                if self.tree_before_restart.is_none() {
                    // The hooks use the UIDs of this tree.
                    self.tree_before_restart = self.tree.take();
                }
                self.tree = None;
                self.tree_refresh_at = Some(Instant::now());
                self.rehook_pending = self.config.reinsert_hooks && !self.hooks.is_empty();
                self.emit(SessionEvent::ExecutorRestarted { old, new });
                if self.recording {
                    self.recording = false;
                    self.emit(SessionEvent::Warning(
                        "the executor restarted, so the recording stopped".to_owned(),
                    ));
                    self.emit(SessionEvent::RecordingStopped);
                }
                if self.is_connected() {
                    self.set_state(ConnectionState::Connected { uuid: new });
                }
            }
            Some(_) => {}
            None => self.uuid = Some(new),
        }
    }

    fn refresh_tree_after_restart(&mut self) {
        // If the request fails, try again after the status interval.
        self.tree_refresh_at = Some(Instant::now() + self.config.poll.status_interval);
        if let Ok((uuid, xml)) = self.call("FULLTREE", Client::full_tree) {
            self.publish_tree(uuid, &xml);
        }
    }

    fn publish_tree(&mut self, uuid: TreeUuid, xml: &str) {
        if self.uuid != Some(uuid) {
            // The reply came from another executor than the one that the
            // session knows. `check_uuid` handled the restart already.
            return;
        }
        self.tree_refresh_at = None;
        self.last_status = None;
        let before_restart = self.tree_before_restart.take();
        match RuntimeTree::parse(xml) {
            Ok(tree) => {
                let tree = Arc::new(tree);
                self.tree_known = true;
                self.tree = Some(Arc::clone(&tree));
                if let Some(old_tree) = before_restart {
                    self.move_hooks_to(&old_tree, &tree);
                }
                self.emit(SessionEvent::Tree { uuid, tree });
            }
            Err(err) => {
                self.tree_known = false;
                self.emit(SessionEvent::Error(format!(
                    "cannot read the tree XML of the executor: {err}"
                )));
                if before_restart.is_some() && !self.hooks.is_empty() {
                    // The hooks use the UIDs of the old tree. Without the new
                    // tree, the session cannot find the same nodes, and an
                    // old UID can belong to another node now. Do not insert
                    // a breakpoint or a replace hook on the wrong node.
                    let count = self.hooks.len();
                    self.hooks.clear();
                    self.rehook_pending = false;
                    self.emit(SessionEvent::Warning(format!(
                        "{count} hook(s) were not inserted again: the tree of the \
                         restarted executor cannot be read"
                    )));
                }
            }
        }
    }

    /// Moves the hooks of the session from the UIDs of `old_tree` to the
    /// UIDs of `new_tree`, by node path (see [`BreakpointSet::resolve`]).
    ///
    /// A hook moves only when the node path of its node is in the new tree.
    /// If the path is not there, the node was removed or renamed, and its
    /// old UID can belong to another node now. Such a hook is dropped with a
    /// [`SessionEvent::Warning`], so that no breakpoint or replace hook goes
    /// to the wrong node. A hook whose UID was not in the old tree (empty
    /// path) keeps its UID, if the new tree has a node with that UID.
    fn move_hooks_to(&mut self, old_tree: &RuntimeTree, new_tree: &RuntimeTree) {
        let hooks: Vec<Hook> = self.hooks.values().copied().collect();
        let set = BreakpointSet {
            version: crate::breakpoints::FORMAT_VERSION,
            breakpoints: hooks
                .iter()
                .map(|hook| Breakpoint::from_hook(hook, old_tree))
                .collect(),
        };
        self.hooks.clear();
        for (hook, resolved) in hooks.iter().zip(set.resolve(new_tree)) {
            let path = set
                .breakpoints
                .get(resolved.index)
                .map(|breakpoint| breakpoint.node_path.as_str())
                .unwrap_or_default();
            let target = match resolved.resolution {
                Resolution::ByPath | Resolution::AmbiguousPath => resolved.uid,
                Resolution::ByStoredUid if path.is_empty() => resolved.uid,
                Resolution::ByStoredUid | Resolution::Unresolved => None,
            };
            match target {
                Some(uid) => {
                    let moved = Hook {
                        uid: uid.0,
                        ..*hook
                    };
                    self.hooks.insert((moved.uid, moved.position), moved);
                }
                None => {
                    self.emit(SessionEvent::Warning(format!(
                        "the hook at node {} ({path:?}) was not inserted again: \
                         the new tree has no node with this path",
                        hook.uid
                    )));
                }
            }
        }
    }

    fn rehook(&mut self) {
        self.rehook_pending = false;
        let hooks: Vec<Hook> = self.hooks.values().copied().collect();
        if hooks.is_empty() {
            return;
        }
        // The result is reported by `call`. After a reconnect the hooks are
        // inserted again.
        let _ = self.call("HOOK_INSERT", |client| client.insert_hooks(&hooks));
    }

    fn poll_status(&mut self) {
        self.next_status_at = Instant::now() + self.config.poll.status_interval;
        if let Ok(snapshot) = self.call("STATUS", Client::status) {
            if self.tree_refresh_at.is_some() {
                // A restart was detected. Send the new tree first.
                return;
            }
            self.publish_status(snapshot);
        }
    }

    fn publish_status(&mut self, snapshot: StatusSnapshot) {
        if self.last_status.as_ref() != Some(&snapshot) {
            self.last_status = Some(snapshot.clone());
            self.emit(SessionEvent::Status { snapshot });
        }
    }

    fn fetch_transitions(&mut self) {
        self.next_transitions_at = Instant::now() + self.config.poll.transitions_interval;
        if let Ok(batch) = self.call("GET_TRANSITIONS", Client::transitions)
            && !batch.is_empty()
        {
            let possible_loss = batch.len() >= RECORDING_BUFFER_CAPACITY;
            self.emit(SessionEvent::Transitions {
                batch,
                possible_loss,
            });
        }
    }

    fn fetch_blackboards(&mut self) {
        let Some(watch) = self.watch.as_mut() else {
            return;
        };
        let names = watch.names.clone();
        if watch.interval.is_zero() {
            self.watch = None;
        } else {
            watch.next_at = Instant::now() + watch.interval;
        }
        if let Ok(value) = self.call("BLACKBOARD", |client| client.blackboards(&names)) {
            self.emit(SessionEvent::Blackboards(value));
        }
    }

    fn poll_notifications(&mut self, wait: Duration) {
        let mut wait = wait;
        for _ in 0..MAX_NOTIFICATIONS_PER_PASS {
            let Some(client) = self.client.as_mut() else {
                thread::sleep(wait);
                return;
            };
            let result = client.poll_notification(wait);
            if !matches!(
                result,
                Err(ClientError::Zmq(_) | ClientError::InvalidConfig(_))
            ) {
                self.notifications_failing = false;
            }
            match result {
                Ok(Some(notification)) => {
                    self.on_breakpoint(notification.uid);
                    wait = Duration::ZERO;
                }
                Ok(None) => return,
                Err(ClientError::Decode(err)) => {
                    self.emit(SessionEvent::Warning(format!(
                        "malformed breakpoint notification: {err}"
                    )));
                    wait = Duration::ZERO;
                }
                Err(err) => {
                    // The loop comes back here every few milliseconds.
                    // Report a failure of the socket once.
                    if !self.notifications_failing {
                        self.notifications_failing = true;
                        self.emit(SessionEvent::Warning(format!(
                            "cannot read notifications: {err}"
                        )));
                    }
                    thread::sleep(wait);
                    return;
                }
            }
        }
    }

    fn on_breakpoint(&mut self, uid: Uid) {
        let hook = self
            .hooks
            .iter()
            .find(|((hook_uid, _), _)| *hook_uid == uid.0)
            .map(|(key, hook)| (*key, *hook));
        match hook {
            Some((key, hook)) if hook.mode == HookMode::Replace => {
                if hook.once {
                    // The publisher removes the hook after it runs.
                    self.hooks.remove(&key);
                }
            }
            _ => {
                self.hit.insert(uid.0);
            }
        }
        self.emit(SessionEvent::BreakpointReached { uid });
    }
}

/// The result of a connection attempt.
enum Attempt {
    Tree((TreeUuid, String)),
    Status(StatusSnapshot),
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn backoff_doubles_up_to_the_limit() {
        let (_commands, receiver) = mpsc::channel();
        let (events, _events) = mpsc::channel();
        let session = LiveSession::new(SessionConfig::default(), receiver, events, None);
        let delays: Vec<u64> = (1..=8)
            .map(|attempt| session.backoff(attempt).as_millis() as u64)
            .collect();
        assert_eq!(delays, vec![100, 200, 400, 800, 1600, 2000, 2000, 2000]);
        assert_eq!(session.backoff(u32::MAX), Duration::from_secs(2));
        assert_eq!(session.backoff(0), Duration::from_millis(100));
    }

    #[test]
    fn config_minimums() {
        let (_commands, receiver) = mpsc::channel();
        let (events, _events) = mpsc::channel();
        let config = SessionConfig {
            poll: PollConfig {
                status_interval: Duration::ZERO,
                heartbeat_interval: Duration::ZERO,
                transitions_interval: Duration::from_millis(1),
            },
            ..SessionConfig::default()
        };
        let session = LiveSession::new(config, receiver, events, None);
        assert_eq!(session.config.poll.status_interval, MIN_POLL_INTERVAL);
        assert_eq!(session.config.poll.heartbeat_interval, MIN_POLL_INTERVAL);
        assert_eq!(session.config.poll.transitions_interval, MIN_POLL_INTERVAL);
        assert!(format!("{session:?}").contains("LiveSession"));
    }

    #[test]
    fn commands_without_connection_give_errors() {
        let (handle, events) = LiveSession::spawn(SessionConfig::default(), None);
        assert!(handle.send(SessionCommand::InsertHook(Hook::breakpoint(1))));
        assert!(handle.send(SessionCommand::DumpHooks));
        let first = events.recv_timeout(Duration::from_secs(2)).unwrap();
        assert!(matches!(first, SessionEvent::Error(ref text) if text.contains("not connected")));
        let second = events.recv_timeout(Duration::from_secs(2)).unwrap();
        assert!(matches!(second, SessionEvent::Error(_)));
        assert!(!handle.is_finished());
        assert!(format!("{handle:?}").contains("SessionHandle"));
        assert!(handle.join());
    }

    #[test]
    fn invalid_connect_gives_error() {
        let (handle, events) = LiveSession::spawn(SessionConfig::default(), None);
        handle.connect("", 1667);
        let event = events.recv_timeout(Duration::from_secs(2)).unwrap();
        assert!(matches!(event, SessionEvent::Error(ref text) if text.contains("cannot connect")));
        handle.connect("127.0.0.1", u16::MAX);
        let event = events.recv_timeout(Duration::from_secs(2)).unwrap();
        assert!(matches!(event, SessionEvent::Error(_)));
        handle.send(SessionCommand::WatchBlackboards {
            names: vec!["a;b".to_owned()],
            interval: Duration::from_millis(100),
        });
        let event = events.recv_timeout(Duration::from_secs(2)).unwrap();
        assert!(matches!(event, SessionEvent::Error(ref text) if text.contains("blackboards")));
        drop(handle);
        // The thread stops when the handle is dropped; the channel closes.
        let deadline = Instant::now() + Duration::from_secs(2);
        while Instant::now() < deadline {
            match events.recv_timeout(Duration::from_millis(50)) {
                Err(RecvTimeoutError::Disconnected) => return,
                _ => continue,
            }
        }
        panic!("the session thread did not stop");
    }

    #[test]
    fn waker_is_called_for_each_event() {
        use std::sync::atomic::{AtomicUsize, Ordering};
        let count = Arc::new(AtomicUsize::new(0));
        let counter = Arc::clone(&count);
        let waker: Waker = Box::new(move || {
            counter.fetch_add(1, Ordering::SeqCst);
        });
        let (handle, events) = LiveSession::spawn(SessionConfig::default(), Some(waker));
        handle.send(SessionCommand::RefreshTree);
        handle.send(SessionCommand::StartRecording);
        events.recv_timeout(Duration::from_secs(2)).unwrap();
        events.recv_timeout(Duration::from_secs(2)).unwrap();
        assert!(handle.join());
        assert!(count.load(Ordering::SeqCst) >= 2);
    }
}
