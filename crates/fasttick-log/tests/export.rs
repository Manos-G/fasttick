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

/// A small RFC 4180 reader for the tests. It also checks that a field
/// without quotes has no comma, quote, CR or LF.
fn parse_csv(text: &str) -> Vec<Vec<String>> {
    let mut rows = Vec::new();
    let mut row = Vec::new();
    let mut field = String::new();
    let mut chars = text.chars().peekable();
    let mut in_quotes = false;
    let mut at_field_start = true;
    while let Some(c) = chars.next() {
        if in_quotes {
            if c == '"' {
                if chars.peek() == Some(&'"') {
                    chars.next();
                    field.push('"');
                } else {
                    in_quotes = false;
                }
            } else {
                field.push(c);
            }
            continue;
        }
        match c {
            '"' => {
                assert!(at_field_start, "quote inside a field without quotes");
                in_quotes = true;
            }
            ',' => {
                row.push(std::mem::take(&mut field));
                at_field_start = true;
                continue;
            }
            '\r' => {
                assert_eq!(chars.next(), Some('\n'), "CR without LF");
                row.push(std::mem::take(&mut field));
                rows.push(std::mem::take(&mut row));
                at_field_start = true;
                continue;
            }
            '\n' => panic!("LF without CR outside quotes"),
            other => field.push(other),
        }
        at_field_start = false;
    }
    assert!(!in_quotes, "unterminated quotes");
    assert!(field.is_empty() && row.is_empty(), "last line without CRLF");
    rows
}

proptest::proptest! {
    #![proptest_config(proptest::prelude::ProptestConfig::with_cases(256))]

    #[test]
    fn prop_csv_fields_read_back(
        names in proptest::collection::vec(
            (
                proptest::prelude::any::<String>(),
                "[a,\" \r\n]{0,8}",
                "[ab\"]{0,4}",
            ),
            0..6,
        ),
    ) {
        let rows: Vec<ExportRow> = names
            .iter()
            .enumerate()
            .map(|(position, (name, path, kind))| ExportRow {
                offset_us: position as u64,
                absolute_time_us: -(position as i64),
                uid: Uid(position as u16),
                node_name: name.clone(),
                node_path: path.clone(),
                node_type: kind.clone(),
                previous_status: NodeStatus::Idle,
                status: NodeStatus::Skipped,
            })
            .collect();
        let table = parse_csv(&csv(&rows));
        proptest::prop_assert_eq!(table.len(), rows.len() + 1);
        proptest::prop_assert_eq!(table[0].len(), 8);
        for (row, fields) in rows.iter().zip(&table[1..]) {
            let expected = vec![
                row.offset_us.to_string(),
                row.absolute_time_us.to_string(),
                row.uid.0.to_string(),
                row.node_name.clone(),
                row.node_path.clone(),
                row.node_type.clone(),
                "IDLE".to_string(),
                "SKIPPED".to_string(),
            ];
            proptest::prop_assert_eq!(fields, &expected);
        }
    }
}
