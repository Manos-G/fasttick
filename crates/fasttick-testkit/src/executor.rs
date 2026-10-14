//! Start and stop of the `fasttick_executor` program.

use std::ffi::OsString;
use std::fmt;
use std::io::{self, BufRead, BufReader, Read};
use std::path::PathBuf;
use std::process::{Child, ExitStatus, Stdio};
use std::sync::{Arc, Condvar, Mutex, MutexGuard, PoisonError};
use std::thread::JoinHandle;
use std::time::{Duration, Instant};

use crate::harness::{Harness, harness};

/// Time that [`Executor::stop`] waits after SIGTERM before SIGKILL.
const STOP_TIMEOUT: Duration = Duration::from_secs(10);

/// Poll period while the code waits for the child process.
const POLL_PERIOD: Duration = Duration::from_millis(10);

/// Command-line options of `fasttick_executor`.
///
/// [`ExecutorOptions::new`] gives the defaults. The defaults are the same as
/// the defaults of the program, with two differences:
///
/// - `port` is 0 (no publisher), because a test must use a port from
///   [`free_port_pair`](crate::free_port_pair).
/// - `exit_with_parent` is `true`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ExecutorOptions {
    /// The XML file. A relative path is relative to
    /// [`Harness::trees_dir`].
    pub xml: PathBuf,
    /// The tree to execute (`--tree`). `None`: `main_tree_to_execute`, else
    /// the first tree of the file.
    pub tree: Option<String>,
    /// The publisher port (`--port`). The publisher also uses `port + 1`.
    /// 0: no publisher.
    pub port: u16,
    /// Path of a FileLogger2 log (`--btlog`). The extension must be `.btlog`.
    pub btlog: Option<PathBuf>,
    /// Path of a SqliteLogger log (`--sqlite`). The extension must be `.db3`
    /// or `.btdb`.
    pub sqlite: Option<PathBuf>,
    /// Sleep between ticks, in milliseconds (`--tick-ms`).
    pub tick_ms: u64,
    /// Number of runs to completion (`--loops`). 0: no limit.
    pub loops: u64,
    /// Time limit in milliseconds (`--duration-ms`). 0: no limit.
    pub duration_ms: u64,
    /// Heartbeat timeout of the publisher in milliseconds (`--heartbeat-ms`).
    /// `None`: the BT.CPP default (5000 ms).
    pub heartbeat_ms: Option<u64>,
    /// Stop the executor cleanly when the test process ends
    /// (`--exit-with-parent`). The default is `true`. Then a test process
    /// that is killed (for example by a CI time limit) does not leave an
    /// executor that runs for ever.
    pub exit_with_parent: bool,
    /// Maximum time to wait for the `READY` line.
    pub ready_timeout: Duration,
}

impl ExecutorOptions {
    /// Gives the default options for an XML file.
    pub fn new(xml: impl Into<PathBuf>) -> ExecutorOptions {
        ExecutorOptions {
            xml: xml.into(),
            tree: None,
            port: 0,
            btlog: None,
            sqlite: None,
            tick_ms: 20,
            loops: 0,
            duration_ms: 0,
            heartbeat_ms: None,
            exit_with_parent: true,
            ready_timeout: Duration::from_secs(30),
        }
    }

    /// Gives the command-line arguments for these options. The XML path is
    /// used as it is.
    pub fn args(&self) -> Vec<OsString> {
        let mut args: Vec<OsString> = vec!["--xml".into(), self.xml.clone().into_os_string()];
        if let Some(tree) = &self.tree {
            args.push("--tree".into());
            args.push(tree.into());
        }
        args.push("--port".into());
        args.push(self.port.to_string().into());
        if let Some(btlog) = &self.btlog {
            args.push("--btlog".into());
            args.push(btlog.clone().into_os_string());
        }
        if let Some(sqlite) = &self.sqlite {
            args.push("--sqlite".into());
            args.push(sqlite.clone().into_os_string());
        }
        args.push("--tick-ms".into());
        args.push(self.tick_ms.to_string().into());
        args.push("--loops".into());
        args.push(self.loops.to_string().into());
        args.push("--duration-ms".into());
        args.push(self.duration_ms.to_string().into());
        if let Some(heartbeat_ms) = self.heartbeat_ms {
            args.push("--heartbeat-ms".into());
            args.push(heartbeat_ms.to_string().into());
        }
        if self.exit_with_parent {
            args.push("--exit-with-parent".into());
        }
        args
    }
}

/// A signal for the executor.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Signal {
    /// SIGTERM. The executor stops cleanly.
    Term,
    /// SIGINT. The executor stops cleanly.
    Int,
    /// SIGKILL. The process ends at once.
    Kill,
}

impl Signal {
    /// The signal name for the `kill` command.
    pub fn name(self) -> &'static str {
        match self {
            Signal::Term => "TERM",
            Signal::Int => "INT",
            Signal::Kill => "KILL",
        }
    }
}

impl fmt::Display for Signal {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "SIG{}", self.name())
    }
}

/// Output of the child process, filled by the reader threads.
#[derive(Debug, Default)]
struct Output {
    stdout: Vec<String>,
    stdout_closed: bool,
    stderr: String,
}

#[derive(Debug, Default)]
struct Shared {
    output: Mutex<Output>,
    changed: Condvar,
}

impl Shared {
    fn lock(&self) -> MutexGuard<'_, Output> {
        self.output.lock().unwrap_or_else(PoisonError::into_inner)
    }

    /// See [`Executor::wait_for_stdout`].
    fn wait_for_line(
        &self,
        timeout: Duration,
        mut predicate: impl FnMut(&str) -> bool,
    ) -> io::Result<String> {
        let deadline = Instant::now() + timeout;
        let mut checked = 0;
        let mut output = self.lock();
        loop {
            if let Some(line) = output
                .stdout
                .iter()
                .skip(checked)
                .find(|line| predicate(line))
            {
                return Ok(line.clone());
            }
            checked = output.stdout.len();
            let failure = if output.stdout_closed {
                Some((io::ErrorKind::UnexpectedEof, "stdout closed"))
            } else if Instant::now() >= deadline {
                Some((io::ErrorKind::TimedOut, "time-out"))
            } else {
                None
            };
            if let Some((kind, what)) = failure {
                return Err(io::Error::new(
                    kind,
                    format!(
                        "{what} before the expected stdout line (waited at most \
                         {timeout:?}). stdout: {:?}. stderr:\n{}",
                        output.stdout, output.stderr
                    ),
                ));
            }
            let wait = deadline.saturating_duration_since(Instant::now());
            output = self
                .changed
                .wait_timeout(output, wait)
                .unwrap_or_else(PoisonError::into_inner)
                .0;
        }
    }
}

/// A running `fasttick_executor` process.
///
/// Drop kills the process (SIGKILL) if it still runs.
#[derive(Debug)]
pub struct Executor {
    child: Child,
    port: u16,
    shared: Arc<Shared>,
    readers: Vec<JoinHandle<()>>,
    status: Option<ExitStatus>,
}

impl Executor {
    /// Builds the harness if necessary, starts the executor and waits for
    /// the `READY` line.
    ///
    /// The process gets `LD_LIBRARY_PATH` with the BT.CPP library directory.
    /// The function fails if the harness is not available, if the process
    /// ends before `READY`, or if `READY` does not come within
    /// [`ExecutorOptions::ready_timeout`]. The error text contains the
    /// stderr of the process.
    pub fn spawn(options: ExecutorOptions) -> io::Result<Executor> {
        let harness =
            harness().map_err(|reason| io::Error::new(io::ErrorKind::NotFound, reason))?;
        Executor::spawn_with(harness, options)
    }

    /// Same as [`Executor::spawn`], with a given harness.
    pub fn spawn_with(harness: &Harness, mut options: ExecutorOptions) -> io::Result<Executor> {
        if options.xml.is_relative() {
            options.xml = harness.tree(&options.xml);
        }
        let mut child = harness
            .command(&harness.executor_path)
            .args(options.args())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()?;

        let shared = Arc::new(Shared::default());
        let mut readers = Vec::with_capacity(2);
        if let Some(stdout) = child.stdout.take() {
            let shared = Arc::clone(&shared);
            readers.push(std::thread::spawn(move || read_stdout(stdout, &shared)));
        }
        if let Some(stderr) = child.stderr.take() {
            let shared = Arc::clone(&shared);
            readers.push(std::thread::spawn(move || read_stderr(stderr, &shared)));
        }

        let mut executor = Executor {
            child,
            port: 0,
            shared,
            readers,
            status: None,
        };
        let port = executor.wait_ready(options.ready_timeout)?;
        if options.port != 0 && port != options.port {
            return Err(io::Error::other(format!(
                "the executor reported port {port}, expected {}",
                options.port
            )));
        }
        executor.port = port;
        Ok(executor)
    }

    /// The port from the `READY` line (0: no publisher).
    pub fn port(&self) -> u16 {
        self.port
    }

    /// The process ID.
    pub fn pid(&self) -> u32 {
        self.child.id()
    }

    /// All stdout lines up to now.
    pub fn stdout_lines(&self) -> Vec<String> {
        self.shared.lock().stdout.clone()
    }

    /// All stderr text up to now.
    pub fn stderr(&self) -> String {
        self.shared.lock().stderr.clone()
    }

    /// True if the executor wrote `DONE`.
    pub fn saw_done(&self) -> bool {
        self.shared.lock().stdout.iter().any(|line| line == "DONE")
    }

    /// The root status of each completed run, from the
    /// `RESULT loop=<n> status=<STATUS>` lines (for example `"SUCCESS"`).
    pub fn results(&self) -> Vec<String> {
        self.shared
            .lock()
            .stdout
            .iter()
            .filter_map(|line| parse_result_line(line))
            .collect()
    }

    /// Waits for a stdout line that matches `predicate`, for at most
    /// `timeout`. Gives the first line that matches. Lines that came
    /// before the call count too.
    ///
    /// The function gives a `TimedOut` error after the timeout, and an
    /// `UnexpectedEof` error if the process closes stdout before a line
    /// matches. The error text contains the stdout lines and the stderr
    /// text.
    ///
    /// For example, to wait for the end of the first run:
    /// `executor.wait_for_stdout(timeout, |line| line.starts_with("RESULT loop=1 "))`.
    pub fn wait_for_stdout(
        &self,
        timeout: Duration,
        predicate: impl FnMut(&str) -> bool,
    ) -> io::Result<String> {
        self.shared.wait_for_line(timeout, predicate)
    }

    /// True if the process has not ended.
    pub fn is_running(&mut self) -> io::Result<bool> {
        Ok(self.try_status()?.is_none())
    }

    /// Sends a signal to the process. Does nothing if the process has ended.
    ///
    /// The function runs `kill -<SIGNAL> <pid>`, because this crate has no
    /// unsafe code.
    pub fn send_signal(&mut self, signal: Signal) -> io::Result<()> {
        if self.try_status()?.is_some() {
            return Ok(());
        }
        // The process is not reaped yet, so the PID still belongs to it.
        let status = std::process::Command::new("kill")
            .arg(format!("-{}", signal.name()))
            .arg(self.child.id().to_string())
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .status()?;
        if status.success() || self.try_status()?.is_some() {
            Ok(())
        } else {
            Err(io::Error::other(format!(
                "kill -{} failed: {status}",
                signal.name()
            )))
        }
    }

    /// Sends SIGTERM and waits for the end of the process.
    ///
    /// If the process does not end within 10 s, the function kills it and
    /// gives a `TimedOut` error.
    pub fn stop(&mut self) -> io::Result<ExitStatus> {
        self.stop_with(Signal::Term)
    }

    /// Sends a signal and waits for the end of the process, as
    /// [`Executor::stop`] does.
    pub fn stop_with(&mut self, signal: Signal) -> io::Result<ExitStatus> {
        if let Some(status) = self.try_status()? {
            return Ok(status);
        }
        self.send_signal(signal)?;
        match self.wait_exit(STOP_TIMEOUT) {
            Ok(status) => Ok(status),
            Err(error) if error.kind() == io::ErrorKind::TimedOut => {
                self.kill_and_reap();
                Err(io::Error::new(
                    io::ErrorKind::TimedOut,
                    format!(
                        "the executor did not end within {STOP_TIMEOUT:?} after {signal}; \
                         it was killed. stderr:\n{}",
                        self.stderr()
                    ),
                ))
            }
            Err(error) => Err(error),
        }
    }

    /// Waits for the end of the process, for at most `timeout`. Gives a
    /// `TimedOut` error if the process still runs; the process then
    /// continues.
    pub fn wait_exit(&mut self, timeout: Duration) -> io::Result<ExitStatus> {
        let deadline = Instant::now() + timeout;
        loop {
            if let Some(status) = self.try_status()? {
                self.join_readers();
                return Ok(status);
            }
            if Instant::now() >= deadline {
                return Err(io::Error::new(
                    io::ErrorKind::TimedOut,
                    format!("the executor did not end within {timeout:?}"),
                ));
            }
            std::thread::sleep(POLL_PERIOD);
        }
    }

    /// Waits for a normal end: the process writes `DONE` and ends with exit
    /// code 0 within `timeout`.
    ///
    /// Gives an error (with the stderr text) for a time-out, a failure exit
    /// code or a missing `DONE` line.
    pub fn wait_done(&mut self, timeout: Duration) -> io::Result<ExitStatus> {
        let status = self.wait_exit(timeout)?;
        if !status.success() {
            return Err(io::Error::other(format!(
                "the executor failed ({status}). stderr:\n{}",
                self.stderr()
            )));
        }
        if !self.saw_done() {
            return Err(io::Error::other(format!(
                "the executor ended ({status}) without DONE. stdout: {:?}. stderr:\n{}",
                self.stdout_lines(),
                self.stderr()
            )));
        }
        Ok(status)
    }

    fn try_status(&mut self) -> io::Result<Option<ExitStatus>> {
        if self.status.is_none() {
            self.status = self.child.try_wait()?;
        }
        Ok(self.status)
    }

    fn wait_ready(&mut self, timeout: Duration) -> io::Result<u16> {
        let deadline = Instant::now() + timeout;
        loop {
            if let Some(port) = self.ready_port() {
                return Ok(port);
            }
            let exited = self.try_status()?;
            let closed = self.shared.lock().stdout_closed;
            if exited.is_some() || closed {
                // A short run can end before this function sees READY. Let
                // the readers take all the output, then look again.
                let status = match exited {
                    Some(status) => status,
                    None => self.wait_exit(Duration::from_secs(5))?,
                };
                self.join_readers();
                if let Some(port) = self.ready_port() {
                    return Ok(port);
                }
                return Err(io::Error::other(format!(
                    "the executor ended ({status}) before READY. stderr:\n{}",
                    self.stderr()
                )));
            }
            let now = Instant::now();
            if now >= deadline {
                self.kill_and_reap();
                return Err(io::Error::new(
                    io::ErrorKind::TimedOut,
                    format!(
                        "no READY line within {timeout:?}; the executor was killed. stderr:\n{}",
                        self.stderr()
                    ),
                ));
            }
            let output = self.shared.lock();
            if output
                .stdout
                .iter()
                .all(|line| parse_ready_line(line).is_none())
            {
                let wait = (deadline - now).min(Duration::from_millis(50));
                drop(
                    self.shared
                        .changed
                        .wait_timeout(output, wait)
                        .unwrap_or_else(PoisonError::into_inner),
                );
            }
        }
    }

    fn ready_port(&self) -> Option<u16> {
        self.shared
            .lock()
            .stdout
            .iter()
            .find_map(|line| parse_ready_line(line))
    }

    fn kill_and_reap(&mut self) {
        if self.status.is_none() {
            let _ = self.child.kill();
            self.status = self.child.wait().ok();
        }
        self.join_readers();
    }

    fn join_readers(&mut self) {
        for reader in self.readers.drain(..) {
            let _ = reader.join();
        }
    }
}

impl Drop for Executor {
    fn drop(&mut self) {
        if !matches!(self.try_status(), Ok(Some(_))) {
            self.kill_and_reap();
        }
        self.join_readers();
    }
}

/// Reads stdout line by line until the end. Bytes that are not valid UTF-8
/// become U+FFFD: a bad byte must not stop the reader, because the pipe
/// must stay drained.
fn read_stdout(stdout: impl Read, shared: &Shared) {
    let mut reader = BufReader::new(stdout);
    let mut buffer = Vec::new();
    loop {
        buffer.clear();
        match reader.read_until(b'\n', &mut buffer) {
            Ok(0) => break,
            Ok(_) => {
                let line = String::from_utf8_lossy(&buffer).trim_end().to_owned();
                shared.lock().stdout.push(line);
                shared.changed.notify_all();
            }
            Err(error) if error.kind() == io::ErrorKind::Interrupted => continue,
            Err(_) => break,
        }
    }
    shared.lock().stdout_closed = true;
    shared.changed.notify_all();
}

fn read_stderr(mut stderr: impl Read, shared: &Shared) {
    let mut buffer = [0_u8; 4096];
    loop {
        match stderr.read(&mut buffer) {
            Ok(0) => break,
            Ok(count) => shared
                .lock()
                .stderr
                .push_str(&String::from_utf8_lossy(&buffer[..count])),
            Err(error) if error.kind() == io::ErrorKind::Interrupted => continue,
            Err(_) => break,
        }
    }
}

/// Parses `READY port=<port>`. Gives the port.
pub(crate) fn parse_ready_line(line: &str) -> Option<u16> {
    line.trim_end().strip_prefix("READY port=")?.parse().ok()
}

/// Parses `RESULT loop=<n> status=<STATUS>`. Gives the status text.
pub(crate) fn parse_result_line(line: &str) -> Option<String> {
    let mut words = line.split_whitespace();
    if words.next()? != "RESULT" {
        return None;
    }
    words.next()?.strip_prefix("loop=")?.parse::<u64>().ok()?;
    let status = words.next()?.strip_prefix("status=")?;
    if words.next().is_some() || status.is_empty() {
        return None;
    }
    Some(status.to_owned())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ready_line_parsing() {
        assert_eq!(parse_ready_line("READY port=1667"), Some(1667));
        assert_eq!(parse_ready_line("READY port=0\r"), Some(0));
        assert_eq!(parse_ready_line("READY port="), None);
        assert_eq!(parse_ready_line("READY port=70000"), None);
        assert_eq!(parse_ready_line("READY port=-1"), None);
        assert_eq!(parse_ready_line(" READY port=1"), None);
        assert_eq!(parse_ready_line("DONE"), None);
    }

    #[test]
    fn result_line_parsing() {
        assert_eq!(
            parse_result_line("RESULT loop=1 status=SUCCESS"),
            Some("SUCCESS".to_owned())
        );
        assert_eq!(
            parse_result_line("RESULT loop=12 status=FAILURE"),
            Some("FAILURE".to_owned())
        );
        assert_eq!(parse_result_line("RESULT loop=x status=SUCCESS"), None);
        assert_eq!(parse_result_line("RESULT loop=1"), None);
        assert_eq!(parse_result_line("RESULT loop=1 status="), None);
        assert_eq!(parse_result_line("RESULT loop=1 status=A extra"), None);
        assert_eq!(parse_result_line("READY port=1"), None);
        assert_eq!(parse_result_line(""), None);
    }

    #[test]
    fn default_options_give_the_expected_arguments() {
        let options = ExecutorOptions::new("/trees/main.xml");
        let args: Vec<String> = options
            .args()
            .into_iter()
            .map(|arg| arg.into_string().unwrap())
            .collect();
        assert_eq!(
            args,
            [
                "--xml",
                "/trees/main.xml",
                "--port",
                "0",
                "--tick-ms",
                "20",
                "--loops",
                "0",
                "--duration-ms",
                "0",
                "--exit-with-parent"
            ]
        );

        let mut options = ExecutorOptions::new("/trees/main.xml");
        options.exit_with_parent = false;
        assert!(!options.args().iter().any(|arg| arg == "--exit-with-parent"));
    }

    #[test]
    fn all_options_give_arguments() {
        let options = ExecutorOptions {
            xml: PathBuf::from("a.xml"),
            tree: Some("Second".to_owned()),
            port: 1700,
            btlog: Some(PathBuf::from("/tmp/x.btlog")),
            sqlite: Some(PathBuf::from("/tmp/x.db3")),
            tick_ms: 5,
            loops: 2,
            duration_ms: 1000,
            heartbeat_ms: Some(300),
            exit_with_parent: true,
            ready_timeout: Duration::from_secs(1),
        };
        let args: Vec<String> = options
            .args()
            .into_iter()
            .map(|arg| arg.into_string().unwrap())
            .collect();
        assert_eq!(
            args,
            [
                "--xml",
                "a.xml",
                "--tree",
                "Second",
                "--port",
                "1700",
                "--btlog",
                "/tmp/x.btlog",
                "--sqlite",
                "/tmp/x.db3",
                "--tick-ms",
                "5",
                "--loops",
                "2",
                "--duration-ms",
                "1000",
                "--heartbeat-ms",
                "300",
                "--exit-with-parent"
            ]
        );
    }

    #[test]
    fn signal_names() {
        assert_eq!(Signal::Term.name(), "TERM");
        assert_eq!(Signal::Int.name(), "INT");
        assert_eq!(Signal::Kill.name(), "KILL");
        assert_eq!(Signal::Term.to_string(), "SIGTERM");
    }

    #[test]
    fn reader_threads_collect_output() {
        let shared = Shared::default();
        read_stdout(
            &b"READY port=5\nRESULT loop=1 status=SUCCESS\r\nDONE"[..],
            &shared,
        );
        read_stderr(&b"warning\n"[..], &shared);
        let output = shared.lock();
        assert_eq!(
            output.stdout,
            ["READY port=5", "RESULT loop=1 status=SUCCESS", "DONE"]
        );
        assert!(output.stdout_closed);
        assert_eq!(output.stderr, "warning\n");
    }

    #[test]
    fn reader_keeps_going_after_invalid_utf8() {
        let shared = Shared::default();
        read_stdout(&b"READY port=1\n\xff\xfe bad\nDONE\n"[..], &shared);
        read_stderr(&b"bad \xc3 byte"[..], &shared);
        let output = shared.lock();
        assert_eq!(
            output.stdout,
            ["READY port=1", "\u{fffd}\u{fffd} bad", "DONE"]
        );
        assert!(output.stdout_closed);
        assert_eq!(output.stderr, "bad \u{fffd} byte");
    }

    #[test]
    fn reader_handles_empty_and_unterminated_output() {
        let shared = Shared::default();
        read_stdout(&b""[..], &shared);
        assert!(shared.lock().stdout.is_empty());
        assert!(shared.lock().stdout_closed);

        let shared = Shared::default();
        read_stdout(&b"\n\nlast"[..], &shared);
        assert_eq!(shared.lock().stdout, ["", "", "last"]);
    }

    #[test]
    fn wait_for_line_sees_old_and_new_lines() {
        let shared = Arc::new(Shared::default());
        shared.lock().stdout.push("READY port=0".to_owned());
        let line = shared
            .wait_for_line(Duration::from_secs(5), |line| line.starts_with("READY"))
            .unwrap();
        assert_eq!(line, "READY port=0");

        let writer = {
            let shared = Arc::clone(&shared);
            std::thread::spawn(move || {
                std::thread::sleep(Duration::from_millis(50));
                read_stdout(&b"RESULT loop=1 status=SUCCESS\nDONE\n"[..], &shared);
            })
        };
        let line = shared
            .wait_for_line(Duration::from_secs(10), |line| line == "DONE")
            .unwrap();
        assert_eq!(line, "DONE");
        writer.join().unwrap();
    }

    #[test]
    fn wait_for_line_fails_on_timeout_and_on_eof() {
        let shared = Shared::default();
        shared.lock().stdout.push("READY port=0".to_owned());
        shared.lock().stderr.push_str("some error text");
        let start = Instant::now();
        let error = shared
            .wait_for_line(Duration::from_millis(100), |line| line == "DONE")
            .unwrap_err();
        assert_eq!(error.kind(), io::ErrorKind::TimedOut);
        assert!(start.elapsed() >= Duration::from_millis(100));
        let text = error.to_string();
        assert!(text.contains("READY port=0"), "{text}");
        assert!(text.contains("some error text"), "{text}");

        shared.lock().stdout_closed = true;
        let error = shared
            .wait_for_line(Duration::from_secs(60), |line| line == "DONE")
            .unwrap_err();
        assert_eq!(error.kind(), io::ErrorKind::UnexpectedEof);

        // A zero timeout still checks the lines that are present.
        let line = shared
            .wait_for_line(Duration::ZERO, |line| line.starts_with("READY"))
            .unwrap();
        assert_eq!(line, "READY port=0");
    }
}
