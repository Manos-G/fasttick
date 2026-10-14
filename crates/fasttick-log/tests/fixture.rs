//! Tests with a real `.btlog` file from BT.CPP 4.9.0.
//!
//! `tests/fixtures/harness/main.btlog` is the output of `BT::FileLogger2` in
//! the FastTick test executor (`test_harness/trees/main.xml`, one run). The
//! expected values below come from a separate decoder (a short script that
//! follows `bt_file_logger_v2.cpp`), not from this crate.

use std::io::Read;

use fasttick_core::runtime::Uid;
use fasttick_core::status::NodeStatus;
use fasttick_log::btlog::{BtLog, BtLogReader, Transition};
use fasttick_log::export::export_rows;
use fasttick_log::filter::TransitionFilter;
use fasttick_log::replay::{NodeState, ReplayIndex};
use fasttick_log::stats::compute_log_stats;

const START_TIME_US: i64 = 1_792_015_886_879_960;
const XML_LEN: usize = 13_436;
const TRANSITIONS: usize = 111;
const LAST_OFFSET_US: u64 = 202_568;

fn fixture_bytes() -> Vec<u8> {
    let path = fasttick_testkit::fixture_path("main.btlog");
    std::fs::read(&path).unwrap_or_else(|err| panic!("cannot read {}: {err}", path.display()))
}

fn t(offset_us: u64, uid: u16, status: NodeStatus) -> Transition {
    Transition::new(offset_us, Uid(uid), status)
}

#[test]
fn header_and_records() {
    let bytes = fixture_bytes();
    let log = BtLog::from_bytes(&bytes).unwrap();
    assert_eq!(log.xml.len(), XML_LEN);
    assert!(log.xml.starts_with("<root BTCPP_format=\"4\">"));
    assert_eq!(log.start_time_us, START_TIME_US);
    assert_eq!(log.transitions.len(), TRANSITIONS);
    assert_eq!(log.invalid_records, 0);
    assert_eq!(log.truncated_bytes, 0);
    assert_eq!(bytes.len(), 18 + 1 + 4 + XML_LEN + 8 + TRANSITIONS * 9);

    // The first records.
    assert_eq!(
        log.transitions[..6],
        [
            t(383, 1, NodeStatus::Running),
            t(413, 2, NodeStatus::Success),
            t(425, 3, NodeStatus::Success),
            t(439, 4, NodeStatus::Success),
            t(441, 5, NodeStatus::Success),
            t(441, 6, NodeStatus::Running),
        ]
    );
    // The root ends with SUCCESS and IDLE in the same microsecond.
    assert_eq!(
        log.transitions[TRANSITIONS - 2..],
        [
            t(LAST_OFFSET_US, 1, NodeStatus::Success),
            t(LAST_OFFSET_US, 1, NodeStatus::Idle),
        ]
    );
    assert_eq!(log.end_offset_us(), LAST_OFFSET_US);
    // FileLogger2 writes in time order.
    assert!(
        log.transitions
            .windows(2)
            .all(|pair| pair[0].offset_us <= pair[1].offset_us)
    );

    // The tree: 43 nodes, two SubTree instances, and all UIDs are known.
    assert_eq!(log.tree.len(), 43);
    assert_eq!(log.tree.instances.len(), 3);
    assert!(log.unknown_uids().is_empty());
    assert_eq!(log.tree.node_by_uid(Uid(1)).unwrap().name, "main_sequence");
    let navigate = log.tree.index_of_uid(Uid(25)).unwrap();
    assert_eq!(log.tree.node_path(navigate), "navigate/drive");
}

#[test]
fn writer_reproduces_the_file() {
    let bytes = fixture_bytes();
    let log = BtLog::from_bytes(&bytes).unwrap();
    assert_eq!(log.to_bytes().unwrap(), bytes);
}

#[test]
fn small_reads_give_the_same_log() {
    /// A reader that gives at most `step` bytes for each call.
    struct Trickle<'a> {
        data: &'a [u8],
        step: usize,
    }
    impl Read for Trickle<'_> {
        fn read(&mut self, buf: &mut [u8]) -> std::io::Result<usize> {
            let count = self.step.min(buf.len()).min(self.data.len());
            buf[..count].copy_from_slice(&self.data[..count]);
            self.data = &self.data[count..];
            Ok(count)
        }
    }

    let bytes = fixture_bytes();
    let all = BtLog::from_bytes(&bytes).unwrap();
    for step in [1, 7, 9, 4096] {
        let mut reader = BtLogReader::new();
        let added = reader
            .read_from(&mut Trickle { data: &bytes, step })
            .unwrap();
        assert_eq!(added, TRANSITIONS, "step {step}");
        assert_eq!(reader.into_log().unwrap(), all, "step {step}");
    }

    // A file that the executor still writes: a growing prefix.
    let mut reader = BtLogReader::new();
    let mut total = 0;
    for chunk in bytes.chunks(1000) {
        total += reader.push(chunk).unwrap();
        let header_done = reader.header().is_some();
        if header_done {
            let log = reader.to_log().unwrap();
            assert_eq!(log.transitions, all.transitions[..log.transitions.len()]);
        }
    }
    assert_eq!(total, TRANSITIONS);
}

#[test]
fn replay_stats_and_export() {
    let log = BtLog::from_bytes(&fixture_bytes()).unwrap();
    let index = ReplayIndex::from_log(&log);
    assert_eq!(index.len(), TRANSITIONS);
    assert_eq!(index.uids().len(), 43);

    // During the run: the root is RUNNING.
    let state = index.state_at(100_000);
    assert_eq!(state.status(Uid(1)), NodeStatus::Running);
    // At the end: all nodes are back to IDLE, and the root shows SUCCESS as
    // its previous result.
    let end = index.state_at(LAST_OFFSET_US);
    assert!(end.iter().all(|(_, node)| node.status == NodeStatus::Idle));
    assert_eq!(
        end.get(Uid(1)),
        NodeState {
            status: NodeStatus::Idle,
            last_non_idle: Some(NodeStatus::Success),
        }
    );
    // The SubTree node "navigate" (UID 22) ran from 101444 to 141798.
    assert_eq!(index.state_at(120_000).status(Uid(22)), NodeStatus::Running);
    assert_eq!(index.state_at(141_798).status(Uid(22)), NodeStatus::Success);

    let stats = compute_log_stats(&log, None);
    assert_eq!(stats.len(), 43);
    let root = stats.iter().find(|node| node.uid == Uid(1)).unwrap();
    assert_eq!(root.running_time_us, LAST_OFFSET_US - 383);
    assert_eq!(root.success_count, 1);
    assert_eq!(root.transition_count, 3);
    let navigate = stats.iter().find(|node| node.uid == Uid(22)).unwrap();
    assert_eq!(navigate.running_time_us, 141_798 - 101_444);
    // Node 31 ("skipped_node") has no transitions.
    let skipped = stats.iter().find(|node| node.uid == Uid(31)).unwrap();
    assert_eq!(skipped.transition_count, 0);
    assert_eq!(skipped.first_transition_us, None);

    let rows = export_rows(&log, &TransitionFilter::new());
    assert_eq!(rows.len(), TRANSITIONS);
    assert_eq!(rows[0].absolute_time_us, START_TIME_US + 383);
    let filter = TransitionFilter {
        include: vec!["navigate/*".into()],
        ..TransitionFilter::default()
    };
    let rows = export_rows(&log, &filter);
    assert!(!rows.is_empty());
    assert!(
        rows.iter()
            .all(|row| row.node_path.starts_with("navigate/"))
    );
}
