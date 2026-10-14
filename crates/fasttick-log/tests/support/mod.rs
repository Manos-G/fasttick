//! Shared test data and naive reference implementations.

#![allow(dead_code)]

use std::collections::{BTreeMap, HashMap};

use fasttick_core::runtime::Uid;
use fasttick_core::status::NodeStatus;
use fasttick_log::btlog::{MAX_OFFSET_US, Transition};
use fasttick_log::replay::NodeState;
use proptest::prelude::*;

/// Runtime XML in the form of `WriteTreeToXML(tree, true, true)`.
///
/// UIDs and node paths:
/// 1 `main_seq` (Sequence), 2 `move_to_goal` (MoveBase),
/// 3 `say, "hi"` (SaySomething), 4 `dock_sub` (SubTree),
/// 5 `dock_sub/dock_fallback` (Fallback), 6 `dock_sub/approach`
/// (ApproachDock), 7 `dock_sub/AlwaysFailure` (AlwaysFailure),
/// 8 `dock_sub/two\nlines` (AlwaysSuccess).
pub const SAMPLE_XML: &str = r#"<root BTCPP_format="4">
    <BehaviorTree ID="MainTree" _fullpath="">
        <Sequence name="main_seq" _uid="1">
            <MoveBase name="move_to_goal" _uid="2" goal="1;2;3"/>
            <SaySomething name="say, &quot;hi&quot;" _uid="3" message="hello"/>
            <SubTree ID="Docking" _fullpath="dock_sub" _uid="4"/>
        </Sequence>
    </BehaviorTree>
    <BehaviorTree ID="Docking" _fullpath="dock_sub">
        <Fallback name="dock_fallback" _uid="5">
            <Action ID="ApproachDock" name="approach" _uid="6"/>
            <AlwaysFailure name="AlwaysFailure" _uid="7"/>
            <AlwaysSuccess name="two&#10;lines" _uid="8"/>
        </Fallback>
    </BehaviorTree>
    <TreeNodesModel>
        <Action ID="MoveBase">
            <input_port name="goal" type="std::string"/>
        </Action>
    </TreeNodesModel>
</root>"#;

/// A start time: 2026-10-14 12:00:00 UTC in microseconds.
pub const START_US: i64 = 1_791_979_200_000_000;

pub fn t(offset_us: u64, uid: u16, status: NodeStatus) -> Transition {
    Transition::new(offset_us, Uid(uid), status)
}

/// Builds `.btlog` bytes by hand, following `bt_file_logger_v2.cpp`. This
/// does not use the crate, so it checks the writer.
pub fn handmade_log(xml: &str, start_time_us: i64, records: &[(u64, u16, u8)]) -> Vec<u8> {
    let mut out = Vec::new();
    out.extend_from_slice(b"BTCPP4-FileLogger2");
    out.push(1);
    out.extend_from_slice(&(xml.len() as i32).to_le_bytes());
    out.extend_from_slice(xml.as_bytes());
    out.extend_from_slice(&start_time_us.to_le_bytes());
    for (offset, uid, status) in records {
        let offset_bytes = offset.to_le_bytes();
        out.extend_from_slice(&offset_bytes[..6]);
        out.extend_from_slice(&uid.to_le_bytes());
        out.push(*status);
    }
    out
}

pub fn status_strategy() -> impl Strategy<Value = NodeStatus> {
    prop::sample::select(NodeStatus::ALL.to_vec())
}

/// Transitions with a few UIDs (1 to `max_uid`) and small times, so that
/// equal times happen often.
pub fn transitions_strategy(
    max_uid: u16,
    max_time: u64,
    max_len: usize,
) -> impl Strategy<Value = Vec<Transition>> {
    prop::collection::vec(
        (0..=max_time, 1..=max_uid, status_strategy())
            .prop_map(|(offset, uid, status)| t(offset, uid, status)),
        0..max_len,
    )
}

/// Transitions with any valid offset and any UID.
pub fn wide_transitions_strategy(max_len: usize) -> impl Strategy<Value = Vec<Transition>> {
    prop::collection::vec(
        (0..=MAX_OFFSET_US, any::<u16>(), status_strategy())
            .prop_map(|(offset, uid, status)| t(offset, uid, status)),
        0..max_len,
    )
}

/// A stable sort by time that does not use `sort_by_key`.
pub fn naive_sort(transitions: &[Transition]) -> Vec<Transition> {
    let mut groups: BTreeMap<u64, Vec<Transition>> = BTreeMap::new();
    for transition in transitions {
        groups
            .entry(transition.offset_us)
            .or_default()
            .push(*transition);
    }
    groups.into_values().flatten().collect()
}

/// The state after the first `count` sorted transitions, by a linear scan.
pub fn naive_state(sorted: &[Transition], count: usize) -> HashMap<Uid, NodeState> {
    let mut state: HashMap<Uid, NodeState> = HashMap::new();
    for transition in &sorted[..count.min(sorted.len())] {
        let node = state.entry(transition.uid).or_default();
        node.status = transition.status;
        if transition.status != NodeStatus::Idle {
            node.last_non_idle = Some(transition.status);
        }
    }
    state
}

/// The state at a time, by a linear scan.
pub fn naive_state_at(sorted: &[Transition], time_us: u64) -> HashMap<Uid, NodeState> {
    let count = sorted
        .iter()
        .filter(|transition| transition.offset_us <= time_us)
        .count();
    naive_state(sorted, count)
}
