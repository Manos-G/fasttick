//! Tests with logs that the BT.CPP test executor writes now.
//!
//! The tests run `fasttick_executor` from `test_harness/` with `FileLogger2`
//! and `SqliteLogger`. They skip (and pass) when BT.CPP 4.x is not available
//! (see `fasttick_testkit::require_harness!`). Set `FASTTICK_BTCPP_PREFIX`
//! to run them.

use std::path::Path;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use fasttick_core::runtime::Uid;
use fasttick_core::status::NodeStatus;
use fasttick_log::btlog::BtLog;
use fasttick_testkit::{Executor, ExecutorOptions, Harness, require_harness};

/// Runs `main.xml` two times with the given loggers.
fn run_executor(harness: &Harness, btlog: &Path, sqlite: Option<&Path>) -> Result<(), String> {
    let mut options = ExecutorOptions::new("main.xml");
    options.btlog = Some(btlog.to_owned());
    options.sqlite = sqlite.map(Path::to_owned);
    options.loops = 2;
    options.tick_ms = 2;
    let mut executor = Executor::spawn_with(harness, options).map_err(|err| err.to_string())?;
    executor
        .wait_done(Duration::from_secs(60))
        .map_err(|err| err.to_string())?;
    assert_eq!(executor.results(), ["SUCCESS", "SUCCESS"]);
    Ok(())
}

fn now_us() -> i64 {
    let since_epoch = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default();
    i64::try_from(since_epoch.as_micros()).unwrap_or(i64::MAX)
}

#[test]
fn btlog_from_the_executor() {
    let harness = require_harness!();
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("run.btlog");
    let before_us = now_us();
    run_executor(harness, &path, None).unwrap();
    let after_us = now_us();

    let bytes = std::fs::read(&path).unwrap();
    let log = BtLog::from_bytes(&bytes).unwrap();
    assert_eq!(log.invalid_records, 0);
    assert_eq!(log.truncated_bytes, 0);
    assert!(log.unknown_uids().is_empty());
    assert!(
        (before_us..=after_us).contains(&log.start_time_us),
        "start time {} is not in {before_us}..={after_us}",
        log.start_time_us
    );
    let end_us = log.absolute_time_us(log.end_offset_us());
    assert!(end_us <= after_us, "end {end_us} after {after_us}");
    assert!(
        log.transitions
            .windows(2)
            .all(|pair| pair[0].offset_us <= pair[1].offset_us)
    );

    // Two runs of the root: RUNNING, SUCCESS, IDLE each.
    let root: Vec<NodeStatus> = log
        .transitions
        .iter()
        .filter(|transition| transition.uid == Uid(1))
        .map(|transition| transition.status)
        .collect();
    use NodeStatus::{Idle, Running, Success};
    assert_eq!(root, [Running, Success, Idle, Running, Success, Idle]);

    // The writer gives the same bytes as FileLogger2.
    assert_eq!(log.to_bytes().unwrap(), bytes);
}

#[cfg(feature = "sqlite")]
#[test]
fn sqlite_matches_btlog() {
    use std::collections::HashMap;

    use fasttick_log::sqlite::SqliteLog;

    let harness = require_harness!();
    let dir = tempfile::tempdir().unwrap();
    let btlog_path = dir.path().join("run.btlog");
    let db_path = dir.path().join("run.db3");
    if let Err(err) = run_executor(harness, &btlog_path, Some(&db_path)) {
        if err.contains("no SqliteLogger") {
            eprintln!("SKIPPED: this BT.CPP build has no SqliteLogger");
            return;
        }
        panic!("{err}");
    }
    let btlog = BtLog::open(&btlog_path).unwrap();

    let db = SqliteLog::open(&db_path).unwrap();
    let sessions = db.sessions().unwrap();
    assert_eq!(sessions.len(), 1);
    let session = db.load_session(sessions[0].id).unwrap();
    assert_eq!(session.info, sessions[0]);
    assert_eq!(session.info.date.len(), "2026-10-14 21:05:00".len());
    let log = &session.log;
    // Both loggers use WriteTreeToXML(tree, true, true).
    assert_eq!(log.xml, btlog.xml);
    assert_eq!(log.invalid_records, 0);
    assert!(log.unknown_uids().is_empty());
    assert_eq!(session.info.transition_count, log.transitions.len() as u64);
    assert_eq!(session.extra.len(), log.transitions.len());

    // The Nodes table has each node of the tree once.
    let mut node_uids: Vec<Uid> = session.nodes.iter().map(|(_, uid)| *uid).collect();
    node_uids.sort();
    let mut tree_uids: Vec<Uid> = log.tree.nodes.iter().filter_map(|node| node.uid).collect();
    tree_uids.sort();
    assert_eq!(node_uids, tree_uids);

    // The SQLite rows are the same transitions in the same order. The
    // logger can lose the last rows when it stops (see the module
    // documentation), so they are a prefix.
    let count = log.transitions.len();
    assert!(count > 0);
    assert!(count <= btlog.transitions.len());
    for (position, (row, reference)) in log.transitions.iter().zip(&btlog.transitions).enumerate() {
        assert_eq!(
            (row.uid, row.status),
            (reference.uid, reference.status),
            "row {position}"
        );
        // Both loggers get the same time from BT.CPP. The SQLite value is
        // not truncated twice and is made strictly increasing (+1 µs).
        let sqlite_us = log.absolute_time_us(row.offset_us);
        let btlog_us = btlog.absolute_time_us(reference.offset_us);
        let difference = sqlite_us - btlog_us;
        assert!(
            (0..=1 + position as i64).contains(&difference),
            "row {position}: {sqlite_us} vs {btlog_us}"
        );
        assert_eq!(session.extra[position].timestamp_us, sqlite_us);
        assert_eq!(session.extra[position].extra_data.as_deref(), Some(""));
    }

    // The duration column, as the module documentation describes it.
    let mut status: HashMap<Uid, NodeStatus> = HashMap::new();
    let mut started: HashMap<Uid, i64> = HashMap::new();
    for (row, extra) in log.transitions.iter().zip(&session.extra) {
        let previous = status.insert(row.uid, row.status).unwrap_or_default();
        if previous == NodeStatus::Idle && row.status == NodeStatus::Running {
            started.insert(row.uid, extra.timestamp_us);
        }
        let expected = if previous == NodeStatus::Running && row.status != NodeStatus::Running {
            extra.timestamp_us - started.get(&row.uid).copied().unwrap_or(0)
        } else {
            0
        };
        assert_eq!(extra.duration_us, Some(expected), "{row:?}");
    }
}
