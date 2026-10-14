//! Replay of a log: the state of all nodes at a time, and a playback cursor.
//!
//! [`ReplayIndex`] sorts the transitions by time. Transitions with the same
//! time keep their order from the log (the sort is stable). The index keeps
//! a full snapshot of all node states after each `K` transitions
//! (`K` = [`DEFAULT_SNAPSHOT_INTERVAL`] by default). The state at a time or
//! at a transition starts from the nearest snapshot before it and applies at
//! most `K - 1` transitions. A query thus costs `O(log n + K + m)`, where `m`
//! is the number of nodes (the size of the returned state).
//!
//! Positions: "the state after `count` transitions" means that the first
//! `count` transitions of the sorted list are applied. The state at a time
//! `t` applies all transitions with an offset at or before `t`.

use std::collections::HashMap;
use std::ops::Range;
use std::sync::Arc;
use std::time::Duration;

use fasttick_core::runtime::Uid;
use fasttick_core::status::NodeStatus;
use serde::{Deserialize, Serialize};

use crate::btlog::{BtLog, Transition};
use crate::ordered_range;

/// The default number of transitions between two snapshots.
pub const DEFAULT_SNAPSHOT_INTERVAL: usize = 1024;

/// The state of one node during replay.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default, Serialize, Deserialize)]
pub struct NodeState {
    /// The status now.
    pub status: NodeStatus,
    /// The last status that was not IDLE, up to now. `None` if the node was
    /// always IDLE. When `status` is IDLE, the UI can show this value as a
    /// faded previous result.
    pub last_non_idle: Option<NodeStatus>,
}

impl NodeState {
    /// Applies a new status.
    pub fn apply(&mut self, status: NodeStatus) {
        self.status = status;
        if status != NodeStatus::Idle {
            self.last_non_idle = Some(status);
        }
    }
}

/// The state of all nodes at one position of a replay.
///
/// Nodes that the index does not know are IDLE with no previous status.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ReplayState {
    uids: Arc<[Uid]>,
    states: Vec<NodeState>,
}

impl ReplayState {
    /// The state of a node.
    pub fn get(&self, uid: Uid) -> NodeState {
        self.uids
            .binary_search(&uid)
            .ok()
            .and_then(|slot| self.states.get(slot).copied())
            .unwrap_or_default()
    }

    /// The status of a node.
    pub fn status(&self, uid: Uid) -> NodeStatus {
        self.get(uid).status
    }

    /// All nodes and their states, in UID order.
    pub fn iter(&self) -> impl Iterator<Item = (Uid, NodeState)> + '_ {
        self.uids.iter().copied().zip(self.states.iter().copied())
    }

    /// The number of nodes.
    pub fn len(&self) -> usize {
        self.states.len()
    }

    /// True if the state has no nodes.
    pub fn is_empty(&self) -> bool {
        self.states.is_empty()
    }
}

/// An index for fast replay of a list of transitions. See the module
/// documentation.
#[derive(Debug, Clone)]
pub struct ReplayIndex {
    /// The transitions, sorted by time (stable).
    transitions: Vec<Transition>,
    /// For each sorted transition: its position in the input list.
    source: Vec<usize>,
    /// For each sorted transition: the slot of its UID.
    slots: Vec<u32>,
    /// The known UIDs, sorted. The slot of a UID is its position here.
    uids: Arc<[Uid]>,
    /// The number of transitions between two snapshots (at least 1).
    interval: usize,
    /// Snapshot `k` is the state after `k * interval` transitions. All
    /// snapshots are in one vector, `uids.len()` states each.
    snapshots: Vec<NodeState>,
}

impl Default for ReplayIndex {
    fn default() -> Self {
        ReplayIndex::new(&[])
    }
}

impl ReplayIndex {
    /// Builds the index with the default snapshot interval.
    pub fn new(transitions: &[Transition]) -> ReplayIndex {
        ReplayIndex::build(transitions, &[], DEFAULT_SNAPSHOT_INTERVAL)
    }

    /// Builds the index with a snapshot after each `interval` transitions.
    /// An interval of 0 is used as 1.
    pub fn with_snapshot_interval(transitions: &[Transition], interval: usize) -> ReplayIndex {
        ReplayIndex::build(transitions, &[], interval)
    }

    /// Builds the index for a log. The index knows all UIDs of the tree and
    /// all UIDs of the transitions.
    pub fn from_log(log: &BtLog) -> ReplayIndex {
        let tree_uids: Vec<Uid> = log.tree.nodes.iter().filter_map(|node| node.uid).collect();
        ReplayIndex::build(&log.transitions, &tree_uids, DEFAULT_SNAPSHOT_INTERVAL)
    }

    /// Builds the index. `extra_uids` are UIDs that the index must know
    /// even if no transition uses them (for example all nodes of the tree).
    pub fn build(transitions: &[Transition], extra_uids: &[Uid], interval: usize) -> ReplayIndex {
        let interval = interval.max(1);
        let mut order: Vec<usize> = (0..transitions.len()).collect();
        // `sort_by_key` is stable: equal times keep the order of the log.
        order.sort_by_key(|&position| transitions[position].offset_us);
        let sorted: Vec<Transition> = order
            .iter()
            .map(|&position| transitions[position])
            .collect();

        let mut uids: Vec<Uid> = extra_uids
            .iter()
            .copied()
            .chain(sorted.iter().map(|transition| transition.uid))
            .collect();
        uids.sort_unstable();
        uids.dedup();
        let slot_of: HashMap<Uid, u32> = uids
            .iter()
            .enumerate()
            .map(|(slot, uid)| (*uid, slot as u32))
            .collect();
        let slots: Vec<u32> = sorted
            .iter()
            .map(|transition| slot_of.get(&transition.uid).copied().unwrap_or(0))
            .collect();

        let mut index = ReplayIndex {
            transitions: Vec::new(),
            source: Vec::new(),
            slots: Vec::new(),
            uids: uids.into(),
            interval,
            snapshots: Vec::new(),
        };
        index.append_sorted(sorted, order, slots);
        index
    }

    /// Adds sorted transitions at the end and makes the missing snapshots.
    fn append_sorted(&mut self, sorted: Vec<Transition>, source: Vec<usize>, slots: Vec<u32>) {
        let width = self.uids.len();
        if self.snapshots.is_empty() {
            self.snapshots = vec![NodeState::default(); width];
        }
        // The state at the last snapshot, moved forward.
        let mut state = self.snapshot(self.snapshot_count() - 1).to_vec();
        let mut position = (self.snapshot_count() - 1) * self.interval;
        self.transitions.extend(sorted);
        self.source.extend(source);
        self.slots.extend(slots);
        while position < self.transitions.len() {
            let slot = self.slots[position] as usize;
            if let Some(node) = state.get_mut(slot) {
                node.apply(self.transitions[position].status);
            }
            position += 1;
            if position.is_multiple_of(self.interval) {
                self.snapshots.extend_from_slice(&state);
            }
        }
    }

    /// Adds transitions, for example from a log that is still written.
    ///
    /// If all new transitions are at or after the last time and all their
    /// UIDs are known, the index grows in place. Otherwise the index is
    /// built again. `source_offset` is the position of `new[0]` in the full
    /// list of transitions (see [`ReplayIndex::source_index`]).
    pub fn extend(&mut self, new: &[Transition], source_offset: usize) {
        let last = self.last_time().unwrap_or(0);
        let in_order = new
            .windows(2)
            .all(|pair| pair[0].offset_us <= pair[1].offset_us)
            && new.first().is_none_or(|first| first.offset_us >= last);
        let slots: Option<Vec<u32>> = new
            .iter()
            .map(|transition| {
                self.uids
                    .binary_search(&transition.uid)
                    .ok()
                    .map(|slot| slot as u32)
            })
            .collect();
        match slots {
            Some(slots) if in_order => {
                let source = (source_offset..source_offset + new.len()).collect();
                self.append_sorted(new.to_vec(), source, slots);
            }
            _ => {
                // Rebuild from the input order.
                let mut all: Vec<(usize, Transition)> = self
                    .source
                    .iter()
                    .copied()
                    .zip(self.transitions.iter().copied())
                    .collect();
                all.extend(
                    new.iter()
                        .enumerate()
                        .map(|(position, transition)| (source_offset + position, *transition)),
                );
                all.sort_by_key(|(position, _)| *position);
                let transitions: Vec<Transition> =
                    all.iter().map(|(_, transition)| *transition).collect();
                let rebuilt = ReplayIndex::build(&transitions, &self.uids, self.interval);
                let sources: Vec<usize> = all.iter().map(|(position, _)| *position).collect();
                self.transitions = rebuilt.transitions;
                self.source = rebuilt
                    .source
                    .iter()
                    .map(|&position| sources[position])
                    .collect();
                self.slots = rebuilt.slots;
                self.uids = rebuilt.uids;
                self.snapshots = rebuilt.snapshots;
            }
        }
    }

    fn snapshot_count(&self) -> usize {
        match self.uids.len() {
            0 => self.transitions.len() / self.interval + 1,
            width => self.snapshots.len() / width,
        }
    }

    fn snapshot(&self, k: usize) -> &[NodeState] {
        let width = self.uids.len();
        self.snapshots
            .get(k * width..(k + 1) * width)
            .unwrap_or_default()
    }

    /// The number of transitions.
    pub fn len(&self) -> usize {
        self.transitions.len()
    }

    /// True if the index has no transitions.
    pub fn is_empty(&self) -> bool {
        self.transitions.is_empty()
    }

    /// The transitions, sorted by time.
    pub fn transitions(&self) -> &[Transition] {
        &self.transitions
    }

    /// The transition at a sorted position.
    pub fn transition(&self, index: usize) -> Option<&Transition> {
        self.transitions.get(index)
    }

    /// The position in the input list of the transition at a sorted position.
    pub fn source_index(&self, index: usize) -> Option<usize> {
        self.source.get(index).copied()
    }

    /// The known UIDs, sorted.
    pub fn uids(&self) -> &[Uid] {
        &self.uids
    }

    /// The snapshot interval.
    pub fn snapshot_interval(&self) -> usize {
        self.interval
    }

    /// The time of the transition at a sorted position.
    pub fn time_of_index(&self, index: usize) -> Option<u64> {
        self.transitions
            .get(index)
            .map(|transition| transition.offset_us)
    }

    /// The time of the first transition.
    pub fn first_time(&self) -> Option<u64> {
        self.transitions
            .first()
            .map(|transition| transition.offset_us)
    }

    /// The time of the last transition.
    pub fn last_time(&self) -> Option<u64> {
        self.transitions
            .last()
            .map(|transition| transition.offset_us)
    }

    /// The number of transitions at or before `time_us`.
    pub fn count_at_time(&self, time_us: u64) -> usize {
        self.transitions
            .partition_point(|transition| transition.offset_us <= time_us)
    }

    /// The number of transitions before `time_us`.
    pub fn count_before_time(&self, time_us: u64) -> usize {
        self.transitions
            .partition_point(|transition| transition.offset_us < time_us)
    }

    /// The sorted position of the last transition at or before `time_us`.
    pub fn index_at_time(&self, time_us: u64) -> Option<usize> {
        self.count_at_time(time_us).checked_sub(1)
    }

    /// The sorted positions of the transitions in an inclusive time range.
    pub fn indices_in_range(&self, range: (u64, u64)) -> Range<usize> {
        let (start, end) = ordered_range(range);
        self.count_before_time(start)..self.count_at_time(end)
    }

    /// The state after the first `count` transitions. A larger `count` is
    /// used as [`ReplayIndex::len`].
    pub fn state_after(&self, count: usize) -> ReplayState {
        let count = count.min(self.transitions.len());
        let k = count / self.interval;
        let mut states = self.snapshot(k).to_vec();
        for position in k * self.interval..count {
            if let Some(node) = states.get_mut(self.slots[position] as usize) {
                node.apply(self.transitions[position].status);
            }
        }
        ReplayState {
            uids: Arc::clone(&self.uids),
            states,
        }
    }

    /// The state after the transition at a sorted position (that transition
    /// is applied).
    pub fn state_at_index(&self, index: usize) -> ReplayState {
        self.state_after(index.saturating_add(1))
    }

    /// The state at a time: all transitions at or before `time_us` are
    /// applied.
    pub fn state_at(&self, time_us: u64) -> ReplayState {
        self.state_after(self.count_at_time(time_us))
    }
}

/// A playback speed from 0.1x to 100x.
///
/// Serde writes the factor as a number. A value out of range is clamped
/// when it is read.
#[derive(Debug, Clone, Copy, PartialEq, PartialOrd, Serialize, Deserialize)]
#[serde(from = "f64", into = "f64")]
pub struct PlaybackSpeed(f64);

impl PlaybackSpeed {
    /// The slowest speed.
    pub const MIN: f64 = 0.1;
    /// The fastest speed.
    pub const MAX: f64 = 100.0;
    /// Normal speed (1x).
    pub const NORMAL: PlaybackSpeed = PlaybackSpeed(1.0);
    /// The speeds that [`PlaybackSpeed::faster`] and
    /// [`PlaybackSpeed::slower`] step through.
    pub const STEPS: [f64; 10] = [0.1, 0.25, 0.5, 1.0, 2.0, 5.0, 10.0, 25.0, 50.0, 100.0];

    /// Makes a speed. Values out of range are clamped. A value that is not
    /// a number gives normal speed.
    pub fn new(factor: f64) -> PlaybackSpeed {
        if factor.is_nan() {
            PlaybackSpeed::NORMAL
        } else {
            PlaybackSpeed(factor.clamp(PlaybackSpeed::MIN, PlaybackSpeed::MAX))
        }
    }

    /// The factor.
    pub fn factor(self) -> f64 {
        self.0
    }

    /// The next faster value of [`PlaybackSpeed::STEPS`].
    pub fn faster(self) -> PlaybackSpeed {
        PlaybackSpeed::STEPS
            .iter()
            .copied()
            .find(|step| *step > self.0 * (1.0 + 1e-9))
            .map_or(PlaybackSpeed(PlaybackSpeed::MAX), PlaybackSpeed)
    }

    /// The next slower value of [`PlaybackSpeed::STEPS`].
    pub fn slower(self) -> PlaybackSpeed {
        PlaybackSpeed::STEPS
            .iter()
            .rev()
            .copied()
            .find(|step| *step < self.0 * (1.0 - 1e-9))
            .map_or(PlaybackSpeed(PlaybackSpeed::MIN), PlaybackSpeed)
    }
}

impl From<f64> for PlaybackSpeed {
    fn from(factor: f64) -> Self {
        PlaybackSpeed::new(factor)
    }
}

impl From<PlaybackSpeed> for f64 {
    fn from(speed: PlaybackSpeed) -> Self {
        speed.0
    }
}

impl Default for PlaybackSpeed {
    fn default() -> Self {
        PlaybackSpeed::NORMAL
    }
}

/// The playback position of a replay.
///
/// The cursor does not hold the index. Its methods take the
/// [`ReplayIndex`] as a parameter. Use the cursor only with one index (or
/// with an index that only grows, see [`ReplayIndex::extend`]).
///
/// The position has two parts:
///
/// - `applied`: the number of sorted transitions that are applied. The
///   shown state is [`ReplayIndex::state_after`]`(applied)`.
/// - `time_us`: the time on the timeline.
///
/// The bounds are the selected time range, or else `0` to the time of the
/// last transition. Play, seek and step stay in the bounds. At the start of
/// the bounds `(start, end)`, `applied` can be the number of transitions
/// before `start`.
#[derive(Debug, Clone, PartialEq)]
pub struct Cursor {
    applied: usize,
    time_us: u64,
    /// Log time in picoseconds that `advance` did not use yet (less than
    /// one microsecond). Integer arithmetic keeps playback exact.
    carry_ps: u128,
    playing: bool,
    speed: PlaybackSpeed,
    range: Option<(u64, u64)>,
}

impl Default for Cursor {
    fn default() -> Self {
        Cursor {
            applied: 0,
            time_us: 0,
            carry_ps: 0,
            playing: false,
            speed: PlaybackSpeed::NORMAL,
            range: None,
        }
    }
}

impl Cursor {
    /// A paused cursor at the start of the index, with no transition
    /// applied.
    pub fn new() -> Cursor {
        Cursor::default()
    }

    /// The number of applied transitions.
    pub fn applied(&self) -> usize {
        self.applied
    }

    /// The sorted position of the last applied transition (the current
    /// transition), or `None` if no transition is applied.
    pub fn current_index(&self) -> Option<usize> {
        self.applied.checked_sub(1)
    }

    /// The time on the timeline.
    pub fn time_us(&self) -> u64 {
        self.time_us
    }

    /// True during playback.
    pub fn is_playing(&self) -> bool {
        self.playing
    }

    /// The playback speed.
    pub fn speed(&self) -> PlaybackSpeed {
        self.speed
    }

    /// Sets the playback speed.
    pub fn set_speed(&mut self, speed: PlaybackSpeed) {
        self.speed = speed;
    }

    /// The selected time range, if any.
    pub fn range(&self) -> Option<(u64, u64)> {
        self.range
    }

    /// The time bounds: the selected range, or `0` to the last time.
    pub fn bounds(&self, index: &ReplayIndex) -> (u64, u64) {
        match self.range {
            Some(range) => range,
            None => (0, index.last_time().unwrap_or(0)),
        }
    }

    /// The smallest and largest values of `applied` in the bounds.
    fn applied_bounds(&self, index: &ReplayIndex) -> (usize, usize) {
        let (start, end) = self.bounds(index);
        (index.count_before_time(start), index.count_at_time(end))
    }

    /// The shown state.
    pub fn state(&self, index: &ReplayIndex) -> ReplayState {
        index.state_after(self.applied)
    }

    /// Selects a time range (zoom) or removes it. The cursor moves into the
    /// new bounds if it is outside.
    pub fn set_range(&mut self, index: &ReplayIndex, range: Option<(u64, u64)>) {
        self.range = range.map(ordered_range);
        self.clamp(index);
    }

    /// Moves the cursor into the bounds. Call this after the index grows.
    pub fn clamp(&mut self, index: &ReplayIndex) {
        let (start, end) = self.bounds(index);
        if self.time_us < start || self.time_us > end {
            self.seek(index, self.time_us.clamp(start, end));
            return;
        }
        let (low, high) = self.applied_bounds(index);
        self.applied = self.applied.clamp(low, high);
    }

    /// Starts playback. At the end of the bounds, playback starts again at
    /// the start of the bounds.
    pub fn play(&mut self, index: &ReplayIndex) {
        let (start, end) = self.bounds(index);
        let (_, high) = self.applied_bounds(index);
        if self.time_us >= end && self.applied >= high {
            self.time_us = start;
            self.applied = index.count_before_time(start);
        }
        self.carry_ps = 0;
        self.playing = true;
    }

    /// Stops playback.
    pub fn pause(&mut self) {
        self.playing = false;
    }

    /// Starts or stops playback.
    pub fn toggle_play(&mut self, index: &ReplayIndex) {
        if self.playing {
            self.pause();
        } else {
            self.play(index);
        }
    }

    /// Moves to a time. The time is clamped to the bounds. All transitions
    /// at or before the time are applied.
    pub fn seek(&mut self, index: &ReplayIndex, time_us: u64) {
        let (start, end) = self.bounds(index);
        self.time_us = time_us.clamp(start, end);
        self.applied = index.count_at_time(self.time_us);
        self.carry_ps = 0;
    }

    /// Moves to the transition at a sorted position (it is applied). The
    /// position is clamped to the transitions in the bounds.
    pub fn seek_to_index(&mut self, index: &ReplayIndex, position: usize) {
        let (low, high) = self.applied_bounds(index);
        let applied = position.saturating_add(1).clamp(low, high);
        self.set_applied(index, applied);
    }

    fn set_applied(&mut self, index: &ReplayIndex, applied: usize) {
        let (low, _) = self.applied_bounds(index);
        let (start, _) = self.bounds(index);
        self.applied = applied;
        self.time_us = if applied > low {
            index.time_of_index(applied - 1).unwrap_or(start)
        } else {
            start
        };
        self.carry_ps = 0;
    }

    /// Applies the next transition. Returns false at the end of the bounds.
    pub fn step_forward(&mut self, index: &ReplayIndex) -> bool {
        let (_, high) = self.applied_bounds(index);
        if self.applied >= high {
            return false;
        }
        self.set_applied(index, self.applied + 1);
        true
    }

    /// Takes back the current transition. Returns false at the start of the
    /// bounds.
    pub fn step_back(&mut self, index: &ReplayIndex) -> bool {
        let (low, _) = self.applied_bounds(index);
        if self.applied <= low {
            return false;
        }
        self.set_applied(index, self.applied - 1);
        true
    }

    /// Moves to the start of the bounds, with no transition of the bounds
    /// applied.
    pub fn go_to_start(&mut self, index: &ReplayIndex) {
        let (low, _) = self.applied_bounds(index);
        self.set_applied(index, low);
    }

    /// Moves to the end of the bounds.
    pub fn go_to_end(&mut self, index: &ReplayIndex) {
        let (_, end) = self.bounds(index);
        self.seek(index, end);
    }

    /// Moves the time forward during playback: `real_elapsed` times the
    /// speed. Playback stops at the end of the bounds. Returns true if the
    /// shown state changed. Does nothing when paused.
    pub fn advance(&mut self, index: &ReplayIndex, real_elapsed: Duration) -> bool {
        if !self.playing {
            return false;
        }
        let (_, end) = self.bounds(index);
        // Log picoseconds = real nanoseconds * speed * 1000. The speed has a
        // resolution of 0.001.
        let speed_milli = (self.speed.factor() * 1000.0).round() as u128;
        let total_ps = real_elapsed
            .as_nanos()
            .saturating_mul(speed_milli)
            .saturating_add(self.carry_ps);
        self.carry_ps = total_ps % 1_000_000;
        let whole_us = u64::try_from(total_ps / 1_000_000).unwrap_or(u64::MAX);
        let time = self.time_us.saturating_add(whole_us);
        let before = self.applied;
        if time >= end {
            self.time_us = end;
            self.playing = false;
            self.carry_ps = 0;
        } else {
            self.time_us = time;
        }
        self.applied = self.applied.max(index.count_at_time(self.time_us));
        self.applied != before
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn t(offset_us: u64, uid: u16, status: NodeStatus) -> Transition {
        Transition::new(offset_us, Uid(uid), status)
    }

    #[test]
    fn node_state_keeps_last_non_idle() {
        let mut state = NodeState::default();
        assert_eq!(state.last_non_idle, None);
        state.apply(NodeStatus::Running);
        state.apply(NodeStatus::Success);
        state.apply(NodeStatus::Idle);
        assert_eq!(state.status, NodeStatus::Idle);
        assert_eq!(state.last_non_idle, Some(NodeStatus::Success));
    }

    #[test]
    fn sort_is_stable() {
        let input = [
            t(10, 1, NodeStatus::Running),
            t(5, 2, NodeStatus::Running),
            t(10, 1, NodeStatus::Success),
            t(5, 2, NodeStatus::Failure),
        ];
        let index = ReplayIndex::new(&input);
        assert_eq!(
            index.transitions(),
            &[input[1], input[3], input[0], input[2]]
        );
        assert_eq!(index.source_index(0), Some(1));
        assert_eq!(index.source_index(2), Some(0));
        assert_eq!(index.state_at(5).status(Uid(2)), NodeStatus::Failure);
        assert_eq!(index.state_at(10).status(Uid(1)), NodeStatus::Success);
    }

    #[test]
    fn empty_index() {
        let index = ReplayIndex::default();
        assert!(index.is_empty());
        assert_eq!(index.first_time(), None);
        assert_eq!(index.index_at_time(100), None);
        assert!(index.state_at(100).is_empty());
        assert_eq!(index.state_at(100).get(Uid(3)), NodeState::default());
        let mut cursor = Cursor::new();
        assert!(!cursor.step_forward(&index));
        assert!(!cursor.step_back(&index));
        cursor.play(&index);
        assert!(!cursor.advance(&index, Duration::from_secs(1)));
        assert!(!cursor.is_playing());
    }

    #[test]
    fn speed_steps() {
        assert_eq!(PlaybackSpeed::new(0.0).factor(), 0.1);
        assert_eq!(PlaybackSpeed::new(1e9).factor(), 100.0);
        assert_eq!(PlaybackSpeed::new(f64::NAN).factor(), 1.0);
        assert_eq!(PlaybackSpeed::NORMAL.faster().factor(), 2.0);
        assert_eq!(PlaybackSpeed::NORMAL.slower().factor(), 0.5);
        assert_eq!(PlaybackSpeed::new(100.0).faster().factor(), 100.0);
        assert_eq!(PlaybackSpeed::new(0.1).slower().factor(), 0.1);
        assert_eq!(PlaybackSpeed::new(3.0).faster().factor(), 5.0);
        assert_eq!(PlaybackSpeed::new(3.0).slower().factor(), 2.0);
    }
}
