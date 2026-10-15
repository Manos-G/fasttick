//! User settings, saved with the eframe storage.
//!
//! The settings are one serde struct under the key [`SETTINGS_KEY`]. Unknown
//! or missing fields take their default values, so old settings files
//! stay readable.

use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

use crate::canvas::{EdgeStyle, LayoutDirection};
use crate::theme::ThemeChoice;

/// The storage key of the settings.
pub const SETTINGS_KEY: &str = "fasttick.settings";
/// The most entries in the recent-files list.
pub const MAX_RECENT: usize = 12;
/// The default port of the BT.CPP publisher.
pub const DEFAULT_PORT: u16 = 1667;

/// The kind of a recent file.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum RecentKind {
    /// A tree XML file.
    Tree,
    /// A FastTick project.
    Project,
    /// A `.btlog` file.
    Log,
}

impl RecentKind {
    /// The kind for a path, from its extension.
    pub fn for_path(path: &Path) -> RecentKind {
        match path
            .extension()
            .and_then(|e| e.to_str())
            .map(str::to_ascii_lowercase)
            .as_deref()
        {
            Some("btlog" | "db3" | "btdb" | "sqlite" | "sqlite3" | "db") => RecentKind::Log,
            Some("ftproj") => RecentKind::Project,
            _ => RecentKind::Tree,
        }
    }
}

/// One entry of the recent-files list.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RecentFile {
    /// The file path.
    pub path: PathBuf,
    /// The kind.
    pub kind: RecentKind,
}

/// All user settings.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Settings {
    /// The theme.
    pub theme: ThemeChoice,
    /// Recently opened files, newest first.
    pub recent: Vec<RecentFile>,
    /// The last host for live connections.
    pub live_host: String,
    /// The last port for live connections.
    pub live_port: u16,
    /// The status poll interval, in milliseconds.
    pub poll_interval_ms: u32,
    /// The layout direction of new views.
    pub layout_direction: LayoutDirection,
    /// The edge style.
    pub edge_style: EdgeStyle,
    /// Show the node that reached a breakpoint.
    pub auto_focus_on_breakpoint: bool,
    /// Show the minimap when the tree does not fit the view.
    pub show_minimap: bool,
}

impl Default for Settings {
    fn default() -> Self {
        Settings {
            theme: ThemeChoice::System,
            recent: Vec::new(),
            live_host: "localhost".to_owned(),
            live_port: DEFAULT_PORT,
            poll_interval_ms: 100,
            layout_direction: LayoutDirection::TopDown,
            edge_style: EdgeStyle::Orthogonal,
            auto_focus_on_breakpoint: true,
            show_minimap: true,
        }
    }
}

/// The smallest poll interval (the session accepts 10 ms and more).
pub const MIN_POLL_MS: u32 = 10;
/// The largest poll interval. The session also sends a heartbeat each
/// second, so a slow poll does not disconnect.
pub const MAX_POLL_MS: u32 = 5000;

impl Settings {
    /// Loads the settings, or the defaults if there are none.
    pub fn load(storage: Option<&dyn eframe::Storage>) -> Settings {
        let mut settings: Settings = storage
            .and_then(|s| eframe::get_value(s, SETTINGS_KEY))
            .unwrap_or_default();
        settings.sanitize();
        settings
    }

    /// Saves the settings.
    pub fn save(&self, storage: &mut dyn eframe::Storage) {
        eframe::set_value(storage, SETTINGS_KEY, self);
    }

    /// Puts values that are out of range back into range.
    pub fn sanitize(&mut self) {
        self.poll_interval_ms = self.poll_interval_ms.clamp(MIN_POLL_MS, MAX_POLL_MS);
        if self.live_host.trim().is_empty() {
            self.live_host = "localhost".to_owned();
        }
        if self.live_port == 0 {
            self.live_port = DEFAULT_PORT;
        }
        self.recent.truncate(MAX_RECENT);
    }

    /// Adds a file at the top of the recent-files list.
    pub fn add_recent(&mut self, path: &Path) {
        let kind = RecentKind::for_path(path);
        self.recent.retain(|entry| entry.path != path);
        self.recent.insert(
            0,
            RecentFile {
                path: path.to_owned(),
                kind,
            },
        );
        self.recent.truncate(MAX_RECENT);
    }

    /// Removes a file from the recent-files list.
    pub fn remove_recent(&mut self, path: &Path) {
        self.recent.retain(|entry| entry.path != path);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashMap;

    #[derive(Default)]
    struct MemoryStorage(HashMap<String, String>);

    impl eframe::Storage for MemoryStorage {
        fn get_string(&self, key: &str) -> Option<String> {
            self.0.get(key).cloned()
        }
        fn set_string(&mut self, key: &str, value: String) {
            self.0.insert(key.to_owned(), value);
        }
        fn remove_string(&mut self, key: &str) {
            self.0.remove(key);
        }
        fn flush(&mut self) {}
    }

    #[test]
    fn round_trip_through_storage() {
        let mut storage = MemoryStorage::default();
        let mut settings = Settings {
            theme: ThemeChoice::Dark,
            live_port: 2000,
            layout_direction: LayoutDirection::LeftRight,
            ..Settings::default()
        };
        settings.add_recent(Path::new("/tmp/a.xml"));
        settings.save(&mut storage);
        let loaded = Settings::load(Some(&storage));
        assert_eq!(loaded, settings);
        assert_eq!(Settings::load(None), Settings::default());
    }

    #[test]
    fn bad_or_partial_values() {
        let mut storage = MemoryStorage::default();
        // A partial settings record (for example from an older version).
        storage.0.insert(
            SETTINGS_KEY.to_owned(),
            "(theme: Light, poll_interval_ms: 0)".to_owned(),
        );
        let loaded = Settings::load(Some(&storage));
        assert_eq!(loaded.theme, ThemeChoice::Light);
        assert_eq!(loaded.poll_interval_ms, MIN_POLL_MS);
        assert_eq!(loaded.live_port, DEFAULT_PORT);
        // Garbage gives the defaults.
        storage
            .0
            .insert(SETTINGS_KEY.to_owned(), "not ron".to_owned());
        assert_eq!(Settings::load(Some(&storage)), Settings::default());
    }

    #[test]
    fn recent_files() {
        let mut settings = Settings::default();
        for i in 0..20 {
            settings.add_recent(Path::new(&format!("/f{i}.xml")));
        }
        assert_eq!(settings.recent.len(), MAX_RECENT);
        assert_eq!(settings.recent[0].path, Path::new("/f19.xml"));
        settings.add_recent(Path::new("/f10.xml"));
        assert_eq!(settings.recent[0].path, Path::new("/f10.xml"));
        assert_eq!(
            settings
                .recent
                .iter()
                .filter(|r| r.path == Path::new("/f10.xml"))
                .count(),
            1
        );
        settings.add_recent(Path::new("/x/run.BTLOG"));
        assert_eq!(settings.recent[0].kind, RecentKind::Log);
        settings.remove_recent(Path::new("/x/run.BTLOG"));
        assert_ne!(settings.recent[0].kind, RecentKind::Log);
        assert_eq!(
            RecentKind::for_path(Path::new("p.ftproj")),
            RecentKind::Project
        );
        assert_eq!(RecentKind::for_path(Path::new("noext")), RecentKind::Tree);
    }
}
