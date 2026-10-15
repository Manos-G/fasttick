//! Background work: file dialogs and file loading.
//!
//! File dialogs block, and large logs take time to read. Each job runs on
//! its own thread and sends its result through a channel. The UI thread
//! reads the results each frame with [`Jobs::poll`]; the worker asks for a
//! repaint when it is done, so the UI does not need to poll.

use std::path::PathBuf;
use std::sync::mpsc::{Receiver, Sender, channel};
use std::sync::Arc;

use fasttick_core::runtime::RuntimeTree;
use fasttick_log::btlog::BtLog;
use fasttick_log::replay::ReplayIndex;

/// What a file dialog is for.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DialogPurpose {
    /// Open a project or a tree XML file.
    OpenFile,
    /// Open a `.btlog` file.
    OpenLog,
    /// Choose a file name to save a copy of the active document.
    SaveAs,
}

/// A loaded log.
#[derive(Debug)]
pub struct LoadedLog {
    /// The log.
    pub log: BtLog,
    /// Its replay index.
    pub index: ReplayIndex,
}

/// The result of a job.
#[derive(Debug)]
pub enum JobResult {
    /// A file dialog closed. `None` if the user cancelled.
    Picked {
        /// The purpose of the dialog.
        purpose: DialogPurpose,
        /// The chosen file.
        path: Option<PathBuf>,
    },
    /// A tree file was read.
    Tree {
        /// The file.
        path: PathBuf,
        /// The parsed tree or an error message.
        result: Result<Arc<RuntimeTree>, String>,
    },
    /// A log file was read.
    Log {
        /// The file.
        path: PathBuf,
        /// The log or an error message.
        result: Result<Box<LoadedLog>, String>,
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
}

impl Default for Jobs {
    fn default() -> Self {
        let (sender, receiver) = channel();
        Jobs {
            sender,
            receiver,
            running: 0,
            dialog_open: false,
        }
    }
}

/// The largest tree file that FastTick reads (64 MiB).
pub const MAX_TREE_FILE: u64 = 64 * 1024 * 1024;

impl Jobs {
    /// The number of jobs that did not finish.
    pub fn running(&self) -> usize {
        self.running
    }

    /// True while a file dialog is open.
    pub fn dialog_open(&self) -> bool {
        self.dialog_open
    }

    fn spawn(&mut self, ctx: &egui::Context, name: &str, job: impl FnOnce() -> JobResult + Send + 'static) {
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
    pub fn pick_file(&mut self, ctx: &egui::Context, purpose: DialogPurpose, start: Option<PathBuf>) {
        if self.dialog_open {
            return;
        }
        self.dialog_open = true;
        self.spawn(ctx, "dialog", move || {
            let mut dialog = rfd::FileDialog::new();
            if let Some(dir) = start.as_ref().and_then(|p| p.parent()) {
                dialog = dialog.set_directory(dir);
            }
            let path = match purpose {
                DialogPurpose::OpenFile => dialog
                    .set_title("Open a project or a tree XML file")
                    .add_filter("Behavior trees and projects", &["xml", "ftproj"])
                    .add_filter("All files", &["*"])
                    .pick_file(),
                DialogPurpose::OpenLog => dialog
                    .set_title("Open a log")
                    .add_filter("BT.CPP logs", &["btlog"])
                    .add_filter("All files", &["*"])
                    .pick_file(),
                DialogPurpose::SaveAs => dialog
                    .set_title("Save a copy")
                    .add_filter("Behavior trees", &["xml"])
                    .save_file(),
            };
            JobResult::Picked { purpose, path }
        });
    }

    /// Reads and parses a tree file on a worker thread.
    pub fn load_tree(&mut self, ctx: &egui::Context, path: PathBuf) {
        self.spawn(ctx, "load-tree", move || {
            let result = read_tree(&path).map(Arc::new);
            JobResult::Tree { path, result }
        });
    }

    /// Reads a log file and builds its replay index on a worker thread.
    pub fn load_log(&mut self, ctx: &egui::Context, path: PathBuf) {
        self.spawn(ctx, "load-log", move || {
            let result = read_log(&path).map(Box::new);
            JobResult::Log { path, result }
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
            if matches!(result, JobResult::Picked { .. }) {
                self.dialog_open = false;
            }
            out.push(result);
        }
        out
    }
}

/// Reads and parses a tree XML file (blocking).
pub fn read_tree(path: &std::path::Path) -> Result<RuntimeTree, String> {
    let meta = std::fs::metadata(path).map_err(|err| err.to_string())?;
    if meta.len() > MAX_TREE_FILE {
        return Err(format!(
            "the file is too large ({} MiB; the limit is {} MiB)",
            meta.len() / (1024 * 1024),
            MAX_TREE_FILE / (1024 * 1024)
        ));
    }
    let bytes = std::fs::read(path).map_err(|err| err.to_string())?;
    let text = String::from_utf8(bytes).map_err(|_| "the file is not UTF-8 text".to_owned())?;
    RuntimeTree::parse(&text).map_err(|err| err.to_string())
}

/// Reads a log file and builds its replay index (blocking).
pub fn read_log(path: &std::path::Path) -> Result<LoadedLog, String> {
    let log = BtLog::open(path).map_err(|err| err.to_string())?;
    let index = ReplayIndex::from_log(&log);
    Ok(LoadedLog { log, index })
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
        std::fs::write(&tree_path, r#"<root><BehaviorTree ID="T"><AlwaysSuccess/></BehaviorTree></root>"#).unwrap();
        let mut jobs = Jobs::default();
        jobs.load_tree(&ctx, tree_path.clone());
        assert_eq!(jobs.running(), 1);
        match wait(&mut jobs) {
            JobResult::Tree { path, result } => {
                assert_eq!(path, tree_path);
                assert_eq!(result.unwrap().len(), 1);
            }
            other => panic!("{other:?}"),
        }
        assert_eq!(jobs.running(), 0);

        jobs.load_tree(&ctx, dir.path().join("missing.xml"));
        assert!(matches!(wait(&mut jobs), JobResult::Tree { result: Err(_), .. }));

        let bad = dir.path().join("bad.xml");
        std::fs::write(&bad, "<root>").unwrap();
        jobs.load_tree(&ctx, bad);
        assert!(matches!(wait(&mut jobs), JobResult::Tree { result: Err(_), .. }));

        let out = dir.path().join("copy.xml");
        jobs.save_text(&ctx, out.clone(), "abc".into());
        assert!(matches!(wait(&mut jobs), JobResult::Saved { result: Ok(()), .. }));
        assert_eq!(std::fs::read_to_string(out).unwrap(), "abc");

        jobs.load_log(&ctx, dir.path().join("none.btlog"));
        assert!(matches!(wait(&mut jobs), JobResult::Log { result: Err(_), .. }));
    }

    #[test]
    fn reads_the_harness_log() {
        let path = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../tests/fixtures/harness/main.btlog");
        let loaded = read_log(&path).unwrap();
        assert!(!loaded.log.tree.is_empty());
        assert!(!loaded.index.is_empty());
    }
}
