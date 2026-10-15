//! Breakpoints, replace hooks (fault injection) and the publisher heartbeat,
//! tested with [`Client`](fasttick_protocol::Client) against the real
//! executor (`test_harness`, BT.CPP 4.x).
//!
//! The tests use `test_harness/trees/probe.xml`. The node "probe" adds 1 to
//! the blackboard entry `{probe_ticks}` only when it really ticks, so the
//! tests can tell a real tick from a status that a hook gives. The executor
//! writes a RESULT line for each run of the tree, and run `n` is the run
//! that sets `{runs}` to `n`.
//!
//! The hook logic of the publisher is in `insertHook`, `unlockBreakpoint`,
//! `removeHook`, `enableAllHooks` and `heartbeatLoop` (the publisher source
//! of BT.CPP), and the use of the pre-tick
//! callback is in `TreeNode::executeTick` (`src/tree_node.cpp`).

mod real_support;

use std::time::{Duration, Instant};

use fasttick_core::runtime::Uid;
use fasttick_core::status::NodeStatus;
use fasttick_protocol::client::ClientError;
use fasttick_protocol::messages::{Hook, HookMode, HookPosition, UnlockRequest};
use real_support::{
    Live, ProbeCounters, QUIET, WAIT, options, wait_hit_with_heartbeat, wait_until,
};

/// Starts `probe.xml`, makes sure that notifications arrive, inserts an
/// enabled breakpoint on "probe" and waits until the tree stops there.
/// Gives the executor and the counters at the stop.
fn stopped_at_probe() -> Option<(Live, ProbeCounters)> {
    let harness = real_support::harness()?;
    let mut live = Live::probe(harness);
    let probe = live.uid("probe");
    live.sync_notifications(live.uid("count_runs"));
    live.client.insert_hook(&Hook::breakpoint(probe)).unwrap();
    live.wait_hit(probe);
    let counters = live.assert_blocked();
    Some((live, counters))
}

/// Checks that all RESULT lines of the given runs have this status.
fn assert_all(runs: &[(u64, String)], status: &str) {
    assert!(
        runs.iter().all(|(_, result)| result == status),
        "expected only {status}: {runs:?}"
    );
}

#[test]
fn breakpoint_stops_the_node_until_an_unlock() {
    let Some((mut live, mut before)) = stopped_at_probe() else {
        return;
    };
    let probe = live.uid("probe");

    // The node waits before its tick: the run started ("count_runs" is
    // complete), "probe" did not tick yet and the sequence is RUNNING.
    assert_eq!(before.probe_ticks + 1, before.runs, "{before:?}");
    let statuses = live.statuses();
    assert_eq!(statuses["probe_sequence"], NodeStatus::Running);
    assert_eq!(statuses["count_runs"], NodeStatus::Success);
    assert_eq!(statuses["probe"], NodeStatus::Idle);

    // Each step unlocks the node with a status. The run then completes and
    // the next run stops at the breakpoint again.
    // - SKIPPED (and any status other than SUCCESS and FAILURE) lets the
    //   node tick normally.
    // - SUCCESS and FAILURE: the node returns the status without a tick.
    let steps = [
        (NodeStatus::Skipped, "SUCCESS", 1, "success"),
        (NodeStatus::Success, "SUCCESS", 0, "success"),
        (NodeStatus::Failure, "FAILURE", 0, "failure"),
        (NodeStatus::Running, "SUCCESS", 1, "success"),
        (NodeStatus::Failure, "FAILURE", 0, "failure"),
        (NodeStatus::Idle, "SUCCESS", 1, "success"),
    ];
    for (status, result, ticks, probe_result) in steps {
        let step = format!("unlock with {status}");
        live.client
            .unlock_breakpoint(&UnlockRequest::with_status(probe, status))
            .unwrap();
        assert_eq!(live.wait_result(before.runs), result, "{step}");
        live.wait_hit(probe);
        let after = live.probe_counters();
        assert_eq!(after.runs, before.runs + 1, "{step}");
        assert_eq!(after.probe_ticks, before.probe_ticks + ticks, "{step}");
        let done = u64::from(result == "SUCCESS");
        assert_eq!(after.done, before.done + done, "{step}");
        assert_eq!(after.probe_result.as_deref(), Some(probe_result), "{step}");

        // The STATUS byte of the IDLE node keeps the result of the last
        // run (10 + status, callback of the publisher).
        let snapshot = live.client.status().unwrap();
        let entry = snapshot.get(Uid(probe)).unwrap();
        assert_eq!(entry.current, NodeStatus::Idle, "{step}");
        let previous = if result == "SUCCESS" {
            NodeStatus::Success
        } else {
            NodeStatus::Failure
        };
        assert_eq!(entry.previous, Some(previous), "{step}");
        before = after;
    }
    // The hook is still there, the node stays blocked.
    assert_eq!(live.hooks(), [Hook::breakpoint(probe)]);
    live.assert_blocked();
    assert!(live.executor.is_running().unwrap());
}

#[test]
fn once_breakpoint_is_removed_after_the_first_hit() {
    let Some(harness) = real_support::harness() else {
        return;
    };
    let mut live = Live::probe(harness);
    let probe = live.uid("probe");
    live.sync_notifications(live.uid("count_runs"));

    let once = Hook {
        once: true,
        ..Hook::breakpoint(probe)
    };
    live.client.insert_hook(&once).unwrap();
    live.wait_hit(probe);
    let before = live.assert_blocked();
    assert_eq!(live.hooks(), [once]);

    live.client
        .unlock_breakpoint(&UnlockRequest::resume(probe))
        .unwrap();
    assert_eq!(live.wait_result(before.runs), "SUCCESS");
    // The publisher removes the hook when the node continues.
    wait_until("the once hook to go away", || {
        live.hooks().is_empty().then_some(())
    });
    let runs = live.wait_more_runs(3);
    assert_all(&runs, "SUCCESS");
    live.assert_no_hit(probe);
    let after = live.probe_counters();
    assert!(after.probe_ticks >= before.probe_ticks + 4, "{after:?}");

    // An unlock with "remove_when_done" removes a normal breakpoint too.
    live.client.insert_hook(&Hook::breakpoint(probe)).unwrap();
    live.wait_hit(probe);
    let before = live.assert_blocked();
    live.client
        .unlock_breakpoint(&UnlockRequest {
            remove_when_done: true,
            ..UnlockRequest::with_status(probe, NodeStatus::Success)
        })
        .unwrap();
    assert_eq!(live.wait_result(before.runs), "SUCCESS");
    wait_until("the hook to go away", || {
        live.hooks().is_empty().then_some(())
    });
    let runs = live.wait_more_runs(2);
    assert_all(&runs, "SUCCESS");
    let after = live.probe_counters();
    // No tick in the unlocked run, a real tick in each later run.
    assert!(after.probe_ticks >= before.probe_ticks + 2, "{after:?}");
    assert!(after.probe_ticks < after.runs, "{after:?}");
}

#[test]
fn disabled_breakpoint_does_not_stop_until_it_is_enabled() {
    let Some(harness) = real_support::harness() else {
        return;
    };
    let mut live = Live::probe(harness);
    let probe = live.uid("probe");
    live.sync_notifications(live.uid("count_runs"));

    let disabled = Hook {
        enabled: false,
        ..Hook::breakpoint(probe)
    };
    live.client.insert_hook(&disabled).unwrap();
    let before = live.probe_counters();
    let runs = live.wait_more_runs(3);
    assert_all(&runs, "SUCCESS");
    // A disabled hook sends no notification and the node ticks.
    live.assert_no_hit(probe);
    let after = live.probe_counters();
    assert!(after.probe_ticks >= before.probe_ticks + 3, "{after:?}");
    assert_eq!(live.hooks(), [disabled]);

    // HOOK_INSERT with "enabled": true enables the same hook.
    live.client.insert_hook(&Hook::breakpoint(probe)).unwrap();
    live.wait_hit(probe);
    live.assert_blocked();
    assert_eq!(live.hooks(), [Hook::breakpoint(probe)]);
}

#[test]
fn disabling_a_blocked_breakpoint_needs_an_unlock() {
    // Protocol fact 4 (docs/PLAN.md): HOOK_INSERT with "enabled": false
    // does not wake a node that waits at the breakpoint. The publisher
    // notifies the waiting tick only on unlock, remove, disable all and a
    // change to replace mode.
    let Some((mut live, before)) = stopped_at_probe() else {
        return;
    };
    let probe = live.uid("probe");
    let disabled = Hook {
        enabled: false,
        ..Hook::breakpoint(probe)
    };
    live.client.insert_hook(&disabled).unwrap();
    assert_eq!(live.assert_blocked(), before);
    assert_eq!(live.hooks(), [disabled]);

    // The unlock wakes the node. The hook is disabled, so the node ticks.
    live.client
        .unlock_breakpoint(&UnlockRequest::resume(probe))
        .unwrap();
    assert_eq!(live.wait_result(before.runs), "SUCCESS");
    let runs = live.wait_more_runs(3);
    assert_all(&runs, "SUCCESS");
    live.assert_no_hit(probe);
    let after = live.probe_counters();
    assert!(after.probe_ticks >= before.probe_ticks + 4, "{after:?}");
}

#[test]
fn disable_all_hooks_releases_a_blocked_node() {
    let Some((mut live, before)) = stopped_at_probe() else {
        return;
    };
    let probe = live.uid("probe");
    live.client.disable_all_hooks().unwrap();
    // The woken node sees a disabled hook and ticks normally.
    assert_eq!(live.wait_result(before.runs), "SUCCESS");
    let runs = live.wait_more_runs(3);
    assert_all(&runs, "SUCCESS");
    live.assert_no_hit(probe);
    let after = live.probe_counters();
    assert!(after.probe_ticks >= before.probe_ticks + 4, "{after:?}");
    let disabled = Hook {
        enabled: false,
        ..Hook::breakpoint(probe)
    };
    assert_eq!(live.hooks(), [disabled]);

    // The hook stays installed: enable it again and the node stops.
    live.client.insert_hook(&Hook::breakpoint(probe)).unwrap();
    live.wait_hit(probe);
    live.assert_blocked();
}

#[test]
fn removing_the_hooks_releases_a_blocked_node() {
    let Some((mut live, before)) = stopped_at_probe() else {
        return;
    };
    let probe = live.uid("probe");

    // HOOK_REMOVE.
    live.client.remove_hook(probe, HookPosition::Pre).unwrap();
    assert_eq!(live.wait_result(before.runs), "SUCCESS");
    assert!(live.hooks().is_empty());
    let after = live.probe_counters();
    assert!(after.probe_ticks > before.probe_ticks, "{after:?}");

    // REMOVE_ALL_HOOKS.
    live.client.insert_hook(&Hook::breakpoint(probe)).unwrap();
    live.wait_hit(probe);
    let before = live.assert_blocked();
    live.client.remove_all_hooks().unwrap();
    assert_eq!(live.wait_result(before.runs), "SUCCESS");
    assert!(live.hooks().is_empty());
    let runs = live.wait_more_runs(2);
    assert_all(&runs, "SUCCESS");
    live.assert_no_hit(probe);
}

#[test]
fn an_unlock_before_the_hit_lets_one_hit_pass() {
    // BT.CPP sets the "ready" flag of a breakpoint also when the node does
    // not wait (unlockBreakpoint of the publisher). The next hit then sends
    // a notification but does not stop.
    let Some(harness) = real_support::harness() else {
        return;
    };
    let mut live = Live::probe(harness);
    let probe = live.uid("probe");
    live.sync_notifications(live.uid("count_runs"));

    live.client
        .insert_hook(&Hook {
            enabled: false,
            ..Hook::breakpoint(probe)
        })
        .unwrap();
    live.client
        .unlock_breakpoint(&UnlockRequest::resume(probe))
        .unwrap();
    live.client.insert_hook(&Hook::breakpoint(probe)).unwrap();
    // The first hit does not stop (no unlock follows), the second hit stops.
    live.wait_hit(probe);
    live.wait_hit(probe);
    let stopped = live.assert_blocked();
    assert_eq!(live.wait_result(stopped.runs - 1), "SUCCESS");
    assert_eq!(stopped.probe_ticks + 1, stopped.runs);
}

#[test]
fn post_hooks_are_stored_and_run_as_pre_hooks() {
    // Protocol facts 1 and 2 (docs/PLAN.md): BT.CPP 4.9.0 runs a hook with
    // position POST before the tick and stores it with the PRE hooks.
    let Some(harness) = real_support::harness() else {
        return;
    };
    let mut live = Live::probe(harness);
    let probe = live.uid("probe");
    live.sync_notifications(live.uid("count_runs"));

    let post = Hook {
        position: HookPosition::Post,
        ..Hook::breakpoint(probe)
    };
    live.client.insert_hook(&post).unwrap();
    assert_eq!(live.hooks(), [post]);
    live.wait_hit(probe);
    let before = live.assert_blocked();
    // The node stopped before its tick, not after it.
    assert_eq!(before.probe_ticks + 1, before.runs, "{before:?}");

    // Unlock and remove with position POST look in the empty POST map.
    for result in [
        live.client.unlock_breakpoint(&UnlockRequest {
            position: HookPosition::Post,
            ..UnlockRequest::resume(probe)
        }),
        live.client.remove_hook(probe, HookPosition::Post),
    ] {
        match result {
            Err(ClientError::Server(message)) => assert_eq!(message, "Node ID not found"),
            other => panic!("expected an error reply, got {other:?}"),
        }
    }
    live.assert_blocked();

    // With position PRE, both work.
    live.client
        .unlock_breakpoint(&UnlockRequest::resume(probe))
        .unwrap();
    assert_eq!(live.wait_result(before.runs), "SUCCESS");
    live.wait_hit(probe);
    let stopped = live.probe_counters();
    live.client.remove_hook(probe, HookPosition::Pre).unwrap();
    assert!(live.hooks().is_empty());
    assert_eq!(live.wait_result(stopped.runs), "SUCCESS");
}

#[test]
fn replace_hooks_return_the_status_without_a_tick() {
    let Some(harness) = real_support::harness() else {
        return;
    };
    let mut live = Live::probe(harness);
    let probe = live.uid("probe");
    live.sync_notifications(live.uid("count_runs"));

    // Replace with SUCCESS: the runs succeed and "probe" does not tick.
    live.client
        .insert_hook(&Hook::replace(probe, NodeStatus::Success))
        .unwrap();
    let start = live.probe_counters();
    let runs = live.wait_more_runs(3);
    assert_all(&runs, "SUCCESS");
    let after = live.probe_counters();
    assert_eq!(after.probe_ticks, start.probe_ticks, "{after:?}");
    assert!(after.done >= start.done + 3, "{after:?}");
    assert_eq!(after.probe_result.as_deref(), Some("success"));
    // A replace hook also sends a notification on each hit.
    live.wait_hit(probe);
    live.wait_hit(probe);

    // Change the same hook to FAILURE: the runs fail at "probe".
    live.client
        .insert_hook(&Hook::replace(probe, NodeStatus::Failure))
        .unwrap();
    let changed = live.probe_counters();
    let runs = live.wait_more_runs(3);
    assert_all(&runs, "FAILURE");
    let after = live.probe_counters();
    assert_eq!(after.probe_ticks, start.probe_ticks, "{after:?}");
    // Only a run that was past "probe" at the change can still complete.
    assert!(after.done <= changed.done + 1, "{after:?} {changed:?}");
    assert_eq!(after.probe_result.as_deref(), Some("failure"));
    assert_eq!(
        live.hooks(),
        [Hook::replace(probe, NodeStatus::Failure)],
        "HOOK_INSERT changes the hook in place"
    );

    // Remove the hook: normal behaviour comes back.
    live.client.remove_hook(probe, HookPosition::Pre).unwrap();
    let removed = live.probe_counters();
    let runs = live.wait_more_runs(3);
    assert_all(&runs, "SUCCESS");
    let after = live.probe_counters();
    assert!(after.probe_ticks >= removed.probe_ticks + 3, "{after:?}");
    assert_eq!(after.probe_result.as_deref(), Some("success"));
}

#[test]
fn replace_hook_with_once_replaces_one_tick() {
    let Some(harness) = real_support::harness() else {
        return;
    };
    let mut live = Live::probe(harness);
    let probe = live.uid("probe");

    let once = Hook {
        once: true,
        ..Hook::replace(probe, NodeStatus::Failure)
    };
    live.client.insert_hook(&once).unwrap();
    wait_until("the once hook to go away", || {
        live.hooks().is_empty().then_some(())
    });
    let runs = live.wait_more_runs(3);
    assert_all(&runs, "SUCCESS");
    // Exactly one run failed: the run in which the hook replaced the tick.
    let results = live.executor.results();
    let failures = results.iter().filter(|status| *status == "FAILURE").count();
    assert_eq!(failures, 1, "{results:?}");
    // One run has no real tick of "probe". The read can also fall between
    // "count_runs" and "probe" of a run.
    let counters = live.probe_counters();
    let missing = counters.runs - counters.probe_ticks;
    assert!(missing == 1 || missing == 2, "{counters:?}");
}

#[test]
fn changing_a_blocked_breakpoint_to_replace_releases_it() {
    let Some((mut live, before)) = stopped_at_probe() else {
        return;
    };
    let probe = live.uid("probe");
    let replace = Hook::replace(probe, NodeStatus::Failure);
    live.client.insert_hook(&replace).unwrap();
    // The waiting node wakes up and returns the new desired status.
    assert_eq!(live.wait_result(before.runs), "FAILURE");
    let runs = live.wait_more_runs(2);
    assert_all(&runs, "FAILURE");
    let after = live.probe_counters();
    assert_eq!(after.probe_ticks, before.probe_ticks, "{after:?}");
    assert_eq!(after.done, before.done, "{after:?}");
    assert_eq!(after.probe_result.as_deref(), Some("failure"));
    assert_eq!(live.hooks(), [replace]);
    assert_eq!(replace.mode, HookMode::Replace);
}

#[test]
fn heartbeat_loss_disables_the_hooks_and_requests_enable_them_again() {
    let Some(harness) = real_support::harness() else {
        return;
    };
    let heartbeat = Duration::from_millis(500);
    let mut options = options(harness, "probe.xml");
    options.heartbeat_ms = Some(heartbeat.as_millis() as u64);
    let mut live = Live::start(options);
    let probe = live.uid("probe");
    live.sync_notifications(live.uid("count_runs"));

    live.client.insert_hook(&Hook::breakpoint(probe)).unwrap();
    wait_hit_with_heartbeat(&mut live.client, probe);
    // The checks send requests, so the node stays blocked.
    let before = live.assert_blocked();
    live.assert_blocked();

    // No more requests: after the heartbeat timeout the publisher disables
    // all hooks and the node continues (heartbeatLoop of the publisher).
    let silent_since = Instant::now();
    assert_eq!(live.wait_result(before.runs), "SUCCESS");
    let released_after = silent_since.elapsed();
    assert!(
        released_after >= heartbeat - Duration::from_millis(150),
        "the node continued after {released_after:?}, before the heartbeat timeout"
    );
    // While the client stays silent, the runs go on without a stop and
    // without notifications.
    assert_eq!(live.wait_result(before.runs + 3), "SUCCESS");
    let end = Instant::now() + QUIET;
    while let Some(remaining) = end.checked_duration_since(Instant::now()) {
        if let Some(notification) = live.client.poll_notification(remaining).unwrap() {
            assert_ne!(
                notification.uid,
                Uid(probe),
                "a disabled hook sent a notification"
            );
        }
    }
    assert!(silent_since.elapsed() < WAIT);

    // Requests again: the publisher enables the hooks again, and the node
    // stops at the next run.
    wait_hit_with_heartbeat(&mut live.client, probe);
    let stopped = live.assert_blocked();
    assert!(stopped.runs > before.runs + 3, "{stopped:?}");
    assert!(stopped.probe_ticks > before.probe_ticks + 3, "{stopped:?}");
    assert_eq!(live.hooks(), [Hook::breakpoint(probe)]);
}
