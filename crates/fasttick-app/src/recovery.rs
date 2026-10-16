//! Copies of unsaved changes, so that a crash or a power cut does not lose
//! them.
//!
//! Each running FastTick has its own folder in the recovery folder, with a
//! lock file that it keeps locked while it runs. Every [`INTERVAL`],
//! FastTick writes one record (a JSON file) for each document with unsaved
//! changes. It deletes the record when the document has no unsaved changes
//! any more or closes, and the whole folder when FastTick closes normally.
//!
//! At start, a folder whose lock file is not locked is from a FastTick that
//! stopped without a normal close (an [`Orphan`]): the user can restore its
//! records. The lock of the orphan stays with the new FastTick until the
//! user decides, so two FastTick windows do not restore the same changes.

use std::collections::HashMap;
use std::fs::{self, File};
use std::io;
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use serde::{Deserialize, Serialize};

use crate::documents::DocId;

/// The time between two copies of a document that changes.
pub const INTERVAL: Duration = Duration::from_secs(30);

/// The environment variable that sets the recovery folder.
pub const ENV_DIR: &str = "FASTTICK_RECOVERY_DIR";

const LOCK_FILE: &str = "lock";
const RECORD_EXTENSION: &str = "json";
const VERSION: u32 = 1;

/// The unsaved state of one document.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Record {
    /// The format version.
    pub version: u32,
    /// The tab title.
    pub title: String,
    /// When FastTick wrote the record (seconds after 1970-01-01 UTC).
    pub saved_at: u64,
    /// The main XML file, `None` for a new file.
    pub main: Option<PathBuf>,
    /// The project file, if the document is a project.
    pub project: Option<PathBuf>,
    /// The XML files with unsaved changes.
    pub files: Vec<RecordFile>,
}

/// The unsaved text of one XML file.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RecordFile {
    /// The file, `None` for a new file.
    pub path: Option<PathBuf>,
    /// The XML text.
    pub text: String,
}

impl Record {
    /// A record with the time of now.
    pub fn new(
        title: impl Into<String>,
        main: Option<PathBuf>,
        project: Option<PathBuf>,
        files: Vec<(Option<PathBuf>, String)>,
    ) -> Record {
        Record {
            version: VERSION,
            title: title.into(),
            saved_at: now_seconds(),
            main,
            project,
            files: files
                .into_iter()
                .map(|(path, text)| RecordFile { path, text })
                .collect(),
        }
    }

    /// The paths and texts of the files, for
    /// [`crate::editor::EditorDoc::restore_files`].
    pub fn texts(&self) -> Vec<(Option<PathBuf>, String)> {
        self.files
            .iter()
            .map(|f| (f.path.clone(), f.text.clone()))
            .collect()
    }

    /// How long ago FastTick wrote the record, as text ("5 minutes ago").
    pub fn age_text(&self) -> String {
        age_text(now_seconds().saturating_sub(self.saved_at))
    }
}

/// The records of a FastTick that stopped without a normal close.
#[derive(Debug)]
pub struct Orphan {
    dir: PathBuf,
    lock: Option<File>,
    /// The records, the oldest first.
    pub records: Vec<Record>,
}

impl Orphan {
    /// Deletes the folder of the records.
    pub fn delete(mut self) {
        // Windows cannot delete a file that is open.
        drop(self.lock.take());
        let _ = fs::remove_dir_all(&self.dir);
    }
}

/// The recovery folder of this FastTick. See the [module
/// documentation](self).
#[derive(Debug)]
pub struct Recovery {
    root: PathBuf,
    dir: PathBuf,
    lock: Option<File>,
    written: HashMap<DocId, u64>,
    last_check: Option<Instant>,
}

impl Recovery {
    /// The recovery folder: [`ENV_DIR`] if it is set, else the folder
    /// `recovery` in the FastTick data folder.
    pub fn default_root() -> Option<PathBuf> {
        if let Some(dir) = std::env::var_os(ENV_DIR).filter(|d| !d.is_empty()) {
            return Some(PathBuf::from(dir));
        }
        eframe::storage_dir("fasttick").map(|dir| dir.join("recovery"))
    }

    /// Makes the folder of this FastTick in `root` and locks it.
    pub fn start(root: &Path) -> io::Result<Recovery> {
        let nanos = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map(|d| d.as_nanos())
            .unwrap_or_default();
        let dir = root.join(format!("{}-{nanos:x}", std::process::id()));
        fs::create_dir_all(&dir)?;
        let lock = File::create(dir.join(LOCK_FILE))?;
        lock.try_lock()?;
        Ok(Recovery {
            root: root.to_path_buf(),
            dir,
            lock: Some(lock),
            written: HashMap::new(),
            last_check: None,
        })
    }

    /// The folder of this FastTick.
    pub fn dir(&self) -> &Path {
        &self.dir
    }

    /// The records of the FastTick windows that stopped without a normal
    /// close. Folders without records are deleted.
    pub fn orphans(&self) -> Vec<Orphan> {
        let Ok(entries) = fs::read_dir(&self.root) else {
            return Vec::new();
        };
        let mut dirs: Vec<PathBuf> = entries
            .flatten()
            .map(|e| e.path())
            .filter(|p| p.is_dir() && *p != self.dir)
            .collect();
        dirs.sort();
        let mut out = Vec::new();
        for dir in dirs {
            let lock = match File::options()
                .read(true)
                .write(true)
                .open(dir.join(LOCK_FILE))
            {
                Ok(file) => match file.try_lock() {
                    Ok(()) => Some(file),
                    // That FastTick still runs.
                    Err(_) => continue,
                },
                Err(_) => None,
            };
            let records = read_records(&dir);
            let orphan = Orphan { dir, lock, records };
            if orphan.records.is_empty() {
                orphan.delete();
            } else {
                out.push(orphan);
            }
        }
        out
    }

    /// True if it is time to write the copies again.
    pub fn due(&self) -> bool {
        self.last_check.is_none_or(|t| t.elapsed() >= INTERVAL)
    }

    /// Records the time of a check, for [`Recovery::due`].
    pub fn checked(&mut self) {
        self.last_check = Some(Instant::now());
    }

    /// Makes [`Recovery::due`] true, so the next check writes at once.
    pub fn reset_due(&mut self) {
        self.last_check = None;
    }

    /// True if the record of a document is older than `revision`.
    pub fn needs_write(&self, doc: DocId, revision: u64) -> bool {
        self.written.get(&doc) != Some(&revision)
    }

    /// The documents that have a record.
    pub fn recorded(&self) -> Vec<DocId> {
        self.written.keys().copied().collect()
    }

    /// Writes the record of a document at a revision of its editor.
    pub fn write(&mut self, doc: DocId, revision: u64, record: &Record) -> io::Result<()> {
        let path = self.record_path(doc);
        let temp = path.with_extension("tmp");
        let json = serde_json::to_vec_pretty(record).map_err(io::Error::other)?;
        fs::write(&temp, json)?;
        fs::rename(&temp, &path)?;
        self.written.insert(doc, revision);
        Ok(())
    }

    /// Deletes the record of a document.
    pub fn remove(&mut self, doc: DocId) {
        if self.written.remove(&doc).is_some() {
            let _ = fs::remove_file(self.record_path(doc));
        }
    }

    /// Deletes the folder of this FastTick (at a normal close).
    pub fn finish(&mut self) {
        drop(self.lock.take());
        self.written.clear();
        let _ = fs::remove_dir_all(&self.dir);
    }

    fn record_path(&self, doc: DocId) -> PathBuf {
        self.dir
            .join(format!("document-{}.{RECORD_EXTENSION}", doc.0))
    }
}

fn read_records(dir: &Path) -> Vec<Record> {
    let Ok(entries) = fs::read_dir(dir) else {
        return Vec::new();
    };
    let mut records: Vec<Record> = entries
        .flatten()
        .map(|e| e.path())
        .filter(|p| p.extension().is_some_and(|e| e == RECORD_EXTENSION))
        .filter_map(|p| serde_json::from_slice::<Record>(&fs::read(p).ok()?).ok())
        .filter(|r| r.version == VERSION && !r.files.is_empty())
        .collect();
    records.sort_by(|a, b| a.saved_at.cmp(&b.saved_at).then(a.title.cmp(&b.title)));
    records
}

fn now_seconds() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or_default()
}

/// A time in the past as text, for example "5 minutes ago".
pub fn age_text(seconds: u64) -> String {
    let (count, unit) = match seconds {
        0..60 => return "less than a minute ago".to_owned(),
        60..3600 => (seconds / 60, "minute"),
        3600..86_400 => (seconds / 3600, "hour"),
        _ => (seconds / 86_400, "day"),
    };
    let plural = if count == 1 { "" } else { "s" };
    format!("{count} {unit}{plural} ago")
}

#[cfg(test)]
mod tests {
    use super::*;

    fn record(title: &str) -> Record {
        Record::new(
            title,
            Some(PathBuf::from("/robot/main.xml")),
            None,
            vec![(Some(PathBuf::from("/robot/main.xml")), "<root/>".to_owned())],
        )
    }

    #[test]
    fn records_of_a_running_fasttick_are_not_orphans() {
        let root = tempfile::tempdir().unwrap();
        let mut first = Recovery::start(root.path()).unwrap();
        first.write(DocId(1), 3, &record("main.xml")).unwrap();
        let second = Recovery::start(root.path()).unwrap();
        assert!(second.orphans().is_empty(), "the first FastTick runs");
        assert!(first.orphans().is_empty(), "the second has no records");
    }

    #[test]
    fn records_of_a_stopped_fasttick_are_orphans() {
        let root = tempfile::tempdir().unwrap();
        let mut first = Recovery::start(root.path()).unwrap();
        first.write(DocId(1), 3, &record("b.xml")).unwrap();
        first.write(DocId(2), 5, &record("a.xml")).unwrap();
        // A crash: the lock goes away, the folder stays.
        drop(first.lock.take());
        let first_dir = first.dir().to_path_buf();

        let second = Recovery::start(root.path()).unwrap();
        let orphans = second.orphans();
        assert_eq!(orphans.len(), 1);
        let titles: Vec<&str> = orphans[0]
            .records
            .iter()
            .map(|r| r.title.as_str())
            .collect();
        assert_eq!(titles.len(), 2);
        assert!(titles.contains(&"a.xml") && titles.contains(&"b.xml"));
        assert_eq!(
            orphans[0].records[0].texts(),
            vec![(Some(PathBuf::from("/robot/main.xml")), "<root/>".to_owned())]
        );

        // While the second FastTick decides, a third does not see them.
        let third = Recovery::start(root.path()).unwrap();
        assert!(third.orphans().is_empty());

        for orphan in orphans {
            orphan.delete();
        }
        assert!(!first_dir.exists());
    }

    #[test]
    fn a_normal_close_and_saves_leave_no_records() {
        let root = tempfile::tempdir().unwrap();
        let mut first = Recovery::start(root.path()).unwrap();
        assert!(first.needs_write(DocId(1), 1));
        first.write(DocId(1), 1, &record("a.xml")).unwrap();
        assert!(!first.needs_write(DocId(1), 1));
        assert!(first.needs_write(DocId(1), 2));
        first.write(DocId(2), 1, &record("b.xml")).unwrap();
        first.remove(DocId(1));
        assert_eq!(first.recorded(), vec![DocId(2)]);
        assert_eq!(read_records(first.dir()).len(), 1);
        first.finish();
        assert!(!first.dir().exists());

        let second = Recovery::start(root.path()).unwrap();
        assert!(second.orphans().is_empty());
    }

    #[test]
    fn empty_folders_of_stopped_fasttick_windows_are_deleted() {
        let root = tempfile::tempdir().unwrap();
        let mut first = Recovery::start(root.path()).unwrap();
        drop(first.lock.take());
        let second = Recovery::start(root.path()).unwrap();
        assert!(second.orphans().is_empty());
        assert!(!first.dir().exists());
    }

    #[test]
    fn age_texts() {
        assert_eq!(age_text(5), "less than a minute ago");
        assert_eq!(age_text(60), "1 minute ago");
        assert_eq!(age_text(150), "2 minutes ago");
        assert_eq!(age_text(7200), "2 hours ago");
        assert_eq!(age_text(86_400 * 3), "3 days ago");
    }
}
