//! Readers and tools for BehaviorTree.CPP log files (`.btlog`, SQLite).
//!
//! The crate has these parts:
//!
//! - [`btlog`]: the `.btlog` format of `BT::FileLogger2`. It has a reader
//!   for complete files ([`BtLog`]), an incremental reader for files that
//!   the executor still writes ([`BtLogReader`]) and a writer for live
//!   recordings ([`BtLogWriter`]).
//! - `sqlite` (feature `sqlite`, on by default): the database of
//!   `BT::SqliteLogger`.
//! - [`replay`]: an index that gives the state of all nodes at a time or at
//!   a transition, and a playback [`Cursor`].
//! - [`stats`]: statistics for each node in a time range.
//! - [`filter`]: include and exclude filters for transitions.
//! - [`export`]: export of transitions to JSON and CSV.
//!
//! The crate has no GUI dependencies. Bad input gives an error, not a panic.

pub mod btlog;
pub mod export;
pub mod filter;
pub mod replay;
#[cfg(feature = "sqlite")]
pub mod sqlite;
pub mod stats;

pub use btlog::{BtLog, BtLogHeader, BtLogReader, BtLogWriter, LogError, Transition};
pub use export::{ExportRow, export_csv, export_json, export_rows};
pub use filter::{CompiledFilter, Pattern, TransitionFilter};
pub use replay::{Cursor, NodeState, PlaybackSpeed, ReplayIndex, ReplayState};
#[cfg(feature = "sqlite")]
pub use sqlite::{SessionInfo, SqliteLog, SqliteSession};
pub use stats::{NodeStats, compute_log_stats, compute_stats};

/// Puts the two ends of an inclusive time range in order.
///
/// All time ranges in this crate are `(start_us, end_us)` pairs of offsets
/// from the log start. Both ends are part of the range. A range with
/// `start_us > end_us` is used as `(end_us, start_us)`.
pub(crate) fn ordered_range((a, b): (u64, u64)) -> (u64, u64) {
    if a <= b { (a, b) } else { (b, a) }
}
