//! Statistics for each node in a time range.
//!
//! Definitions (all times are offsets from the log start, in microseconds):
//!
//! - The range `(start, end)` is inclusive. Without a range, it is `0` to
//!   the time of the last transition.
//! - A RUNNING interval starts at a transition to RUNNING when the node was
//!   not RUNNING. It ends at the next transition of the node to a status
//!   that is not RUNNING. `running_time_us` is the sum of these intervals,
//!   clipped to the range. An interval that started before the range counts
//!   from the range start. An interval that is still open at the range end
//!   counts until the range end.
//! - `transition_count` counts the transitions of the node in the range.
//!   `status_changes` counts only the transitions that change the status
//!   (BT.CPP logs only changes, so the two values are usually equal).
//! - `success_count`, `failure_count` and `skipped_count` count the
//!   transitions in the range to SUCCESS, FAILURE and SKIPPED.

use fasttick_core::runtime::Uid;
use fasttick_core::status::NodeStatus;
use serde::{Deserialize, Serialize};

use crate::btlog::BtLog;
use crate::ordered_range;
use crate::replay::ReplayIndex;

/// Statistics for one node. See the module documentation.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct NodeStats {
    /// The node.
    pub uid: Uid,
    /// The time in RUNNING, in microseconds.
    pub running_time_us: u64,
    /// The number of transitions that change the status.
    pub status_changes: u64,
    /// The number of transitions.
    pub transition_count: u64,
    /// The number of transitions to SUCCESS.
    pub success_count: u64,
    /// The number of transitions to FAILURE.
    pub failure_count: u64,
    /// The number of transitions to SKIPPED.
    pub skipped_count: u64,
    /// The time of the first transition in the range.
    pub first_transition_us: Option<u64>,
    /// The time of the last transition in the range.
    pub last_transition_us: Option<u64>,
}

impl NodeStats {
    /// Empty statistics for a node.
    pub fn new(uid: Uid) -> NodeStats {
        NodeStats {
            uid,
            running_time_us: 0,
            status_changes: 0,
            transition_count: 0,
            success_count: 0,
            failure_count: 0,
            skipped_count: 0,
            first_transition_us: None,
            last_transition_us: None,
        }
    }
}

/// Computes the statistics of all UIDs that the index knows, sorted by
/// UID. `range` is an inclusive time range; `None` means the full log.
///
/// The cost is `O(log n + K + m + r)`: `m` nodes, `r` transitions in the
/// range, `K` the snapshot interval of the index.
pub fn compute_stats(index: &ReplayIndex, range: Option<(u64, u64)>) -> Vec<NodeStats> {
    let (start, end) = match range {
        Some(range) => ordered_range(range),
        None => (0, index.last_time().unwrap_or(0)),
    };
    let uids = index.uids();
    let first = index.count_before_time(start);
    let last = index.count_at_time(end);

    // The status of each node just before the range.
    let initial = index.state_after(first);
    let mut status: Vec<NodeStatus> = uids.iter().map(|uid| initial.status(*uid)).collect();
    let mut running_since: Vec<Option<u64>> = status
        .iter()
        .map(|status| (*status == NodeStatus::Running).then_some(start))
        .collect();
    let mut stats: Vec<NodeStats> = uids.iter().map(|uid| NodeStats::new(*uid)).collect();

    for transition in index.transitions().get(first..last).unwrap_or_default() {
        let Ok(slot) = uids.binary_search(&transition.uid) else {
            continue;
        };
        let time = transition.offset_us;
        let node = &mut stats[slot];
        node.transition_count += 1;
        node.first_transition_us.get_or_insert(time);
        node.last_transition_us = Some(time);
        if transition.status != status[slot] {
            node.status_changes += 1;
        }
        match transition.status {
            NodeStatus::Success => node.success_count += 1,
            NodeStatus::Failure => node.failure_count += 1,
            NodeStatus::Skipped => node.skipped_count += 1,
            NodeStatus::Idle | NodeStatus::Running => {}
        }
        if transition.status == NodeStatus::Running {
            running_since[slot].get_or_insert(time);
        } else if let Some(since) = running_since[slot].take() {
            node.running_time_us += time.saturating_sub(since);
        }
        status[slot] = transition.status;
    }
    for (node, since) in stats.iter_mut().zip(running_since) {
        if let Some(since) = since {
            node.running_time_us += end.saturating_sub(since);
        }
    }
    stats
}

/// Computes the statistics of a log: all nodes of the tree and all UIDs of
/// the transitions. See [`compute_stats`].
pub fn compute_log_stats(log: &BtLog, range: Option<(u64, u64)>) -> Vec<NodeStats> {
    compute_stats(&ReplayIndex::from_log(log), range)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::btlog::Transition;

    fn t(offset_us: u64, uid: u16, status: NodeStatus) -> Transition {
        Transition::new(offset_us, Uid(uid), status)
    }

    fn running(stats: &[NodeStats], uid: u16) -> u64 {
        stats
            .iter()
            .find(|node| node.uid == Uid(uid))
            .map(|node| node.running_time_us)
            .unwrap()
    }

    #[test]
    fn running_time_is_clipped() {
        let index = ReplayIndex::new(&[
            t(10, 1, NodeStatus::Running),
            t(50, 1, NodeStatus::Success),
            t(60, 1, NodeStatus::Idle),
            t(70, 1, NodeStatus::Running),
        ]);
        // Full log: 10..50 and 70..70 (open at the end of the log).
        assert_eq!(running(&compute_stats(&index, None), 1), 40);
        // Starts inside an interval.
        assert_eq!(running(&compute_stats(&index, Some((20, 55))), 1), 30);
        // Open interval counts until the range end.
        assert_eq!(running(&compute_stats(&index, Some((65, 100))), 1), 30);
        // Range in reverse order.
        assert_eq!(running(&compute_stats(&index, Some((100, 65))), 1), 30);
        // A range with no running time.
        assert_eq!(running(&compute_stats(&index, Some((50, 69))), 1), 0);
    }

    #[test]
    fn counts() {
        let index = ReplayIndex::new(&[
            t(1, 1, NodeStatus::Running),
            t(2, 1, NodeStatus::Running),
            t(3, 1, NodeStatus::Success),
            t(4, 1, NodeStatus::Idle),
            t(5, 1, NodeStatus::Failure),
            t(6, 1, NodeStatus::Skipped),
        ]);
        let stats = compute_stats(&index, None);
        assert_eq!(stats.len(), 1);
        let node = stats[0];
        assert_eq!(node.transition_count, 6);
        assert_eq!(node.status_changes, 5);
        assert_eq!(node.success_count, 1);
        assert_eq!(node.failure_count, 1);
        assert_eq!(node.skipped_count, 1);
        assert_eq!(node.first_transition_us, Some(1));
        assert_eq!(node.last_transition_us, Some(6));
        assert_eq!(node.running_time_us, 2);
    }
}
