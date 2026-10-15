//! Background work: file dialogs and file loading.
//!
//! File dialogs block, and large logs take time to read. Each job runs on
//! its own thread and sends its result through a channel. The UI thread
//! reads the results each frame with [`Jobs::poll`]; the worker asks for a
//! repaint when it is done, so the UI does not need to poll.

use std::io::Read;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::{AtomicU8, AtomicU64, Ordering};
use std::sync::mpsc::{Receiver, Sender, channel};

use fasttick_log::btlog::{BtLog, BtLogReader};
use fasttick_log::export::{export_csv, export_json, export_rows};
use fasttick_log::filter::TransitionFilter;
use fasttick_log::replay::ReplayIndex;
use fasttick_log::sqlite::{SessionInfo, SqliteLog};
use fasttick_protocol::breakpoints::BreakpointSet;

use crate::live::recording::{RecordingFile, write_btlog};

/// The export file format of the transitions table.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum ExportFormat {
    /// A JSON array of objects.
    Json,
    /// CSV with a header row.
    Csv,
}

impl ExportFormat {
    /// The file extension.
    pub fn extension(self) -> &'static str {
        match self {
            ExportFormat::Json => "json",
            ExportFormat::Csv => "csv",
        }
    }
}

/// What a file dialog is for.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DialogPurpose {
    /// Open a project or a tree XML file.
    OpenFile,
    /// Open a log (`.btlog` or a SQLite database).
    OpenLog,
    /// Choose a file name to save a copy of the active document.
    SaveAs,
    /// Choose a `.btlog` file for the live recording.
    SaveRecording,
    /// Choose a breakpoint file to write.
    SaveBreakpoints,
    /// Choose a breakpoint file to read.
    LoadBreakpoints,
    /// Choose a file for the filtered transitions.
    ExportTransitions(ExportFormat),
    /// Choose a tree XML file to add to the project of the active edit
    /// document.
    AddTreeFile,
    /// Choose the path of a new tree XML file for the project.
    NewTreeFile,
    /// Choose a node model file (`<TreeNodesModel>` XML) to add to the
    /// project.
    AddModelFile,
    /// Choose the path of the project file (`.ftproj`).
    SaveProject,
}

/// The file extension of breakpoint files.
pub const BREAKPOINT_EXTENSION: &str = fasttick_protocol::breakpoints::FILE_EXTENSION;

/// The extensions of SQLite logs (`BT::SqliteLogger` writes `.db3` or
/// `.btdb`).
pub const SQLITE_EXTENSIONS: [&str; 5] = ["db3", "btdb", "sqlite", "sqlite3", "db"];

/// True if the path has the extension of a SQLite log.
pub fn is_sqlite(path: &Path) -> bool {
    path.extension()
        .and_then(|e| e.to_str())
        .is_some_and(|e| SQLITE_EXTENSIONS.contains(&e.to_ascii_lowercase().as_str()))
}

/// A path with an extension added if the file name does not end with it
/// (save dialogs do not always add it).
pub fn with_extension(path: PathBuf, extension: &str) -> PathBuf {
    let name = path
        .file_name()
        .map(|n| n.to_string_lossy().to_ascii_lowercase())
        .unwrap_or_default();
    if name.ends_with(&format!(".{extension}")) {
        path
    } else {
        let mut text = path.into_os_string();
        text.push(".");
        text.push(extension);
        PathBuf::from(text)
    }
}

/// A loaded log.
#[derive(Debug)]
pub struct LoadedLog {
    /// The log.
    pub log: BtLog,
    /// Its replay index.
    pub index: ReplayIndex,
    /// A label for the source in the file, for example "session 2".
    pub source: Option<String>,
}

/// The phase of a log load.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LoadPhase {
    /// The file is read.
    Reading,
    /// The replay index is built.
    Indexing,
}

/// The progress of a log load, shared with the worker thread.
#[derive(Debug, Default)]
pub struct LoadProgress {
    done: AtomicU64,
    total: AtomicU64,
    phase: AtomicU8,
}

impl LoadProgress {
    /// The read part, from 0 to 1 (0 while the size is not known).
    pub fn fraction(&self) -> f32 {
        let total = self.total.load(Ordering::Relaxed);
        if total == 0 {
            return 0.0;
        }
        (self.done.load(Ordering::Relaxed) as f64 / total as f64).clamp(0.0, 1.0) as f32
    }

    /// The phase.
    pub fn phase(&self) -> LoadPhase {
        match self.phase.load(Ordering::Relaxed) {
            0 => LoadPhase::Reading,
            _ => LoadPhase::Indexing,
        }
    }

    fn set_total(&self, total: u64) {
        self.total.store(total, Ordering::Relaxed);
    }

    fn add(&self, bytes: u64) {
        self.done.fetch_add(bytes, Ordering::Relaxed);
    }

    fn set_phase(&self, phase: LoadPhase) {
        self.phase.store(
            match phase {
                LoadPhase::Reading => 0,
                LoadPhase::Indexing => 1,
            },
            Ordering::Relaxed,
        );
    }
}

/// A log that is loading now.
#[derive(Debug, Clone)]
pub struct LoadingFile {
    /// The file.
    pub path: PathBuf,
    /// The progress.
    pub progress: Arc<LoadProgress>,
}

/// A loaded tree file or project, ready for editing.
#[derive(Debug)]
pub struct LoadedTree {
    /// The editor state: the project, its history and its problems.
    pub editor: Box<crate::editor::EditorDoc>,
    /// The XML files of the folder that have the missing trees and node
    /// models of the project.
    pub missing: fasttick_core::project::MissingFiles,
}

impl LoadedTree {
    /// The index of the main tree: the tree that `main_tree_to_execute`
    /// names, else the first tree.
    pub fn main_instance(&self) -> fasttick_core::runtime::InstanceIndex {
        fasttick_core::runtime::InstanceIndex(self.editor.main_tree_index())
    }
}

/// The result of a job.
#[derive(Debug)]
pub enum JobResult {
    /// A file dialog closed. The list is empty if the user cancelled.
    Picked {
        /// The purpose of the dialog.
        purpose: DialogPurpose,
        /// The chosen files. Only the dialogs that add files to a project
        /// accept more than one file.
        paths: Vec<PathBuf>,
    },
    /// A tree file was read.
    Tree {
        /// The file.
        path: PathBuf,
        /// The parsed tree or an error message.
        result: Result<LoadedTree, String>,
    },
    /// A log file was read.
    Log {
        /// The file.
        path: PathBuf,
        /// The session of a SQLite log.
        session: Option<i64>,
        /// The log or an error message.
        result: Result<Box<LoadedLog>, String>,
    },
    /// The sessions of a SQLite log were read. The user picks one.
    SqliteSessions {
        /// The file.
        path: PathBuf,
        /// The sessions or an error message.
        result: Result<Vec<SessionInfo>, String>,
    },
    /// A recording was written.
    RecordingSaved {
        /// The file.
        path: PathBuf,
        /// The number of transitions, or an error message.
        result: Result<u64, String>,
    },
    /// Transitions were exported.
    Exported {
        /// The file.
        path: PathBuf,
        /// The number of rows, or an error message.
        result: Result<usize, String>,
    },
    /// A breakpoint file was written.
    BreakpointsSaved {
        /// The file.
        path: PathBuf,
        /// Ok, or an error message.
        result: Result<(), String>,
    },
    /// A breakpoint file was read.
    BreakpointsLoaded {
        /// The file.
        path: PathBuf,
        /// The breakpoints, or an error message.
        result: Result<BreakpointSet, String>,
    },
    /// A file was written.
    Saved {
        /// The file.
        path: PathBuf,
        /// Ok, or an error message.
        result: Result<(), String>,
    },
}

/// Runs jobs on worker threads.
#[derive(Debug)]
pub struct Jobs {
    sender: Sender<JobResult>,
    receiver: Receiver<JobResult>,
    running: usize,
    dialog_open: bool,
    loading: Vec<LoadingFile>,
}

impl Default for Jobs {
    fn default() -> Self {
        let (sender, receiver) = channel();
        Jobs {
            sender,
            receiver,
            running: 0,
            dialog_open: false,
            loading: Vec::new(),
        }
    }
}

/// The largest tree file that FastTick reads (64 MiB).
pub const MAX_TREE_FILE: u64 = 64 * 1024 * 1024;

/// The largest `.btlog` file that FastTick reads (4 GiB, about 470
/// million transitions).
pub const MAX_LOG_FILE: u64 = 4 * 1024 * 1024 * 1024;

impl Jobs {
    /// The number of jobs that did not finish.
    pub fn running(&self) -> usize {
        self.running
    }

    /// True while a file dialog is open.
    pub fn dialog_open(&self) -> bool {
        self.dialog_open
    }

    /// The logs that are loading now.
    pub fn loading(&self) -> &[LoadingFile] {
        &self.loading
    }

    fn spawn(
        &mut self,
        ctx: &egui::Context,
        name: &str,
        job: impl FnOnce() -> JobResult + Send + 'static,
    ) {
        let sender = self.sender.clone();
        let ctx = ctx.clone();
        let spawned = std::thread::Builder::new()
            .name(format!("fasttick-{name}"))
            .spawn(move || {
                let result = job();
                let _ = sender.send(result);
                ctx.request_repaint();
            });
        if spawned.is_ok() {
            self.running += 1;
        }
    }

    /// Opens a file dialog on a worker thread. Does nothing while another
    /// dialog is open.
    pub fn pick_file(
        &mut self,
        ctx: &egui::Context,
        purpose: DialogPurpose,
        start: Option<PathBuf>,
    ) {
        if self.dialog_open {
            return;
        }
        self.dialog_open = true;
        self.spawn(ctx, "dialog", move || {
            let mut dialog = rfd::FileDialog::new();
            if let Some(dir) = start.as_ref().and_then(|p| p.parent()) {
                dialog = dialog.set_directory(dir);
            }
            let paths = match purpose {
                DialogPurpose::OpenFile => dialog
                    .set_title("Open a project or a tree XML file")
                    .add_filter("Behavior trees and projects", &["xml", "ftproj"])
                    .add_filter("All files", &["*"])
                    .pick_file(),
                DialogPurpose::OpenLog => dialog
                    .set_title("Open a log")
                    .add_filter("BT.CPP logs", &["btlog"])
                    .add_filter("SQLite logs", &SQLITE_EXTENSIONS)
                    .add_filter("All files", &["*"])
                    .pick_file(),
                DialogPurpose::SaveAs => dialog
                    .set_title("Save a copy")
                    .add_filter("Behavior trees", &["xml"])
                    .save_file(),
                DialogPurpose::SaveRecording => dialog
                    .set_title("Save the recording")
                    .set_file_name("recording.btlog")
                    .add_filter("BT.CPP logs", &["btlog"])
                    .save_file()
                    .map(|p| with_extension(p, "btlog")),
                DialogPurpose::SaveBreakpoints => dialog
                    .set_title("Save the breakpoints")
                    .set_file_name(format!("breakpoints.{BREAKPOINT_EXTENSION}"))
                    .add_filter("FastTick breakpoints", &["json"])
                    .save_file()
                    .map(|p| with_extension(p, BREAKPOINT_EXTENSION)),
                DialogPurpose::LoadBreakpoints => dialog
                    .set_title("Load breakpoints")
                    .add_filter("FastTick breakpoints", &["json"])
                    .add_filter("All files", &["*"])
                    .pick_file(),
                DialogPurpose::AddTreeFile => {
                    let paths = dialog
                        .set_title("Add tree files to the project")
                        .add_filter("Behavior trees", &["xml"])
                        .add_filter("All files", &["*"])
                        .pick_files();
                    return JobResult::Picked {
                        purpose,
                        paths: paths.unwrap_or_default(),
                    };
                }
                DialogPurpose::AddModelFile => {
                    let paths = dialog
                        .set_title("Add node model files to the project")
                        .add_filter("Node models", &["xml"])
                        .add_filter("All files", &["*"])
                        .pick_files();
                    return JobResult::Picked {
                        purpose,
                        paths: paths.unwrap_or_default(),
                    };
                }
                DialogPurpose::NewTreeFile => dialog
                    .set_title("New tree file")
                    .set_file_name("tree.xml")
                    .add_filter("Behavior trees", &["xml"])
                    .save_file()
                    .map(|p| with_extension(p, "xml")),
                DialogPurpose::SaveProject => dialog
                    .set_title("Save the project")
                    .set_file_name("project.ftproj")
                    .add_filter("FastTick projects", &["ftproj"])
                    .save_file()
                    .map(|p| with_extension(p, "ftproj")),
                DialogPurpose::ExportTransitions(format) => {
                    let extension = format.extension();
                    dialog
                        .set_title("Export the transitions")
                        .set_file_name(format!("transitions.{extension}"))
                        .add_filter(extension.to_ascii_uppercase(), &[extension])
                        .save_file()
                        .map(|p| with_extension(p, extension))
                }
            };
            JobResult::Picked {
                purpose,
                paths: paths.into_iter().collect(),
            }
        });
    }

    /// Reads and parses a tree file on a worker thread.
    pub fn load_tree(&mut self, ctx: &egui::Context, path: PathBuf) {
        self.spawn(ctx, "load-tree", move || {
            let result = read_tree(&path);
            JobResult::Tree { path, result }
        });
    }

    /// Reads a log file and builds its replay index on a worker thread.
    /// For a SQLite log, the job reads the list of sessions first; with
    /// one session it loads that session at once.
    pub fn load_log(&mut self, ctx: &egui::Context, path: PathBuf) {
        let progress = Arc::new(LoadProgress::default());
        self.loading.push(LoadingFile {
            path: path.clone(),
            progress: Arc::clone(&progress),
        });
        if is_sqlite(&path) {
            self.spawn(ctx, "load-log", move || match read_sqlite_sessions(&path) {
                Ok(sessions) if sessions.len() == 1 => {
                    let id = sessions[0].id;
                    progress.set_phase(LoadPhase::Indexing);
                    let result = read_sqlite_session(&path, id).map(Box::new);
                    JobResult::Log {
                        path,
                        session: Some(id),
                        result,
                    }
                }
                result => JobResult::SqliteSessions { path, result },
            });
            return;
        }
        self.spawn(ctx, "load-log", move || {
            let result = read_log_with_progress(&path, &progress).map(Box::new);
            JobResult::Log {
                path,
                session: None,
                result,
            }
        });
    }

    /// Loads one session of a SQLite log on a worker thread.
    pub fn load_sqlite_session(&mut self, ctx: &egui::Context, path: PathBuf, id: i64) {
        let progress = Arc::new(LoadProgress::default());
        progress.set_phase(LoadPhase::Indexing);
        self.loading.push(LoadingFile {
            path: path.clone(),
            progress,
        });
        self.spawn(ctx, "load-log", move || {
            let result = read_sqlite_session(&path, id).map(Box::new);
            JobResult::Log {
                path,
                session: Some(id),
                result,
            }
        });
    }

    /// Writes a live recording as a `.btlog` file on a worker thread.
    pub fn save_recording(&mut self, ctx: &egui::Context, path: PathBuf, file: RecordingFile) {
        self.spawn(ctx, "save-recording", move || {
            let result = write_btlog(&path, &file).map_err(|err| err.to_string());
            JobResult::RecordingSaved { path, result }
        });
    }

    /// Exports the transitions of a log that pass a filter, on a worker
    /// thread.
    pub fn export_transitions(
        &mut self,
        ctx: &egui::Context,
        path: PathBuf,
        format: ExportFormat,
        log: Arc<BtLog>,
        filter: TransitionFilter,
    ) {
        self.spawn(ctx, "export", move || {
            let result = write_export(&path, format, &log, &filter);
            JobResult::Exported { path, result }
        });
    }

    /// Writes a breakpoint file on a worker thread.
    pub fn save_breakpoints(&mut self, ctx: &egui::Context, path: PathBuf, set: BreakpointSet) {
        self.spawn(ctx, "save-breakpoints", move || {
            let result = set.save(&path).map_err(|err| err.to_string());
            JobResult::BreakpointsSaved { path, result }
        });
    }

    /// Reads a breakpoint file on a worker thread.
    pub fn load_breakpoints(&mut self, ctx: &egui::Context, path: PathBuf) {
        self.spawn(ctx, "load-breakpoints", move || {
            let result = BreakpointSet::load(&path).map_err(|err| err.to_string());
            JobResult::BreakpointsLoaded { path, result }
        });
    }

    /// Writes text to a file on a worker thread.
    pub fn save_text(&mut self, ctx: &egui::Context, path: PathBuf, text: String) {
        self.spawn(ctx, "save", move || {
            let result = std::fs::write(&path, text).map_err(|err| err.to_string());
            JobResult::Saved { path, result }
        });
    }

    /// The results of the jobs that finished since the last call.
    pub fn poll(&mut self) -> Vec<JobResult> {
        let mut out = Vec::new();
        while let Ok(result) = self.receiver.try_recv() {
            self.running = self.running.saturating_sub(1);
            match &result {
                JobResult::Picked { .. } => self.dialog_open = false,
                JobResult::Log { path, .. } | JobResult::SqliteSessions { path, .. } => {
                    if let Some(position) = self.loading.iter().position(|l| l.path == *path) {
                        self.loading.remove(position);
                    }
                }
                _ => {}
            }
            out.push(result);
        }
        out
    }
}

/// Reads and parses a tree XML file (blocking).
pub fn read_tree(path: &Path) -> Result<LoadedTree, String> {
    let meta = std::fs::metadata(path).map_err(|err| err.to_string())?;
    if meta.len() > MAX_TREE_FILE {
        return Err(format!(
            "the file is too large ({} MiB; the limit is {} MiB)",
            meta.len() / (1024 * 1024),
            MAX_TREE_FILE / (1024 * 1024)
        ));
    }
    let editor = crate::editor::EditorDoc::open(path)?;
    let missing = editor.project().find_missing_files();
    Ok(LoadedTree {
        editor: Box::new(editor),
        missing,
    })
}

/// Reads a log file and builds its replay index (blocking). A SQLite log
/// must have exactly one session.
pub fn read_log(path: &Path) -> Result<LoadedLog, String> {
    if is_sqlite(path) {
        let sessions = read_sqlite_sessions(path)?;
        return match sessions.as_slice() {
            [single] => read_sqlite_session(path, single.id),
            [] => Err("the database has no sessions".to_owned()),
            _ => Err(format!(
                "the database has {} sessions; choose one in the menu",
                sessions.len()
            )),
        };
    }
    read_log_with_progress(path, &LoadProgress::default())
}

/// Reads a `.btlog` file in parts and reports the progress, then builds
/// the replay index (blocking).
pub fn read_log_with_progress(path: &Path, progress: &LoadProgress) -> Result<LoadedLog, String> {
    let mut file = std::fs::File::open(path).map_err(|err| err.to_string())?;
    let total = file.metadata().map_err(|err| err.to_string())?.len();
    if total > MAX_LOG_FILE {
        return Err(format!(
            "the file is too large ({} MiB; the limit is {} MiB)",
            total / (1024 * 1024),
            MAX_LOG_FILE / (1024 * 1024)
        ));
    }
    progress.set_total(total);
    let mut reader = BtLogReader::new();
    let mut buffer = vec![0u8; 1 << 20];
    loop {
        let count = match file.read(&mut buffer) {
            Ok(0) => break,
            Ok(count) => count,
            Err(err) if err.kind() == std::io::ErrorKind::Interrupted => continue,
            Err(err) => return Err(err.to_string()),
        };
        reader
            .push(&buffer[..count])
            .map_err(|err| err.to_string())?;
        progress.add(count as u64);
    }
    let log = reader.into_log().map_err(|err| err.to_string())?;
    progress.set_phase(LoadPhase::Indexing);
    let index = ReplayIndex::from_log(&log);
    Ok(LoadedLog {
        log,
        index,
        source: None,
    })
}

/// Reads the sessions of a SQLite log (blocking).
pub fn read_sqlite_sessions(path: &Path) -> Result<Vec<SessionInfo>, String> {
    let db = SqliteLog::open(path).map_err(|err| err.to_string())?;
    db.sessions().map_err(|err| err.to_string())
}

/// Reads one session of a SQLite log and builds its replay index
/// (blocking).
pub fn read_sqlite_session(path: &Path, id: i64) -> Result<LoadedLog, String> {
    let db = SqliteLog::open(path).map_err(|err| err.to_string())?;
    let session = db.load_session(id).map_err(|err| err.to_string())?;
    let index = ReplayIndex::from_log(&session.log);
    Ok(LoadedLog {
        log: session.log,
        index,
        source: Some(format!("session {id}")),
    })
}

/// Writes the transitions of a log that pass a filter (blocking). Returns
/// the number of rows.
pub fn write_export(
    path: &Path,
    format: ExportFormat,
    log: &BtLog,
    filter: &TransitionFilter,
) -> Result<usize, String> {
    let rows = export_rows(log, filter);
    let file = std::fs::File::create(path).map_err(|err| err.to_string())?;
    let writer = std::io::BufWriter::new(file);
    match format {
        ExportFormat::Json => export_json(&rows, writer),
        ExportFormat::Csv => export_csv(&rows, writer),
    }
    .map_err(|err| err.to_string())?;
    Ok(rows.len())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn wait(jobs: &mut Jobs) -> JobResult {
        for _ in 0..500 {
            if let Some(result) = jobs.poll().into_iter().next() {
                return result;
            }
            std::thread::sleep(std::time::Duration::from_millis(10));
        }
        panic!("job did not finish");
    }

    #[test]
    fn loads_and_saves_on_worker_threads() {
        let ctx = egui::Context::default();
        let dir = tempfile::tempdir().unwrap();
        let tree_path = dir.path().join("t.xml");
        std::fs::write(
            &tree_path,
            r#"<root><BehaviorTree ID="T"><AlwaysSuccess/></BehaviorTree></root>"#,
        )
        .unwrap();
        let mut jobs = Jobs::default();
        jobs.load_tree(&ctx, tree_path.clone());
        assert_eq!(jobs.running(), 1);
        match wait(&mut jobs) {
            JobResult::Tree { path, result } => {
                assert_eq!(path, tree_path);
                let loaded = result.unwrap();
                assert_eq!(loaded.editor.trees().len(), 1);
                assert_eq!(loaded.main_instance().0, 0);
            }
            other => panic!("{other:?}"),
        }
        assert_eq!(jobs.running(), 0);

        jobs.load_tree(&ctx, dir.path().join("missing.xml"));
        assert!(matches!(
            wait(&mut jobs),
            JobResult::Tree { result: Err(_), .. }
        ));

        let bad = dir.path().join("bad.xml");
        std::fs::write(&bad, "<root>").unwrap();
        jobs.load_tree(&ctx, bad);
        assert!(matches!(
            wait(&mut jobs),
            JobResult::Tree { result: Err(_), .. }
        ));

        let out = dir.path().join("copy.xml");
        jobs.save_text(&ctx, out.clone(), "abc".into());
        assert!(matches!(
            wait(&mut jobs),
            JobResult::Saved { result: Ok(()), .. }
        ));
        assert_eq!(std::fs::read_to_string(out).unwrap(), "abc");

        jobs.load_log(&ctx, dir.path().join("none.btlog"));
        assert_eq!(jobs.loading().len(), 1);
        assert!(matches!(
            wait(&mut jobs),
            JobResult::Log { result: Err(_), .. }
        ));
        assert!(jobs.loading().is_empty());
    }

    fn harness_log() -> PathBuf {
        PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../tests/fixtures/harness/main.btlog")
    }

    #[test]
    fn loads_a_log_with_progress_and_exports() {
        let ctx = egui::Context::default();
        let mut jobs = Jobs::default();
        jobs.load_log(&ctx, harness_log());
        let progress = Arc::clone(&jobs.loading()[0].progress);
        let loaded = match wait(&mut jobs) {
            JobResult::Log {
                result: Ok(loaded),
                session: None,
                ..
            } => loaded,
            other => panic!("{other:?}"),
        };
        assert!((progress.fraction() - 1.0).abs() < 1e-6);
        assert_eq!(progress.phase(), LoadPhase::Indexing);
        assert_eq!(loaded.index.len(), loaded.log.transitions.len());

        let dir = tempfile::tempdir().unwrap();
        let log = Arc::new(loaded.log);
        let filter = TransitionFilter {
            include: vec!["say*".into()],
            ..TransitionFilter::default()
        };
        for format in [ExportFormat::Json, ExportFormat::Csv] {
            let path = dir.path().join(format!("out.{}", format.extension()));
            jobs.export_transitions(&ctx, path.clone(), format, Arc::clone(&log), filter.clone());
            let count = match wait(&mut jobs) {
                JobResult::Exported { result: Ok(n), .. } => n,
                other => panic!("{other:?}"),
            };
            assert!(count > 0);
            let text = std::fs::read_to_string(&path).unwrap();
            match format {
                ExportFormat::Json => {
                    let rows: serde_json::Value = serde_json::from_str(&text).unwrap();
                    assert_eq!(rows.as_array().unwrap().len(), count);
                }
                ExportFormat::Csv => assert_eq!(text.lines().count(), count + 1),
            }
        }
        // A bad path gives an error.
        assert!(
            write_export(
                &dir.path().join("no/dir/x.csv"),
                ExportFormat::Csv,
                &log,
                &filter
            )
            .is_err()
        );
    }

    #[test]
    fn breakpoint_files_round_trip() {
        let ctx = egui::Context::default();
        let mut jobs = Jobs::default();
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("b.ftbp.json");
        let mut set = BreakpointSet::new();
        set.breakpoints
            .push(fasttick_protocol::breakpoints::Breakpoint {
                node_path: "a/b".into(),
                uid: Some(3),
                position: Default::default(),
                mode: Default::default(),
                enabled: true,
                once: false,
                desired_status: fasttick_core::status::NodeStatus::Skipped,
                label: None,
            });
        jobs.save_breakpoints(&ctx, path.clone(), set.clone());
        assert!(matches!(
            wait(&mut jobs),
            JobResult::BreakpointsSaved { result: Ok(()), .. }
        ));
        jobs.load_breakpoints(&ctx, path);
        match wait(&mut jobs) {
            JobResult::BreakpointsLoaded {
                result: Ok(loaded), ..
            } => assert_eq!(loaded, set),
            other => panic!("{other:?}"),
        }
        jobs.load_breakpoints(&ctx, dir.path().join("missing.ftbp.json"));
        assert!(matches!(
            wait(&mut jobs),
            JobResult::BreakpointsLoaded { result: Err(_), .. }
        ));
    }

    #[test]
    fn sqlite_logs_and_names() {
        assert!(is_sqlite(Path::new("/x/log.db3")));
        assert!(is_sqlite(Path::new("/x/log.BTDB")));
        assert!(!is_sqlite(Path::new("/x/log.btlog")));
        assert_eq!(
            with_extension(PathBuf::from("/x/rec"), "btlog"),
            PathBuf::from("/x/rec.btlog")
        );
        assert_eq!(
            with_extension(PathBuf::from("/x/rec.BTLOG"), "btlog"),
            PathBuf::from("/x/rec.BTLOG")
        );
        assert_eq!(
            with_extension(PathBuf::from("/x/b"), BREAKPOINT_EXTENSION),
            PathBuf::from("/x/b.ftbp.json")
        );
        // Not a database.
        let dir = tempfile::tempdir().unwrap();
        let bad = dir.path().join("bad.db3");
        std::fs::write(&bad, b"not sqlite").unwrap();
        assert!(read_sqlite_sessions(&bad).is_err());
        assert!(read_log(&bad).is_err());
    }

    #[test]
    fn main_tree_to_execute_picks_the_tree() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("two.xml");
        std::fs::write(
            &path,
            r#"<root BTCPP_format="4" main_tree_to_execute="Second">
                <BehaviorTree ID="First"><AlwaysSuccess/></BehaviorTree>
                <BehaviorTree ID="Second"><AlwaysFailure/></BehaviorTree>
            </root>"#,
        )
        .unwrap();
        let loaded = read_tree(&path).unwrap();
        assert_eq!(loaded.main_instance().0, 1);
        // An unknown name gives the first tree.
        std::fs::write(
            &path,
            r#"<root BTCPP_format="4" main_tree_to_execute="Nope">
                <BehaviorTree ID="First"><AlwaysSuccess/></BehaviorTree>
                <BehaviorTree ID="Second"><AlwaysFailure/></BehaviorTree>
            </root>"#,
        )
        .unwrap();
        assert_eq!(read_tree(&path).unwrap().main_instance().0, 0);
    }

    #[test]
    fn reads_the_harness_log() {
        let loaded = read_log(&harness_log()).unwrap();
        assert!(!loaded.log.tree.is_empty());
        assert!(!loaded.index.is_empty());
    }
}
