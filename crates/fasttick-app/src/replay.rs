//! The view model of a replay document: the log, its replay index, the
//! playback cursor, the transition filter of the table, the statistics and
//! the transition density of the timeline.
//!
//! All format and replay logic is in `fasttick-log` ([`ReplayIndex`],
//! [`Cursor`], [`TransitionFilter`], [`compute_stats`], `export`). This
//! module keeps the UI state and caches the results.

use std::collections::{BTreeSet, HashMap};
use std::sync::Arc;
use std::time::Duration;

use fasttick_core::runtime::{NodeIndex, Uid};
use fasttick_core::status::NodeStatus;
use fasttick_log::btlog::{BtLog, Transition};
use fasttick_log::filter::TransitionFilter;
use fasttick_log::replay::{Cursor, PlaybackSpeed, ReplayIndex, ReplayState};
use fasttick_log::stats::{NodeStats, compute_stats};

/// The name, path and type of a node of the log tree.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NodeInfo {
    /// The instance name.
    pub name: String,
    /// The node path (`RuntimeTree::node_path`).
    pub path: String,
    /// The type ID.
    pub type_id: String,
    /// The index in the tree.
    pub index: NodeIndex,
}

/// The filter fields of the transitions table, as the user typed them.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FilterForm {
    /// Include patterns, separated by commas.
    pub include: String,
    /// Exclude patterns, separated by commas.
    pub exclude: String,
    /// The accepted new statuses, by status value (0 to 4).
    pub statuses: [bool; 5],
    /// The start of the time range, in seconds from the log start. Empty
    /// means the log start.
    pub from_s: String,
    /// The end of the time range, in seconds. Empty means the log end.
    pub to_s: String,
}

impl Default for FilterForm {
    fn default() -> Self {
        FilterForm {
            include: String::new(),
            exclude: String::new(),
            statuses: [true; 5],
            from_s: String::new(),
            to_s: String::new(),
        }
    }
}

impl FilterForm {
    /// The filter for `fasttick_log`. Returns an error text if a time
    /// field is not a number.
    pub fn to_filter(&self) -> Result<TransitionFilter, String> {
        let patterns = |text: &str| -> Vec<String> {
            text.split(',')
                .map(str::trim)
                .filter(|p| !p.is_empty())
                .map(str::to_owned)
                .collect()
        };
        let statuses = if self.statuses.iter().all(|on| *on) {
            None
        } else {
            Some(
                NodeStatus::ALL
                    .into_iter()
                    .filter(|status| self.statuses[usize::from(status.as_u8())])
                    .collect::<BTreeSet<_>>(),
            )
        };
        let from = parse_seconds(&self.from_s, "From")?;
        let to = parse_seconds(&self.to_s, "To")?;
        let time_range = match (from, to) {
            (None, None) => None,
            (from, to) => Some((from.unwrap_or(0), to.unwrap_or(u64::MAX))),
        };
        Ok(TransitionFilter {
            include: patterns(&self.include),
            exclude: patterns(&self.exclude),
            statuses,
            time_range,
        })
    }

    /// Sets the time fields to a range in microseconds.
    pub fn set_time_range(&mut self, range: Option<(u64, u64)>) {
        match range {
            Some((start, end)) => {
                self.from_s = seconds_text(start);
                self.to_s = seconds_text(end);
            }
            None => {
                self.from_s.clear();
                self.to_s.clear();
            }
        }
    }
}

/// Parses a time in seconds (empty gives `None`).
fn parse_seconds(text: &str, field: &str) -> Result<Option<u64>, String> {
    let text = text.trim();
    if text.is_empty() {
        return Ok(None);
    }
    match text.parse::<f64>() {
        Ok(seconds) if seconds.is_finite() && seconds >= 0.0 => {
            Ok(Some((seconds * 1e6).round().min(u64::MAX as f64) as u64))
        }
        _ => Err(format!("{field}: enter a time in seconds, for example 1.5")),
    }
}

/// A time in microseconds as seconds text, without trailing zeros.
fn seconds_text(us: u64) -> String {
    let text = format!("{:.6}", us as f64 / 1e6);
    text.trim_end_matches('0').trim_end_matches('.').to_owned()
}

/// A column of the statistics table.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default)]
pub enum StatsColumn {
    /// The node name.
    #[default]
    Node,
    /// The node path.
    Path,
    /// The time in RUNNING.
    Running,
    /// The number of status changes.
    Changes,
    /// The number of SUCCESS results.
    Success,
    /// The number of FAILURE results.
    Failure,
    /// The number of SKIPPED results.
    Skipped,
}

impl StatsColumn {
    /// All columns, in table order.
    pub const ALL: [StatsColumn; 7] = [
        StatsColumn::Node,
        StatsColumn::Path,
        StatsColumn::Running,
        StatsColumn::Changes,
        StatsColumn::Success,
        StatsColumn::Failure,
        StatsColumn::Skipped,
    ];

    /// The column title.
    pub fn title(self) -> &'static str {
        match self {
            StatsColumn::Node => "Node",
            StatsColumn::Path => "Path",
            StatsColumn::Running => "Running time",
            StatsColumn::Changes => "Changes",
            StatsColumn::Success => "Success",
            StatsColumn::Failure => "Failure",
            StatsColumn::Skipped => "Skipped",
        }
    }
}

/// The sort order of the statistics table.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct StatsSort {
    /// The column.
    pub column: StatsColumn,
    /// True for ascending order.
    pub ascending: bool,
}

impl Default for StatsSort {
    fn default() -> Self {
        StatsSort {
            column: StatsColumn::Running,
            ascending: false,
        }
    }
}

/// The cached statistics for one range and one sort order.
#[derive(Debug, Clone)]
struct StatsCache {
    range: Option<(u64, u64)>,
    sort: StatsSort,
    hide_unchanged: bool,
    stats: Vec<NodeStats>,
    order: Vec<usize>,
}

/// The cached transition density for one range and one number of bins.
#[derive(Debug, Clone)]
struct DensityCache {
    bounds: (u64, u64),
    bins: usize,
    counts: Arc<Vec<u32>>,
}

/// The view model of one replay document. See the module documentation.
#[derive(Debug)]
pub struct ReplayModel {
    log: Arc<BtLog>,
    index: ReplayIndex,
    cursor: Cursor,
    nodes: HashMap<Uid, NodeInfo>,
    /// For each sorted transition: the status of the node before it.
    previous: Vec<NodeStatus>,
    /// A label for the source, for example "session 2" of a database.
    pub source: Option<String>,
    /// The filter fields of the transitions table.
    pub filter: FilterForm,
    applied_filter: Option<FilterForm>,
    filter_error: Option<String>,
    rows: Vec<u32>,
    /// The sort order of the statistics table.
    pub stats_sort: StatsSort,
    /// Hide the nodes without transitions in the statistics table.
    pub hide_unchanged: bool,
    stats: Option<StatsCache>,
    density: Option<DensityCache>,
    last_frame: Option<f64>,
    /// The sorted position of the transition that the user clicked in the
    /// table, if any.
    pub selected_transition: Option<usize>,
}

impl ReplayModel {
    /// Makes the model. The cursor starts at the end of the log, so the
    /// tree shows the last status of each node.
    pub fn new(log: BtLog, index: ReplayIndex) -> ReplayModel {
        let mut nodes = HashMap::new();
        for (position, node) in log.tree.nodes.iter().enumerate() {
            if let Some(uid) = node.uid {
                nodes.insert(
                    uid,
                    NodeInfo {
                        name: node.name.clone(),
                        path: log.tree.node_path(NodeIndex(position)),
                        type_id: node.type_id.clone(),
                        index: NodeIndex(position),
                    },
                );
            }
        }
        // The status before each transition, as in `fasttick_log::export`.
        let mut last: HashMap<Uid, NodeStatus> = HashMap::new();
        let previous = index
            .transitions()
            .iter()
            .map(|t| last.insert(t.uid, t.status).unwrap_or_default())
            .collect();
        let mut cursor = Cursor::new();
        cursor.go_to_end(&index);
        ReplayModel {
            log: Arc::new(log),
            index,
            cursor,
            nodes,
            previous,
            source: None,
            filter: FilterForm::default(),
            applied_filter: None,
            filter_error: None,
            rows: Vec::new(),
            stats_sort: StatsSort::default(),
            hide_unchanged: true,
            stats: None,
            density: None,
            last_frame: None,
            selected_transition: None,
        }
    }

    /// The log.
    pub fn log(&self) -> &Arc<BtLog> {
        &self.log
    }

    /// The replay index.
    pub fn index(&self) -> &ReplayIndex {
        &self.index
    }

    /// The playback cursor.
    pub fn cursor(&self) -> &Cursor {
        &self.cursor
    }

    /// The start time of the log (microseconds since the Unix epoch).
    pub fn start_time_us(&self) -> i64 {
        self.log.start_time_us
    }

    /// The time of the last transition.
    pub fn end_us(&self) -> u64 {
        self.index.last_time().unwrap_or(0)
    }

    /// The node of a UID, if the tree has it.
    pub fn node(&self, uid: Uid) -> Option<&NodeInfo> {
        self.nodes.get(&uid)
    }

    /// The transition at a sorted position and the status before it.
    pub fn transition(&self, position: usize) -> Option<(Transition, NodeStatus)> {
        let transition = *self.index.transition(position)?;
        let previous = self.previous.get(position).copied().unwrap_or_default();
        Some((transition, previous))
    }

    /// The state that the views show.
    pub fn state(&self) -> ReplayState {
        self.cursor.state(&self.index)
    }

    /// A key that changes when the shown state changes. A view decorates
    /// its tree again when it sees a new value.
    pub fn decoration_key(&self) -> u64 {
        self.cursor.applied() as u64
    }

    // ----- Playback -----

    /// Starts or stops playback.
    pub fn toggle_play(&mut self) {
        self.cursor.toggle_play(&self.index);
        self.last_frame = None;
    }

    /// Starts playback.
    pub fn play(&mut self) {
        self.cursor.play(&self.index);
        self.last_frame = None;
    }

    /// Stops playback.
    pub fn pause(&mut self) {
        self.cursor.pause();
    }

    /// Applies the next transition.
    pub fn step_forward(&mut self) -> bool {
        self.cursor.pause();
        self.cursor.step_forward(&self.index)
    }

    /// Takes back the current transition.
    pub fn step_back(&mut self) -> bool {
        self.cursor.pause();
        self.cursor.step_back(&self.index)
    }

    /// Moves to the start of the bounds.
    pub fn go_to_start(&mut self) {
        self.cursor.pause();
        self.cursor.go_to_start(&self.index);
    }

    /// Moves to the end of the bounds.
    pub fn go_to_end(&mut self) {
        self.cursor.pause();
        self.cursor.go_to_end(&self.index);
    }

    /// Moves to a time (microseconds from the log start).
    pub fn seek(&mut self, time_us: u64) {
        self.cursor.seek(&self.index, time_us);
    }

    /// Moves to a transition (it is applied). Removes a zoom range that
    /// does not contain the transition.
    pub fn seek_to_transition(&mut self, position: usize) {
        if let (Some(time), Some((start, end))) =
            (self.index.time_of_index(position), self.cursor.range())
            && (time < start || time > end)
        {
            self.cursor.set_range(&self.index, None);
        }
        self.cursor.pause();
        self.cursor.seek_to_index(&self.index, position);
    }

    /// Sets the playback speed.
    pub fn set_speed(&mut self, speed: PlaybackSpeed) {
        self.cursor.set_speed(speed);
    }

    /// Zooms the timeline to a range, or back to the whole log.
    pub fn set_range(&mut self, range: Option<(u64, u64)>) {
        let range = range.filter(|(a, b)| a != b);
        self.cursor.set_range(&self.index, range);
    }

    /// Moves the playback forward to the time `now` (seconds, egui time).
    /// Returns true if the shown state changed.
    pub fn advance(&mut self, now: f64) -> bool {
        if !self.cursor.is_playing() {
            self.last_frame = None;
            return false;
        }
        let elapsed = match self.last_frame {
            Some(last) if now > last => now - last,
            _ => 0.0,
        };
        self.last_frame = Some(now);
        // Limit a long frame (for example after the window was hidden).
        let elapsed = Duration::from_secs_f64(elapsed.min(0.25));
        self.cursor.advance(&self.index, elapsed)
    }

    // ----- Transitions table -----

    /// The sorted positions of the transitions that pass the filter. The
    /// rows are made again when the filter fields changed.
    pub fn rows(&mut self) -> &[u32] {
        if self.applied_filter.as_ref() != Some(&self.filter) {
            self.applied_filter = Some(self.filter.clone());
            match self.filter.to_filter() {
                Ok(filter) => {
                    self.filter_error = None;
                    let compiled = filter.compile(&self.log.tree);
                    self.rows = self
                        .index
                        .transitions()
                        .iter()
                        .enumerate()
                        .filter(|(_, t)| compiled.matches(t))
                        .map(|(position, _)| position as u32)
                        .collect();
                }
                Err(message) => {
                    self.filter_error = Some(message);
                }
            }
        }
        &self.rows
    }

    /// The rows of the last call of [`ReplayModel::rows`] (no update).
    pub fn current_rows(&self) -> &[u32] {
        &self.rows
    }

    /// The error of the time fields, if any.
    pub fn filter_error(&self) -> Option<&str> {
        self.filter_error.as_deref()
    }

    /// The filter for an export. Returns an error text if the fields are
    /// not valid.
    pub fn export_filter(&self) -> Result<TransitionFilter, String> {
        self.filter.to_filter()
    }

    // ----- Statistics -----

    /// The statistics for the selected time range (the timeline zoom), and
    /// the row order for the sort order. The values are cached.
    pub fn stats(&mut self) -> (&[NodeStats], &[usize]) {
        let range = self.cursor.range();
        let stale = self.stats.as_ref().is_none_or(|cache| {
            cache.range != range
                || cache.sort != self.stats_sort
                || cache.hide_unchanged != self.hide_unchanged
        });
        if stale {
            let stats = match &self.stats {
                Some(cache) if cache.range == range => cache.stats.clone(),
                _ => compute_stats(&self.index, range),
            };
            let order = self.sort_stats(&stats);
            self.stats = Some(StatsCache {
                range,
                sort: self.stats_sort,
                hide_unchanged: self.hide_unchanged,
                stats,
                order,
            });
        }
        match &self.stats {
            Some(cache) => (&cache.stats, &cache.order),
            None => (&[], &[]),
        }
    }

    fn sort_stats(&self, stats: &[NodeStats]) -> Vec<usize> {
        let mut order: Vec<usize> = (0..stats.len())
            .filter(|i| !self.hide_unchanged || stats[*i].transition_count > 0)
            .collect();
        let name = |s: &NodeStats| self.nodes.get(&s.uid).map_or("", |n| n.name.as_str());
        let path = |s: &NodeStats| self.nodes.get(&s.uid).map_or("", |n| n.path.as_str());
        order.sort_by(|a, b| {
            let (a, b) = (&stats[*a], &stats[*b]);
            let ordering = match self.stats_sort.column {
                StatsColumn::Node => name(a).cmp(name(b)),
                StatsColumn::Path => path(a).cmp(path(b)),
                StatsColumn::Running => a.running_time_us.cmp(&b.running_time_us),
                StatsColumn::Changes => a.status_changes.cmp(&b.status_changes),
                StatsColumn::Success => a.success_count.cmp(&b.success_count),
                StatsColumn::Failure => a.failure_count.cmp(&b.failure_count),
                StatsColumn::Skipped => a.skipped_count.cmp(&b.skipped_count),
            };
            let ordering = if self.stats_sort.ascending {
                ordering
            } else {
                ordering.reverse()
            };
            // Equal values: tree order (UID).
            ordering.then(a.uid.cmp(&b.uid))
        });
        order
    }

    /// Sorts the statistics by a column. A second click on the same column
    /// changes the direction.
    pub fn sort_stats_by(&mut self, column: StatsColumn) {
        if self.stats_sort.column == column {
            self.stats_sort.ascending = !self.stats_sort.ascending;
        } else {
            self.stats_sort = StatsSort {
                column,
                // Text columns sort A to Z first, numbers largest first.
                ascending: matches!(column, StatsColumn::Node | StatsColumn::Path),
            };
        }
    }

    // ----- Timeline -----

    /// The number of transitions in each of `bins` equal parts of the time
    /// range `bounds`. The result is cached.
    pub fn density(&mut self, bounds: (u64, u64), bins: usize) -> Arc<Vec<u32>> {
        let bins = bins.clamp(1, 4096);
        if let Some(cache) = &self.density
            && cache.bounds == bounds
            && cache.bins == bins
        {
            return Arc::clone(&cache.counts);
        }
        let counts = Arc::new(density(&self.index, bounds, bins));
        self.density = Some(DensityCache {
            bounds,
            bins,
            counts: Arc::clone(&counts),
        });
        counts
    }
}

/// Counts the transitions in each of `bins` equal parts of an inclusive
/// time range.
pub fn density(index: &ReplayIndex, (start, end): (u64, u64), bins: usize) -> Vec<u32> {
    let bins = bins.max(1);
    let mut counts = vec![0u32; bins];
    let span = end.saturating_sub(start).max(1) as f64;
    for transition in &index.transitions()[index.indices_in_range((start, end))] {
        let fraction = (transition.offset_us.saturating_sub(start)) as f64 / span;
        let bin = ((fraction * bins as f64) as usize).min(bins - 1);
        counts[bin] = counts[bin].saturating_add(1);
    }
    counts
}

/// A time offset as `m:ss.mmm` (or `h:mm:ss.mmm` from one hour).
pub fn format_clock(us: u64) -> String {
    let ms = us / 1000;
    let (h, m, s, ms) = (ms / 3_600_000, ms / 60_000 % 60, ms / 1000 % 60, ms % 1000);
    if h > 0 {
        format!("{h}:{m:02}:{s:02}.{ms:03}")
    } else {
        format!("{m}:{s:02}.{ms:03}")
    }
}

/// A time offset in seconds with microseconds, for example `12.345678`.
pub fn format_seconds(us: u64) -> String {
    format!("{}.{:06}", us / 1_000_000, us % 1_000_000)
}

/// A duration in a short form: `850 µs`, `12.5 ms`, `3.250 s`.
pub fn format_duration(us: u64) -> String {
    if us < 1000 {
        format!("{us} \u{00B5}s")
    } else if us < 1_000_000 {
        format!("{:.1} ms", us as f64 / 1000.0)
    } else {
        format!("{:.3} s", us as f64 / 1e6)
    }
}

/// An absolute time (microseconds since the Unix epoch) as UTC date and
/// time, for example `2026-10-15 05:45:26.123456 UTC`.
pub fn format_wall_time(us: i64) -> String {
    let days = us.div_euclid(86_400_000_000);
    let rest = us.rem_euclid(86_400_000_000);
    let (year, month, day) = civil_from_days(days);
    let seconds = rest / 1_000_000;
    format!(
        "{year:04}-{month:02}-{day:02} {:02}:{:02}:{:02}.{:06} UTC",
        seconds / 3600,
        seconds / 60 % 60,
        seconds % 60,
        rest % 1_000_000
    )
}

/// The civil date of a day number (days since 1970-01-01), with the
/// algorithm of H. Hinnant ("chrono-compatible low-level date
/// algorithms").
fn civil_from_days(days: i64) -> (i64, u32, u32) {
    let z = days + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z.rem_euclid(146_097);
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let day = (doy - (153 * mp + 2) / 5 + 1) as u32;
    let month = if mp < 10 { mp + 3 } else { mp - 9 } as u32;
    let year = yoe + era * 400 + i64::from(month <= 2);
    (year, month, day)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;

    fn t(offset_us: u64, uid: u16, status: NodeStatus) -> Transition {
        Transition::new(offset_us, Uid(uid), status)
    }

    const XML: &str = r#"<root BTCPP_format="4"><BehaviorTree ID="T" _fullpath=""><Sequence name="seq" _uid="1"><AlwaysSuccess name="ok" _uid="2"/><AlwaysFailure name="bad" _uid="3"/></Sequence></BehaviorTree></root>"#;

    fn model() -> ReplayModel {
        let log = BtLog::from_parts(
            XML.to_owned(),
            1_000_000,
            vec![
                t(10, 1, NodeStatus::Running),
                t(20, 2, NodeStatus::Success),
                t(30, 3, NodeStatus::Failure),
                t(40, 1, NodeStatus::Failure),
                t(50, 2, NodeStatus::Idle),
                t(50, 3, NodeStatus::Idle),
                t(60, 1, NodeStatus::Idle),
            ],
        )
        .unwrap();
        let index = ReplayIndex::from_log(&log);
        ReplayModel::new(log, index)
    }

    #[test]
    fn starts_at_the_end_and_steps() {
        let mut m = model();
        assert_eq!(m.cursor().applied(), 7);
        assert_eq!(m.end_us(), 60);
        assert_eq!(
            m.state().get(Uid(1)).last_non_idle,
            Some(NodeStatus::Failure)
        );
        assert_eq!(m.node(Uid(3)).unwrap().path, "bad");
        m.go_to_start();
        assert_eq!(m.cursor().applied(), 0);
        assert!(m.step_forward());
        assert_eq!(m.decoration_key(), 1);
        assert_eq!(m.state().status(Uid(1)), NodeStatus::Running);
        assert!(m.step_back());
        assert!(!m.step_back());
        m.seek(35);
        assert_eq!(m.cursor().applied(), 3);
        m.go_to_end();
        assert!(!m.step_forward());
        let (transition, previous) = m.transition(3).unwrap();
        assert_eq!(transition.status, NodeStatus::Failure);
        assert_eq!(previous, NodeStatus::Running);
        assert!(m.transition(99).is_none());
    }

    #[test]
    fn playback_advances_with_egui_time() {
        let mut m = model();
        m.go_to_start();
        m.set_speed(PlaybackSpeed::new(1.0));
        m.play();
        assert!(m.cursor().is_playing());
        assert!(!m.advance(10.0), "the first frame sets the clock");
        // 30 µs later (real time): three transitions.
        assert!(m.advance(10.000_030));
        assert_eq!(m.cursor().applied(), 3);
        // A long frame is limited, and playback stops at the end.
        m.advance(20.0);
        assert!(!m.cursor().is_playing());
        assert_eq!(m.cursor().applied(), 7);
        m.toggle_play();
        assert!(m.cursor().is_playing());
        m.pause();
        assert!(!m.advance(30.0));
    }

    #[test]
    fn range_zoom_and_seek_to_transition() {
        let mut m = model();
        m.set_range(Some((25, 45)));
        assert_eq!(m.cursor().bounds(m.index()), (25, 45));
        // A zero-length range is ignored.
        m.set_range(Some((30, 30)));
        assert_eq!(m.cursor().range(), None);
        m.set_range(Some((25, 45)));
        // A transition outside the range removes the zoom.
        m.seek_to_transition(0);
        assert_eq!(m.cursor().range(), None);
        assert_eq!(m.cursor().applied(), 1);
    }

    #[test]
    fn filter_rows_and_errors() {
        let mut m = model();
        assert_eq!(m.rows().len(), 7);
        m.filter.include = "ok, bad".into();
        assert_eq!(m.rows(), &[1, 2, 4, 5]);
        m.filter.exclude = "bad".into();
        assert_eq!(m.rows(), &[1, 4]);
        m.filter = FilterForm::default();
        m.filter.statuses[usize::from(NodeStatus::Idle.as_u8())] = false;
        assert_eq!(m.rows().len(), 4);
        m.filter = FilterForm::default();
        m.filter.from_s = "0.000030".into();
        m.filter.to_s = "0.00005".into();
        assert_eq!(m.rows(), &[2, 3, 4, 5]);
        m.filter.to_s = "soon".into();
        // The old rows stay, and the error shows.
        assert_eq!(m.rows().len(), 4);
        assert!(m.filter_error().unwrap().starts_with("To:"));
        assert!(m.export_filter().is_err());
        m.filter.set_time_range(Some((1_500_000, 2_000_000)));
        assert_eq!(m.filter.from_s, "1.5");
        assert_eq!(m.filter.to_s, "2");
        m.filter.set_time_range(None);
        assert!(m.filter.from_s.is_empty());
        assert!(m.export_filter().unwrap().is_empty());
    }

    #[test]
    fn statistics_sort_and_range() {
        let mut m = model();
        let (stats, order) = m.stats();
        assert_eq!(stats.len(), 3);
        // Default: running time, largest first. Node 1 ran 10..40.
        assert_eq!(stats[order[0]].uid, Uid(1));
        assert_eq!(stats[order[0]].running_time_us, 30);
        m.sort_stats_by(StatsColumn::Node);
        let (stats, order) = m.stats();
        let names: Vec<Uid> = order.iter().map(|i| stats[*i].uid).collect();
        // "bad", "ok", "seq".
        assert_eq!(names, [Uid(3), Uid(2), Uid(1)]);
        m.sort_stats_by(StatsColumn::Node);
        assert!(!m.stats_sort.ascending);
        m.set_range(Some((45, 60)));
        let (stats, order) = m.stats();
        let failure: u64 = order.iter().map(|i| stats[*i].failure_count).sum();
        assert_eq!(failure, 0);
        m.hide_unchanged = false;
        m.set_range(Some((0, 5)));
        let (_, order) = m.stats();
        assert_eq!(order.len(), 3);
        m.hide_unchanged = true;
        let (_, order) = m.stats();
        assert!(order.is_empty());
    }

    #[test]
    fn density_counts_each_part() {
        let mut m = model();
        let counts = m.density((0, 60), 6);
        assert_eq!(counts.iter().sum::<u32>(), 7);
        assert_eq!(counts.as_slice(), &[0, 1, 1, 1, 1, 3]);
        let again = m.density((0, 60), 6);
        assert!(Arc::ptr_eq(&counts, &again));
        assert_eq!(m.density((20, 30), 1).as_slice(), &[2]);
        assert_eq!(density(m.index(), (100, 200), 3), vec![0, 0, 0]);
    }

    #[test]
    fn time_formats() {
        assert_eq!(format_clock(0), "0:00.000");
        assert_eq!(format_clock(62_345_678), "1:02.345");
        assert_eq!(format_clock(3_723_004_000), "1:02:03.004");
        assert_eq!(format_seconds(12_000_345), "12.000345");
        assert_eq!(format_duration(850), "850 \u{00B5}s");
        assert_eq!(format_duration(12_500), "12.5 ms");
        assert_eq!(format_duration(3_250_000), "3.250 s");
        assert_eq!(format_wall_time(0), "1970-01-01 00:00:00.000000 UTC");
        // 2026-10-15 05:45:26.123456 UTC.
        assert_eq!(
            format_wall_time(1_792_043_126_123_456),
            "2026-10-15 05:45:26.123456 UTC"
        );
        assert_eq!(format_wall_time(-1), "1969-12-31 23:59:59.999999 UTC");
        // A leap day.
        assert_eq!(
            format_wall_time(951_782_400_000_000),
            "2000-02-29 00:00:00.000000 UTC"
        );
    }

    #[test]
    fn harness_log_model() {
        let path = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .join("../../tests/fixtures/harness/main.btlog");
        let log = BtLog::open(&path).unwrap();
        let index = ReplayIndex::from_log(&log);
        let count = index.len();
        let mut m = ReplayModel::new(log, index);
        assert_eq!(m.rows().len(), count);
        let (stats, _) = m.stats();
        assert!(!stats.is_empty());
        assert!(!format_wall_time(m.start_time_us()).is_empty());
    }
}
