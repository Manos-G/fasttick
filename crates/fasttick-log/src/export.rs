//! Export of transitions to JSON and CSV.
//!
//! Each transition becomes one [`ExportRow`]. The rows are in time order
//! (stable for equal times). `previous_status` is the status of the node
//! before the transition. It comes from all transitions of the log, also
//! from the transitions that the filter removes.

use std::collections::HashMap;
use std::io::{self, Write};

use fasttick_core::runtime::{RuntimeTree, Uid};
use fasttick_core::status::NodeStatus;
use serde::{Deserialize, Serialize};

use crate::btlog::{BtLog, Transition, absolute_time_us};
use crate::filter::{CompiledFilter, TransitionFilter};

/// One exported transition.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct ExportRow {
    /// Microseconds since the log start.
    pub offset_us: u64,
    /// Microseconds since the Unix epoch.
    pub absolute_time_us: i64,
    /// The node UID.
    pub uid: Uid,
    /// The node name. Empty if the UID is not in the tree.
    pub node_name: String,
    /// The node path. Empty if the UID is not in the tree.
    pub node_path: String,
    /// The node type ID. Empty if the UID is not in the tree.
    pub node_type: String,
    /// The status before the transition.
    pub previous_status: NodeStatus,
    /// The new status.
    pub status: NodeStatus,
}

/// The CSV column names, in order.
pub const CSV_HEADER: [&str; 8] = [
    "offset_us",
    "absolute_time_us",
    "uid",
    "node_name",
    "node_path",
    "node_type",
    "previous_status",
    "status",
];

/// Makes the rows for transitions in the given order.
///
/// `filter` selects the rows. The previous status uses all transitions.
pub fn build_rows<'a>(
    tree: &RuntimeTree,
    start_time_us: i64,
    transitions: impl IntoIterator<Item = &'a Transition>,
    filter: Option<&CompiledFilter>,
) -> Vec<ExportRow> {
    // Name, path and type for each UID, made once.
    let mut names: HashMap<Uid, (String, String, String)> = HashMap::new();
    for (position, node) in tree.nodes.iter().enumerate() {
        if let Some(uid) = node.uid {
            let path = tree.node_path(fasttick_core::runtime::NodeIndex(position));
            names.insert(uid, (node.name.clone(), path, node.type_id.clone()));
        }
    }
    let mut previous: HashMap<Uid, NodeStatus> = HashMap::new();
    let mut rows = Vec::new();
    for transition in transitions {
        let before = previous
            .insert(transition.uid, transition.status)
            .unwrap_or_default();
        if filter.is_some_and(|filter| !filter.matches(transition)) {
            continue;
        }
        let (node_name, node_path, node_type) =
            names.get(&transition.uid).cloned().unwrap_or_default();
        rows.push(ExportRow {
            offset_us: transition.offset_us,
            absolute_time_us: absolute_time_us(start_time_us, transition.offset_us),
            uid: transition.uid,
            node_name,
            node_path,
            node_type,
            previous_status: before,
            status: transition.status,
        });
    }
    rows
}

/// Makes the rows of a log that pass a filter, in time order (stable for
/// equal times).
pub fn export_rows(log: &BtLog, filter: &TransitionFilter) -> Vec<ExportRow> {
    let mut sorted: Vec<&Transition> = log.transitions.iter().collect();
    sorted.sort_by_key(|transition| transition.offset_us);
    let compiled = filter.compile(&log.tree);
    build_rows(&log.tree, log.start_time_us, sorted, Some(&compiled))
}

/// Writes the rows as a pretty JSON array of objects. The statuses are the
/// BT.CPP names (for example `"SUCCESS"`).
pub fn export_json<W: Write>(rows: &[ExportRow], mut writer: W) -> io::Result<()> {
    serde_json::to_writer_pretty(&mut writer, rows)?;
    writer.write_all(b"\n")?;
    writer.flush()
}

/// Writes the rows as CSV (RFC 4180): a header row, then one row for each
/// transition. Lines end with CRLF. A field that contains a comma, a double
/// quote, CR or LF is in double quotes, and each double quote in it is
/// doubled.
pub fn export_csv<W: Write>(rows: &[ExportRow], writer: W) -> io::Result<()> {
    let mut writer = io::BufWriter::new(writer);
    write_csv_record(&mut writer, CSV_HEADER.iter().copied())?;
    for row in rows {
        let offset = row.offset_us.to_string();
        let absolute = row.absolute_time_us.to_string();
        let uid = row.uid.0.to_string();
        let fields = [
            offset.as_str(),
            absolute.as_str(),
            uid.as_str(),
            row.node_name.as_str(),
            row.node_path.as_str(),
            row.node_type.as_str(),
            row.previous_status.as_str(),
            row.status.as_str(),
        ];
        write_csv_record(&mut writer, fields.into_iter())?;
    }
    writer.flush()
}

fn write_csv_record<'a, W: Write>(
    writer: &mut W,
    fields: impl Iterator<Item = &'a str>,
) -> io::Result<()> {
    for (position, field) in fields.enumerate() {
        if position > 0 {
            writer.write_all(b",")?;
        }
        write_csv_field(writer, field)?;
    }
    writer.write_all(b"\r\n")
}

/// Writes one CSV field with RFC 4180 quoting.
pub fn write_csv_field<W: Write>(writer: &mut W, field: &str) -> io::Result<()> {
    let needs_quotes = field.contains([',', '"', '\r', '\n']);
    if !needs_quotes {
        return writer.write_all(field.as_bytes());
    }
    writer.write_all(b"\"")?;
    writer.write_all(field.replace('"', "\"\"").as_bytes())?;
    writer.write_all(b"\"")
}

#[cfg(test)]
mod tests {
    use super::*;

    fn field(text: &str) -> String {
        let mut out = Vec::new();
        write_csv_field(&mut out, text).unwrap();
        String::from_utf8(out).unwrap()
    }

    #[test]
    fn csv_quoting() {
        assert_eq!(field("plain"), "plain");
        assert_eq!(field(""), "");
        assert_eq!(field("a,b"), "\"a,b\"");
        assert_eq!(field("say \"hi\""), "\"say \"\"hi\"\"\"");
        assert_eq!(field("line\nbreak"), "\"line\nbreak\"");
        assert_eq!(field("cr\rhere"), "\"cr\rhere\"");
        assert_eq!(field("\""), "\"\"\"\"");
        assert_eq!(field(" spaces "), " spaces ");
        assert_eq!(field("ünïcode"), "ünïcode");
    }
}
