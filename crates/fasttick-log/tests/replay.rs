//! Tests of the replay index and the playback cursor.

mod support;

use std::collections::HashMap;
use std::time::Duration;

use fasttick_core::runtime::Uid;
use fasttick_core::status::NodeStatus;
use fasttick_log::btlog::{BtLog, BtLogWriter, Transition};
use fasttick_log::replay::{Cursor, NodeState, PlaybackSpeed, ReplayIndex, ReplayState};
use proptest::prelude::*;
use support::{
    SAMPLE_XML, START_US, naive_sort, naive_state, naive_state_at, t, transitions_strategy,
};

fn as_map(state: &ReplayState) -> HashMap<Uid, NodeState> {
    state
        .iter()
        .filter(|(_, node)| *node != NodeState::default())
        .collect()
}

fn same_state(state: &ReplayState, expected: &HashMap<Uid, NodeState>) -> bool {
    let without_default: HashMap<Uid, NodeState> = expected
        .iter()
        .filter(|(_, node)| **node != NodeState::default())
        .map(|(uid, node)| (*uid, *node))
        .collect();
    as_map(state) == without_default && expected.iter().all(|(uid, node)| state.get(*uid) == *node)
}

/// A small log: two ticks of a sequence with two children.
fn ticks() -> Vec<Transition> {
    vec![
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
    ]
}

#[test]
fn queries_on_a_small_log() {
    let index = ReplayIndex::with_snapshot_interval(&ticks(), 3);
    assert_eq!(index.len(), 10);
    assert_eq!(index.first_time(), Some(100));
    assert_eq!(index.last_time(), Some(1_000));
    assert_eq!(index.time_of_index(4), Some(300));
    assert_eq!(index.time_of_index(10), None);
    assert_eq!(index.index_at_time(99), None);
    assert_eq!(index.index_at_time(100), Some(1));
    assert_eq!(index.index_at_time(305), Some(5));
    assert_eq!(index.index_at_time(5_000), Some(9));
    assert_eq!(index.indices_in_range((200, 310)), 2..9);
    assert_eq!(index.indices_in_range((310, 200)), 2..9);
    assert_eq!(index.uids(), &[Uid(1), Uid(2), Uid(3)]);

    let state = index.state_at(305);
    assert_eq!(state.status(Uid(1)), NodeStatus::Failure);
    assert_eq!(state.status(Uid(2)), NodeStatus::Success);
    assert_eq!(state.status(Uid(3)), NodeStatus::Failure);

    let state = index.state_at(310);
    assert_eq!(
        state.get(Uid(2)),
        NodeState {
            status: NodeStatus::Idle,
            last_non_idle: Some(NodeStatus::Success)
        }
    );
    assert_eq!(state.get(Uid(3)).last_non_idle, Some(NodeStatus::Failure));
    // An unknown UID is IDLE.
    assert_eq!(state.get(Uid(99)), NodeState::default());

    assert_eq!(index.state_at_index(0).status(Uid(1)), NodeStatus::Running);
    assert_eq!(index.state_at_index(0).status(Uid(2)), NodeStatus::Idle);
    assert_eq!(index.state_after(0), index.state_at(0));
    assert_eq!(index.state_after(1000), index.state_at(u64::MAX));
}

#[test]
fn from_log_knows_all_tree_nodes() {
    let mut writer = BtLogWriter::new(Vec::new(), SAMPLE_XML, START_US).unwrap();
    writer.push(t(5, 300, NodeStatus::Running)).unwrap();
    let log = BtLog::from_bytes(&writer.into_inner()).unwrap();
    let index = ReplayIndex::from_log(&log);
    let uids: Vec<u16> = index.uids().iter().map(|uid| uid.0).collect();
    assert_eq!(uids, vec![1, 2, 3, 4, 5, 6, 7, 8, 300]);
    let state = index.state_at(10);
    assert_eq!(state.len(), 9);
    assert_eq!(state.status(Uid(300)), NodeStatus::Running);
}

#[test]
fn extend_in_order_and_out_of_order() {
    let all = ticks();
    for split in 0..=all.len() {
        let mut index = ReplayIndex::with_snapshot_interval(&all[..split], 2);
        index.extend(&all[split..], split);
        let full = ReplayIndex::with_snapshot_interval(&all, 2);
        assert_eq!(index.transitions(), full.transitions(), "split {split}");
        for count in 0..=all.len() {
            assert_eq!(index.state_after(count), full.state_after(count));
            assert_eq!(index.source_index(count), full.source_index(count));
        }
    }

    // New transitions before the last time, and a new UID: rebuild.
    let mut index = ReplayIndex::with_snapshot_interval(&all, 4);
    let late = [
        t(150, 7, NodeStatus::Running),
        t(2_000, 1, NodeStatus::Success),
    ];
    assert!(!index.extend(&late, all.len()));
    let mut combined = all.clone();
    combined.extend_from_slice(&late);
    let full = ReplayIndex::with_snapshot_interval(&combined, 4);
    assert_eq!(index.transitions(), full.transitions());
    assert_eq!(index.uids(), full.uids());
    for count in 0..=combined.len() {
        assert_eq!(index.state_after(count), full.state_after(count));
        assert_eq!(index.source_index(count), full.source_index(count));
    }
    assert_eq!(index.source_index(2), Some(10));
}

#[test]
fn cursor_steps() {
    let index = ReplayIndex::new(&ticks());
    let mut cursor = Cursor::new();
    assert_eq!(cursor.applied(), 0);
    assert_eq!(cursor.current_index(), None);
    assert!(!cursor.step_back(&index));

    assert!(cursor.step_forward(&index));
    assert_eq!(cursor.current_index(), Some(0));
    assert_eq!(cursor.time_us(), 100);
    assert!(cursor.step_forward(&index));
    // Two transitions at the same time: steps go one by one.
    assert_eq!(cursor.applied(), 2);
    assert_eq!(cursor.time_us(), 100);
    assert_eq!(cursor.state(&index), index.state_after(2));

    for _ in 0..20 {
        cursor.step_forward(&index);
    }
    assert_eq!(cursor.applied(), 10);
    assert_eq!(cursor.time_us(), 1_000);
    assert!(!cursor.step_forward(&index));

    assert!(cursor.step_back(&index));
    assert_eq!(cursor.applied(), 9);
    assert_eq!(cursor.time_us(), 310);
    cursor.go_to_start(&index);
    assert_eq!(cursor.applied(), 0);
    assert_eq!(cursor.time_us(), 0);
    cursor.go_to_end(&index);
    assert_eq!(cursor.applied(), 10);

    cursor.seek_to_index(&index, 4);
    assert_eq!(cursor.applied(), 5);
    assert_eq!(cursor.time_us(), 300);
    cursor.seek_to_index(&index, 500);
    assert_eq!(cursor.applied(), 10);
}

#[test]
fn cursor_seek_and_range() {
    let index = ReplayIndex::new(&ticks());
    let mut cursor = Cursor::new();
    cursor.seek(&index, 250);
    assert_eq!(cursor.applied(), 4);
    assert_eq!(cursor.time_us(), 250);
    cursor.seek(&index, 50_000);
    assert_eq!(cursor.time_us(), 1_000);
    assert_eq!(cursor.applied(), 10);

    // Zoom to 200..310: the cursor moves into the range.
    cursor.set_range(&index, Some((310, 200)));
    assert_eq!(cursor.range(), Some((200, 310)));
    assert_eq!(cursor.bounds(&index), (200, 310));
    assert_eq!(cursor.time_us(), 310);
    assert_eq!(cursor.applied(), 9);
    cursor.seek(&index, 0);
    assert_eq!(cursor.time_us(), 200);
    assert_eq!(cursor.applied(), 3);

    // Step back to the start of the range: nothing of the range applied.
    assert!(cursor.step_back(&index));
    assert_eq!(cursor.applied(), 2);
    assert_eq!(cursor.time_us(), 200);
    assert!(!cursor.step_back(&index));
    cursor.seek_to_index(&index, 0);
    assert_eq!(cursor.applied(), 2);

    // Step forward stops at the end of the range.
    for _ in 0..20 {
        cursor.step_forward(&index);
    }
    assert_eq!(cursor.applied(), 9);
    assert_eq!(cursor.time_us(), 310);

    cursor.set_range(&index, None);
    assert!(cursor.step_forward(&index));
    assert_eq!(cursor.applied(), 10);
}

#[test]
fn cursor_playback() {
    let index = ReplayIndex::new(&ticks());
    let mut cursor = Cursor::new();
    assert!(!cursor.advance(&index, Duration::from_millis(10)));
    cursor.play(&index);
    assert!(cursor.is_playing());

    // 150 µs of real time at 1x.
    assert!(cursor.advance(&index, Duration::from_micros(150)));
    assert_eq!(cursor.time_us(), 150);
    assert_eq!(cursor.applied(), 2);
    // No new transition: the state does not change.
    assert!(!cursor.advance(&index, Duration::from_micros(10)));
    assert_eq!(cursor.time_us(), 160);

    // Fractions of a microsecond add up.
    cursor.set_speed(PlaybackSpeed::new(0.1));
    for _ in 0..10 {
        cursor.advance(&index, Duration::from_nanos(1_000));
    }
    assert_eq!(cursor.time_us(), 161);

    // 100x: 10 µs of real time is 1000 µs of log time; stops at the end.
    cursor.set_speed(PlaybackSpeed::new(100.0));
    assert!(cursor.advance(&index, Duration::from_micros(10)));
    assert_eq!(cursor.time_us(), 1_000);
    assert_eq!(cursor.applied(), 10);
    assert!(!cursor.is_playing());

    // Play at the end starts again at the start.
    cursor.play(&index);
    assert_eq!(cursor.time_us(), 0);
    assert_eq!(cursor.applied(), 0);
    cursor.toggle_play(&index);
    assert!(!cursor.is_playing());
    assert!(!cursor.advance(&index, Duration::from_secs(1)));

    // Playback in a range stops at the range end.
    cursor.set_range(&index, Some((200, 305)));
    assert_eq!(cursor.time_us(), 200);
    assert_eq!(cursor.applied(), 3);
    cursor.toggle_play(&index);
    cursor.set_speed(PlaybackSpeed::NORMAL);
    cursor.advance(&index, Duration::from_secs(1));
    assert_eq!(cursor.time_us(), 305);
    assert_eq!(cursor.applied(), 6);
    assert!(!cursor.is_playing());
    cursor.play(&index);
    assert_eq!(cursor.time_us(), 200);
    assert_eq!(cursor.applied(), 2);
    assert!(cursor.advance(&index, Duration::from_micros(1)));
    assert_eq!(cursor.applied(), 3);
}

#[test]
fn speed_serde_clamps() {
    let speed: PlaybackSpeed = serde_json::from_str("500.0").unwrap();
    assert_eq!(speed.factor(), 100.0);
    assert_eq!(
        serde_json::to_string(&PlaybackSpeed::new(2.0)).unwrap(),
        "2.0"
    );
}

proptest! {
    #![proptest_config(ProptestConfig::with_cases(96))]

    #[test]
    fn prop_state_matches_linear_scan(
        transitions in transitions_strategy(8, 60, 120),
        interval in 1usize..12,
        times in prop::collection::vec(0u64..70, 1..10),
    ) {
        let index = ReplayIndex::with_snapshot_interval(&transitions, interval);
        let sorted = naive_sort(&transitions);
        prop_assert_eq!(index.transitions(), &sorted[..]);
        for count in 0..=sorted.len() {
            let expected = naive_state(&sorted, count);
            prop_assert!(same_state(&index.state_after(count), &expected), "count {}", count);
        }
        for time in times {
            let expected = naive_state_at(&sorted, time);
            prop_assert!(same_state(&index.state_at(time), &expected), "time {}", time);
            let last = sorted.iter().rposition(|tr| tr.offset_us <= time);
            prop_assert_eq!(index.index_at_time(time), last);
        }
        for (position, transition) in sorted.iter().enumerate() {
            prop_assert_eq!(index.time_of_index(position), Some(transition.offset_us));
            let source = index.source_index(position).unwrap();
            prop_assert_eq!(transitions[source], *transition);
        }
    }

    #[test]
    fn prop_default_interval_matches_small_interval(
        transitions in transitions_strategy(30, 5_000, 3_000),
        time in 0u64..5_100,
    ) {
        let index = ReplayIndex::new(&transitions);
        let small = ReplayIndex::with_snapshot_interval(&transitions, 7);
        prop_assert_eq!(index.state_at(time), small.state_at(time));
        let sorted = naive_sort(&transitions);
        prop_assert!(same_state(&index.state_at(time), &naive_state_at(&sorted, time)));
    }

    #[test]
    fn prop_cursor_stays_consistent(
        transitions in transitions_strategy(5, 100, 40),
        actions in prop::collection::vec((0u8..8, 0u64..120), 0..40),
    ) {
        let index = ReplayIndex::with_snapshot_interval(&transitions, 3);
        let mut cursor = Cursor::new();
        for (action, value) in actions {
            match action {
                0 => { cursor.step_forward(&index); }
                1 => { cursor.step_back(&index); }
                2 => cursor.seek(&index, value),
                3 => cursor.set_range(&index, Some((value, value.saturating_add(30)))),
                4 => cursor.set_range(&index, None),
                5 => cursor.play(&index),
                6 => { cursor.advance(&index, Duration::from_micros(value)); }
                _ => cursor.seek_to_index(&index, value as usize),
            }
            let (start, end) = cursor.bounds(&index);
            prop_assert!(start <= end);
            prop_assert!(cursor.time_us() >= start && cursor.time_us() <= end);
            prop_assert!(cursor.applied() >= index.count_before_time(start));
            prop_assert!(cursor.applied() <= index.count_at_time(end));
            prop_assert!(cursor.applied() <= index.count_at_time(cursor.time_us()));
            prop_assert!(cursor.applied() >= index.count_before_time(cursor.time_us()));
            if action == 2 {
                prop_assert_eq!(cursor.state(&index), index.state_at(cursor.time_us()));
            }
        }
    }
}

#[test]
fn extend_in_place_keeps_positions() {
    let all = ticks();
    let mut index = ReplayIndex::with_snapshot_interval(&all[..6], 4);
    let before: Vec<Transition> = index.transitions().to_vec();
    // Known UIDs, at or after the last time (300): in place.
    assert!(index.extend(&all[6..], 6));
    assert_eq!(&index.transitions()[..6], &before[..]);
    assert_eq!(index.snapshot_interval(), 4);
    // Nothing new: in place.
    assert!(index.extend(&[], 10));
    assert_eq!(index.len(), 10);
}

#[test]
fn default_interval_grows_with_the_number_of_uids() {
    // Few UIDs: the default interval.
    assert_eq!(ReplayIndex::new(&ticks()).snapshot_interval(), 1024);
    // 3000 different UIDs: one snapshot for each 3000 transitions.
    let many: Vec<Transition> = (0..9_000u16)
        .map(|position| t(u64::from(position), position % 3_000, NodeStatus::Running))
        .collect();
    let index = ReplayIndex::new(&many);
    assert_eq!(index.uids().len(), 3_000);
    assert_eq!(index.snapshot_interval(), 3_000);
    let small = ReplayIndex::with_snapshot_interval(&many, 5);
    assert_eq!(small.snapshot_interval(), 5);
    for time in [0, 1, 2_999, 3_000, 4_500, 8_999, 10_000] {
        assert_eq!(index.state_at(time), small.state_at(time), "time {time}");
    }

    // A rebuild with more UIDs makes the interval larger again.
    let mut index = ReplayIndex::new(&ticks());
    let more: Vec<Transition> = (0..2_000u16)
        .map(|uid| t(50, uid + 10, NodeStatus::Success))
        .collect();
    assert!(!index.extend(&more, 10));
    assert_eq!(index.uids().len(), 2_003);
    assert_eq!(index.snapshot_interval(), 2_003);
    let mut combined = ticks();
    combined.extend_from_slice(&more);
    let full = ReplayIndex::with_snapshot_interval(&combined, 3);
    for count in [0, 1, 500, 2_003, 2_009, 2_010] {
        assert_eq!(
            index.state_after(count),
            full.state_after(count),
            "count {count}"
        );
    }
}

#[test]
fn cursor_after_a_rebuild() {
    let all = ticks();
    let mut index = ReplayIndex::new(&all);
    let mut cursor = Cursor::new();
    cursor.seek(&index, 305);
    assert_eq!(cursor.applied(), 6);
    // Late transitions before the cursor time: the cursor is stale.
    assert!(!index.extend(
        &[
            t(250, 3, NodeStatus::Running),
            t(260, 5, NodeStatus::Running)
        ],
        10
    ));
    cursor.clamp(&index);
    assert_eq!(cursor.time_us(), 305);
    assert_eq!(cursor.applied(), 8);
    assert_eq!(cursor.state(&index), index.state_at(305));
}

#[test]
fn cursor_extreme_values() {
    let index = ReplayIndex::new(&ticks());
    let mut cursor = Cursor::new();
    cursor.seek_to_index(&index, usize::MAX);
    assert_eq!(cursor.applied(), 10);
    cursor.seek(&index, u64::MAX);
    assert_eq!(cursor.time_us(), 1_000);
    cursor.go_to_start(&index);
    cursor.set_speed(PlaybackSpeed::new(f64::INFINITY));
    assert_eq!(cursor.speed().factor(), 100.0);
    cursor.play(&index);
    assert!(cursor.advance(&index, Duration::MAX));
    assert_eq!(cursor.time_us(), 1_000);
    assert_eq!(cursor.applied(), 10);
    assert!(!cursor.is_playing());

    // A range after the last transition.
    cursor.set_range(&index, Some((5_000, u64::MAX)));
    assert_eq!(cursor.time_us(), 5_000);
    assert_eq!(cursor.applied(), 10);
    assert!(!cursor.step_forward(&index));
    assert!(!cursor.step_back(&index));
    cursor.set_speed(PlaybackSpeed::NORMAL);
    cursor.play(&index);
    assert!(!cursor.advance(&index, Duration::from_secs(1)));
    assert_eq!(cursor.time_us(), 5_000 + 1_000_000);
    assert!(cursor.is_playing());
}

proptest! {
    #![proptest_config(ProptestConfig::with_cases(96))]

    /// Adding transitions in chunks (in order or not) gives the same index
    /// as one build of all transitions.
    #[test]
    fn prop_extend_equals_full_build(
        transitions in transitions_strategy(8, 80, 120),
        cuts in prop::collection::vec(any::<prop::sample::Index>(), 0..6),
        interval in 1usize..10,
        with_cursor in any::<bool>(),
    ) {
        let mut points: Vec<usize> = cuts.iter().map(|cut| cut.index(transitions.len() + 1)).collect();
        points.push(0);
        points.push(transitions.len());
        points.sort_unstable();
        let mut index = ReplayIndex::with_snapshot_interval(&[], interval);
        let mut cursor = Cursor::new();
        for pair in points.windows(2) {
            index.extend(&transitions[pair[0]..pair[1]], pair[0]);
            if with_cursor {
                cursor.clamp(&index);
                prop_assert!(cursor.applied() >= index.count_before_time(cursor.time_us()));
                prop_assert!(cursor.applied() <= index.count_at_time(cursor.time_us()));
                cursor.step_forward(&index);
            }
        }
        let full = ReplayIndex::with_snapshot_interval(&transitions, interval);
        prop_assert_eq!(index.transitions(), full.transitions());
        prop_assert_eq!(index.uids(), full.uids());
        for count in 0..=transitions.len() {
            prop_assert_eq!(index.state_after(count), full.state_after(count));
            prop_assert_eq!(index.source_index(count), full.source_index(count));
        }
    }
}
