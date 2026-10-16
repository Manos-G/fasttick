//! Runs commands on a remote computer with the `ssh` program of the system.
//!
//! Each operation runs a short POSIX shell script on the remote computer
//! (`ssh host sh -c '<script>'`), with data on its standard input and
//! output. On Linux and macOS the commands share one connection (OpenSSH
//! "control master"), so a password is asked only once and each operation
//! is fast. Questions of `ssh` (password, key passphrase, unknown host key)
//! go to FastTick through [`super::askpass`].

use std::io::Write;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::Mutex;

use super::askpass::AskpassEnv;
use super::{RemoteHost, shell_quote};

/// The seconds that `ssh` waits for a connection.
pub const CONNECT_TIMEOUT_SECONDS: u32 = 10;

/// The seconds that a shared connection stays open after its last use.
pub const CONTROL_PERSIST_SECONDS: u32 = 300;

/// How FastTick runs `ssh`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SshConfig {
    /// The `ssh` program: `FASTTICK_SSH` if it is set, else `ssh`.
    pub program: PathBuf,
    /// A configuration file for `ssh` (`-F`) instead of the user's own:
    /// `FASTTICK_SSH_CONFIG` if it is set.
    pub config_file: Option<PathBuf>,
    /// The dialog for the questions of `ssh`. Without it, `ssh` runs in
    /// batch mode: it fails instead of asking.
    pub askpass: Option<AskpassEnv>,
    /// A private folder for the sockets of shared connections, or `None`
    /// for a new connection for each command (Windows).
    pub control_dir: Option<PathBuf>,
}

impl SshConfig {
    /// The configuration from the environment. `control_dir` is used only
    /// on Unix, and only if its socket paths are short enough.
    pub fn from_env(askpass: Option<AskpassEnv>, control_dir: Option<PathBuf>) -> SshConfig {
        let env_path = |name: &str| {
            std::env::var_os(name)
                .filter(|v| !v.is_empty())
                .map(PathBuf::from)
        };
        SshConfig {
            program: env_path("FASTTICK_SSH").unwrap_or_else(|| PathBuf::from("ssh")),
            config_file: env_path("FASTTICK_SSH_CONFIG"),
            askpass,
            control_dir: control_dir.filter(|dir| usable_control_dir(dir)),
        }
    }
}

/// True if sockets in `dir` have short enough paths (a Unix socket path has
/// at most about 104 bytes; `%C` is 40 characters) and the folder can be
/// made private.
fn usable_control_dir(dir: &Path) -> bool {
    if !cfg!(unix) || dir.as_os_str().len() + 42 > 100 {
        return false;
    }
    make_private_dir(dir).is_ok()
}

#[cfg(unix)]
fn make_private_dir(dir: &Path) -> std::io::Result<()> {
    use std::os::unix::fs::{DirBuilderExt, PermissionsExt};
    std::fs::DirBuilder::new()
        .recursive(true)
        .mode(0o700)
        .create(dir)?;
    std::fs::set_permissions(dir, std::fs::Permissions::from_mode(0o700))
}

#[cfg(not(unix))]
fn make_private_dir(dir: &Path) -> std::io::Result<()> {
    std::fs::create_dir_all(dir)
}

/// An error of a remote operation.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SshError {
    /// The `ssh` program could not start.
    NotFound(String),
    /// `ssh` could not connect or log in.
    Connect(String),
    /// The remote command failed.
    Command {
        /// The exit code.
        code: Option<i32>,
        /// The error output.
        message: String,
    },
    /// The output was not as expected, or a local file operation failed.
    Other(String),
}

impl std::fmt::Display for SshError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            SshError::NotFound(message) => write!(
                f,
                "FastTick cannot start the ssh program ({message}). Install OpenSSH."
            ),
            SshError::Connect(message) if message.is_empty() => {
                write!(f, "FastTick cannot connect to the computer.")
            }
            SshError::Connect(message) => {
                write!(f, "FastTick cannot connect to the computer: {message}")
            }
            SshError::Command { code, message } => {
                let message = if message.is_empty() {
                    "no message"
                } else {
                    message
                };
                match code {
                    Some(code) => write!(f, "The command failed (exit code {code}): {message}"),
                    None => write!(f, "The command failed: {message}"),
                }
            }
            SshError::Other(message) => write!(f, "{message}"),
        }
    }
}

impl std::error::Error for SshError {}

/// The output of a remote command.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Output {
    /// The standard output.
    pub stdout: Vec<u8>,
    /// The error output, as text.
    pub stderr: String,
}

/// The `ssh` options for one computer, without the destination.
fn base_command(config: &SshConfig, host: &RemoteHost) -> Command {
    let mut command = Command::new(&config.program);
    if let Some(file) = &config.config_file {
        command.arg("-F").arg(file);
    }
    command.args([
        "-T",
        "-o",
        &format!("ConnectTimeout={CONNECT_TIMEOUT_SECONDS}"),
        "-o",
        "ServerAliveInterval=15",
        "-o",
        "ServerAliveCountMax=3",
    ]);
    if let Some(port) = host.port {
        command.arg("-p").arg(port.to_string());
    }
    match &config.askpass {
        Some(askpass) => askpass.apply(&mut command),
        None => {
            command.args(["-o", "BatchMode=yes"]);
        }
    }
    hide_console(&mut command);
    command
}

#[cfg(windows)]
fn hide_console(command: &mut Command) {
    use std::os::windows::process::CommandExt;
    // CREATE_NO_WINDOW: no console window for each command.
    command.creation_flags(0x0800_0000);
}

#[cfg(not(windows))]
fn hide_console(_command: &mut Command) {}

/// The socket of the shared connection to a computer.
fn control_path(dir: &Path) -> String {
    format!("ControlPath={}", dir.join("%C").display())
}

/// One shared connection start at a time (two operations must not start
/// two master connections to the same computer).
static MASTER_LOCK: Mutex<()> = Mutex::new(());

/// Starts the shared connection to a computer if it is not running.
fn ensure_master(config: &SshConfig, host: &RemoteHost, dir: &Path) -> Result<(), SshError> {
    let _guard = MASTER_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    make_private_dir(dir).map_err(|err| SshError::Other(err.to_string()))?;
    let path = control_path(dir);
    let check = base_command(config, host)
        .args(["-o", &path, "-O", "check", "--", &host.destination()])
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status()
        .map_err(|err| SshError::NotFound(err.to_string()))?;
    if check.success() {
        return Ok(());
    }
    // The master goes to the background after the login (-f). Its error
    // output goes to a file, not to a pipe: a pipe would stay open as long
    // as the master runs.
    let log_path = dir.join(format!("master-{}.log", std::process::id()));
    let log = std::fs::File::create(&log_path).map_err(|err| SshError::Other(err.to_string()))?;
    let status = base_command(config, host)
        .args([
            "-M",
            "-N",
            "-f",
            "-o",
            "ControlMaster=yes",
            "-o",
            &path,
            "-o",
            &format!("ControlPersist={CONTROL_PERSIST_SECONDS}"),
            "--",
            &host.destination(),
        ])
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(log)
        .status()
        .map_err(|err| SshError::NotFound(err.to_string()))?;
    let message = std::fs::read_to_string(&log_path).unwrap_or_default();
    let _ = std::fs::remove_file(&log_path);
    if status.success() {
        Ok(())
    } else {
        Err(SshError::Connect(clean_message(&message)))
    }
}

/// Closes the shared connection to a computer, if there is one.
pub fn close_master(config: &SshConfig, host: &RemoteHost) {
    if let Some(dir) = &config.control_dir {
        let _ = base_command(config, host)
            .args([
                "-o",
                &control_path(dir),
                "-O",
                "exit",
                "--",
                &host.destination(),
            ])
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .status();
    }
}

/// The command that runs `script` with `sh` on the computer.
fn script_command(
    config: &SshConfig,
    host: &RemoteHost,
    script: &str,
) -> Result<Command, SshError> {
    let mut command = base_command(config, host);
    if let Some(dir) = &config.control_dir {
        ensure_master(config, host, dir)?;
        command.args(["-o", "ControlMaster=no", "-o", &control_path(dir)]);
    }
    command
        .arg("--")
        .arg(host.destination())
        .arg(format!("sh -c {}", shell_quote(script)));
    Ok(command)
}

/// Runs `script` with `sh` on the computer, with `stdin` as its input.
/// A non-zero exit code is an error ([`SshError::Command`]); the exit code
/// 255 is a connection error.
pub fn run(
    config: &SshConfig,
    host: &RemoteHost,
    script: &str,
    stdin: &[u8],
) -> Result<Output, SshError> {
    let mut child = script_command(config, host, script)?
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .map_err(|err| SshError::NotFound(err.to_string()))?;
    // Write the input on another thread: the command can fill its output
    // pipe before it reads all of its input.
    let input = child.stdin.take();
    let data = stdin.to_vec();
    let writer = std::thread::spawn(move || {
        if let Some(mut input) = input {
            let _ = input.write_all(&data);
        }
    });
    let output = child
        .wait_with_output()
        .map_err(|err| SshError::Other(err.to_string()))?;
    let _ = writer.join();
    let stderr = String::from_utf8_lossy(&output.stderr).into_owned();
    match output.status.code() {
        Some(0) => Ok(Output {
            stdout: output.stdout,
            stderr,
        }),
        Some(255) | None => Err(SshError::Connect(clean_message(&stderr))),
        Some(code) => Err(SshError::Command {
            code: Some(code),
            message: clean_message(&stderr),
        }),
    }
}

/// Runs `script` on the computer and writes its output to a local file
/// (through a temporary file, so the file is complete or not there).
pub fn run_to_file(
    config: &SshConfig,
    host: &RemoteHost,
    script: &str,
    out: &Path,
) -> Result<(), SshError> {
    if let Some(dir) = out.parent() {
        std::fs::create_dir_all(dir).map_err(|err| SshError::Other(err.to_string()))?;
    }
    let tmp = out.with_extension("part");
    let file = std::fs::File::create(&tmp).map_err(|err| SshError::Other(err.to_string()))?;
    let result = script_command(config, host, script)?
        .stdin(Stdio::null())
        .stdout(file)
        .stderr(Stdio::piped())
        .output();
    let output = match result {
        Ok(output) => output,
        Err(err) => {
            let _ = std::fs::remove_file(&tmp);
            return Err(SshError::NotFound(err.to_string()));
        }
    };
    let stderr = String::from_utf8_lossy(&output.stderr).into_owned();
    match output.status.code() {
        Some(0) => std::fs::rename(&tmp, out).map_err(|err| SshError::Other(err.to_string())),
        code => {
            let _ = std::fs::remove_file(&tmp);
            Err(match code {
                Some(255) | None => SshError::Connect(clean_message(&stderr)),
                code => SshError::Command {
                    code,
                    message: clean_message(&stderr),
                },
            })
        }
    }
}

/// The useful part of an error output of `ssh`: the lines without the
/// warnings about known hosts, joined.
fn clean_message(stderr: &str) -> String {
    stderr
        .lines()
        .map(str::trim)
        .filter(|line| !line.is_empty())
        .filter(|line| !line.starts_with("Warning: Permanently added"))
        .collect::<Vec<_>>()
        .join(" ")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn messages_drop_known_host_warnings() {
        assert_eq!(
            clean_message(
                "Warning: Permanently added 'x' (ED25519) to the list of known hosts.\n\
                 Permission denied (publickey).\n"
            ),
            "Permission denied (publickey)."
        );
    }

    #[test]
    fn the_command_has_the_options_and_the_quoted_script() {
        let config = SshConfig {
            program: PathBuf::from("ssh"),
            config_file: Some(PathBuf::from("/cfg")),
            askpass: None,
            control_dir: None,
        };
        let host = RemoteHost::parse("pi@robot:2222").unwrap();
        let command = script_command(&config, &host, "echo 'hi'").unwrap();
        let args: Vec<String> = command
            .get_args()
            .map(|a| a.to_string_lossy().into_owned())
            .collect();
        assert_eq!(&args[..2], ["-F", "/cfg"]);
        assert!(args.contains(&"BatchMode=yes".to_owned()));
        let port = args.iter().position(|a| a == "-p").unwrap();
        assert_eq!(args[port + 1], "2222");
        let end = args.iter().position(|a| a == "--").unwrap();
        assert_eq!(args[end + 1], "pi@robot");
        assert_eq!(args[end + 2], r"sh -c 'echo '\''hi'\'''");
    }

    #[test]
    fn a_missing_program_is_reported() {
        let config = SshConfig {
            program: PathBuf::from("/nonexistent/ssh"),
            config_file: None,
            askpass: None,
            control_dir: None,
        };
        let host = RemoteHost::parse("robot").unwrap();
        let error = run(&config, &host, "true", b"").unwrap_err();
        assert!(matches!(error, SshError::NotFound(_)), "{error:?}");
        assert!(error.to_string().contains("Install OpenSSH"));
    }
}
