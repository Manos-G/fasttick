//! The blackboard watch of live mode, and helpers for the JSON tree view.
//!
//! The publisher sends the watched blackboards as one JSON object with a
//! key for each blackboard name (`generateBlackboardsDump` of the publisher
//! in BT.CPP 4.9.0). Each blackboard is an object with one key for each
//! entry (`ExportBlackboardToJSON`). A custom type that a `JsonExporter`
//! converter writes has a `"__type"` key with the C++ type name
//! (`JsonExporter::addConverter`, `json_export.h`); the view shows it as a
//! type hint.

use std::collections::BTreeSet;
use std::time::{Duration, Instant};

use fasttick_core::runtime::RuntimeTree;
use fasttick_protocol::session::{SessionCommand, SessionHandle};
use serde_json::Value;

/// The key of a type hint in a JSON object (BT.CPP `JsonExporter`).
pub const TYPE_KEY: &str = "__type";

/// The shortest read interval of the blackboard watch, in milliseconds.
pub const MIN_INTERVAL_MS: u32 = 50;
/// The longest read interval of the blackboard watch, in milliseconds.
pub const MAX_INTERVAL_MS: u32 = 10_000;
/// The default read interval, in milliseconds.
pub const DEFAULT_INTERVAL_MS: u32 = 500;

/// Which blackboards to read, how often, and the last values.
#[derive(Debug, Clone, PartialEq)]
pub struct BlackboardWatch {
    selected: BTreeSet<String>,
    interval_ms: u32,
    auto_refresh: bool,
    /// True after the user changed the selection. Before that, the root
    /// blackboard is selected for each new tree.
    user_selected: bool,
    values: Option<Value>,
    updated_at: Option<Instant>,
    updates: u64,
}

impl Default for BlackboardWatch {
    fn default() -> Self {
        BlackboardWatch {
            selected: BTreeSet::new(),
            interval_ms: DEFAULT_INTERVAL_MS,
            auto_refresh: true,
            user_selected: false,
            values: None,
            updated_at: None,
            updates: 0,
        }
    }
}

impl BlackboardWatch {
    /// The watched blackboard names.
    pub fn selected(&self) -> &BTreeSet<String> {
        &self.selected
    }

    /// True if a blackboard is watched.
    pub fn is_selected(&self, name: &str) -> bool {
        self.selected.contains(name)
    }

    /// Watches a blackboard or stops watching it.
    pub fn set_selected(&mut self, name: &str, selected: bool) {
        self.user_selected = true;
        if selected {
            self.selected.insert(name.to_owned());
        } else {
            self.selected.remove(name);
        }
    }

    /// The read interval in milliseconds.
    pub fn interval_ms(&self) -> u32 {
        self.interval_ms
    }

    /// Sets the read interval (clamped to the allowed range).
    pub fn set_interval_ms(&mut self, interval_ms: u32) {
        self.interval_ms = interval_ms.clamp(MIN_INTERVAL_MS, MAX_INTERVAL_MS);
    }

    /// True if the blackboards are read again at each interval.
    pub fn auto_refresh(&self) -> bool {
        self.auto_refresh
    }

    /// Turns the periodic read on or off.
    pub fn set_auto_refresh(&mut self, on: bool) {
        self.auto_refresh = on;
    }

    /// The last values: an object with one key for each blackboard that
    /// the executor knows.
    pub fn values(&self) -> Option<&Value> {
        self.values.as_ref()
    }

    /// The last value of one blackboard.
    pub fn value_of(&self, name: &str) -> Option<&Value> {
        self.values.as_ref()?.get(name)
    }

    /// The time of the last update.
    pub fn updated_at(&self) -> Option<Instant> {
        self.updated_at
    }

    /// The number of updates so far.
    pub fn updates(&self) -> u64 {
        self.updates
    }

    /// The command for the session.
    pub fn command(&self) -> SessionCommand {
        let names: Vec<String> = if self.auto_refresh {
            self.selected.iter().cloned().collect()
        } else {
            Vec::new()
        };
        SessionCommand::WatchBlackboards {
            names,
            interval: Duration::from_millis(u64::from(self.interval_ms)),
        }
    }

    /// Sends the watch to the session.
    pub(crate) fn resend(&self, handle: &SessionHandle) {
        handle.send(self.command());
    }

    /// Reads the watched blackboards once now. With the periodic read on,
    /// a new watch command reads at once and keeps the interval.
    pub(crate) fn refresh(&self, handle: &SessionHandle) {
        if self.auto_refresh {
            self.resend(handle);
        } else if !self.selected.is_empty() {
            handle.send(SessionCommand::WatchBlackboards {
                names: self.selected.iter().cloned().collect(),
                interval: Duration::ZERO,
            });
        }
    }

    /// Keeps only the names that the new tree has. Before the user chose,
    /// the root blackboard is watched.
    pub(crate) fn tree_changed(&mut self, tree: &RuntimeTree, handle: Option<&SessionHandle>) {
        let names: BTreeSet<String> = tree
            .instances
            .iter()
            .map(|instance| instance.blackboard_name().to_owned())
            .collect();
        let before = self.selected.clone();
        self.selected.retain(|name| names.contains(name));
        if !self.user_selected
            && self.selected.is_empty()
            && let Some(root) = tree.instances.first()
        {
            self.selected.insert(root.blackboard_name().to_owned());
        }
        if self.selected != before
            && let Some(handle) = handle
        {
            self.resend(handle);
        }
    }

    pub(crate) fn set_values(&mut self, values: Value, now: Instant) {
        self.values = Some(values);
        self.updated_at = Some(now);
        self.updates += 1;
    }
}

/// The type hint of a JSON value: the `"__type"` text of an object.
pub fn type_hint(value: &Value) -> Option<&str> {
    value.get(TYPE_KEY).and_then(Value::as_str)
}

/// The text of a scalar value. Strings are in double quotes, so that the
/// string `"1"` and the number `1` look different.
pub fn scalar_text(value: &Value) -> String {
    match value {
        Value::Null => "null".to_owned(),
        Value::String(text) => format!("\"{text}\""),
        Value::Bool(_) | Value::Number(_) => value.to_string(),
        Value::Array(items) => format!("[{} items]", items.len()),
        Value::Object(map) => {
            let count = map.keys().filter(|key| *key != TYPE_KEY).count();
            format!("{{{count} keys}}")
        }
    }
}

/// True for objects and arrays.
pub fn is_container(value: &Value) -> bool {
    matches!(value, Value::Array(_) | Value::Object(_))
}

/// The children of a container, with their keys: object keys (without the
/// type hint) or array indices as `[0]`, `[1]`, ...
pub fn children(value: &Value) -> Vec<(String, &Value)> {
    match value {
        Value::Object(map) => map
            .iter()
            .filter(|(key, _)| *key != TYPE_KEY)
            .map(|(key, child)| (key.clone(), child))
            .collect(),
        Value::Array(items) => items
            .iter()
            .enumerate()
            .map(|(index, child)| (format!("[{index}]"), child))
            .collect(),
        _ => Vec::new(),
    }
}

/// True if an entry passes the filter: its key or its scalar value
/// contains the filter text, or a child entry passes. `filter` must be in
/// lower case; an empty filter accepts all entries.
pub fn entry_matches(key: &str, value: &Value, filter: &str) -> bool {
    if filter.is_empty() || key.to_lowercase().contains(filter) {
        return true;
    }
    if type_hint(value).is_some_and(|hint| hint.to_lowercase().contains(filter)) {
        return true;
    }
    if is_container(value) {
        children(value)
            .iter()
            .any(|(child_key, child)| entry_matches(child_key, child, filter))
    } else {
        scalar_text(value).to_lowercase().contains(filter)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn text_and_children() {
        assert_eq!(scalar_text(&json!("a")), "\"a\"");
        assert_eq!(scalar_text(&json!(1.5)), "1.5");
        assert_eq!(scalar_text(&json!(true)), "true");
        assert_eq!(scalar_text(&json!(null)), "null");
        assert_eq!(scalar_text(&json!([1, 2])), "[2 items]");
        let pose = json!({"__type": "Pose2D", "x": 1.0, "y": 2.0});
        assert_eq!(scalar_text(&pose), "{2 keys}");
        assert_eq!(type_hint(&pose), Some("Pose2D"));
        let keys: Vec<String> = children(&pose).into_iter().map(|(k, _)| k).collect();
        assert_eq!(keys, ["x", "y"]);
        let keys: Vec<String> = children(&json!(["a", "b"]))
            .into_iter()
            .map(|(k, _)| k)
            .collect();
        assert_eq!(keys, ["[0]", "[1]"]);
        assert!(children(&json!(3)).is_empty());
    }

    #[test]
    fn filter_finds_keys_values_and_types() {
        let board = json!({
            "pose": {"__type": "Pose2D", "x": 1.0, "theta": 0.5},
            "names": ["alpha", "beta"],
            "count": 3
        });
        assert!(entry_matches("board", &board, ""));
        assert!(entry_matches("board", &board, "theta"));
        assert!(entry_matches("board", &board, "beta"));
        assert!(entry_matches("board", &board, "pose2d"));
        assert!(!entry_matches("board", &board, "gamma"));
        assert!(entry_matches("count", &json!(3), "3"));
        assert!(!entry_matches("names", &json!(["alpha"]), "beta"));
    }

    #[test]
    fn watch_selection_and_commands() {
        let tree = RuntimeTree::parse(
            r#"<root><BehaviorTree ID="Main" _fullpath=""><SubTree ID="S" _fullpath="s" _uid="1"/></BehaviorTree>
               <BehaviorTree ID="S" _fullpath="s"><AlwaysSuccess _uid="2"/></BehaviorTree></root>"#,
        )
        .unwrap();
        let mut watch = BlackboardWatch::default();
        watch.tree_changed(&tree, None);
        // The root blackboard is watched by default.
        assert!(watch.is_selected("Main"));
        watch.set_selected("s", true);
        watch.set_selected("Main", false);
        assert_eq!(watch.selected().len(), 1);
        watch.set_interval_ms(1);
        assert_eq!(watch.interval_ms(), MIN_INTERVAL_MS);
        match watch.command() {
            SessionCommand::WatchBlackboards { names, interval } => {
                assert_eq!(names, ["s"]);
                assert_eq!(interval, Duration::from_millis(u64::from(MIN_INTERVAL_MS)));
            }
            other => panic!("{other:?}"),
        }
        watch.set_auto_refresh(false);
        assert!(matches!(
            watch.command(),
            SessionCommand::WatchBlackboards { names, .. } if names.is_empty()
        ));
        // A tree without "s": the name goes, and the user choice stays.
        let other = RuntimeTree::parse(
            r#"<root><BehaviorTree ID="Other"><AlwaysSuccess _uid="1"/></BehaviorTree></root>"#,
        )
        .unwrap();
        watch.tree_changed(&other, None);
        assert!(watch.selected().is_empty());
        watch.set_values(json!({"Other": {"a": 1}}), Instant::now());
        assert_eq!(watch.value_of("Other"), Some(&json!({"a": 1})));
        assert_eq!(watch.updates(), 1);
        assert!(watch.updated_at().is_some());
    }
}
