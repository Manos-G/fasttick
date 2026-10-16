//! The remote file features of the app: the "Open from robot" dialog, the
//! dialog for the questions of `ssh`, and the jobs that open and upload
//! remote files (see [`crate::remote`]).
//!
//! The jobs run on worker threads ([`crate::jobs::Jobs::run_remote`]) and
//! return a [`RemoteResult`]; the app applies the result on the GUI thread.

use std::collections::{HashMap, HashSet, VecDeque};
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use egui::{Key, RichText};
use serde::{Deserialize, Serialize};

use crate::remote::askpass::{AskpassServer, Prompt, PromptKind};
use crate::remote::ops::{self, FetchProblem, Listing, Upload};
use crate::remote::ssh::{SshConfig, SshError};
use crate::remote::{
    NO_FILE, RemoteCache, RemoteFile, RemoteHost, RemoteIndex, RemoteUrl, posix_join, posix_parent,
};

/// The most computers in the list of recent computers.
pub const MAX_RECENT_HOSTS: usize = 8;

/// The settings of one computer.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct RemoteHostSettings {
    /// The address, for example `pi@robot.local`.
    pub address: String,
    /// The last folder.
    pub folder: String,
    /// A command that runs on the computer after each upload (for example
    /// a restart of the robot program). Empty: no command.
    pub restart_command: String,
}

/// The file extensions of logs (they open in replay mode).
const LOG_EXTENSIONS: [&str; 6] = ["btlog", "db3", "btdb", "sqlite", "sqlite3", "db"];

/// True if a remote file name is a log.
pub fn is_log_name(name: &str) -> bool {
    let lower = name.to_ascii_lowercase();
    LOG_EXTENSIONS
        .iter()
        .any(|ext| lower.ends_with(&format!(".{ext}")))
}

/// True if FastTick can open a remote file (tree, project or log).
pub fn is_openable_name(name: &str) -> bool {
    let lower = name.to_ascii_lowercase();
    lower.ends_with(".xml") || lower.ends_with(".ftproj") || is_log_name(name)
}

/// The remote state of the app.
#[derive(Debug)]
pub struct RemoteState {
    /// The local copies.
    pub cache: RemoteCache,
    /// What FastTick knows about the local copies.
    pub index: RemoteIndex,
    askpass: Option<AskpassServer>,
    askpass_tried: bool,
    /// The "Open from robot" dialog, if it is open.
    pub dialog: Option<OpenDialog>,
    prompt: Option<PromptDialog>,
    /// Local copies whose upload found a change on the remote computer;
    /// the app asks about the first one.
    pub conflicts: VecDeque<PathBuf>,
    /// The number of upload jobs that run.
    pub uploading: usize,
    /// An `ssh` configuration file instead of the user's (see
    /// [`RemoteState::configure`]).
    config_file: Option<PathBuf>,
    /// The question program instead of this program.
    askpass_program: Option<PathBuf>,
    /// The number of check jobs that run.
    pub checking: usize,
    /// When FastTick last checked a document (its local main file) for
    /// changes on the robot.
    checked: HashMap<PathBuf, Instant>,
    /// The robot checksums that a notice told the user about.
    notified: HashMap<PathBuf, String>,
    was_focused: bool,
    last_active: Option<PathBuf>,
}

/// The shortest time between two checks of a document for changes on the
/// robot.
pub const CHECK_INTERVAL: Duration = Duration::from_secs(10);

impl RemoteState {
    /// The state with the cache in `root` (its index is loaded).
    pub fn new(root: PathBuf) -> RemoteState {
        let cache = RemoteCache::new(root);
        let index = RemoteIndex::load(&cache);
        RemoteState {
            cache,
            index,
            askpass: None,
            askpass_tried: false,
            dialog: None,
            prompt: None,
            conflicts: VecDeque::new(),
            uploading: 0,
            config_file: None,
            askpass_program: None,
            checking: 0,
            checked: HashMap::new(),
            notified: HashMap::new(),
            was_focused: true,
            last_active: None,
        }
    }

    /// Like [`RemoteState::ssh_config`], but `ssh` never asks: a check in
    /// the background fails instead of a password question. A shared
    /// connection that is open still serves it.
    pub fn quiet_ssh_config(&mut self, ctx: &egui::Context) -> SshConfig {
        let mut config = self.ssh_config(ctx);
        config.askpass = None;
        config
    }

    /// True if FastTick must check the active remote document (its local
    /// main file is `active`) now: the window got the focus again, or the
    /// user went to the document from another tab.
    pub fn check_trigger(&mut self, focused: bool, active: Option<&Path>) -> bool {
        let gained = focused && !self.was_focused;
        self.was_focused = focused;
        let switched = active != self.last_active.as_deref();
        self.last_active = active.map(Path::to_path_buf);
        (gained || switched) && active.is_some()
    }

    /// Forgets the times of the checks, so the next check runs at once
    /// (for tests).
    pub fn forget_checks(&mut self) {
        self.checked.clear();
    }

    /// Starts a check of a document if the last check of it is older than
    /// [`CHECK_INTERVAL`]. Returns false if it is too soon.
    pub fn begin_check(&mut self, main: &Path) -> bool {
        let now = Instant::now();
        if self
            .checked
            .get(main)
            .is_some_and(|t| now.duration_since(*t) < CHECK_INTERVAL)
        {
            return false;
        }
        self.checked.insert(main.to_path_buf(), now);
        true
    }

    /// The copies that changed on the robot and that no notice told the
    /// user about yet. A file counts as changed if its robot checksum is
    /// different from the checksum at the start of the check and from the
    /// one that FastTick knows now (an upload during the check changes
    /// it).
    pub fn new_changes(&mut self, checked: Vec<CheckedFile>) -> Vec<PathBuf> {
        let mut out = Vec::new();
        for file in checked {
            let Some(entry) = self.index.files.get(&file.local) else {
                continue;
            };
            if file.current == file.base || file.current == entry.base {
                self.notified.remove(&file.local);
                continue;
            }
            if self.notified.get(&file.local) == Some(&file.current) {
                continue;
            }
            self.notified.insert(file.local.clone(), file.current);
            out.push(file.local);
        }
        out
    }

    /// Uses another cache folder, `ssh` configuration file and question
    /// program (for tests and tools). The index of the new cache is loaded.
    pub fn configure(
        &mut self,
        cache_root: PathBuf,
        config_file: Option<PathBuf>,
        askpass_program: Option<PathBuf>,
    ) {
        self.cache = RemoteCache::new(cache_root);
        self.index = RemoteIndex::load(&self.cache);
        self.config_file = config_file;
        self.askpass_program = askpass_program;
    }

    /// The `ssh` configuration. The first call starts the server for the
    /// questions of `ssh` (if it cannot start, `ssh` runs in batch mode:
    /// keys without passphrase only).
    pub fn ssh_config(&mut self, ctx: &egui::Context) -> SshConfig {
        if !self.askpass_tried {
            self.askpass_tried = true;
            let ctx = ctx.clone();
            self.askpass = AskpassServer::start(move || ctx.request_repaint()).ok();
        }
        let program = self
            .askpass_program
            .clone()
            .or_else(|| std::env::current_exe().ok());
        let askpass = self.askpass.as_ref().zip(program).map(|(s, p)| s.env(p));
        let mut config = SshConfig::from_env(askpass, Some(self.cache.root().join("ctl")));
        if let Some(file) = &self.config_file {
            config.config_file = Some(file.clone());
        }
        config
    }

    /// Forgets the remembered passwords (after a failed login).
    pub fn forget_passwords(&self) {
        if let Some(askpass) = &self.askpass {
            askpass.forget_all();
        }
    }

    /// Saves the index (errors are ignored: the next save tries again).
    pub fn save_index(&self) {
        let _ = self.index.save(&self.cache);
    }

    /// The remote file of a local copy.
    pub fn url_of(&self, local: &Path) -> Option<RemoteUrl> {
        self.cache.url_of(local)
    }

    /// For a local copy: the folder of its computer, so that absolute
    /// `<include>` paths resolve to the copies of the remote files.
    pub fn mirror_root(&self, local: &Path) -> Option<fasttick_core::project::MirrorRoot> {
        let url = self.url_of(local)?;
        Some(fasttick_core::project::MirrorRoot {
            root: self.cache.host_dir(&url.host),
            encode: crate::remote::encode_component,
        })
    }

    /// The tab title of a local copy: `host: file`.
    pub fn title(&self, local: &Path) -> Option<String> {
        self.url_of(local)
            .map(|url| format!("{}: {}", url.host.label(), url.file_name()))
    }

    /// True if a local copy has changes that are not uploaded.
    pub fn is_pending(&self, local: &Path) -> bool {
        self.index.files.get(local).is_some_and(|f| f.pending)
    }

    /// The local copies with changes that are not uploaded.
    pub fn pending(&self) -> HashSet<PathBuf> {
        self.index.pending().into_iter().collect()
    }

    /// The upload of local copies: for each, the remote file and the
    /// checksum that the remote file must still have. A copy that the
    /// index does not know is a new file (it must not exist yet).
    pub fn upload_items(&self, locals: &[PathBuf], force: bool) -> Vec<UploadItem> {
        locals
            .iter()
            .filter_map(|local| {
                let url = self.url_of(local)?;
                let expected = if force {
                    None
                } else {
                    Some(
                        self.index
                            .files
                            .get(local)
                            .map_or(NO_FILE.to_owned(), |f| f.base.clone()),
                    )
                };
                Some(UploadItem {
                    local: local.clone(),
                    url,
                    expected,
                })
            })
            .collect()
    }

    /// Marks local copies as changed and not uploaded.
    pub fn mark_pending(&mut self, items: &[UploadItem]) {
        for item in items {
            let entry = self
                .index
                .files
                .entry(item.local.clone())
                .or_insert_with(|| RemoteFile {
                    url: item.url.clone(),
                    base: NO_FILE.to_owned(),
                    pending: true,
                });
            entry.pending = true;
        }
        self.save_index();
    }

    /// Shows the next question of `ssh`, if one waits. Returns true while a
    /// question is open.
    pub fn show_prompt(&mut self, ctx: &egui::Context) -> bool {
        if self.prompt.is_none()
            && let Some(prompt) = self.askpass.as_ref().and_then(AskpassServer::next_prompt)
        {
            self.prompt = Some(PromptDialog {
                prompt,
                input: String::new(),
                remember: true,
                focused: false,
            });
        }
        let Some(dialog) = &mut self.prompt else {
            return false;
        };
        if let Some(answer) = prompt_dialog(ctx, dialog)
            && let Some(dialog) = self.prompt.take()
        {
            if dialog.remember
                && dialog.prompt.kind() == PromptKind::Secret
                && let (Some(server), Some(secret)) = (&self.askpass, &answer)
            {
                server.remember(&dialog.prompt.text, secret);
            }
            dialog.prompt.answer(answer);
        }
        self.prompt.is_some()
    }

    /// The text of the open question of `ssh`, if any.
    pub fn prompt_text(&self) -> Option<&str> {
        self.prompt.as_ref().map(|p| p.prompt.text.as_str())
    }
}

/// The result of a remote job.
#[derive(Debug)]
pub enum RemoteResult {
    /// A folder was listed.
    Listed {
        /// The computer.
        host: RemoteHost,
        /// The listing, or an error message.
        result: Result<Listing, String>,
        /// True if the error is a failed connection or login.
        connect_error: bool,
    },
    /// A remote file was copied to the cache.
    Opened {
        /// The remote file.
        url: RemoteUrl,
        /// The copies, or an error message.
        result: Result<Opened, String>,
        /// True if the error is a failed connection or login.
        connect_error: bool,
    },
    /// The files of a document were compared with the robot.
    Checked {
        /// The computer.
        host: RemoteHost,
        /// The local main file of the document.
        main: PathBuf,
        /// The files, or an error message (for example the robot is off).
        result: Result<Vec<CheckedFile>, String>,
    },
    /// Local copies were uploaded.
    Uploaded {
        /// The computer.
        host: RemoteHost,
        /// The result for each copy.
        outcomes: Vec<(PathBuf, UploadOutcome)>,
        /// The result of the command after the upload, if one ran.
        restart: Option<Result<String, String>>,
    },
}

/// The copies of an opened remote file.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Opened {
    /// The local copy of the opened file.
    pub local: PathBuf,
    /// The copies that were written, with their index entries.
    pub files: Vec<(PathBuf, RemoteFile)>,
    /// Copies with changes that were not uploaded: FastTick kept them and
    /// did not overwrite them.
    pub kept: Vec<PathBuf>,
    /// Files that FastTick did not copy (too large).
    pub skipped: Vec<String>,
}

/// One local copy to upload.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct UploadItem {
    /// The local copy.
    pub local: PathBuf,
    /// The remote file.
    pub url: RemoteUrl,
    /// The checksum that the remote file must have, or `None` to overwrite
    /// it without the check.
    pub expected: Option<String>,
}

/// The result of the upload of one copy.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum UploadOutcome {
    /// Uploaded; the new checksum.
    Done(String),
    /// The remote file changed; nothing was written.
    Conflict,
    /// The upload failed.
    Failed(String),
}

/// Writes a file through a temporary file in the same folder.
fn write_file(path: &Path, bytes: &[u8]) -> std::io::Result<()> {
    if let Some(dir) = path.parent() {
        std::fs::create_dir_all(dir)?;
    }
    let tmp = path.with_extension("fasttick-part");
    std::fs::write(&tmp, bytes)?;
    std::fs::rename(tmp, path)
}

/// Lists a remote folder (a job).
pub fn list_job(config: SshConfig, host: RemoteHost, dir: String) -> RemoteResult {
    let result = ops::list_dir(&config, &host, &dir);
    RemoteResult::Listed {
        connect_error: matches!(result, Err(SshError::Connect(_))),
        result: result.map_err(|err| err.to_string()),
        host,
    }
}

/// Copies a remote file and the files that it needs to the cache (a job).
/// Copies in `pending` (changes not uploaded) are not overwritten.
pub fn open_job(
    config: SshConfig,
    cache: RemoteCache,
    pending: HashSet<PathBuf>,
    url: RemoteUrl,
) -> RemoteResult {
    let result = open_files(&config, &cache, &pending, &url);
    RemoteResult::Opened {
        connect_error: matches!(result, Err(SshError::Connect(_))),
        result: result.map_err(|err| err.to_string()),
        url,
    }
}

fn open_files(
    config: &SshConfig,
    cache: &RemoteCache,
    pending: &HashSet<PathBuf>,
    url: &RemoteUrl,
) -> Result<Opened, SshError> {
    let local = cache.local_path(url);
    if is_log_name(url.file_name()) {
        ops::fetch_to_file(config, &url.host, &url.path, &local)?;
        return Ok(Opened {
            local,
            files: Vec::new(),
            kept: Vec::new(),
            skipped: Vec::new(),
        });
    }
    let fetched = ops::collect(config, &url.host, &url.path)?;
    let mut opened = Opened {
        local,
        files: Vec::new(),
        kept: Vec::new(),
        skipped: Vec::new(),
    };
    for file in fetched {
        let file_url = RemoteUrl::new(url.host.clone(), file.path.clone());
        let path = cache.local_path(&file_url);
        match file.content {
            Ok(content) => {
                if pending.contains(&path) && path.exists() {
                    opened.kept.push(path);
                    continue;
                }
                write_file(&path, &content).map_err(|err| {
                    SshError::Other(format!("FastTick cannot write {}: {err}", path.display()))
                })?;
                opened.files.push((
                    path,
                    RemoteFile {
                        url: file_url,
                        base: file.base,
                        pending: false,
                    },
                ));
            }
            Err(FetchProblem::TooLarge(_)) => opened.skipped.push(file.path),
            Err(FetchProblem::Missing) => {}
        }
    }
    Ok(opened)
}

/// A local copy and the checksum of its file on the robot.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CheckedFile {
    /// The local copy.
    pub local: PathBuf,
    /// The checksum that FastTick knew at the start of the check.
    pub base: String,
    /// The checksum on the robot ([`NO_FILE`] if the file is gone).
    pub current: String,
}

/// A file to check: the local copy, the remote path and the checksum that
/// FastTick knows.
pub type CheckItem = (PathBuf, String, String);

/// Compares local copies with their files on one computer (a job).
pub fn check_job(
    config: SshConfig,
    host: RemoteHost,
    main: PathBuf,
    items: Vec<CheckItem>,
) -> RemoteResult {
    let paths: Vec<String> = items.iter().map(|(_, path, _)| path.clone()).collect();
    let result = ops::checksums(&config, &host, &paths)
        .map(|sums| {
            items
                .into_iter()
                .zip(sums)
                .map(|((local, _, base), current)| CheckedFile {
                    local,
                    base,
                    current,
                })
                .collect()
        })
        .map_err(|err| err.to_string());
    RemoteResult::Checked { host, main, result }
}

/// Uploads local copies to one computer, then runs the command after the
/// upload if all copies were uploaded (a job).
pub fn upload_job(
    config: SshConfig,
    host: RemoteHost,
    items: Vec<UploadItem>,
    restart: Option<String>,
) -> RemoteResult {
    let mut outcomes = Vec::with_capacity(items.len());
    let mut connection: Option<String> = None;
    for item in items {
        let outcome = if let Some(message) = &connection {
            UploadOutcome::Failed(message.clone())
        } else {
            match std::fs::read(&item.local) {
                Err(err) => UploadOutcome::Failed(err.to_string()),
                Ok(bytes) => {
                    match ops::upload(
                        &config,
                        &host,
                        &item.url.path,
                        &bytes,
                        item.expected.as_deref(),
                    ) {
                        Ok(Upload::Done { base }) => UploadOutcome::Done(base),
                        Ok(Upload::Conflict { .. }) => UploadOutcome::Conflict,
                        Err(err) => {
                            // The other uploads fail the same way: do not
                            // ask again.
                            if matches!(err, SshError::Connect(_)) {
                                connection = Some(err.to_string());
                            }
                            UploadOutcome::Failed(err.to_string())
                        }
                    }
                }
            }
        };
        outcomes.push((item.local, outcome));
    }
    let all_done = outcomes
        .iter()
        .all(|(_, o)| matches!(o, UploadOutcome::Done(_)));
    let restart = restart
        .filter(|command| all_done && !command.trim().is_empty())
        .map(|command| ops::run_command(&config, &host, &command).map_err(|err| err.to_string()));
    RemoteResult::Uploaded {
        host,
        outcomes,
        restart,
    }
}

/// The "Open from robot" dialog.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct OpenDialog {
    /// The address, for example `pi@robot.local`.
    pub address: String,
    /// The folder to show (empty: the home folder).
    pub folder: String,
    /// The command after each upload.
    pub restart_command: String,
    /// The last listing and its computer.
    pub listing: Option<(RemoteHost, Listing)>,
    /// True while a job runs.
    pub busy: bool,
    /// The last error.
    pub error: Option<String>,
    /// The selected file name of the listing.
    pub selected: Option<String>,
    /// Show all files, not only trees, projects and logs.
    pub show_all: bool,
}

impl OpenDialog {
    /// A dialog with the values of the last computer.
    pub fn new(recent: Option<&RemoteHostSettings>) -> OpenDialog {
        let mut dialog = OpenDialog::default();
        if let Some(host) = recent {
            dialog.address = host.address.clone();
            dialog.folder = host.folder.clone();
            dialog.restart_command = host.restart_command.clone();
        }
        dialog
    }

    /// The settings of the computer of the dialog, with the folder of an
    /// opened file.
    pub fn host_settings(&self, folder: &str) -> RemoteHostSettings {
        RemoteHostSettings {
            address: self.address.trim().to_owned(),
            folder: folder.to_owned(),
            restart_command: self.restart_command.trim().to_owned(),
        }
    }
}

/// What the dialog asks the app to do.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum OpenDialogAction {
    /// Nothing.
    None,
    /// Close the dialog.
    Close,
    /// List a folder.
    List(RemoteHost, String),
    /// Open a file.
    Open(RemoteUrl),
}

/// Draws the "Open from robot" dialog.
pub fn open_dialog(
    ctx: &egui::Context,
    dialog: &mut OpenDialog,
    recent: &[RemoteHostSettings],
) -> OpenDialogAction {
    let mut action = OpenDialogAction::None;
    let mut open = true;
    egui::Window::new("Open from robot")
        .id(egui::Id::new("fasttick_remote_open"))
        .open(&mut open)
        .collapsible(false)
        .resizable(true)
        .default_width(560.0)
        .anchor(egui::Align2::CENTER_CENTER, egui::Vec2::ZERO)
        .show(ctx, |ui| {
            ui.label(
                RichText::new(
                    "FastTick connects with the ssh program of this computer. Your SSH keys \
                     and ~/.ssh/config work as in a terminal.",
                )
                .small()
                .weak(),
            );
            ui.add_space(6.0);
            let mut connect = false;
            egui::Grid::new("fasttick_remote_fields")
                .num_columns(2)
                .spacing([8.0, 6.0])
                .show(ui, |ui| {
                    ui.label("Robot");
                    ui.horizontal(|ui| {
                        let response = ui.add(
                            egui::TextEdit::singleline(&mut dialog.address)
                                .hint_text("pi@robot.local")
                                .desired_width(300.0),
                        );
                        if response.lost_focus() && ui.input(|i| i.key_pressed(Key::Enter)) {
                            connect = true;
                        }
                        if !recent.is_empty() {
                            ui.menu_button("Recent", |ui| {
                                for host in recent {
                                    if ui.button(&host.address).clicked() {
                                        dialog.address = host.address.clone();
                                        dialog.folder = host.folder.clone();
                                        dialog.restart_command = host.restart_command.clone();
                                        dialog.listing = None;
                                        dialog.error = None;
                                        connect = true;
                                        ui.close();
                                    }
                                }
                            });
                        }
                    });
                    ui.end_row();
                    ui.label("Folder");
                    ui.horizontal(|ui| {
                        let response = ui.add(
                            egui::TextEdit::singleline(&mut dialog.folder)
                                .hint_text("your home folder")
                                .desired_width(300.0),
                        );
                        if response.lost_focus() && ui.input(|i| i.key_pressed(Key::Enter)) {
                            connect = true;
                        }
                        if ui
                            .add_enabled(!dialog.busy, egui::Button::new("Connect"))
                            .clicked()
                        {
                            connect = true;
                        }
                    });
                    ui.end_row();
                });
            if connect && !dialog.busy {
                match RemoteHost::parse(&dialog.address) {
                    Ok(host) => action = OpenDialogAction::List(host, dialog.folder.clone()),
                    Err(message) => dialog.error = Some(message),
                }
            }
            if dialog.busy {
                ui.horizontal(|ui| {
                    ui.spinner();
                    ui.label("Connecting\u{2026}");
                });
            }
            if let Some(error) = &dialog.error {
                ui.colored_label(ui.visuals().error_fg_color, error);
            }
            if let Some((host, listing)) = &dialog.listing {
                ui.separator();
                ui.horizontal(|ui| {
                    if ui
                        .add_enabled(listing.dir != "/" && !dialog.busy, egui::Button::new("Up"))
                        .on_hover_text("The parent folder")
                        .clicked()
                    {
                        action = OpenDialogAction::List(host.clone(), posix_parent(&listing.dir));
                    }
                    let place = format!("{host}:{}", listing.dir);
                    ui.add(egui::Label::new(
                        RichText::new(short_path(&place, 64)).monospace(),
                    ))
                    .on_hover_text(place);
                });
                let mut double = None;
                egui::ScrollArea::vertical()
                    .max_height(280.0)
                    .auto_shrink([false, true])
                    .show(ui, |ui| {
                        let visible: Vec<_> = listing
                            .entries
                            .iter()
                            .filter(|e| e.is_dir || dialog.show_all || is_openable_name(&e.name))
                            .collect();
                        if visible.is_empty() {
                            ui.label(
                                RichText::new("No trees, projects or logs in this folder.").weak(),
                            );
                        }
                        for entry in visible {
                            let label = if entry.is_dir {
                                format!("\u{25B8} {}/", entry.name)
                            } else {
                                format!("   {}", entry.name)
                            };
                            let selected = dialog.selected.as_deref() == Some(&entry.name);
                            let response = ui.add_enabled(
                                !dialog.busy && (entry.is_dir || is_openable_name(&entry.name)),
                                egui::Button::selectable(selected, label),
                            );
                            if entry.is_dir && response.clicked() {
                                action = OpenDialogAction::List(
                                    host.clone(),
                                    posix_join(&listing.dir, &entry.name),
                                );
                            } else if !entry.is_dir {
                                if response.clicked() {
                                    dialog.selected = Some(entry.name.clone());
                                }
                                if response.double_clicked() {
                                    double = Some(entry.name.clone());
                                }
                            }
                        }
                    });
                ui.checkbox(&mut dialog.show_all, "Show all files");
                if let Some(name) = double {
                    action = OpenDialogAction::Open(RemoteUrl::new(
                        host.clone(),
                        posix_join(&listing.dir, &name),
                    ));
                }
            }
            ui.separator();
            egui::CollapsingHeader::new("After a save")
                .default_open(!dialog.restart_command.is_empty())
                .show(ui, |ui| {
                    ui.label(
                        RichText::new(
                            "FastTick can run a command on the robot after each upload, for \
                             example to restart the robot program (it reads the XML when it \
                             starts). Leave it empty for no command.",
                        )
                        .small(),
                    );
                    ui.add(
                        egui::TextEdit::singleline(&mut dialog.restart_command)
                            .hint_text("systemctl --user restart my-robot")
                            .desired_width(f32::INFINITY),
                    );
                });
            ui.add_space(4.0);
            ui.horizontal(|ui| {
                let selected_url = dialog.listing.as_ref().and_then(|(host, listing)| {
                    let name = dialog.selected.as_ref()?;
                    Some(RemoteUrl::new(host.clone(), posix_join(&listing.dir, name)))
                });
                if ui
                    .add_enabled(
                        selected_url.is_some() && !dialog.busy,
                        egui::Button::new("Open"),
                    )
                    .clicked()
                    && let Some(url) = selected_url
                {
                    action = OpenDialogAction::Open(url);
                }
                if ui.button("Cancel").clicked() {
                    action = OpenDialogAction::Close;
                }
            });
        });
    if !open {
        action = OpenDialogAction::Close;
    }
    action
}

/// A long text shortened at its start: `…` and the last `max` characters
/// (the end of a path is the part that the user needs).
fn short_path(text: &str, max: usize) -> String {
    let count = text.chars().count();
    if count <= max {
        return text.to_owned();
    }
    let tail: String = text.chars().skip(count - max).collect();
    format!("\u{2026}{tail}")
}

/// The dialog of one question of `ssh`.
#[derive(Debug)]
struct PromptDialog {
    prompt: Prompt,
    input: String,
    remember: bool,
    focused: bool,
}

/// Draws the question dialog. Returns `Some(answer)` when the user answers
/// (`Some(None)` cancels).
fn prompt_dialog(ctx: &egui::Context, dialog: &mut PromptDialog) -> Option<Option<String>> {
    let mut result = None;
    let confirm = dialog.prompt.kind() == PromptKind::Confirm;
    egui::Window::new("SSH")
        .id(egui::Id::new("fasttick_ssh_prompt"))
        .collapsible(false)
        .resizable(false)
        .default_width(460.0)
        .anchor(egui::Align2::CENTER_CENTER, egui::Vec2::ZERO)
        .order(egui::Order::Foreground)
        .show(ctx, |ui| {
            ui.label(dialog.prompt.text.trim());
            ui.add_space(6.0);
            if confirm {
                ui.label(
                    RichText::new(
                        "Answer yes only if this is your robot. Then ssh keeps the key of \
                         the robot in your known hosts file and does not ask again.",
                    )
                    .small()
                    .weak(),
                );
                ui.horizontal(|ui| {
                    if ui.button("Yes").clicked() {
                        result = Some(Some("yes".to_owned()));
                    }
                    if ui.button("No").clicked() {
                        result = Some(Some("no".to_owned()));
                    }
                });
                return;
            }
            let edit = ui.add(
                egui::TextEdit::singleline(&mut dialog.input)
                    .password(true)
                    .desired_width(f32::INFINITY),
            );
            if !dialog.focused {
                edit.request_focus();
                dialog.focused = true;
            }
            let enter = edit.lost_focus() && ui.input(|i| i.key_pressed(Key::Enter));
            ui.checkbox(&mut dialog.remember, "Remember until FastTick closes");
            ui.horizontal(|ui| {
                if ui.button("OK").clicked() || enter {
                    result = Some(Some(std::mem::take(&mut dialog.input)));
                }
                if ui.button("Cancel").clicked() {
                    result = Some(None);
                }
            });
        });
    result
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn checks_start_on_focus_and_on_a_new_tab() {
        let dir = tempfile::tempdir().unwrap();
        let mut state = RemoteState::new(dir.path().to_path_buf());
        let a = Path::new("/cache/a.xml");
        let b = Path::new("/cache/b.xml");
        assert!(state.check_trigger(true, Some(a)), "a new tab");
        assert!(!state.check_trigger(true, Some(a)), "nothing changed");
        assert!(!state.check_trigger(false, Some(a)), "the focus went away");
        assert!(state.check_trigger(true, Some(a)), "the focus came back");
        assert!(state.check_trigger(true, Some(b)), "another tab");
        assert!(!state.check_trigger(true, None), "no remote tab");
        assert!(!state.check_trigger(false, None));
        assert!(
            !state.check_trigger(true, None),
            "focus without a remote tab"
        );

        assert!(state.begin_check(a));
        assert!(!state.begin_check(a), "too soon");
        assert!(state.begin_check(b));
        state.forget_checks();
        assert!(state.begin_check(a));
    }

    #[test]
    fn a_change_on_the_robot_gives_one_notice() {
        let dir = tempfile::tempdir().unwrap();
        let mut state = RemoteState::new(dir.path().to_path_buf());
        let local = PathBuf::from("/cache/main.xml");
        state.index.files.insert(
            local.clone(),
            RemoteFile {
                url: RemoteUrl::parse("ssh://robot/main.xml").unwrap(),
                base: "1 10".to_owned(),
                pending: false,
            },
        );
        let checked = |current: &str, base: &str| {
            vec![CheckedFile {
                local: local.clone(),
                base: base.to_owned(),
                current: current.to_owned(),
            }]
        };
        assert!(state.new_changes(checked("1 10", "1 10")).is_empty());
        assert_eq!(
            state.new_changes(checked("2 11", "1 10")),
            std::slice::from_ref(&local)
        );
        assert!(
            state.new_changes(checked("2 11", "1 10")).is_empty(),
            "told"
        );
        assert_eq!(
            state.new_changes(checked("3 12", "1 10")),
            std::slice::from_ref(&local)
        );
        // An upload during the check: the robot has the new base.
        state.index.files.get_mut(&local).unwrap().base = "4 13".to_owned();
        assert!(state.new_changes(checked("4 13", "3 12")).is_empty());
        // A file that FastTick does not know.
        let other = CheckedFile {
            local: PathBuf::from("/cache/other.xml"),
            base: "1".to_owned(),
            current: "2".to_owned(),
        };
        assert!(state.new_changes(vec![other]).is_empty());
    }

    #[test]
    fn long_paths_keep_their_end() {
        assert_eq!(short_path("robot:/home", 20), "robot:/home");
        assert_eq!(
            short_path("robot:/a/very/long/path/trees", 10),
            "\u{2026}path/trees"
        );
    }

    #[test]
    fn file_kinds() {
        assert!(is_log_name("run.btlog"));
        assert!(is_log_name("RUN.DB3"));
        assert!(!is_log_name("main.xml"));
        assert!(is_openable_name("main.XML"));
        assert!(is_openable_name("p.ftproj"));
        assert!(!is_openable_name("notes.txt"));
    }

    #[test]
    fn upload_items_use_the_index() {
        let dir = tempfile::tempdir().unwrap();
        let mut state = RemoteState::new(dir.path().to_path_buf());
        let known = RemoteUrl::parse("ssh://pi@robot/home/pi/a.xml").unwrap();
        let new = RemoteUrl::parse("ssh://pi@robot/home/pi/b.xml").unwrap();
        let known_local = state.cache.local_path(&known);
        state.index.files.insert(
            known_local.clone(),
            RemoteFile {
                url: known.clone(),
                base: "1 2".into(),
                pending: false,
            },
        );
        let new_local = state.cache.local_path(&new);
        let outside = dir.path().parent().unwrap().join("x.xml");
        let items = state.upload_items(&[known_local.clone(), new_local.clone(), outside], false);
        assert_eq!(items.len(), 2);
        assert_eq!(items[0].expected.as_deref(), Some("1 2"));
        assert_eq!(items[1].expected.as_deref(), Some(NO_FILE));
        assert_eq!(items[1].url, new);
        let forced = state.upload_items(std::slice::from_ref(&known_local), true);
        assert_eq!(forced[0].expected, None);
        state.mark_pending(&items);
        assert!(state.is_pending(&known_local) && state.is_pending(&new_local));
        assert_eq!(state.title(&known_local).as_deref(), Some("robot: a.xml"));
        // The index is on disk.
        assert_eq!(RemoteIndex::load(&state.cache).pending().len(), 2);
    }
}
