//! Tests of the JSON and CSV export.

mod support;

use fasttick_core::runtime::Uid;
use fasttick_core::status::NodeStatus;
use fasttick_log::btlog::{BtLog, BtLogWriter, Transition};
use fasttick_log::export::{ExportRow, build_rows, export_csv, export_json, export_rows};
use fasttick_log::filter::TransitionFilter;
use support::{SAMPLE_XML, START_US, t};

fn log(transitions: &[Transition]) -> BtLog {
    let mut writer = BtLogWriter::new(Vec::new(), SAMPLE_XML, START_US).unwrap();
    writer.push_batch(transitions).unwrap();
    BtLog::from_bytes(&writer.into_inner()).unwrap()
}

fn sample() -> BtLog {
    log(&[
        t(20, 3, NodeStatus::Running),
        t(10, 2, NodeStatus::Running),
        t(30, 3, NodeStatus::Success),
        t(40, 8, NodeStatus::Failure),
        t(50, 3, NodeStatus::Idle),
        t(60, 999, NodeStatus::Running),
    ])
}

fn csv(rows: &[ExportRow]) -> String {
    let mut out = Vec::new();
    export_csv(rows, &mut out).unwrap();
    String::from_utf8(out).unwrap()
}

#[test]
fn rows_have_names_paths_and_previous_status() {
    let rows = export_rows(&sample(), &TransitionFilter::new());
    assert_eq!(rows.len(), 6);
    // Time order.
    let offsets: Vec<u64> = rows.iter().map(|row| row.offset_us).collect();
    assert_eq!(offsets, vec![10, 20, 30, 40, 50, 60]);

    assert_eq!(
        rows[0],
        ExportRow {
            offset_us: 10,
            absolute_time_us: START_US + 10,
            uid: Uid(2),
            node_name: "move_to_goal".into(),
            node_path: "move_to_goal".into(),
            node_type: "MoveBase".into(),
            previous_status: NodeStatus::Idle,
            status: NodeStatus::Running,
        }
    );
    assert_eq!(rows[2].previous_status, NodeStatus::Running);
    assert_eq!(rows[2].status, NodeStatus::Success);
    assert_eq!(rows[3].node_path, "dock_sub/two\nlines");
    assert_eq!(rows[4].previous_status, NodeStatus::Success);
    // Unknown UID: empty texts.
    assert_eq!(rows[5].uid, Uid(999));
    assert_eq!(rows[5].node_name, "");
    assert_eq!(rows[5].node_type, "");
}

#[test]
fn previous_status_uses_filtered_out_transitions() {
    let filter = TransitionFilter {
        statuses: Some([NodeStatus::Idle].into()),
        ..TransitionFilter::default()
    };
    let rows = export_rows(&sample(), &filter);
    assert_eq!(rows.len(), 1);
    assert_eq!(rows[0].uid, Uid(3));
    assert_eq!(rows[0].previous_status, NodeStatus::Success);
}

#[test]
fn build_rows_keeps_the_given_order() {
    let log = sample();
    let rows = build_rows(&log.tree, log.start_time_us, &log.transitions, None);
    let offsets: Vec<u64> = rows.iter().map(|row| row.offset_us).collect();
    assert_eq!(offsets, vec![20, 10, 30, 40, 50, 60]);
}

#[test]
fn csv_output_is_exact() {
    let log = log(&[t(5, 3, NodeStatus::Running), t(7, 8, NodeStatus::Skipped)]);
    let rows = export_rows(&log, &TransitionFilter::new());
    let expected = format!(
        "offset_us,absolute_time_us,uid,node_name,node_path,node_type,previous_status,status\r\n\
         5,{},3,\"say, \"\"hi\"\"\",\"say, \"\"hi\"\"\",SaySomething,IDLE,RUNNING\r\n\
         7,{},8,\"two\nlines\",\"dock_sub/two\nlines\",AlwaysSuccess,IDLE,SKIPPED\r\n",
        START_US + 5,
        START_US + 7
    );
    assert_eq!(csv(&rows), expected);
}

#[test]
fn csv_without_rows_has_the_header() {
    assert_eq!(
        csv(&[]),
        "offset_us,absolute_time_us,uid,node_name,node_path,node_type,previous_status,status\r\n"
    );
}

#[test]
fn json_output() {
    let rows = export_rows(&sample(), &TransitionFilter::new());
    let mut out = Vec::new();
    export_json(&rows, &mut out).unwrap();
    let text = String::from_utf8(out).unwrap();
    // Pretty output: one field per line.
    assert!(text.contains("\n    \"offset_us\": 10,\n"), "{text}");
    let value: serde_json::Value = serde_json::from_str(&text).unwrap();
    let array = value.as_array().unwrap();
    assert_eq!(array.len(), 6);
    assert_eq!(array[0]["uid"], 2);
    assert_eq!(array[0]["node_type"], "MoveBase");
    assert_eq!(array[0]["previous_status"], "IDLE");
    assert_eq!(array[0]["status"], "RUNNING");
    assert_eq!(array[0]["absolute_time_us"], START_US + 10);
    let back: Vec<ExportRow> = serde_json::from_value(value).unwrap();
    assert_eq!(back, rows);

    let mut out = Vec::new();
    export_json(&[], &mut out).unwrap();
    assert_eq!(String::from_utf8(out).unwrap(), "[]\n");
}
