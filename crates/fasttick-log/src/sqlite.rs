//! The SQLite database of `BT::SqliteLogger` (feature `sqlite`).
//!
//! The schema comes from `src/loggers/bt_sqlite_logger.cpp` of BT.CPP 4.9.0
//! (constructor `SqliteLogger::SqliteLogger`):
//!
//! ```sql
//! CREATE TABLE IF NOT EXISTS Transitions (
//!     timestamp  INTEGER PRIMARY KEY NOT NULL,
//!     session_id INTEGER NOT NULL,
//!     node_uid   INTEGER NOT NULL,
//!     duration   INTEGER,
//!     state      INTEGER NOT NULL,
//!     extra_data VARCHAR );
//! CREATE TABLE IF NOT EXISTS Nodes (
//!     session_id INTEGER NOT NULL,
//!     fullpath   VARCHAR,
//!     node_uid   INTEGER NOT NULL );
//! CREATE TABLE IF NOT EXISTS Definitions (
//!     session_id INTEGER PRIMARY KEY AUTOINCREMENT,
//!     date       TEXT NOT NULL,
//!     xml_tree   TEXT NOT NULL);
//! ```
//!
//! What the columns contain (from `SqliteLogger::callback` and the
//! constructor):
//!
//! - `Definitions`: one row for each session (one logger). `date` is
//!   `datetime('now','localtime')`, a local time text such as
//!   `2026-10-14 21:05:00` without a time zone. `xml_tree` is
//!   `WriteTreeToXML(tree, true, true)`.
//! - `Nodes`: one row for each node of the session: the node path
//!   (`TreeNode::fullPath()`) and the UID.
//! - `Transitions.timestamp`: microseconds since the Unix epoch (the logger
//!   default, `TimestampType::absolute`). The logger makes the values
//!   strictly increasing: `max(previous + 1, now)`. The value is the primary
//!   key of the table.
//! - `Transitions.duration`: for a transition from RUNNING to another
//!   status, the microseconds since the last IDLE to RUNNING transition of
//!   that node; otherwise 0.
//! - `Transitions.state`: the new status, 0 to 4 (`BT::NodeStatus`).
//! - `Transitions.extra_data`: text from an optional user callback; usually
//!   empty.
//!
//! How FastTick maps a session to a [`BtLog`]:
//!
//! - `xml` and `tree` come from `xml_tree`.
//! - `start_time_us` is the smallest `timestamp` of the session (0 if the
//!   session has no transitions). The table does not store the logger start
//!   time, and `date` has no time zone.
//! - Each row becomes a [`Transition`]: `offset_us = timestamp -
//!   start_time_us`, `uid = node_uid`, `status = state`. The rows are in
//!   `timestamp` order.
//! - A row with a `state` that is not 0 to 4, or a `node_uid` that is not a
//!   `u16`, or a value of the wrong type is counted in
//!   [`BtLog::invalid_records`] and skipped.
//! - `duration` and `extra_data` are in [`SqliteSession::extra`].
//! - `truncated_bytes` is always 0.

use std::path::Path;
use std::time::Duration;

use fasttick_core::runtime::{RuntimeTree, Uid};
use fasttick_core::status::NodeStatus;
use rusqlite::types::ValueRef;
use rusqlite::{Connection, OpenFlags, OptionalExtension};

use crate::btlog::{BtLog, LogError, Transition};

/// One session (one `Definitions` row).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SessionInfo {
    /// The session ID.
    pub id: i64,
    /// The local date and time of the session start, as text.
    pub date: String,
    /// The tree XML.
    pub xml: String,
    /// The number of rows in `Transitions` for this session.
    pub transition_count: u64,
}

/// The columns of a transition row that [`Transition`] does not have.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SqliteTransitionExtra {
    /// The `timestamp` column (microseconds since the Unix epoch).
    pub timestamp_us: i64,
    /// The `duration` column (microseconds), if it is not NULL.
    pub duration_us: Option<i64>,
    /// The `extra_data` column, if it is not NULL.
    pub extra_data: Option<String>,
}

/// One session, loaded.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SqliteSession {
    /// The session.
    pub info: SessionInfo,
    /// The session as a log. See the module documentation for the mapping.
    pub log: BtLog,
    /// For each transition of [`SqliteSession::log`]: the other columns.
    pub extra: Vec<SqliteTransitionExtra>,
    /// The `Nodes` rows of the session: node path and UID. Empty if the
    /// table does not exist.
    pub nodes: Vec<(String, Uid)>,
}

/// A read-only connection to a `BT::SqliteLogger` database.
#[derive(Debug)]
pub struct SqliteLog {
    connection: Connection,
    has_nodes_table: bool,
}

impl SqliteLog {
    /// Opens a database file read-only. Returns an error if the file does
    /// not exist or does not have the tables `Definitions` and
    /// `Transitions`.
    pub fn open(path: impl AsRef<Path>) -> Result<SqliteLog, LogError> {
        let connection = Connection::open_with_flags(
            path,
            OpenFlags::SQLITE_OPEN_READ_ONLY | OpenFlags::SQLITE_OPEN_NO_MUTEX,
        )?;
        SqliteLog::from_connection(connection)
    }

    /// Uses an open connection. Checks the tables like [`SqliteLog::open`].
    pub fn from_connection(connection: Connection) -> Result<SqliteLog, LogError> {
        // The executor can write the database at the same time.
        connection.busy_timeout(Duration::from_millis(500))?;
        let mut statement =
            connection.prepare("SELECT name FROM sqlite_master WHERE type = 'table'")?;
        let tables: Vec<String> = statement
            .query_map([], |row| row.get::<_, String>(0))?
            .collect::<Result<_, _>>()?;
        drop(statement);
        for required in ["Definitions", "Transitions"] {
            if !tables.iter().any(|name| name == required) {
                return Err(LogError::MissingTable(required));
            }
        }
        let has_nodes_table = tables.iter().any(|name| name == "Nodes");
        Ok(SqliteLog {
            connection,
            has_nodes_table,
        })
    }

    /// The connection.
    pub fn connection(&self) -> &Connection {
        &self.connection
    }

    /// All sessions, in ID order.
    pub fn sessions(&self) -> Result<Vec<SessionInfo>, LogError> {
        let mut statement = self.connection.prepare(
            "SELECT d.session_id, d.date, d.xml_tree, COALESCE(c.n, 0) \
             FROM Definitions d LEFT JOIN \
             (SELECT session_id, COUNT(*) AS n FROM Transitions GROUP BY session_id) c \
             ON c.session_id = d.session_id ORDER BY d.session_id",
        )?;
        let rows = statement.query_map([], |row| {
            Ok(SessionInfo {
                id: row.get(0)?,
                date: text(row.get_ref(1)?),
                xml: text(row.get_ref(2)?),
                transition_count: u64::try_from(row.get::<_, i64>(3)?).unwrap_or(0),
            })
        })?;
        Ok(rows.collect::<Result<_, _>>()?)
    }

    /// Loads one session. See the module documentation for the mapping.
    pub fn load_session(&self, id: i64) -> Result<SqliteSession, LogError> {
        let definition = self
            .connection
            .query_row(
                "SELECT date, xml_tree FROM Definitions WHERE session_id = ?1",
                [id],
                |row| Ok((text(row.get_ref(0)?), text(row.get_ref(1)?))),
            )
            .optional()?;
        let Some((date, xml)) = definition else {
            return Err(LogError::SessionNotFound(id));
        };
        let tree = RuntimeTree::parse(&xml).map_err(LogError::InvalidXml)?;

        let mut statement = self.connection.prepare(
            "SELECT timestamp, node_uid, duration, state, extra_data \
             FROM Transitions WHERE session_id = ?1 ORDER BY timestamp",
        )?;
        let rows = statement.query_map([id], |row| {
            Ok(decode_row(
                row.get_ref(0)?,
                row.get_ref(1)?,
                row.get_ref(2)?,
                row.get_ref(3)?,
                row.get_ref(4)?,
            ))
        })?;
        let mut decoded: Vec<(i64, Uid, NodeStatus, SqliteTransitionExtra)> = Vec::new();
        let mut invalid_records = 0;
        let mut row_count: u64 = 0;
        for row in rows {
            row_count += 1;
            match row? {
                Some(item) => decoded.push(item),
                None => invalid_records += 1,
            }
        }
        drop(statement);

        let start_time_us = decoded
            .iter()
            .map(|(timestamp, ..)| *timestamp)
            .min()
            .unwrap_or(0);
        let mut transitions = Vec::with_capacity(decoded.len());
        let mut extra = Vec::with_capacity(decoded.len());
        for (timestamp, uid, status, more) in decoded {
            let offset_us = u64::try_from(i128::from(timestamp) - i128::from(start_time_us))
                .unwrap_or(u64::MAX);
            transitions.push(Transition::new(offset_us, uid, status));
            extra.push(more);
        }

        let nodes = if self.has_nodes_table {
            self.session_nodes(id)?
        } else {
            Vec::new()
        };

        Ok(SqliteSession {
            info: SessionInfo {
                id,
                date,
                xml: xml.clone(),
                transition_count: row_count,
            },
            log: BtLog {
                xml,
                tree,
                start_time_us,
                transitions,
                truncated_bytes: 0,
                invalid_records,
            },
            extra,
            nodes,
        })
    }

    fn session_nodes(&self, id: i64) -> Result<Vec<(String, Uid)>, LogError> {
        let mut statement = self
            .connection
            .prepare("SELECT fullpath, node_uid FROM Nodes WHERE session_id = ?1")?;
        let rows = statement.query_map([id], |row| {
            let path = text(row.get_ref(0)?);
            let uid = integer(row.get_ref(1)?).and_then(|value| u16::try_from(value).ok());
            Ok(uid.map(|uid| (path, Uid(uid))))
        })?;
        let mut out = Vec::new();
        for row in rows {
            if let Some(item) = row? {
                out.push(item);
            }
        }
        Ok(out)
    }
}

/// Decodes one `Transitions` row. Returns `None` for an invalid row.
fn decode_row(
    timestamp: ValueRef<'_>,
    node_uid: ValueRef<'_>,
    duration: ValueRef<'_>,
    state: ValueRef<'_>,
    extra_data: ValueRef<'_>,
) -> Option<(i64, Uid, NodeStatus, SqliteTransitionExtra)> {
    let timestamp = integer(timestamp)?;
    let uid = u16::try_from(integer(node_uid)?).ok()?;
    let status = u8::try_from(integer(state)?)
        .ok()
        .and_then(NodeStatus::from_u8)?;
    let duration_us = match duration {
        ValueRef::Null => None,
        other => Some(integer(other)?),
    };
    let extra_data = match extra_data {
        ValueRef::Null => None,
        other => Some(text(other)),
    };
    Some((
        timestamp,
        Uid(uid),
        status,
        SqliteTransitionExtra {
            timestamp_us: timestamp,
            duration_us,
            extra_data,
        },
    ))
}

/// An integer value. Text with an integer is also accepted, because SQLite
/// columns have no strict type.
fn integer(value: ValueRef<'_>) -> Option<i64> {
    match value {
        ValueRef::Integer(value) => Some(value),
        ValueRef::Text(bytes) => std::str::from_utf8(bytes).ok()?.trim().parse().ok(),
        _ => None,
    }
}

/// A text value. Other types are converted; NULL gives an empty text.
fn text(value: ValueRef<'_>) -> String {
    match value {
        ValueRef::Null => String::new(),
        ValueRef::Integer(value) => value.to_string(),
        ValueRef::Real(value) => value.to_string(),
        ValueRef::Text(bytes) | ValueRef::Blob(bytes) => {
            String::from_utf8_lossy(bytes).into_owned()
        }
    }
}
