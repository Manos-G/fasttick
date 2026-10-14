//! Breakpoint files (`.ftbp.json`).
//!
//! A [`BreakpointSet`] stores breakpoints by node path and by UID. The node
//! path is stable when the tree changes a little, but the UIDs of BT.CPP
//! change when nodes are added or removed. [`BreakpointSet::resolve`]
//! therefore uses the path first and the stored UID only as a fallback.
//!
//! The file is pretty-printed JSON:
//!
//! ```json
//! {
//!   "version": 1,
//!   "breakpoints": [
//!     {
//!       "node_path": "sub/fb",
//!       "uid": 5,
//!       "position": "pre",
//!       "mode": "breakpoint",
//!       "enabled": true,
//!       "once": false,
//!       "desired_status": "SKIPPED",
//!       "label": null
//!     }
//!   ]
//! }
//! ```

use std::path::Path;

use fasttick_core::runtime::{NodeIndex, RuntimeTree, Uid};
use fasttick_core::status::NodeStatus;
use serde::{Deserialize, Serialize};

use crate::messages::{Hook, HookMode, HookPosition};

/// The file extension of a breakpoint file, without the leading dot.
pub const FILE_EXTENSION: &str = "ftbp.json";

/// The file format version that this crate writes and reads.
pub const FORMAT_VERSION: u32 = 1;

/// An error when a breakpoint file is read or written.
#[derive(Debug, thiserror::Error)]
pub enum BreakpointFileError {
    /// The file cannot be read or written.
    #[error("I/O error: {0}")]
    Io(#[from] std::io::Error),
    /// The file is not valid JSON or does not have the expected fields.
    #[error("invalid breakpoint file: {0}")]
    Json(#[from] serde_json::Error),
    /// The file has a format version that this crate does not know.
    #[error("unsupported breakpoint file version {0} (expected {FORMAT_VERSION})")]
    UnsupportedVersion(u32),
}

/// One breakpoint or replace hook, as stored in a breakpoint file.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct Breakpoint {
    /// The full path of the node (`RuntimeTree::node_path`), for example
    /// `"sub_a/fb"`.
    pub node_path: String,
    /// The UID of the node when the breakpoint was saved.
    #[serde(default)]
    pub uid: Option<u16>,
    /// Before or after the tick. The file uses `"pre"` or `"post"`.
    #[serde(with = "position_name", default)]
    pub position: HookPosition,
    /// Breakpoint or replace. The file uses `"breakpoint"` or `"replace"`.
    #[serde(with = "mode_name", default)]
    pub mode: HookMode,
    /// False to keep the breakpoint but not install it as active.
    #[serde(default = "default_true")]
    pub enabled: bool,
    /// Remove the hook after it runs once.
    #[serde(default)]
    pub once: bool,
    /// The status that the node returns in replace mode.
    #[serde(default = "default_status")]
    pub desired_status: NodeStatus,
    /// A text that the user gave to the breakpoint.
    #[serde(default)]
    pub label: Option<String>,
}

fn default_true() -> bool {
    true
}

fn default_status() -> NodeStatus {
    NodeStatus::Skipped
}

impl Breakpoint {
    /// A breakpoint before the tick of the node at `index` in `tree`.
    pub fn at_node(tree: &RuntimeTree, index: NodeIndex) -> Breakpoint {
        Breakpoint {
            node_path: tree.node_path(index),
            uid: tree.node(index).uid.map(|uid| uid.0),
            position: HookPosition::Pre,
            mode: HookMode::Breakpoint,
            enabled: true,
            once: false,
            desired_status: NodeStatus::Skipped,
            label: None,
        }
    }

    /// A breakpoint from a hook. The node path comes from `tree`. It is
    /// empty if the tree has no node with the UID of the hook.
    pub fn from_hook(hook: &Hook, tree: &RuntimeTree) -> Breakpoint {
        let node_path = tree
            .index_of_uid(Uid(hook.uid))
            .map(|index| tree.node_path(index))
            .unwrap_or_default();
        Breakpoint {
            node_path,
            uid: Some(hook.uid),
            position: hook.position,
            mode: hook.mode,
            enabled: hook.enabled,
            once: hook.once,
            desired_status: hook.desired_status,
            label: None,
        }
    }

    /// The hook for this breakpoint on the node `uid`.
    pub fn to_hook(&self, uid: Uid) -> Hook {
        Hook {
            desired_status: self.desired_status,
            enabled: self.enabled,
            mode: self.mode,
            once: self.once,
            position: self.position,
            uid: uid.0,
        }
    }
}

/// How [`BreakpointSet::resolve`] found the node of a breakpoint.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Resolution {
    /// Exactly one node has the stored path.
    ByPath,
    /// More than one node has the stored path. The node with the stored UID
    /// was taken if it is one of them, else the first one in document order.
    AmbiguousPath,
    /// No node has the stored path (or the path is empty). The node with
    /// the stored UID was taken.
    ByStoredUid,
    /// No node was found.
    Unresolved,
}

/// The result of [`BreakpointSet::resolve`] for one breakpoint.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct ResolvedBreakpoint {
    /// The index of the breakpoint in [`BreakpointSet::breakpoints`].
    pub index: usize,
    /// The UID of the node in the current tree.
    pub uid: Option<Uid>,
    /// How the node was found.
    pub resolution: Resolution,
}

/// The content of a breakpoint file.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct BreakpointSet {
    /// The format version, [`FORMAT_VERSION`].
    pub version: u32,
    /// The breakpoints.
    pub breakpoints: Vec<Breakpoint>,
}

impl Default for BreakpointSet {
    fn default() -> Self {
        BreakpointSet::new()
    }
}

impl BreakpointSet {
    /// An empty set with the current format version.
    pub fn new() -> BreakpointSet {
        BreakpointSet {
            version: FORMAT_VERSION,
            breakpoints: Vec::new(),
        }
    }

    /// The pretty-printed JSON text of the set.
    pub fn to_json(&self) -> Result<String, BreakpointFileError> {
        let mut text = serde_json::to_string_pretty(self)?;
        text.push('\n');
        Ok(text)
    }

    /// Reads a set from JSON text. The version must be [`FORMAT_VERSION`].
    pub fn from_json(text: &str) -> Result<BreakpointSet, BreakpointFileError> {
        #[derive(Deserialize)]
        struct VersionOnly {
            version: u32,
        }
        let version: VersionOnly = serde_json::from_str(text)?;
        if version.version != FORMAT_VERSION {
            return Err(BreakpointFileError::UnsupportedVersion(version.version));
        }
        Ok(serde_json::from_str(text)?)
    }

    /// Writes the set to a file. The function writes a temporary file next
    /// to `path` and then renames it, so a failure does not leave a partial
    /// file.
    pub fn save(&self, path: impl AsRef<Path>) -> Result<(), BreakpointFileError> {
        let path = path.as_ref();
        let text = self.to_json()?;
        let mut temporary = path.as_os_str().to_owned();
        temporary.push(".tmp");
        std::fs::write(&temporary, text)?;
        if let Err(err) = std::fs::rename(&temporary, path) {
            let _ = std::fs::remove_file(&temporary);
            return Err(err.into());
        }
        Ok(())
    }

    /// Reads a set from a file.
    pub fn load(path: impl AsRef<Path>) -> Result<BreakpointSet, BreakpointFileError> {
        let text = std::fs::read_to_string(path)?;
        BreakpointSet::from_json(&text)
    }

    /// Finds the node of each breakpoint in `tree`.
    ///
    /// The function looks for the node path first. It uses the stored UID
    /// only when no node has the path (or the path is empty), and only if
    /// the tree has a node with that UID. The result has one entry for each
    /// breakpoint, in the same order.
    pub fn resolve(&self, tree: &RuntimeTree) -> Vec<ResolvedBreakpoint> {
        let mut by_path: std::collections::HashMap<String, Vec<NodeIndex>> =
            std::collections::HashMap::new();
        for index in (0..tree.len()).map(NodeIndex) {
            by_path
                .entry(tree.node_path(index))
                .or_default()
                .push(index);
        }
        self.breakpoints
            .iter()
            .enumerate()
            .map(|(index, breakpoint)| {
                let stored = breakpoint
                    .uid
                    .map(Uid)
                    .filter(|uid| tree.node_by_uid(*uid).is_some());
                let candidates = if breakpoint.node_path.is_empty() {
                    None
                } else {
                    by_path.get(&breakpoint.node_path)
                };
                let (uid, resolution) = match candidates.map(Vec::as_slice) {
                    Some([single]) => (tree.node(*single).uid, Resolution::ByPath),
                    Some(several @ [first, ..]) => {
                        let matching = several
                            .iter()
                            .find(|candidate| {
                                stored.is_some() && tree.node(**candidate).uid == stored
                            })
                            .unwrap_or(first);
                        (tree.node(*matching).uid, Resolution::AmbiguousPath)
                    }
                    _ => match stored {
                        Some(uid) => (Some(uid), Resolution::ByStoredUid),
                        None => (None, Resolution::Unresolved),
                    },
                };
                let resolution = if uid.is_none() {
                    Resolution::Unresolved
                } else {
                    resolution
                };
                ResolvedBreakpoint {
                    index,
                    uid,
                    resolution,
                }
            })
            .collect()
    }

    /// The hooks for all resolved breakpoints in `tree`.
    ///
    /// This includes the breakpoints found only by the stored UID
    /// ([`Resolution::ByStoredUid`]). If the tree changed since the file was
    /// saved, such a UID can belong to another node. To install only the
    /// breakpoints found by path, filter the result of
    /// [`BreakpointSet::resolve`] and use [`Breakpoint::to_hook`].
    pub fn hooks(&self, tree: &RuntimeTree) -> Vec<Hook> {
        self.resolve(tree)
            .into_iter()
            .filter_map(|resolved| {
                let uid = resolved.uid?;
                let breakpoint = self.breakpoints.get(resolved.index)?;
                Some(breakpoint.to_hook(uid))
            })
            .collect()
    }
}

/// Serde for [`HookPosition`] as a name (`"pre"`, `"post"`).
mod position_name {
    use serde::{Deserialize, Deserializer, Serializer};

    use crate::messages::HookPosition;

    pub fn serialize<S: Serializer>(
        value: &HookPosition,
        serializer: S,
    ) -> Result<S::Ok, S::Error> {
        serializer.serialize_str(match value {
            HookPosition::Pre => "pre",
            HookPosition::Post => "post",
        })
    }

    pub fn deserialize<'de, D: Deserializer<'de>>(
        deserializer: D,
    ) -> Result<HookPosition, D::Error> {
        let text = String::deserialize(deserializer)?;
        match text.as_str() {
            "pre" => Ok(HookPosition::Pre),
            "post" => Ok(HookPosition::Post),
            other => Err(serde::de::Error::custom(format_args!(
                "unknown position {other:?} (expected \"pre\" or \"post\")"
            ))),
        }
    }
}

/// Serde for [`HookMode`] as a name (`"breakpoint"`, `"replace"`).
mod mode_name {
    use serde::{Deserialize, Deserializer, Serializer};

    use crate::messages::HookMode;

    pub fn serialize<S: Serializer>(value: &HookMode, serializer: S) -> Result<S::Ok, S::Error> {
        serializer.serialize_str(match value {
            HookMode::Breakpoint => "breakpoint",
            HookMode::Replace => "replace",
        })
    }

    pub fn deserialize<'de, D: Deserializer<'de>>(deserializer: D) -> Result<HookMode, D::Error> {
        let text = String::deserialize(deserializer)?;
        match text.as_str() {
            "breakpoint" => Ok(HookMode::Breakpoint),
            "replace" => Ok(HookMode::Replace),
            other => Err(serde::de::Error::custom(format_args!(
                "unknown mode {other:?} (expected \"breakpoint\" or \"replace\")"
            ))),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const TREE: &str = r#"<root BTCPP_format="4">
    <BehaviorTree ID="MainTree" _fullpath="">
        <Sequence name="main" _uid="1">
            <AlwaysSuccess name="ok" _uid="2"/>
            <SubTree ID="Sub" _fullpath="sub" _uid="3"/>
            <AlwaysFailure name="twin" _uid="4"/>
            <AlwaysFailure name="twin" _uid="5"/>
        </Sequence>
    </BehaviorTree>
    <BehaviorTree ID="Sub" _fullpath="sub">
        <Fallback name="fb" _uid="6">
            <AlwaysSuccess name="leaf" _uid="7"/>
        </Fallback>
    </BehaviorTree>
</root>"#;

    fn tree() -> RuntimeTree {
        RuntimeTree::parse(TREE).unwrap()
    }

    fn breakpoint(path: &str, uid: Option<u16>) -> Breakpoint {
        Breakpoint {
            node_path: path.to_owned(),
            uid,
            position: HookPosition::Pre,
            mode: HookMode::Breakpoint,
            enabled: true,
            once: false,
            desired_status: NodeStatus::Skipped,
            label: None,
        }
    }

    #[test]
    fn json_text_is_stable() {
        let mut set = BreakpointSet::new();
        set.breakpoints.push(Breakpoint {
            label: Some("check".to_owned()),
            ..breakpoint("sub/fb", Some(6))
        });
        set.breakpoints.push(Breakpoint {
            position: HookPosition::Post,
            mode: HookMode::Replace,
            enabled: false,
            once: true,
            desired_status: NodeStatus::Failure,
            ..breakpoint("ok", None)
        });
        let expected = r#"{
  "version": 1,
  "breakpoints": [
    {
      "node_path": "sub/fb",
      "uid": 6,
      "position": "pre",
      "mode": "breakpoint",
      "enabled": true,
      "once": false,
      "desired_status": "SKIPPED",
      "label": "check"
    },
    {
      "node_path": "ok",
      "uid": null,
      "position": "post",
      "mode": "replace",
      "enabled": false,
      "once": true,
      "desired_status": "FAILURE",
      "label": null
    }
  ]
}
"#;
        assert_eq!(set.to_json().unwrap(), expected);
        assert_eq!(BreakpointSet::from_json(expected).unwrap(), set);
    }

    #[test]
    fn defaults_for_missing_fields() {
        let set = BreakpointSet::from_json(r#"{"version":1,"breakpoints":[{"node_path":"ok"}]}"#)
            .unwrap();
        assert_eq!(set.breakpoints, vec![breakpoint("ok", None)]);
    }

    #[test]
    fn file_errors() {
        assert!(matches!(
            BreakpointSet::from_json(r#"{"version":2,"breakpoints":[]}"#),
            Err(BreakpointFileError::UnsupportedVersion(2))
        ));
        for bad in [
            "",
            "{",
            "[]",
            r#"{"breakpoints":[]}"#,
            r#"{"version":1}"#,
            r#"{"version":1,"breakpoints":[{"uid":3}]}"#,
            r#"{"version":1,"breakpoints":[{"node_path":"a","position":"middle"}]}"#,
            r#"{"version":1,"breakpoints":[{"node_path":"a","mode":1}]}"#,
            r#"{"version":1,"breakpoints":[{"node_path":"a","desired_status":"DONE"}]}"#,
            r#"{"version":1,"breakpoints":[{"node_path":"a","uid":70000}]}"#,
            r#"{"version":"1","breakpoints":[]}"#,
        ] {
            assert!(
                matches!(
                    BreakpointSet::from_json(bad),
                    Err(BreakpointFileError::Json(_))
                ),
                "{bad}"
            );
        }
    }

    #[test]
    fn save_and_load() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join(format!("set.{FILE_EXTENSION}"));
        let mut set = BreakpointSet::new();
        set.breakpoints.push(breakpoint("sub/fb/leaf", Some(7)));
        set.save(&path).unwrap();
        assert_eq!(BreakpointSet::load(&path).unwrap(), set);
        assert!(
            !dir.path()
                .join(format!("set.{FILE_EXTENSION}.tmp"))
                .exists()
        );
        assert!(matches!(
            BreakpointSet::load(dir.path().join("missing.ftbp.json")),
            Err(BreakpointFileError::Io(_))
        ));
        std::fs::write(&path, b"\xff\xfe").unwrap();
        assert!(BreakpointSet::load(&path).is_err());
        // A directory that does not exist.
        assert!(matches!(
            set.save(dir.path().join("no/such/dir/x.ftbp.json")),
            Err(BreakpointFileError::Io(_))
        ));
    }

    #[test]
    fn resolve_by_path_first() {
        let tree = tree();
        let mut set = BreakpointSet::new();
        set.breakpoints = vec![
            // Path found; the stored UID is out of date.
            breakpoint("sub/fb", Some(2)),
            // Path missing; the stored UID exists.
            breakpoint("old/name", Some(7)),
            // Path missing; the stored UID does not exist.
            breakpoint("old/name", Some(99)),
            // Path missing and no UID.
            breakpoint("gone", None),
            // Empty path; the stored UID is used.
            breakpoint("", Some(1)),
            // A SubTree node: its path is the instance path.
            breakpoint("sub", None),
        ];
        let resolved = set.resolve(&tree);
        let summary: Vec<(Option<u16>, Resolution)> = resolved
            .iter()
            .map(|item| (item.uid.map(|uid| uid.0), item.resolution))
            .collect();
        assert_eq!(
            summary,
            vec![
                (Some(6), Resolution::ByPath),
                (Some(7), Resolution::ByStoredUid),
                (None, Resolution::Unresolved),
                (None, Resolution::Unresolved),
                (Some(1), Resolution::ByStoredUid),
                (Some(3), Resolution::ByPath),
            ]
        );
        assert_eq!(
            resolved.iter().map(|item| item.index).collect::<Vec<_>>(),
            vec![0, 1, 2, 3, 4, 5]
        );
    }

    #[test]
    fn resolve_ambiguous_path() {
        let tree = tree();
        let mut set = BreakpointSet::new();
        set.breakpoints = vec![
            breakpoint("twin", Some(5)),
            breakpoint("twin", Some(99)),
            breakpoint("twin", None),
        ];
        let resolved = set.resolve(&tree);
        assert_eq!(resolved[0].uid, Some(Uid(5)));
        assert_eq!(resolved[0].resolution, Resolution::AmbiguousPath);
        assert_eq!(resolved[1].uid, Some(Uid(4)));
        assert_eq!(resolved[2].uid, Some(Uid(4)));
    }

    #[test]
    fn hooks_from_set() {
        let tree = tree();
        let mut set = BreakpointSet::new();
        set.breakpoints = vec![
            Breakpoint {
                mode: HookMode::Replace,
                desired_status: NodeStatus::Success,
                ..breakpoint("ok", Some(40))
            },
            breakpoint("gone", None),
        ];
        assert_eq!(
            set.hooks(&tree),
            vec![Hook::replace(2, NodeStatus::Success)]
        );
    }

    #[test]
    fn from_and_to_hook() {
        let tree = tree();
        let hook = Hook::replace(7, NodeStatus::Failure);
        let converted = Breakpoint::from_hook(&hook, &tree);
        assert_eq!(converted.node_path, "sub/leaf");
        assert_eq!(converted.to_hook(Uid(7)), hook);
        let unknown = Breakpoint::from_hook(&Hook::breakpoint(500), &tree);
        assert_eq!(unknown.node_path, "");
        let at = Breakpoint::at_node(&tree, tree.index_of_uid(Uid(6)).unwrap());
        assert_eq!(at, breakpoint("sub/fb", Some(6)));
    }
}
