//! Tests of the SQLite reader. The database uses the schema of
//! `bt_sqlite_logger.cpp` (BT.CPP 4.9.0).

#![cfg(feature = "sqlite")]

mod support;

use std::path::Path;

use fasttick_core::runtime::Uid;
use fasttick_core::status::NodeStatus;
use fasttick_log::btlog::LogError;
use fasttick_log::sqlite::{SqliteLog, SqliteTransitionExtra};
use rusqlite::{Connection, params};
use support::{SAMPLE_XML, START_US, t};

/// The statements of the `SqliteLogger` constructor, unchanged.
const SCHEMA: [&str; 3] = [
    "CREATE TABLE IF NOT EXISTS Transitions (\
     timestamp  INTEGER PRIMARY KEY NOT NULL, \
     session_id INTEGER NOT NULL, \
     node_uid   INTEGER NOT NULL, \
     duration   INTEGER, \
     state      INTEGER NOT NULL,\
     extra_data VARCHAR );",
    "CREATE TABLE IF NOT EXISTS Nodes (\
     session_id INTEGER NOT NULL, \
     fullpath   VARCHAR, \
     node_uid   INTEGER NOT NULL );",
    "CREATE TABLE IF NOT EXISTS Definitions (\
     session_id INTEGER PRIMARY KEY AUTOINCREMENT, \
     date       TEXT NOT NULL,\
     xml_tree   TEXT NOT NULL);",
];

fn create_database(path: &Path) -> Connection {
    let connection = Connection::open(path).unwrap();
    for statement in SCHEMA {
        connection.execute_batch(statement).unwrap();
    }
    connection
}

/// Adds a session like the logger does. Returns the session ID.
fn add_session(connection: &Connection, xml: &str, nodes: &[(&str, u16)]) -> i64 {
    connection
        .execute(
            "INSERT into Definitions (date, xml_tree) VALUES (datetime('now','localtime'),?);",
            [xml],
        )
        .unwrap();
    let id: i64 = connection
        .query_row(
            "SELECT MAX(session_id) FROM Definitions LIMIT 1;",
            [],
            |row| row.get(0),
        )
        .unwrap();
    for (path, uid) in nodes {
        connection
            .execute("INSERT INTO Nodes VALUES (?, ?, ?)", params![id, path, uid])
            .unwrap();
    }
    id
}

fn add_transition(
    connection: &Connection,
    session: i64,
    timestamp: i64,
    uid: i64,
    duration: i64,
    state: i64,
) {
    connection
        .execute(
            "INSERT INTO Transitions VALUES (?, ?, ?, ?, ?, ?)",
            params![timestamp, session, uid, duration, state, ""],
        )
        .unwrap();
}

#[test]
fn read_sessions_and_transitions() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("log.db3");
    let connection = create_database(&path);
    let first = add_session(
        &connection,
        SAMPLE_XML,
        &[("main_seq", 1), ("move_to_goal", 2)],
    );
    // Inserted out of order; the reader sorts by timestamp.
    add_transition(&connection, first, START_US + 500, 2, 400, 2);
    add_transition(&connection, first, START_US + 100, 1, 0, 1);
    add_transition(&connection, first, START_US + 101, 2, 0, 1);
    add_transition(&connection, first, START_US + 600, 1, 500, 2);
    let second = add_session(&connection, SAMPLE_XML, &[]);
    add_transition(&connection, second, START_US + 10_000, 3, 0, 1);
    drop(connection);

    let db = SqliteLog::open(&path).unwrap();
    let sessions = db.sessions().unwrap();
    assert_eq!(sessions.len(), 2);
    assert_eq!(sessions[0].id, first);
    assert_eq!(sessions[1].id, second);
    assert_eq!(sessions[0].transition_count, 4);
    assert_eq!(sessions[1].transition_count, 1);
    assert_eq!(sessions[0].xml, SAMPLE_XML);
    // datetime('now','localtime'): "YYYY-MM-DD HH:MM:SS".
    assert_eq!(sessions[0].date.len(), 19, "{}", sessions[0].date);

    let session = db.load_session(first).unwrap();
    assert_eq!(session.info, sessions[0]);
    let log = &session.log;
    assert_eq!(log.start_time_us, START_US + 100);
    assert_eq!(
        log.transitions,
        vec![
            t(0, 1, NodeStatus::Running),
            t(1, 2, NodeStatus::Running),
            t(400, 2, NodeStatus::Success),
            t(500, 1, NodeStatus::Success),
        ]
    );
    assert_eq!(log.invalid_records, 0);
    assert_eq!(log.truncated_bytes, 0);
    assert_eq!(log.tree.len(), 8);
    assert_eq!(
        session.extra[2],
        SqliteTransitionExtra {
            timestamp_us: START_US + 500,
            duration_us: Some(400),
            extra_data: Some(String::new()),
        }
    );
    assert_eq!(
        session.nodes,
        vec![
            ("main_seq".to_string(), Uid(1)),
            ("move_to_goal".to_string(), Uid(2))
        ]
    );

    let session = db.load_session(second).unwrap();
    assert_eq!(session.log.transitions, vec![t(0, 3, NodeStatus::Running)]);
    assert!(session.nodes.is_empty());
}

#[test]
fn invalid_rows_are_counted() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("log.btdb");
    let connection = create_database(&path);
    let id = add_session(&connection, SAMPLE_XML, &[]);
    add_transition(&connection, id, 10, 1, 0, 1);
    add_transition(&connection, id, 11, 1, 0, 7); // unknown state
    add_transition(&connection, id, 12, 70_000, 0, 1); // UID too large
    add_transition(&connection, id, 13, -1, 0, 1); // negative UID
    connection
        .execute(
            "INSERT INTO Transitions VALUES (14, ?, 'abc', NULL, 2, NULL)",
            [id],
        )
        .unwrap();
    connection
        .execute(
            "INSERT INTO Transitions VALUES (15, ?, '2', NULL, 3, NULL)",
            [id],
        )
        .unwrap();
    drop(connection);

    let db = SqliteLog::open(&path).unwrap();
    let session = db.load_session(id).unwrap();
    assert_eq!(session.log.invalid_records, 4);
    assert_eq!(session.info.transition_count, 6);
    assert_eq!(
        session.log.transitions,
        vec![t(0, 1, NodeStatus::Running), t(5, 2, NodeStatus::Failure)]
    );
    assert_eq!(session.extra[1].duration_us, None);
    assert_eq!(session.extra[1].extra_data, None);
}

#[test]
fn empty_session() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("log.db3");
    let connection = create_database(&path);
    let id = add_session(&connection, SAMPLE_XML, &[]);
    drop(connection);
    let session = SqliteLog::open(&path).unwrap().load_session(id).unwrap();
    assert!(session.log.transitions.is_empty());
    assert_eq!(session.log.start_time_us, 0);
}

#[test]
fn errors() {
    let dir = tempfile::tempdir().unwrap();

    // Missing file: read-only open does not create it.
    let missing = dir.path().join("missing.db3");
    assert!(matches!(
        SqliteLog::open(&missing),
        Err(LogError::Sqlite(_))
    ));
    assert!(!missing.exists());

    // Not a database.
    let text = dir.path().join("text.db3");
    std::fs::write(&text, "this is not a database, it is plain text....").unwrap();
    assert!(SqliteLog::open(&text).is_err());

    // A database without the tables.
    let other = dir.path().join("other.db3");
    Connection::open(&other)
        .unwrap()
        .execute_batch("CREATE TABLE Things (x INTEGER);")
        .unwrap();
    assert!(matches!(
        SqliteLog::open(&other),
        Err(LogError::MissingTable("Definitions"))
    ));

    // Unknown session and invalid XML.
    let path = dir.path().join("log.db3");
    let connection = create_database(&path);
    let id = add_session(&connection, "<root><BehaviorTree", &[]);
    drop(connection);
    let db = SqliteLog::open(&path).unwrap();
    assert!(matches!(
        db.load_session(id + 1),
        Err(LogError::SessionNotFound(_))
    ));
    assert!(matches!(db.load_session(id), Err(LogError::InvalidXml(_))));
}

#[test]
fn other_schemas_give_errors() {
    let dir = tempfile::tempdir().unwrap();
    // Tables with the right names but other columns.
    let path = dir.path().join("other_columns.db3");
    Connection::open(&path)
        .unwrap()
        .execute_batch(
            "CREATE TABLE Definitions (id INTEGER PRIMARY KEY, tree TEXT);
             CREATE TABLE Transitions (t INTEGER, uid INTEGER);
             INSERT INTO Definitions VALUES (1, '<root/>');",
        )
        .unwrap();
    let db = SqliteLog::open(&path).unwrap();
    assert!(matches!(db.sessions(), Err(LogError::Sqlite(_))));
    assert!(matches!(db.load_session(1), Err(LogError::Sqlite(_))));

    // Values of unusual types in the right columns.
    let path = dir.path().join("odd_values.db3");
    let connection = create_database(&path);
    let id = add_session(&connection, SAMPLE_XML, &[("main_seq", 1)]);
    connection
        .execute_batch(&format!(
            "INSERT INTO Transitions VALUES (20, {id}, 1, 'x', 1, 7);
             INSERT INTO Transitions VALUES (21, {id}, 1.5, 0, 1, NULL);
             INSERT INTO Transitions VALUES (22, {id}, 2, 0, X'02', NULL);
             INSERT INTO Transitions VALUES (23, {id}, 2, NULL, ' 2 ', 3.5);
             INSERT INTO Nodes VALUES ({id}, NULL, 'bad');"
        ))
        .unwrap();
    drop(connection);
    let session = SqliteLog::open(&path).unwrap().load_session(id).unwrap();
    // Row 21: a REAL UID, and row 22: a BLOB state, are invalid. Row 20
    // has a text duration: the transition is kept, without the duration.
    // Row 23 is valid: a text with a number is accepted.
    assert_eq!(session.log.invalid_records, 2);
    assert_eq!(
        session.log.transitions,
        vec![t(0, 1, NodeStatus::Running), t(3, 2, NodeStatus::Success)]
    );
    assert_eq!(session.log.start_time_us, 20);
    assert_eq!(session.extra[0].duration_us, None);
    assert_eq!(session.extra[0].extra_data.as_deref(), Some("7"));
    assert_eq!(session.extra[1].duration_us, None);
    assert_eq!(session.extra[1].extra_data.as_deref(), Some("3.5"));
    // The Nodes row with a bad UID is left out.
    assert_eq!(session.nodes, vec![("main_seq".to_string(), Uid(1))]);
}
