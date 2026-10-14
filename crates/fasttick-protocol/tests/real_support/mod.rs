//! Support for the tests against the real executor (`test_harness`, with
//! BehaviorTree.CPP 4.x).
//!
//! Each test starts its own `fasttick_executor` on a free pair of ports.
//! The [`Executor`] value stops the process on drop, also when a test fails.
//! All waits poll with a deadline and stop early when the condition is true.
//!
//! The executor writes `RESULT loop=<n> status=<STATUS>` on stdout each time
//! the root node completes a run. In `probe.xml`, the run `n` is also the run
//! that sets the blackboard entry `{runs}` to `n` (see the tree file).

#![allow(dead_code)]

use std::collections::BTreeMap;
use std::time::{Duration, Instant};

use fasttick_core::runtime::{RuntimeTree, Uid};
use fasttick_core::status::NodeStatus;
use fasttick_protocol::client::{Client, ClientConfig};
use fasttick_protocol::messages::{Hook, HookPosition, StatusSnapshot};
use fasttick_protocol::wire::TreeUuid;
use fasttick_testkit::{Executor, ExecutorOptions, Harness, free_port_pair};
use serde_json::Value;

/// The longest wait for a condition that should come soon. The waits end
/// early, so a long value costs nothing when the test passes. It is long
/// because other test processes can load the machine.
pub const WAIT: Duration = Duration::from_secs(20);

/// The time that a "nothing happens" check observes the executor.
pub const QUIET: Duration = Duration::from_millis(300);

/// A heartbeat timeout that never ends during a test. Tests that are not
/// about the heartbeat use it, so that a slow test machine cannot disable
/// the hooks between two requests.
pub const NO_HEARTBEAT_MS: u64 = 600_000;

/// The request timeout of the test clients.
pub const REQUEST_TIMEOUT: Duration = Duration::from_secs(5);

/// The blackboard name of the root tree of `probe.xml`.
pub const PROBE_BLACKBOARD: &str = "Probe";

/// Executor options for a test tree. The port is a new free pair.
pub fn options(harness: &Harness, tree_file: &str) -> ExecutorOptions {
    let mut options = ExecutorOptions::new(harness.tree(tree_file));
    options.port = free_port_pair();
    options.heartbeat_ms = Some(NO_HEARTBEAT_MS);
    options.tick_ms = 20;
    options
}

/// A client for the executor on `port`.
pub fn client(port: u16) -> Client {
    Client::connect(ClientConfig {
        request_timeout: REQUEST_TIMEOUT,
        ..ClientConfig::new("127.0.0.1", port)
    })
    .expect("the client configuration is valid")
}

/// Polls `check` until it gives a value, for at most [`WAIT`].
pub fn wait_until<T>(what: &str, mut check: impl FnMut() -> Option<T>) -> T {
    let deadline = Instant::now() + WAIT;
    loop {
        if let Some(value) = check() {
            return value;
        }
        assert!(
            Instant::now() < deadline,
            "timeout after {WAIT:?} while waiting for {what}"
        );
        std::thread::sleep(Duration::from_millis(10));
    }
}

/// A running executor, a client and the tree of the executor.
pub struct Live {
    /// The executor process. Drop stops it.
    pub executor: Executor,
    /// The server port of the publisher.
    pub port: u16,
    /// A client that is connected to the executor.
    pub client: Client,
    /// The tree UUID of the executor.
    pub uuid: TreeUuid,
    /// The FULLTREE XML.
    pub xml: String,
    /// The parsed tree.
    pub tree: RuntimeTree,
}

impl Live {
    /// Starts the executor and gets the tree.
    pub fn start(options: ExecutorOptions) -> Live {
        let port = options.port;
        let executor = Executor::spawn(options).expect("the executor starts");
        let mut client = client(port);
        let (uuid, xml) = client.full_tree().expect("FULLTREE");
        let tree = RuntimeTree::parse(&xml).expect("the FULLTREE XML is a runtime tree");
        Live {
            executor,
            port,
            client,
            uuid,
            xml,
            tree,
        }
    }

    /// Starts `probe.xml` with the default test options.
    pub fn probe(harness: &Harness) -> Live {
        Live::start(options(harness, "probe.xml"))
    }

    /// The UID of the node with this name. The name must be unique.
    pub fn uid(&self, name: &str) -> u16 {
        uid_of(&self.tree, name)
    }

    /// Waits for a BREAKPOINT_REACHED notification of the node `uid`. Other
    /// notifications are ignored. Gives the number of ignored notifications.
    pub fn wait_hit(&mut self, uid: u16) -> usize {
        wait_hit(&mut self.client, uid)
    }

    /// Makes sure that the notification socket of the client receives
    /// messages: a replace hook with SKIPPED on `uid` (a node that ticks
    /// often) sends a notification on each tick but does not change the
    /// tick. The function waits for one, then removes the hook.
    ///
    /// The PUB socket of the publisher drops messages while the
    /// subscription is not complete, so a test must do this before it
    /// waits for a single notification.
    pub fn sync_notifications(&mut self, uid: u16) {
        sync_notifications(&mut self.client, uid);
    }

    /// Reads the blackboard of the root tree of `probe.xml`.
    pub fn probe_counters(&mut self) -> ProbeCounters {
        let value = self
            .client
            .blackboards(&[PROBE_BLACKBOARD])
            .expect("BLACKBOARD");
        ProbeCounters::from_json(&value[PROBE_BLACKBOARD])
    }

    /// Reads STATUS and gives the status of each node by name.
    pub fn statuses(&mut self) -> BTreeMap<String, NodeStatus> {
        let snapshot = self.client.status().expect("STATUS");
        statuses_by_name(&self.tree, &snapshot)
    }

    /// The installed hooks, sorted by UID.
    pub fn hooks(&mut self) -> Vec<Hook> {
        let mut hooks = self.client.hooks_dump().expect("HOOKS_DUMP");
        hooks.sort_by_key(|hook| (hook.uid, hook.position));
        hooks
    }

    /// The number of RESULT lines up to now.
    pub fn result_count(&self) -> usize {
        self.executor.results().len()
    }

    /// Waits for the RESULT line of run `run` and gives its status text.
    pub fn wait_result(&self, run: u64) -> String {
        let prefix = format!("RESULT loop={run} ");
        let line = self
            .executor
            .wait_for_stdout(WAIT, |line| line.starts_with(&prefix))
            .unwrap_or_else(|err| panic!("no RESULT line for run {run}: {err}"));
        line[prefix.len()..]
            .strip_prefix("status=")
            .unwrap_or_else(|| panic!("bad RESULT line {line:?}"))
            .to_owned()
    }

    /// Checks for [`QUIET`] that the executor makes no progress: no new
    /// RESULT line, and the blackboard counters of `probe.xml` stay the
    /// same. The client sends requests during the check (heartbeat).
    pub fn assert_blocked(&mut self) -> ProbeCounters {
        let counters = self.probe_counters();
        let results = self.result_count();
        let end = Instant::now() + QUIET;
        while Instant::now() < end {
            std::thread::sleep(Duration::from_millis(50));
            assert_eq!(self.probe_counters(), counters, "the tree made progress");
        }
        assert_eq!(self.result_count(), results, "the tree completed a run");
        counters
    }

    /// Checks for [`QUIET`] that the STATUS reply does not change (for any
    /// tree). The client sends requests during the check (heartbeat).
    pub fn assert_frozen(&mut self) {
        let snapshot = self.client.status().expect("STATUS");
        let end = Instant::now() + QUIET;
        while Instant::now() < end {
            std::thread::sleep(Duration::from_millis(50));
            assert_eq!(
                self.client.status().expect("STATUS"),
                snapshot,
                "the tree made progress"
            );
        }
    }

    /// Waits until at least `count` more runs are complete after the run
    /// that the blackboard shows now. Gives the runs (number and status)
    /// that started after this call.
    pub fn wait_more_runs(&mut self, count: u64) -> Vec<(u64, String)> {
        let first = self.probe_counters().runs + 1;
        (first..first + count)
            .map(|run| (run, self.wait_result(run)))
            .collect()
    }

    /// Checks that no notification for `uid` comes for [`QUIET`].
    pub fn assert_no_hit(&mut self, uid: u16) {
        let end = Instant::now() + QUIET;
        while let Some(remaining) = end.checked_duration_since(Instant::now()) {
            if let Some(notification) = self
                .client
                .poll_notification(remaining)
                .expect("notification socket")
            {
                assert_ne!(
                    notification.uid.0, uid,
                    "unexpected notification for node {uid}"
                );
            }
        }
    }
}

/// The UID of the node with this name. The name must be unique.
pub fn uid_of(tree: &RuntimeTree, name: &str) -> u16 {
    let mut found = tree.nodes.iter().filter(|node| node.name == name);
    let node = found
        .next()
        .unwrap_or_else(|| panic!("the tree has no node {name:?}"));
    assert!(found.next().is_none(), "the name {name:?} is not unique");
    node.uid.expect("runtime nodes have a UID").0
}

/// The status of each node by name (the names of `probe.xml` and
/// `long_running.xml` are unique for the named nodes).
pub fn statuses_by_name(tree: &RuntimeTree, snapshot: &StatusSnapshot) -> BTreeMap<String, NodeStatus> {
    snapshot
        .entries
        .iter()
        .filter_map(|entry| {
            let node = tree.node_by_uid(entry.uid)?;
            Some((node.name.clone(), entry.current))
        })
        .collect()
}

/// See [`Live::wait_hit`].
pub fn wait_hit(client: &mut Client, uid: u16) -> usize {
    let deadline = Instant::now() + WAIT;
    let mut ignored = 0;
    loop {
        let remaining = deadline.saturating_duration_since(Instant::now());
        assert!(
            !remaining.is_zero(),
            "no BREAKPOINT_REACHED for node {uid} within {WAIT:?}"
        );
        let notification = client
            .poll_notification(remaining.min(Duration::from_millis(200)))
            .expect("notification socket");
        match notification {
            Some(notification) if notification.uid == Uid(uid) => return ignored,
            Some(_) => ignored += 1,
            None => {}
        }
    }
}

/// Same as [`wait_hit`], but the client sends a STATUS request every
/// 100 ms during the wait, so that the publisher keeps its heartbeat.
pub fn wait_hit_with_heartbeat(client: &mut Client, uid: u16) {
    let deadline = Instant::now() + WAIT;
    loop {
        assert!(
            Instant::now() < deadline,
            "no BREAKPOINT_REACHED for node {uid} within {WAIT:?}"
        );
        if let Some(notification) = client
            .poll_notification(Duration::from_millis(100))
            .expect("notification socket")
            && notification.uid == Uid(uid)
        {
            return;
        }
        client.status().expect("STATUS");
    }
}

/// See [`Live::sync_notifications`].
pub fn sync_notifications(client: &mut Client, uid: u16) {
    client
        .insert_hook(&Hook::replace(uid, NodeStatus::Skipped))
        .expect("HOOK_INSERT");
    wait_hit(client, uid);
    client
        .remove_hook(uid, HookPosition::Pre)
        .expect("HOOK_REMOVE");
    // A notification that was sent before the removal can still arrive.
    while client
        .poll_notification(Duration::from_millis(50))
        .expect("notification socket")
        .is_some()
    {}
}

/// The blackboard entries of `probe.xml` (see the tree file).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ProbeCounters {
    /// `{runs}`: the number of runs that started.
    pub runs: u64,
    /// `{probe_ticks}`: the number of real ticks of the node "probe".
    pub probe_ticks: u64,
    /// `{done}`: the number of runs that got past "probe".
    pub done: u64,
    /// `{probe_result}`: "success" or "failure", from the post-conditions.
    pub probe_result: Option<String>,
}

impl ProbeCounters {
    /// Reads the entries from the blackboard JSON. A missing entry counts
    /// as 0 (the node did not run yet).
    pub fn from_json(blackboard: &Value) -> ProbeCounters {
        let count = |key: &str| match &blackboard[key] {
            Value::Null => 0,
            value => value
                .as_u64()
                .unwrap_or_else(|| panic!("{key} is not a count: {value}")),
        };
        ProbeCounters {
            runs: count("runs"),
            probe_ticks: count("probe_ticks"),
            done: count("done"),
            probe_result: blackboard["probe_result"].as_str().map(str::to_owned),
        }
    }
}
