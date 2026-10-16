//! The project file format (`*.ftproj`, JSON) and the list of recent
//! projects.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

use crate::fsio::normalize;

/// The version of the project file format that this code writes and reads.
pub const PROJECT_FILE_VERSION: u32 = 1;

/// The file name extension of project files (without the dot).
pub const PROJECT_FILE_EXTENSION: &str = "ftproj";

/// The direction of the tree layout on the canvas.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum LayoutDirection {
    /// The root is at the top; children are below their parent.
    #[default]
    TopDown,
    /// The root is at the left; children are to the right of their parent.
    LeftRight,
}

/// The JSON form of a project file (`docs/DESIGN.md` section 2.3):
///
/// ```json
/// {
///   "version": 1,
///   "name": "demo",
///   "files": ["trees/main.xml", "trees/sub.xml"],
///   "model_files": ["models/custom_models.xml"],
///   "layout": {
///     "trees/main.xml#MainTree": {
///       "direction": "top_down",
///       "structure_hash": "0123456789abcdef",
///       "positions": { "0": [0.0, 0.0], "0/1": [120.0, 80.0] }
///     }
///   },
///   "unconnected": [
///     { "file": "trees/main.xml", "tree": "MainTree", "xml": "<Sleep msec=\"10\"/>", "position": [300.0, 40.0] }
///   ]
/// }
/// ```
///
/// Paths are relative to the directory of the project file and use `/`.
/// Unknown fields are ignored on load. All fields except `version` are
/// optional on load.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ProjectFile {
    /// The format version. Only [`PROJECT_FILE_VERSION`] is supported.
    pub version: u32,
    /// The project name.
    #[serde(default)]
    pub name: String,
    /// The tree files.
    #[serde(default)]
    pub files: Vec<String>,
    /// The model files.
    #[serde(default)]
    pub model_files: Vec<String>,
    /// The layout state of each tree, by `"<file>#<tree ID>"`.
    #[serde(default)]
    pub layout: BTreeMap<String, LayoutEntry>,
    /// The unconnected nodes.
    #[serde(default)]
    pub unconnected: Vec<UnconnectedEntry>,
}

impl Default for ProjectFile {
    fn default() -> Self {
        ProjectFile {
            version: PROJECT_FILE_VERSION,
            name: String::new(),
            files: Vec::new(),
            model_files: Vec::new(),
            layout: BTreeMap::new(),
            unconnected: Vec::new(),
        }
    }
}

impl ProjectFile {
    /// Reads a project file from JSON text. The version must be
    /// [`PROJECT_FILE_VERSION`].
    pub fn from_json(text: &str) -> Result<ProjectFile, ProjectFileError> {
        // Read the version first, so that a newer file gives a clear error
        // even if the rest of its schema is different.
        #[derive(Deserialize)]
        struct VersionOnly {
            version: u32,
        }
        let version: VersionOnly = serde_json::from_str(text)?;
        if version.version != PROJECT_FILE_VERSION {
            return Err(ProjectFileError::UnsupportedVersion(version.version));
        }
        Ok(serde_json::from_str(text)?)
    }

    /// Writes the project file as JSON with two-space indentation and a
    /// line end at the end.
    pub fn to_json(&self) -> String {
        // Serialization of these types cannot fail: all map keys are
        // strings and all numbers are finite or written as null.
        let mut text = serde_json::to_string_pretty(self).unwrap_or_default();
        text.push('\n');
        text
    }
}

/// An error in the JSON text of a project file.
#[derive(Debug, thiserror::Error)]
pub enum ProjectFileError {
    /// The text is not valid JSON or does not match the schema.
    #[error("invalid project file: {0}")]
    Json(#[from] serde_json::Error),
    /// The file has a version that this FastTick cannot read.
    #[error(
        "the project file has version {0}; this FastTick reads only version {v}",
        v = PROJECT_FILE_VERSION
    )]
    UnsupportedVersion(u32),
}

/// The JSON form of the layout state of one tree.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct LayoutEntry {
    /// The layout direction.
    #[serde(default)]
    pub direction: LayoutDirection,
    /// The structure hash of the tree when the positions were saved
    /// (see [`crate::bt::structure_hash`]), as 16 hexadecimal digits.
    #[serde(default)]
    pub structure_hash: String,
    /// The node positions by child-index path (`"0"`, `"0/1"`, ...).
    #[serde(default)]
    pub positions: BTreeMap<String, [f32; 2]>,
}

/// The JSON form of one unconnected node.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct UnconnectedEntry {
    /// The tree file, relative to the project file.
    pub file: String,
    /// The ID of the tree whose canvas shows the node.
    pub tree: String,
    /// The XML of the node and its subtree.
    pub xml: String,
    /// The position on the canvas.
    #[serde(default)]
    pub position: [f32; 2],
}

/// The list of recently opened projects: the newest first, without
/// duplicates, at most [`RecentProjects::MAX`] items.
///
/// The JSON form is an array of paths.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(from = "Vec<PathBuf>", into = "Vec<PathBuf>")]
pub struct RecentProjects {
    paths: Vec<PathBuf>,
}

impl RecentProjects {
    /// The largest number of items.
    pub const MAX: usize = 12;

    /// Makes an empty list.
    pub fn new() -> RecentProjects {
        RecentProjects::default()
    }

    /// Puts a path at the top of the list. The path is made absolute and
    /// normalized. An equal path that is in the list already moves to the
    /// top. The oldest items go when the list is too long.
    pub fn add(&mut self, path: impl AsRef<Path>) {
        let path = normalize(path.as_ref());
        self.paths.retain(|p| *p != path);
        self.paths.insert(0, path);
        self.paths.truncate(Self::MAX);
    }

    /// Removes a path. Returns true if it was in the list.
    pub fn remove(&mut self, path: impl AsRef<Path>) -> bool {
        let path = normalize(path.as_ref());
        let before = self.paths.len();
        self.paths.retain(|p| *p != path);
        self.paths.len() != before
    }

    /// Removes the paths of files that do not exist.
    pub fn retain_existing(&mut self) {
        self.paths.retain(|p| p.is_file());
    }

    /// Removes all paths.
    pub fn clear(&mut self) {
        self.paths.clear();
    }

    /// The paths, the newest first.
    pub fn paths(&self) -> &[PathBuf] {
        &self.paths
    }

    /// True if the list is empty.
    pub fn is_empty(&self) -> bool {
        self.paths.is_empty()
    }

    /// The number of paths.
    pub fn len(&self) -> usize {
        self.paths.len()
    }
}

impl From<Vec<PathBuf>> for RecentProjects {
    /// Makes a list from paths, the newest first. Duplicates (after
    /// normalization) and items after [`RecentProjects::MAX`] are dropped.
    fn from(paths: Vec<PathBuf>) -> Self {
        let mut list = RecentProjects::new();
        for path in paths.iter().rev() {
            list.add(path);
        }
        list
    }
}

impl From<RecentProjects> for Vec<PathBuf> {
    fn from(list: RecentProjects) -> Self {
        list.paths
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const EXAMPLE: &str = r#"{
  "version": 1,
  "name": "demo",
  "files": ["trees/main.xml", "trees/sub.xml"],
  "model_files": ["models/custom_models.xml"],
  "layout": {
    "trees/main.xml#MainTree": {
      "direction": "top_down",
      "structure_hash": "0123456789abcdef",
      "positions": { "0": [0.0, 0.0], "0/1": [120.0, 80.0] }
    }
  },
  "unconnected": [
    { "file": "trees/main.xml", "tree": "MainTree", "xml": "<Sleep msec=\"10\"/>", "position": [300.0, 40.0] }
  ],
  "editor_settings": { "unknown": true }
}"#;

    #[test]
    fn design_example() {
        let file = ProjectFile::from_json(EXAMPLE).unwrap();
        assert_eq!(file.version, 1);
        assert_eq!(file.name, "demo");
        assert_eq!(file.files, vec!["trees/main.xml", "trees/sub.xml"]);
        assert_eq!(file.model_files, vec!["models/custom_models.xml"]);
        let layout = &file.layout["trees/main.xml#MainTree"];
        assert_eq!(layout.direction, LayoutDirection::TopDown);
        assert_eq!(layout.structure_hash, "0123456789abcdef");
        assert_eq!(layout.positions["0/1"], [120.0, 80.0]);
        assert_eq!(file.unconnected[0].xml, "<Sleep msec=\"10\"/>");
        assert_eq!(file.unconnected[0].position, [300.0, 40.0]);

        // Round trip.
        let json = file.to_json();
        assert!(json.ends_with("}\n"));
        assert!(!json.contains("editor_settings"));
        assert_eq!(ProjectFile::from_json(&json).unwrap(), file);
        assert!(json.contains("\"direction\": \"top_down\""));
    }

    #[test]
    fn minimal_and_errors() {
        let file = ProjectFile::from_json(r#"{"version": 1}"#).unwrap();
        assert_eq!(file, ProjectFile::default());
        let file = ProjectFile::from_json(
            r#"{"version": 1, "layout": {"a#b": {"direction": "left_right"}}}"#,
        )
        .unwrap();
        assert_eq!(file.layout["a#b"].direction, LayoutDirection::LeftRight);
        assert!(file.layout["a#b"].positions.is_empty());

        assert!(matches!(
            ProjectFile::from_json(r#"{"version": 2, "files": 5}"#),
            Err(ProjectFileError::UnsupportedVersion(2))
        ));
        let err = ProjectFile::from_json(r#"{"version": 2}"#).unwrap_err();
        assert!(err.to_string().contains("version 2"), "{err}");
        assert!(matches!(
            ProjectFile::from_json(r#"{"name": "x"}"#),
            Err(ProjectFileError::Json(_))
        ));
        assert!(matches!(
            ProjectFile::from_json(r#"{"version": 1, "files": "x"}"#),
            Err(ProjectFileError::Json(_))
        ));
        assert!(matches!(
            ProjectFile::from_json("not json"),
            Err(ProjectFileError::Json(_))
        ));
        assert!(matches!(
            ProjectFile::from_json(
                r#"{"version": 1, "layout": {"a#b": {"direction": "diagonal"}}}"#
            ),
            Err(ProjectFileError::Json(_))
        ));
    }

    // The paths are Unix paths (on Windows, they get a drive letter).
    #[cfg(unix)]
    #[test]
    fn recent_projects() {
        let mut recent = RecentProjects::new();
        assert!(recent.is_empty());
        for i in 0..20 {
            recent.add(format!("/p/{i}.ftproj"));
        }
        assert_eq!(recent.len(), RecentProjects::MAX);
        assert_eq!(recent.paths()[0], PathBuf::from("/p/19.ftproj"));
        assert_eq!(recent.paths()[11], PathBuf::from("/p/8.ftproj"));
        // Dedupe: an existing path moves to the top.
        recent.add("/p/x/../10.ftproj");
        assert_eq!(recent.len(), RecentProjects::MAX);
        assert_eq!(recent.paths()[0], PathBuf::from("/p/10.ftproj"));
        assert_eq!(
            recent
                .paths()
                .iter()
                .filter(|p| p.ends_with("10.ftproj"))
                .count(),
            1
        );
        assert!(recent.remove("/p/10.ftproj"));
        assert!(!recent.remove("/p/10.ftproj"));
        assert_eq!(recent.len(), 11);

        let json = serde_json::to_string(&recent).unwrap();
        assert!(json.starts_with("[\"/p/19.ftproj\""));
        let back: RecentProjects = serde_json::from_str(&json).unwrap();
        assert_eq!(back, recent);

        // Loading cleans the list.
        let back: RecentProjects =
            serde_json::from_str(r#"["/a.ftproj", "/b.ftproj", "/a.ftproj"]"#).unwrap();
        assert_eq!(
            back.paths(),
            &[PathBuf::from("/a.ftproj"), PathBuf::from("/b.ftproj")]
        );

        recent.retain_existing();
        assert!(recent.is_empty());
        recent.add("/x");
        recent.clear();
        assert!(recent.is_empty());
    }
}
