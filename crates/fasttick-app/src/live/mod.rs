//! The live connection: the UI side of a
//! [`LiveSession`](fasttick_protocol::session::LiveSession).
//!
//! [`LiveController`] holds the channel ends of the session thread. It
//! never blocks the UI thread: commands go through a channel, and events
//! are read with `try_recv` each frame ([`LiveController::poll`]). The
//! session calls a waker that asks egui for a repaint.
//!
//! The controller also keeps the live view model:
//!
//! - the tree and the last statuses of the executor,
//! - a mirror of the hooks (breakpoints and forced results),
//! - the nodes that wait at a breakpoint now,
//! - the watched blackboards and their last values ([`blackboard`]),
//! - the transitions of a recording ([`recording`]).
//!
//! The session thread does not confirm a hook change. The mirror changes
//! at once when the user acts, and the controller reads the hook list of
//! the executor (HOOKS_DUMP) after each change and every
//! [`HOOK_DUMP_INTERVAL`]. A hook list replaces the mirror only if no
//! change was made after the request, so an old list does not undo a new
//! change.

pub mod blackboard;
pub mod recording;

use std::collections::{BTreeMap, VecDeque};
use std::sync::Arc;
use std::sync::mpsc::Receiver;
use std::time::{Duration, Instant};

use fasttick_core::runtime::{NodeIndex, RuntimeTree, Uid};
use fasttick_core::status::NodeStatus;
use fasttick_protocol::breakpoints::{Breakpoint, BreakpointSet, Resolution};
use fasttick_protocol::messages::{Hook, HookMode, HookPosition, StatusSnapshot, UnlockRequest};
use fasttick_protocol::session::{
    ConnectionState, LiveSession, PollConfig, SessionCommand, SessionConfig, SessionEvent,
    SessionHandle,
};
use fasttick_protocol::wire::TreeUuid;

use crate::canvas::{BreakpointMarker, CanvasTree, Substitution};
use crate::dialogs::NoticeLevel;

pub use blackboard::BlackboardWatch;
pub use recording::Recording;

/// How often the controller reads the hook list of the executor while it
/// is connected.
pub const HOOK_DUMP_INTERVAL: Duration = Duration::from_secs(2);

/// Why the UI does not offer breakpoints after a node (plan decision D4).
/// BT.CPP 4.9.0 runs every hook before the tick
/// (`insertHook` of the publisher stores a POST hook with the PRE hooks).
pub const AFTER_BREAKPOINT_UNAVAILABLE: &str = "BT.CPP 4.9.0 cannot stop after a node runs";

/// The same notice text is not shown again within this time.
const NOTICE_REPEAT: Duration = Duration::from_secs(5);

/// How to release a node that waits at a breakpoint.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum ResumeAction {
    /// The node ticks normally.
    Continue,
    /// The node does not tick and returns SUCCESS.
    ForceSuccess,
    /// The node does not tick and returns FAILURE.
    ForceFailure,
}

impl ResumeAction {
    /// The status for BREAKPOINT_UNLOCK. BT.CPP uses only SUCCESS and
    /// FAILURE; any other value lets the node tick
    /// (`serverLoop` of the publisher, case `BREAKPOINT_UNLOCK`).
    pub fn status(self) -> NodeStatus {
        match self {
            ResumeAction::Continue => NodeStatus::Skipped,
            ResumeAction::ForceSuccess => NodeStatus::Success,
            ResumeAction::ForceFailure => NodeStatus::Failure,
        }
    }

    /// The button text.
    pub fn label(self) -> &'static str {
        match self {
            ResumeAction::Continue => "Continue",
            ResumeAction::ForceSuccess => "Force SUCCESS",
            ResumeAction::ForceFailure => "Force FAILURE",
        }
    }
}

/// Something that the app must react to after [`LiveController::poll`].
#[derive(Debug, Clone, PartialEq)]
pub enum LiveUpdate {
    /// A new tree: the first tree of a connection, or the tree after a
    /// restart of the executor.
    Tree(Arc<RuntimeTree>),
    /// The executor stopped before this node (a breakpoint, not a forced
    /// result).
    Stopped(Uid),
    /// A message for the user.
    Notice(NoticeLevel, String),
}

/// The live connection state of the app. See the module documentation.
pub struct LiveController {
    session: Option<(SessionHandle, Receiver<SessionEvent>)>,
    state: ConnectionState,
    endpoint: Option<(String, u16)>,
    /// The last error or warning of the session.
    pub last_message: Option<String>,
    tree: Option<Arc<RuntimeTree>>,
    uuid: Option<TreeUuid>,
    status: Option<StatusSnapshot>,
    hooks: BTreeMap<u16, Hook>,
    hits: Vec<Uid>,
    /// Counts the local changes of the hook mirror.
    hook_revision: u64,
    /// The hook revision at each HOOKS_DUMP request that has no reply yet.
    dump_requests: VecDeque<u64>,
    next_dump_at: Option<Instant>,
    blackboard: BlackboardWatch,
    recording: Recording,
    recording_requested: bool,
    poll_interval_ms: u32,
    generation: u64,
    last_notice: Option<(String, Instant)>,
}

impl std::fmt::Debug for LiveController {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("LiveController")
            .field("state", &self.state)
            .field("endpoint", &self.endpoint)
            .field("hooks", &self.hooks.len())
            .field("hits", &self.hits)
            .field("recording", &self.recording.is_active())
            .finish_non_exhaustive()
    }
}

impl Default for LiveController {
    fn default() -> Self {
        LiveController {
            session: None,
            state: ConnectionState::Disconnected,
            endpoint: None,
            last_message: None,
            tree: None,
            uuid: None,
            status: None,
            hooks: BTreeMap::new(),
            hits: Vec::new(),
            hook_revision: 0,
            dump_requests: VecDeque::new(),
            next_dump_at: None,
            blackboard: BlackboardWatch::default(),
            recording: Recording::default(),
            recording_requested: false,
            poll_interval_ms: 100,
            generation: 1,
            last_notice: None,
        }
    }
}

impl LiveController {
    // ----- Connection -----

    /// The connection state (disconnected if no session runs).
    pub fn state(&self) -> ConnectionState {
        self.state
    }

    /// True if the executor replies now.
    pub fn is_connected(&self) -> bool {
        matches!(self.state, ConnectionState::Connected { .. })
    }

    /// The host and port of the current or last connection.
    pub fn endpoint(&self) -> Option<(&str, u16)> {
        self.endpoint.as_ref().map(|(h, p)| (h.as_str(), *p))
    }

    /// The endpoint as `host:port`.
    pub fn endpoint_text(&self) -> Option<String> {
        self.endpoint.as_ref().map(|(h, p)| format!("{h}:{p}"))
    }

    /// True if a session is connected or tries to connect.
    pub fn is_active(&self) -> bool {
        !matches!(self.state, ConnectionState::Disconnected)
    }

    /// The UUID of the running tree, if known.
    pub fn uuid(&self) -> Option<TreeUuid> {
        self.uuid
    }

    /// Connects to an executor. Starts the session thread if necessary.
    pub fn connect(&mut self, ctx: &egui::Context, host: &str, port: u16, poll_interval_ms: u32) {
        let waker_ctx = ctx.clone();
        let waker: fasttick_protocol::session::Waker =
            Box::new(move || waker_ctx.request_repaint());
        self.connect_with_waker(Some(waker), host, port, poll_interval_ms);
    }

    /// Connects with an optional waker (tests use no waker).
    pub fn connect_with_waker(
        &mut self,
        waker: Option<fasttick_protocol::session::Waker>,
        host: &str,
        port: u16,
        poll_interval_ms: u32,
    ) {
        self.poll_interval_ms = poll_interval_ms.max(10);
        let interval = Duration::from_millis(u64::from(self.poll_interval_ms));
        if self.session.as_ref().is_some_and(|(h, _)| h.is_finished()) {
            self.session = None;
        }
        match &self.session {
            None => {
                let config = SessionConfig {
                    poll: PollConfig {
                        status_interval: interval,
                        ..PollConfig::default()
                    },
                    ..SessionConfig::default()
                };
                self.session = Some(LiveSession::spawn(config, waker));
            }
            Some((handle, _)) => {
                handle.send(SessionCommand::SetPollInterval(interval));
            }
        }
        if let Some((handle, _)) = &self.session {
            handle.connect(host, port);
            // The blackboard watch belongs to the session: send it again
            // for a new session thread.
            self.blackboard.resend(handle);
        }
        self.endpoint = Some((host.to_owned(), port));
        self.state = ConnectionState::Connecting;
        self.last_message = None;
        self.clear_hooks();
        self.bump();
    }

    /// Disconnects. The session thread stops the recording and removes the
    /// hooks of this session.
    pub fn disconnect(&mut self) {
        self.send(SessionCommand::Disconnect);
        self.state = ConnectionState::Disconnected;
        self.clear_hooks();
        self.bump();
    }

    /// The status poll interval in milliseconds.
    pub fn poll_interval_ms(&self) -> u32 {
        self.poll_interval_ms
    }

    /// Changes the status poll interval.
    pub fn set_poll_interval(&mut self, poll_interval_ms: u32) {
        self.poll_interval_ms = poll_interval_ms.max(10);
        self.send(SessionCommand::SetPollInterval(Duration::from_millis(
            u64::from(self.poll_interval_ms),
        )));
    }

    /// Stops the session thread and waits for its cleanup (short).
    pub fn shutdown(&mut self) {
        if let Some((handle, _)) = self.session.take() {
            handle.join();
        }
        self.state = ConnectionState::Disconnected;
        self.clear_hooks();
    }

    /// A short text for the status bar.
    pub fn state_text(&self) -> String {
        match self.state {
            ConnectionState::Disconnected => "Not connected".to_owned(),
            ConnectionState::Connecting => match self.endpoint_text() {
                Some(e) => format!("Connecting to {e}"),
                None => "Connecting".to_owned(),
            },
            ConnectionState::Connected { .. } => match self.endpoint_text() {
                Some(e) => format!("Connected to {e}"),
                None => "Connected".to_owned(),
            },
            ConnectionState::Reconnecting { attempt } => {
                format!("Reconnecting (attempt {attempt})")
            }
        }
    }

    fn send(&self, command: SessionCommand) -> bool {
        match &self.session {
            Some((handle, _)) => handle.send(command),
            None => false,
        }
    }

    /// Increments the generation: the views decorate their trees again.
    fn bump(&mut self) {
        self.generation = self.generation.wrapping_add(1).max(1);
    }

    /// A number that changes when the statuses, the tree or the markers
    /// change. A view decorates its tree again when it sees a new value.
    pub fn generation(&self) -> u64 {
        self.generation
    }

    // ----- Tree and statuses -----

    /// The tree of the executor, if known.
    pub fn tree(&self) -> Option<&Arc<RuntimeTree>> {
        self.tree.as_ref()
    }

    /// The last statuses.
    pub fn status(&self) -> Option<&StatusSnapshot> {
        self.status.as_ref()
    }

    /// The node path and name of a UID in the current tree.
    pub fn node_text(&self, uid: Uid) -> Option<(String, String)> {
        let tree = self.tree.as_ref()?;
        let index = tree.index_of_uid(uid)?;
        Some((tree.node(index).name.clone(), tree.node_path(index)))
    }

    /// The index of a UID in the current tree.
    pub fn node_index(&self, uid: Uid) -> Option<NodeIndex> {
        self.tree.as_ref()?.index_of_uid(uid)
    }

    // ----- Events -----

    /// Reads the events of the session thread (never blocks) and updates
    /// the model. Returns what the app must do.
    pub fn poll(&mut self) -> Vec<LiveUpdate> {
        let mut events = Vec::new();
        if let Some((_, receiver)) = &self.session {
            while let Ok(event) = receiver.try_recv() {
                events.push(event);
            }
        }
        let now = Instant::now();
        let mut updates = Vec::new();
        for event in events {
            self.apply_event(event, now, &mut updates);
        }
        self.tick(now);
        updates
    }

    /// Periodic work: reads the hook list of the executor every
    /// [`HOOK_DUMP_INTERVAL`] while connected.
    pub fn tick(&mut self, now: Instant) {
        if !self.is_connected() {
            self.next_dump_at = None;
            return;
        }
        match self.next_dump_at {
            Some(at) if now < at => {}
            _ => {
                self.request_dump();
                self.next_dump_at = Some(now + HOOK_DUMP_INTERVAL);
            }
        }
    }

    /// Applies one session event to the model. [`LiveController::poll`]
    /// calls this; the unit tests call it directly.
    pub fn apply_event(&mut self, event: SessionEvent, now: Instant, out: &mut Vec<LiveUpdate>) {
        match event {
            SessionEvent::State(state) => self.on_state(state, now, out),
            SessionEvent::Tree { uuid, tree } => {
                let old = self.tree.replace(Arc::clone(&tree));
                self.uuid = Some(uuid);
                self.status = None;
                if let Some(old) = old
                    && old.xml != tree.xml
                {
                    self.move_hooks(&old, &tree);
                }
                self.blackboard
                    .tree_changed(&tree, self.session.as_ref().map(|s| &s.0));
                self.request_dump();
                self.bump();
                out.push(LiveUpdate::Tree(tree));
            }
            SessionEvent::Status { snapshot } => {
                self.status = Some(snapshot);
                self.bump();
            }
            SessionEvent::Blackboards(value) => self.blackboard.set_values(value, now),
            SessionEvent::BreakpointReached { uid } => {
                let hook = self.hooks.get(&uid.0).copied();
                match hook {
                    Some(hook) if hook.mode == HookMode::Replace => {
                        // A forced result ran: the node did not stop.
                        if hook.once {
                            self.hooks.remove(&uid.0);
                            self.hook_revision += 1;
                        }
                    }
                    _ => {
                        if !self.hits.contains(&uid) {
                            self.hits.push(uid);
                        }
                        out.push(LiveUpdate::Stopped(uid));
                    }
                }
                self.bump();
            }
            SessionEvent::Hooks(list) => self.on_hooks(list),
            SessionEvent::RecordingStarted { start_time_us } => {
                self.recording_requested = false;
                let xml = self.tree.as_ref().map(|t| t.xml.clone());
                self.recording.start(start_time_us, xml, now);
            }
            SessionEvent::Transitions {
                batch,
                possible_loss,
            } => {
                self.recording.push(&batch, possible_loss);
            }
            SessionEvent::RecordingStopped => {
                self.recording_requested = false;
                self.recording.stop(now);
            }
            SessionEvent::ExecutorRestarted { .. } => {
                self.hits.clear();
                self.status = None;
                self.bump();
                self.notice(
                    out,
                    now,
                    NoticeLevel::Info,
                    "The executor restarted. FastTick reads its tree again.".to_owned(),
                );
            }
            SessionEvent::Warning(message) => {
                self.last_message = Some(message.clone());
                self.notice(out, now, NoticeLevel::Warning, message);
            }
            SessionEvent::Error(message) => {
                self.last_message = Some(message.clone());
                if message.contains("HOOKS_DUMP") || message.contains("dump hooks") {
                    self.dump_requests.pop_front();
                }
                if message.contains("TOGGLE_RECORDING") || message.contains("recording") {
                    self.recording_requested = false;
                }
                self.notice(out, now, NoticeLevel::Error, message);
            }
        }
    }

    fn notice(
        &mut self,
        out: &mut Vec<LiveUpdate>,
        now: Instant,
        level: NoticeLevel,
        text: String,
    ) {
        if let Some((last, at)) = &self.last_notice
            && *last == text
            && now.saturating_duration_since(*at) < NOTICE_REPEAT
        {
            return;
        }
        self.last_notice = Some((text.clone(), now));
        out.push(LiveUpdate::Notice(level, text));
    }

    fn on_state(&mut self, state: ConnectionState, now: Instant, out: &mut Vec<LiveUpdate>) {
        let before = self.state;
        self.state = state;
        match state {
            ConnectionState::Disconnected => {
                let had_hooks = !self.hooks.is_empty();
                self.clear_hooks();
                if had_hooks && before != ConnectionState::Disconnected {
                    self.notice(
                        out,
                        now,
                        NoticeLevel::Info,
                        "The connection closed. The breakpoints and forced results of this \
                         session were removed from the executor."
                            .to_owned(),
                    );
                }
            }
            ConnectionState::Connected { uuid } => {
                self.uuid = Some(uuid);
            }
            ConnectionState::Connecting | ConnectionState::Reconnecting { .. } => {
                // The session keeps its hooks and inserts them again.
                self.dump_requests.clear();
            }
        }
        self.bump();
    }

    fn clear_hooks(&mut self) {
        self.hooks.clear();
        self.hits.clear();
        self.dump_requests.clear();
        self.next_dump_at = None;
        self.hook_revision += 1;
    }

    /// A hook list from the executor. It replaces the mirror if the mirror
    /// did not change after the request.
    fn on_hooks(&mut self, list: Vec<Hook>) {
        let requested_at = self.dump_requests.pop_front();
        if requested_at == Some(self.hook_revision) {
            let hooks: BTreeMap<u16, Hook> = list
                .into_iter()
                .filter(|hook| hook.position == HookPosition::Pre)
                .map(|hook| (hook.uid, hook))
                .collect();
            if hooks != self.hooks {
                self.hooks = hooks;
                // A node can only wait at a breakpoint that exists.
                let hooks = &self.hooks;
                self.hits.retain(|uid| {
                    hooks
                        .get(&uid.0)
                        .is_some_and(|hook| hook.mode == HookMode::Breakpoint && hook.enabled)
                });
                self.bump();
            }
        } else if self.dump_requests.is_empty() {
            // The mirror changed after the request: ask again.
            self.request_dump();
        }
    }

    /// Moves the hooks of the mirror to the UIDs of a new tree, by node
    /// path. The session thread does the same with its own hooks
    /// (`LiveSession::move_hooks_to`); a hook whose path is not in the new
    /// tree is dropped.
    fn move_hooks(&mut self, old: &RuntimeTree, new: &RuntimeTree) {
        if self.hooks.is_empty() {
            return;
        }
        let hooks: Vec<Hook> = self.hooks.values().copied().collect();
        let set = BreakpointSet {
            version: fasttick_protocol::breakpoints::FORMAT_VERSION,
            breakpoints: hooks
                .iter()
                .map(|h| Breakpoint::from_hook(h, old))
                .collect(),
        };
        self.hooks.clear();
        for (hook, resolved) in hooks.iter().zip(set.resolve(new)) {
            let path_empty = set
                .breakpoints
                .get(resolved.index)
                .is_none_or(|b| b.node_path.is_empty());
            let target = match resolved.resolution {
                Resolution::ByPath | Resolution::AmbiguousPath => resolved.uid,
                Resolution::ByStoredUid if path_empty => resolved.uid,
                Resolution::ByStoredUid | Resolution::Unresolved => None,
            };
            if let Some(uid) = target {
                self.hooks.insert(
                    uid.0,
                    Hook {
                        uid: uid.0,
                        ..*hook
                    },
                );
            }
        }
        self.hook_revision += 1;
    }

    // ----- Hooks -----

    /// All hooks (breakpoints and forced results), by node UID.
    pub fn hooks(&self) -> &BTreeMap<u16, Hook> {
        &self.hooks
    }

    /// The hook of a node, if any.
    pub fn hook(&self, uid: Uid) -> Option<&Hook> {
        self.hooks.get(&uid.0)
    }

    /// The breakpoints (hooks in breakpoint mode).
    pub fn breakpoints(&self) -> impl Iterator<Item = &Hook> {
        self.hooks
            .values()
            .filter(|hook| hook.mode == HookMode::Breakpoint)
    }

    /// The forced results (hooks in replace mode).
    pub fn forced(&self) -> impl Iterator<Item = &Hook> {
        self.hooks
            .values()
            .filter(|hook| hook.mode == HookMode::Replace)
    }

    /// The nodes that wait at a breakpoint now, in the order of arrival.
    pub fn hits(&self) -> &[Uid] {
        &self.hits
    }

    /// True if the node waits at a breakpoint now.
    pub fn is_hit(&self, uid: Uid) -> bool {
        self.hits.contains(&uid)
    }

    fn set_hook(&mut self, hook: Hook) -> bool {
        if !self.send(SessionCommand::InsertHook(hook)) {
            return false;
        }
        if !hook.enabled || hook.mode == HookMode::Replace {
            // The session releases a waiting node first (disable), and the
            // publisher releases it when the mode changes to replace.
            self.hits.retain(|uid| uid.0 != hook.uid);
        }
        self.hooks.insert(hook.uid, hook);
        self.changed_hooks();
        true
    }

    fn changed_hooks(&mut self) {
        self.hook_revision += 1;
        self.request_dump();
        self.bump();
    }

    /// Reads the hook list of the executor (HOOKS_DUMP).
    pub fn request_dump(&mut self) {
        if self.is_connected() && self.send(SessionCommand::DumpHooks) {
            self.dump_requests.push_back(self.hook_revision);
        }
    }

    /// Adds an enabled breakpoint before the node. A forced result on the
    /// node is replaced: a node has only one hook.
    pub fn add_breakpoint(&mut self, uid: Uid) -> bool {
        self.set_hook(Hook::breakpoint(uid.0))
    }

    /// Adds a breakpoint, or removes the breakpoint of the node.
    pub fn toggle_breakpoint(&mut self, uid: Uid) -> bool {
        match self.hooks.get(&uid.0) {
            Some(hook) if hook.mode == HookMode::Breakpoint => self.remove_hook(uid),
            _ => self.add_breakpoint(uid),
        }
    }

    /// Enables or disables the hook of a node.
    pub fn set_enabled(&mut self, uid: Uid, enabled: bool) -> bool {
        match self.hooks.get(&uid.0) {
            Some(hook) if hook.enabled != enabled => self.set_hook(Hook { enabled, ..*hook }),
            _ => false,
        }
    }

    /// Makes the hook of a node one-shot (removed after it runs once), or
    /// not.
    pub fn set_once(&mut self, uid: Uid, once: bool) -> bool {
        match self.hooks.get(&uid.0) {
            Some(hook) if hook.once != once => self.set_hook(Hook { once, ..*hook }),
            _ => false,
        }
    }

    /// Makes the node return a status without a tick (HOOK_INSERT in
    /// replace mode). Use SUCCESS or FAILURE.
    pub fn force(&mut self, uid: Uid, status: NodeStatus) -> bool {
        self.set_hook(Hook::replace(uid.0, status))
    }

    /// Removes the hook of a node (breakpoint or forced result).
    pub fn remove_hook(&mut self, uid: Uid) -> bool {
        if !self.send(SessionCommand::RemoveHook {
            uid: uid.0,
            position: HookPosition::Pre,
        }) {
            return false;
        }
        self.hooks.remove(&uid.0);
        self.hits.retain(|hit| *hit != uid);
        self.changed_hooks();
        true
    }

    /// Removes all hooks of the executor (also the hooks of other
    /// clients).
    pub fn remove_all_hooks(&mut self) -> bool {
        if !self.send(SessionCommand::RemoveAllHooks) {
            return false;
        }
        self.hooks.clear();
        self.hits.clear();
        self.changed_hooks();
        true
    }

    /// Inserts several hooks (for example from a breakpoint file).
    pub fn insert_hooks(&mut self, hooks: &[Hook]) -> usize {
        let mut count = 0;
        for hook in hooks {
            if self.set_hook(*hook) {
                count += 1;
            }
        }
        count
    }

    /// Releases a node that waits at a breakpoint.
    pub fn resume(&mut self, uid: Uid, action: ResumeAction) -> bool {
        let request = UnlockRequest::with_status(uid.0, action.status());
        if !self.send(SessionCommand::Unlock(request)) {
            return false;
        }
        self.hits.retain(|hit| *hit != uid);
        if self.hooks.get(&uid.0).is_some_and(|hook| hook.once) {
            // The publisher removes a one-shot hook when the node continues.
            self.hooks.remove(&uid.0);
            self.hook_revision += 1;
        }
        self.request_dump();
        self.bump();
        true
    }

    /// The current hooks as a breakpoint set, with the node paths of the
    /// current tree.
    pub fn breakpoint_set(&self) -> BreakpointSet {
        let mut set = BreakpointSet::new();
        if let Some(tree) = &self.tree {
            set.breakpoints = self
                .hooks
                .values()
                .map(|hook| Breakpoint::from_hook(hook, tree))
                .collect();
        }
        set
    }

    /// Puts the breakpoint markers, the forced-result markers and the
    /// statuses on a canvas tree.
    pub fn decorate(&self, tree: &mut CanvasTree) {
        tree.reset_status();
        for node in tree.nodes_mut() {
            node.deco.breakpoint = None;
            node.deco.substitution = None;
        }
        if let Some(status) = &self.status {
            for entry in &status.entries {
                tree.set_status(entry.uid, entry.current, entry.previous);
            }
        }
        for hook in self.hooks.values() {
            let Some(node) = tree.find_uid(Uid(hook.uid)).and_then(|id| tree.get_mut(id)) else {
                continue;
            };
            match hook.mode {
                HookMode::Breakpoint => {
                    node.deco.breakpoint = Some(if self.hits.contains(&Uid(hook.uid)) {
                        BreakpointMarker::Hit
                    } else if hook.enabled {
                        BreakpointMarker::Enabled
                    } else {
                        BreakpointMarker::Disabled
                    });
                }
                HookMode::Replace => {
                    node.deco.substitution = substitution(hook);
                }
            }
        }
        for uid in &self.hits {
            if let Some(node) = tree.find_uid(*uid).and_then(|id| tree.get_mut(id)) {
                node.deco.breakpoint = Some(BreakpointMarker::Hit);
            }
        }
    }

    // ----- Blackboards -----

    /// The blackboard watch.
    pub fn blackboard(&self) -> &BlackboardWatch {
        &self.blackboard
    }

    /// Changes the watched blackboards or the interval, and sends the
    /// change to the session.
    pub fn update_blackboard_watch(&mut self, change: impl FnOnce(&mut BlackboardWatch)) {
        change(&mut self.blackboard);
        if let Some((handle, _)) = &self.session {
            self.blackboard.resend(handle);
        }
    }

    /// Reads the watched blackboards once now.
    pub fn refresh_blackboards(&mut self) {
        if let Some((handle, _)) = &self.session {
            self.blackboard.refresh(handle);
        }
    }

    // ----- Recording -----

    /// The recording (active or finished).
    pub fn recording(&self) -> &Recording {
        &self.recording
    }

    /// True after a start request until the session confirms or fails.
    pub fn recording_requested(&self) -> bool {
        self.recording_requested
    }

    /// Starts a recording. The old recording is discarded when the new
    /// one starts.
    pub fn start_recording(&mut self) -> bool {
        if !self.is_connected() || self.recording.is_active() {
            return false;
        }
        let sent = self.send(SessionCommand::StartRecording);
        self.recording_requested = sent;
        sent
    }

    /// Stops the recording.
    pub fn stop_recording(&mut self) -> bool {
        self.recording.is_active() && self.send(SessionCommand::StopRecording)
    }

    /// Forgets the finished recording.
    pub fn discard_recording(&mut self) {
        if !self.recording.is_active() {
            self.recording = Recording::default();
        }
    }
}

/// The substitution marker of a replace hook. A disabled hook does not
/// act, and BT.CPP ticks the node normally for a status other than SUCCESS
/// or FAILURE (`TreeNode::executeTick`).
pub fn substitution(hook: &Hook) -> Option<Substitution> {
    if hook.mode != HookMode::Replace || !hook.enabled {
        return None;
    }
    match hook.desired_status {
        NodeStatus::Success => Some(Substitution::ForcedSuccess),
        NodeStatus::Failure => Some(Substitution::ForcedFailure),
        _ => None,
    }
}

impl Drop for LiveController {
    fn drop(&mut self) {
        // Do not wait in drop; the session cleans up when the handle drops.
        if let Some((handle, _)) = &self.session {
            handle.shutdown();
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::canvas::{BuiltinCategories, CanvasTree, RuntimeBuildOptions};
    use fasttick_core::status::RawStatus;
    use fasttick_protocol::messages::NodeStatusEntry;

    const XML: &str = r#"<root BTCPP_format="4">
        <BehaviorTree ID="Main" _fullpath=""><Sequence name="seq" _uid="1"><AlwaysSuccess name="a" _uid="2"/><SubTree ID="Sub" name="sub" _fullpath="sub" _uid="3"/></Sequence></BehaviorTree>
        <BehaviorTree ID="Sub" _fullpath="sub"><AlwaysFailure name="f" _uid="4"/></BehaviorTree>
    </root>"#;

    /// The same tree with one more node at the start: the UIDs move.
    const MOVED_XML: &str = r#"<root BTCPP_format="4">
        <BehaviorTree ID="Main" _fullpath=""><Sequence name="seq" _uid="1"><AlwaysSuccess name="new" _uid="2"/><AlwaysSuccess name="a" _uid="3"/><SubTree ID="Sub" name="sub" _fullpath="sub" _uid="4"/></Sequence></BehaviorTree>
        <BehaviorTree ID="Sub" _fullpath="sub"><AlwaysFailure name="f" _uid="5"/></BehaviorTree>
    </root>"#;

    fn uuid(byte: u8) -> TreeUuid {
        TreeUuid([byte; 16])
    }

    fn tree(xml: &str) -> Arc<RuntimeTree> {
        Arc::new(RuntimeTree::parse(xml).unwrap())
    }

    /// A controller that talks to a session on a closed port: the
    /// commands go to the channel, and the tests feed the events.
    fn connected() -> (LiveController, Vec<LiveUpdate>) {
        let mut live = LiveController::default();
        live.connect_with_waker(None, "127.0.0.1", 9, 50);
        let mut out = Vec::new();
        let now = Instant::now();
        live.apply_event(
            SessionEvent::State(ConnectionState::Connected { uuid: uuid(1) }),
            now,
            &mut out,
        );
        live.apply_event(
            SessionEvent::Tree {
                uuid: uuid(1),
                tree: tree(XML),
            },
            now,
            &mut out,
        );
        (live, out)
    }

    fn apply(live: &mut LiveController, event: SessionEvent) -> Vec<LiveUpdate> {
        let mut out = Vec::new();
        live.apply_event(event, Instant::now(), &mut out);
        out
    }

    fn canvas(live: &LiveController) -> CanvasTree {
        let runtime = live.tree().unwrap();
        let options = RuntimeBuildOptions::for_tree(runtime);
        let expansion = CanvasTree::default_expansion(runtime, options);
        let mut tree = CanvasTree::from_runtime(runtime, options, &expansion, &BuiltinCategories);
        live.decorate(&mut tree);
        tree
    }

    fn deco(tree: &CanvasTree, uid: u16) -> crate::canvas::Decorations {
        tree.get(tree.find_uid(Uid(uid)).unwrap()).unwrap().deco
    }

    #[test]
    fn connect_to_nothing_and_shut_down() {
        let ctx = egui::Context::default();
        let mut live = LiveController::default();
        assert!(!live.is_active());
        assert_eq!(live.state_text(), "Not connected");
        // Port 9 (discard) on localhost: nothing answers there.
        live.connect(&ctx, "127.0.0.1", 9, 50);
        assert!(live.is_active());
        assert_eq!(live.endpoint(), Some(("127.0.0.1", 9)));
        assert!(live.state_text().starts_with("Connecting"));
        let _ = live.poll();
        live.set_poll_interval(100);
        assert_eq!(live.poll_interval_ms(), 100);
        live.disconnect();
        assert!(!live.is_active());
        live.shutdown();
        assert!(live.poll().is_empty());
        // Commands without a session do nothing.
        assert!(!live.add_breakpoint(Uid(1)));
        assert!(live.hooks().is_empty());
    }

    #[test]
    fn tree_and_status_decorate_the_canvas() {
        let (mut live, out) = connected();
        assert!(matches!(out.as_slice(), [LiveUpdate::Tree(_)]));
        assert_eq!(live.state_text(), "Connected to 127.0.0.1:9");
        assert_eq!(live.node_text(Uid(4)).unwrap().1, "sub/f");
        let before = live.generation();
        let snapshot = StatusSnapshot {
            entries: vec![
                NodeStatusEntry {
                    uid: Uid(1),
                    raw: RawStatus(1),
                    current: NodeStatus::Running,
                    previous: None,
                },
                NodeStatusEntry {
                    uid: Uid(2),
                    raw: RawStatus(12),
                    current: NodeStatus::Idle,
                    previous: Some(NodeStatus::Success),
                },
            ],
        };
        apply(&mut live, SessionEvent::Status { snapshot });
        assert_ne!(live.generation(), before);
        let tree = canvas(&live);
        assert_eq!(deco(&tree, 1).status, NodeStatus::Running);
        assert_eq!(deco(&tree, 2).status, NodeStatus::Idle);
        assert_eq!(deco(&tree, 2).previous_status, Some(NodeStatus::Success));
    }

    #[test]
    fn breakpoints_hits_and_resume() {
        let (mut live, _) = connected();
        assert!(live.add_breakpoint(Uid(2)));
        assert!(live.force(Uid(4), NodeStatus::Failure));
        assert_eq!(live.breakpoints().count(), 1);
        assert_eq!(live.forced().count(), 1);
        let tree = canvas(&live);
        assert_eq!(deco(&tree, 2).breakpoint, Some(BreakpointMarker::Enabled));
        assert_eq!(
            deco(&tree, 4).substitution,
            Some(Substitution::ForcedFailure)
        );

        // A replace hook reports a hit too: it is not a stop.
        let out = apply(&mut live, SessionEvent::BreakpointReached { uid: Uid(4) });
        assert!(out.is_empty());
        assert!(live.hits().is_empty());

        let out = apply(&mut live, SessionEvent::BreakpointReached { uid: Uid(2) });
        assert_eq!(out, vec![LiveUpdate::Stopped(Uid(2))]);
        assert!(live.is_hit(Uid(2)));
        let tree = canvas(&live);
        assert_eq!(deco(&tree, 2).breakpoint, Some(BreakpointMarker::Hit));

        assert!(live.resume(Uid(2), ResumeAction::ForceSuccess));
        assert!(!live.is_hit(Uid(2)));
        assert_eq!(ResumeAction::ForceSuccess.status(), NodeStatus::Success);
        assert_eq!(ResumeAction::Continue.status(), NodeStatus::Skipped);

        // Disable, one-shot, toggle.
        assert!(live.set_enabled(Uid(2), false));
        assert!(!live.set_enabled(Uid(2), false), "no change");
        assert_eq!(
            deco(&canvas(&live), 2).breakpoint,
            Some(BreakpointMarker::Disabled)
        );
        assert!(live.set_once(Uid(2), true));
        assert!(live.hook(Uid(2)).unwrap().once);
        assert!(live.toggle_breakpoint(Uid(2)));
        assert!(live.hook(Uid(2)).is_none());
        // A breakpoint replaces the forced result of a node.
        assert!(live.toggle_breakpoint(Uid(4)));
        assert_eq!(live.hook(Uid(4)).unwrap().mode, HookMode::Breakpoint);
        assert!(live.remove_all_hooks());
        assert!(live.hooks().is_empty());
    }

    #[test]
    fn a_one_shot_hook_goes_after_it_runs() {
        let (mut live, _) = connected();
        live.force(Uid(2), NodeStatus::Success);
        live.set_once(Uid(2), true);
        apply(&mut live, SessionEvent::BreakpointReached { uid: Uid(2) });
        assert!(live.hook(Uid(2)).is_none());

        live.add_breakpoint(Uid(4));
        live.set_once(Uid(4), true);
        apply(&mut live, SessionEvent::BreakpointReached { uid: Uid(4) });
        live.resume(Uid(4), ResumeAction::Continue);
        assert!(live.hook(Uid(4)).is_none());
    }

    #[test]
    fn hook_lists_replace_the_mirror_only_when_current() {
        let (mut live, _) = connected();
        // The tree event asked for a list; drop that request.
        live.dump_requests.clear();
        live.add_breakpoint(Uid(2));
        // Request 1 (rev r) is out. A second change makes it old.
        live.add_breakpoint(Uid(4));
        assert_eq!(live.dump_requests.len(), 2);
        // The reply to request 1 has only the first hook: ignored.
        apply(&mut live, SessionEvent::Hooks(vec![Hook::breakpoint(2)]));
        assert_eq!(live.hooks().len(), 2);
        // The reply to request 2 has both, and one from another client.
        apply(
            &mut live,
            SessionEvent::Hooks(vec![
                Hook::breakpoint(1),
                Hook::breakpoint(2),
                Hook::breakpoint(4),
            ]),
        );
        assert_eq!(live.hooks().len(), 3);
        assert!(live.dump_requests.is_empty());
        // A list without a request is old: ask again, keep the mirror.
        apply(&mut live, SessionEvent::Hooks(Vec::new()));
        assert_eq!(live.hooks().len(), 3);
        assert_eq!(live.dump_requests.len(), 1);
        // The executor removed a hit hook (for example another client).
        apply(&mut live, SessionEvent::BreakpointReached { uid: Uid(4) });
        assert!(live.is_hit(Uid(4)));
        apply(&mut live, SessionEvent::Hooks(vec![Hook::breakpoint(1)]));
        assert_eq!(live.hooks().len(), 1);
        assert!(!live.is_hit(Uid(4)));
    }

    #[test]
    fn hooks_move_by_path_after_a_restart() {
        let (mut live, _) = connected();
        live.add_breakpoint(Uid(2)); // "a"
        live.force(Uid(4), NodeStatus::Success); // "sub/f"
        apply(&mut live, SessionEvent::BreakpointReached { uid: Uid(2) });
        let out = apply(
            &mut live,
            SessionEvent::ExecutorRestarted {
                old: uuid(1),
                new: uuid(2),
            },
        );
        assert!(matches!(out.as_slice(), [LiveUpdate::Notice(..)]));
        assert!(live.hits().is_empty());
        apply(
            &mut live,
            SessionEvent::Tree {
                uuid: uuid(2),
                tree: tree(MOVED_XML),
            },
        );
        assert_eq!(live.hook(Uid(3)).unwrap().mode, HookMode::Breakpoint);
        assert_eq!(live.hook(Uid(5)).unwrap().mode, HookMode::Replace);
        assert!(live.hook(Uid(2)).is_none());
        assert!(live.hook(Uid(4)).is_none());
        let set = live.breakpoint_set();
        let paths: Vec<&str> = set
            .breakpoints
            .iter()
            .map(|b| b.node_path.as_str())
            .collect();
        assert_eq!(paths, ["a", "sub/f"]);
    }

    #[test]
    fn disconnect_clears_the_hooks_and_tells_the_user() {
        let (mut live, _) = connected();
        live.add_breakpoint(Uid(2));
        apply(&mut live, SessionEvent::BreakpointReached { uid: Uid(2) });
        // A reconnect keeps the hooks: the session inserts them again.
        apply(
            &mut live,
            SessionEvent::State(ConnectionState::Reconnecting { attempt: 1 }),
        );
        assert_eq!(live.hooks().len(), 1);
        let out = apply(
            &mut live,
            SessionEvent::State(ConnectionState::Disconnected),
        );
        assert!(live.hooks().is_empty());
        assert!(live.hits().is_empty());
        assert!(
            matches!(out.as_slice(), [LiveUpdate::Notice(NoticeLevel::Info, text)] if text.contains("removed"))
        );
    }

    #[test]
    fn repeated_warnings_show_once() {
        let (mut live, _) = connected();
        let first = apply(&mut live, SessionEvent::Warning("same".into()));
        let second = apply(&mut live, SessionEvent::Warning("same".into()));
        assert_eq!(first.len(), 1);
        assert!(second.is_empty());
        assert_eq!(live.last_message.as_deref(), Some("same"));
        let error = apply(&mut live, SessionEvent::Error("HOOKS_DUMP: bad".into()));
        assert!(matches!(
            error.as_slice(),
            [LiveUpdate::Notice(NoticeLevel::Error, _)]
        ));
    }

    #[test]
    fn recording_collects_transitions() {
        let (mut live, _) = connected();
        assert!(live.start_recording());
        assert!(live.recording_requested());
        apply(
            &mut live,
            SessionEvent::RecordingStarted {
                start_time_us: 1_000,
            },
        );
        assert!(live.recording().is_active());
        assert!(!live.start_recording(), "already recording");
        apply(
            &mut live,
            SessionEvent::Transitions {
                batch: vec![fasttick_protocol::messages::RecordedTransition {
                    timestamp_us: 5,
                    uid: Uid(2),
                    status: NodeStatus::Running,
                }],
                possible_loss: true,
            },
        );
        assert_eq!(live.recording().len(), 1);
        assert!(live.recording().possible_loss());
        assert!(live.stop_recording());
        apply(&mut live, SessionEvent::RecordingStopped);
        assert!(!live.recording().is_active());
        assert_eq!(
            live.recording().xml(),
            Some(live.tree().unwrap().xml.as_str())
        );
        live.discard_recording();
        assert!(live.recording().is_empty());
    }

    #[test]
    fn substitution_needs_an_enabled_replace_hook() {
        assert_eq!(
            substitution(&Hook::replace(1, NodeStatus::Success)),
            Some(Substitution::ForcedSuccess)
        );
        assert_eq!(substitution(&Hook::replace(1, NodeStatus::Skipped)), None);
        assert_eq!(substitution(&Hook::breakpoint(1)), None);
        let disabled = Hook {
            enabled: false,
            ..Hook::replace(1, NodeStatus::Failure)
        };
        assert_eq!(substitution(&disabled), None);
    }
}
