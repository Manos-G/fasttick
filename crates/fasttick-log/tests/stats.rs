//! Tests of the node statistics.

mod support;

use std::collections::BTreeMap;

use fasttick_core::runtime::Uid;
use fasttick_core::status::NodeStatus;
use fasttick_log::btlog::{BtLog, BtLogWriter, Transition};
use fasttick_log::replay::ReplayIndex;
use fasttick_log::stats::{NodeStats, compute_log_stats, compute_stats};
use proptest::prelude::*;
use support::{SAMPLE_XML, START_US, naive_sort, t, transitions_strategy};

/// A simple, slow computation of the same values.
fn naive_stats(transitions: &[Transition], range: Option<(u64, u64)>) -> BTreeMap<Uid, NodeStats> {
    let sorted = naive_sort(transitions);
    let (start, end) = match range {
        Some((a, b)) => (a.min(b), a.max(b)),
        None => (0, sorted.last().map_or(0, |tr| tr.offset_us)),
    };
    let mut out: BTreeMap<Uid, NodeStats> = BTreeMap::new();
    let mut uids: Vec<Uid> = sorted.iter().map(|tr| tr.uid).collect();
    uids.sort();
    uids.dedup();
    for uid in uids {
        let mine: Vec<&Transition> = sorted.iter().filter(|tr| tr.uid == uid).collect();
        let mut stats = NodeStats::new(uid);

        // RUNNING intervals over the full log: (start, end or None).
        let mut intervals: Vec<(u64, Option<u64>)> = Vec::new();
        let mut open: Option<u64> = None;
        for tr in &mine {
            match (tr.status == NodeStatus::Running, open) {
                (true, None) => open = Some(tr.offset_us),
                (false, Some(since)) => {
                    intervals.push((since, Some(tr.offset_us)));
                    open = None;
                }
                _ => {}
            }
        }
        if let Some(since) = open {
            intervals.push((since, None));
        }
        for (a, b) in intervals {
            let b = b.unwrap_or(u64::MAX).min(end);
            let a = a.max(start);
            stats.running_time_us += b.saturating_sub(a);
        }

        let mut previous = NodeStatus::Idle;
        for tr in &mine {
            let in_range = tr.offset_us >= start && tr.offset_us <= end;
            if in_range {
                stats.transition_count += 1;
                if tr.status != previous {
                    stats.status_changes += 1;
                }
                match tr.status {
                    NodeStatus::Success => stats.success_count += 1,
                    NodeStatus::Failure => stats.failure_count += 1,
                    NodeStatus::Skipped => stats.skipped_count += 1,
                    _ => {}
                }
                if stats.first_transition_us.is_none() {
                    stats.first_transition_us = Some(tr.offset_us);
                }
                stats.last_transition_us = Some(tr.offset_us);
            }
            previous = tr.status;
        }
        out.insert(uid, stats);
    }
    out
}

fn as_map(stats: Vec<NodeStats>) -> BTreeMap<Uid, NodeStats> {
    stats.into_iter().map(|node| (node.uid, node)).collect()
}

#[test]
fn two_ticks() {
    let transitions = vec![
        t(100, 1, NodeStatus::Running),
        t(100, 2, NodeStatus::Running),
        t(200, 2, NodeStatus::Success),
        t(210, 3, NodeStatus::Running),
        t(300, 3, NodeStatus::Failure),
        t(300, 1, NodeStatus::Failure),
        t(310, 2, NodeStatus::Idle),
        t(310, 3, NodeStatus::Idle),
        t(310, 1, NodeStatus::Idle),
        t(1_000, 1, NodeStatus::Running),
        t(1_500, 1, NodeStatus::Success),
    ];
    let index = ReplayIndex::new(&transitions);
    let stats = as_map(compute_stats(&index, None));
    let root = stats[&Uid(1)];
    assert_eq!(root.running_time_us, 200 + 500);
    assert_eq!(root.status_changes, 5);
    assert_eq!(root.success_count, 1);
    assert_eq!(root.failure_count, 1);
    assert_eq!(root.first_transition_us, Some(100));
    assert_eq!(root.last_transition_us, Some(1_500));
    assert_eq!(stats[&Uid(2)].running_time_us, 100);
    assert_eq!(stats[&Uid(3)].running_time_us, 90);
    assert_eq!(stats[&Uid(3)].failure_count, 1);

    // A range in the middle of the second run of node 1.
    let stats = as_map(compute_stats(&index, Some((1_200, 1_300))));
    assert_eq!(stats[&Uid(1)].running_time_us, 100);
    assert_eq!(stats[&Uid(1)].transition_count, 0);
    assert_eq!(stats[&Uid(1)].first_transition_us, None);
    assert_eq!(stats[&Uid(2)].running_time_us, 0);

    // A range after the last transition: node 1 is not running.
    let stats = as_map(compute_stats(&index, Some((2_000, 3_000))));
    assert_eq!(stats[&Uid(1)].running_time_us, 0);
}

#[test]
fn open_interval_counts_until_range_end() {
    let index = ReplayIndex::new(&[t(10, 4, NodeStatus::Running)]);
    let stats = compute_stats(&index, Some((0, 1_000)));
    assert_eq!(stats[0].running_time_us, 990);
    let stats = compute_stats(&index, None);
    assert_eq!(stats[0].running_time_us, 0);
}

#[test]
fn empty_index() {
    let index = ReplayIndex::new(&[]);
    assert!(compute_stats(&index, None).is_empty());
    assert!(compute_stats(&index, Some((0, 10))).is_empty());
}

#[test]
fn log_stats_include_all_tree_nodes() {
    let mut writer = BtLogWriter::new(Vec::new(), SAMPLE_XML, START_US).unwrap();
    writer.push(t(5, 2, NodeStatus::Running)).unwrap();
    writer.push(t(9, 2, NodeStatus::Success)).unwrap();
    writer.push(t(9, 77, NodeStatus::Failure)).unwrap();
    let log = BtLog::from_bytes(&writer.into_inner()).unwrap();
    let stats = compute_log_stats(&log, None);
    let uids: Vec<u16> = stats.iter().map(|node| node.uid.0).collect();
    assert_eq!(uids, vec![1, 2, 3, 4, 5, 6, 7, 8, 77]);
    assert_eq!(stats[1].running_time_us, 4);
    assert_eq!(stats[8].failure_count, 1);
    assert_eq!(stats[0], NodeStats::new(Uid(1)));
}

proptest! {
    #![proptest_config(ProptestConfig::with_cases(128))]

    #[test]
    fn prop_stats_match_naive(
        transitions in transitions_strategy(6, 200, 150),
        interval in 1usize..10,
        range in prop::option::of((0u64..260, 0u64..260)),
    ) {
        let index = ReplayIndex::with_snapshot_interval(&transitions, interval);
        let fast = as_map(compute_stats(&index, range));
        let slow = naive_stats(&transitions, range);
        prop_assert_eq!(fast, slow);
    }
}
