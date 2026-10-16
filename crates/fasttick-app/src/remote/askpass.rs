//! The questions of `ssh` (password, key passphrase, unknown host key) in
//! a dialog of FastTick.
//!
//! `ssh` asks its questions through the program in `SSH_ASKPASS`. FastTick
//! sets that program to itself, with `SSH_ASKPASS_REQUIRE=force`. The
//! FastTick process that `ssh` starts ([`client_main`]) connects to the
//! [`AskpassServer`] of the running FastTick on `127.0.0.1`, sends the
//! question with a secret token, waits for the answer of the user, and
//! prints it for `ssh`.
//!
//! FastTick never writes a password to a file. With "remember", the
//! server keeps it in memory until FastTick closes and answers the same
//! question of a new `ssh` process without the dialog. If the same `ssh`
//! process asks again, the answer was wrong: the server forgets it and
//! asks the user. (The question program sends the process ID of its
//! `ssh`; on systems where it cannot, a wrong remembered password makes
//! the connection fail, and the app forgets the passwords.)

use std::collections::{HashMap, HashSet};
use std::io::{self, BufRead, BufReader, Read, Write};
use std::net::{TcpListener, TcpStream};
use std::path::PathBuf;
use std::process::Command;
use std::sync::mpsc::{self, Receiver, Sender};
use std::sync::{Arc, Mutex};
use std::time::Duration;

/// The environment variable with the address of the server.
pub const ENV_ADDRESS: &str = "FASTTICK_ASKPASS";
/// The environment variable with the token of the server.
pub const ENV_TOKEN: &str = "FASTTICK_ASKPASS_TOKEN";

/// The longest question that the server accepts.
const MAX_PROMPT: usize = 4096;
/// How long the server waits for the answer of the user.
const ANSWER_TIMEOUT: Duration = Duration::from_secs(600);

/// The settings that make `ssh` ask through FastTick.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AskpassEnv {
    /// The program that `ssh` runs for a question: the FastTick binary.
    pub program: PathBuf,
    /// The address of the server.
    pub address: String,
    /// The token of the server.
    pub token: String,
}

impl AskpassEnv {
    /// Sets the environment of an `ssh` command.
    pub fn apply(&self, command: &mut Command) {
        command
            .env("SSH_ASKPASS", &self.program)
            .env("SSH_ASKPASS_REQUIRE", "force")
            .env(ENV_ADDRESS, &self.address)
            .env(ENV_TOKEN, &self.token);
        // Versions of OpenSSH before 8.4 use SSH_ASKPASS only with DISPLAY.
        if cfg!(unix) && std::env::var_os("DISPLAY").is_none() {
            command.env("DISPLAY", ":0");
        }
    }
}

/// The kind of a question.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PromptKind {
    /// A yes or no question, for example about an unknown host key.
    Confirm,
    /// A password or a passphrase.
    Secret,
}

/// A question of `ssh` that waits for the user.
pub struct Prompt {
    /// The text of `ssh`, for example `pi@robot.local's password:`.
    pub text: String,
    reply: Sender<Option<String>>,
}

impl std::fmt::Debug for Prompt {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Prompt").field("text", &self.text).finish()
    }
}

impl Prompt {
    /// The kind of the question.
    pub fn kind(&self) -> PromptKind {
        if self.text.contains("(yes/no") {
            PromptKind::Confirm
        } else {
            PromptKind::Secret
        }
    }

    /// Answers the question; `None` cancels it (and the `ssh` command
    /// fails).
    pub fn answer(self, answer: Option<String>) {
        let _ = self.reply.send(answer);
    }
}

/// A remembered answer.
#[derive(Debug)]
struct Remembered {
    secret: String,
    /// The `ssh` processes that got the answer.
    used_by: HashSet<u32>,
}

type Memory = Arc<Mutex<HashMap<String, Remembered>>>;

/// The server that receives the questions of `ssh`.
#[derive(Debug)]
pub struct AskpassServer {
    address: String,
    token: String,
    prompts: Receiver<Prompt>,
    memory: Memory,
}

impl AskpassServer {
    /// Starts the server on `127.0.0.1`. `wake` runs when a question
    /// arrives (for example to repaint the window).
    pub fn start(wake: impl Fn() + Send + Sync + 'static) -> io::Result<AskpassServer> {
        let listener = TcpListener::bind("127.0.0.1:0")?;
        let address = listener.local_addr()?.to_string();
        let token = random_token();
        let (sender, prompts) = mpsc::channel();
        let memory: Memory = Arc::default();
        let wake: Arc<dyn Fn() + Send + Sync> = Arc::new(wake);
        let server_token = token.clone();
        let server_memory = Arc::clone(&memory);
        std::thread::Builder::new()
            .name("fasttick-askpass".into())
            .spawn(move || {
                for stream in listener.incoming() {
                    let Ok(stream) = stream else {
                        continue;
                    };
                    let token = server_token.clone();
                    let sender: Sender<Prompt> = sender.clone();
                    let memory = Arc::clone(&server_memory);
                    let wake = Arc::clone(&wake);
                    let _ = std::thread::Builder::new()
                        .name("fasttick-askpass-question".into())
                        .spawn(move || {
                            let _ = handle(stream, &token, &sender, &memory, &*wake);
                        });
                }
            })?;
        Ok(AskpassServer {
            address,
            token,
            prompts,
            memory,
        })
    }

    /// The settings for `ssh` commands, with `program` as the question
    /// program (the FastTick binary).
    pub fn env(&self, program: PathBuf) -> AskpassEnv {
        AskpassEnv {
            program,
            address: self.address.clone(),
            token: self.token.clone(),
        }
    }

    /// The next question that waits for the user, if any.
    pub fn next_prompt(&self) -> Option<Prompt> {
        self.prompts.try_recv().ok()
    }

    /// Waits for the next question (for tests).
    pub fn wait_prompt(&self, timeout: Duration) -> Option<Prompt> {
        self.prompts.recv_timeout(timeout).ok()
    }

    /// Keeps the answer to a question in memory until FastTick closes.
    pub fn remember(&self, prompt: &str, secret: &str) {
        if let Ok(mut memory) = self.memory.lock() {
            memory.insert(
                prompt.to_owned(),
                Remembered {
                    secret: secret.to_owned(),
                    used_by: HashSet::new(),
                },
            );
        }
    }

    /// Forgets all remembered answers.
    pub fn forget_all(&self) {
        if let Ok(mut memory) = self.memory.lock() {
            memory.clear();
        }
    }
}

/// One question: reads it, gets the answer, writes it back.
fn handle(
    stream: TcpStream,
    token: &str,
    sender: &Sender<Prompt>,
    memory: &Memory,
    wake: &(dyn Fn() + Send + Sync),
) -> io::Result<()> {
    stream.set_read_timeout(Some(Duration::from_secs(10)))?;
    let mut reader = BufReader::new(stream.try_clone()?);
    let mut line = String::new();
    reader.read_line(&mut line)?;
    if line.trim_end() != token {
        return Ok(());
    }
    line.clear();
    reader.read_line(&mut line)?;
    // The process ID of the ssh that asks (0: unknown).
    let asker: u32 = line.trim().parse().unwrap_or(0);
    line.clear();
    reader.read_line(&mut line)?;
    let length: usize = line
        .trim()
        .parse()
        .map_err(|_| io::Error::other("bad length"))?;
    if length > MAX_PROMPT {
        return Ok(());
    }
    let mut text = vec![0u8; length];
    reader.read_exact(&mut text)?;
    let text = String::from_utf8_lossy(&text).trim().to_owned();
    let answer = match remembered_answer(memory, &text, asker) {
        Some(secret) => Some(secret),
        None => {
            let (reply, answers) = mpsc::channel();
            sender
                .send(Prompt { text, reply })
                .map_err(|_| io::Error::other("FastTick is closing"))?;
            wake();
            answers.recv_timeout(ANSWER_TIMEOUT).ok().flatten()
        }
    };
    let mut stream = stream;
    match answer {
        Some(secret) => write!(stream, "1\n{secret}")?,
        None => writeln!(stream, "0")?,
    }
    stream.flush()
}

/// The remembered answer to a question, unless the same `ssh` process got
/// it before (then it was wrong, and FastTick forgets it).
fn remembered_answer(memory: &Memory, text: &str, asker: u32) -> Option<String> {
    let mut memory = memory.lock().ok()?;
    let entry = memory.get_mut(text)?;
    if asker != 0 && !entry.used_by.insert(asker) {
        memory.remove(text);
        return None;
    }
    Some(entry.secret.clone())
}

/// A random token (128 bits from the random keys of the standard hash
/// maps, which come from the operating system).
fn random_token() -> String {
    use std::hash::{BuildHasher, Hasher};
    let mut out = String::new();
    for i in 0..2u64 {
        let mut hasher = std::collections::hash_map::RandomState::new().build_hasher();
        hasher.write_u64(i);
        hasher.write_u128(
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map_or(0, |d| d.as_nanos()),
        );
        out.push_str(&format!("{:016x}", hasher.finish()));
    }
    out
}

/// True if this process was started by `ssh` as the question program.
pub fn is_askpass_call() -> bool {
    std::env::var_os(ENV_ADDRESS).is_some_and(|v| !v.is_empty())
        && std::env::var_os("SSH_ASKPASS").is_some()
}

/// The question program: sends the question (the arguments) to the server
/// and prints the answer. Returns the exit code: 0 with an answer, 1 if
/// the user cancelled or the server is not there.
pub fn client_main(args: &[String]) -> i32 {
    let (Some(address), Some(token)) = (
        std::env::var(ENV_ADDRESS).ok(),
        std::env::var(ENV_TOKEN).ok(),
    ) else {
        return 1;
    };
    let text = args.join(" ");
    match ask(&address, &token, &text, parent_id()) {
        Ok(Some(answer)) => {
            let mut out = io::stdout().lock();
            let _ = writeln!(out, "{answer}");
            let _ = out.flush();
            0
        }
        _ => 1,
    }
}

/// The process ID of the parent process (the `ssh` that asks), or 0.
fn parent_id() -> u32 {
    #[cfg(unix)]
    {
        std::os::unix::process::parent_id()
    }
    #[cfg(not(unix))]
    {
        0
    }
}

/// Sends a question to a server and waits for the answer. `asker` is the
/// process ID of the `ssh` that asks (0: unknown).
fn ask(address: &str, token: &str, text: &str, asker: u32) -> io::Result<Option<String>> {
    let address = address
        .parse()
        .map_err(|_| io::Error::other("bad address"))?;
    let mut stream = TcpStream::connect_timeout(&address, Duration::from_secs(5))?;
    stream.set_read_timeout(Some(ANSWER_TIMEOUT + Duration::from_secs(10)))?;
    write!(stream, "{token}\n{asker}\n{}\n{text}", text.len())?;
    stream.flush()?;
    let mut reply = String::new();
    stream.read_to_string(&mut reply)?;
    Ok(reply.strip_prefix("1\n").map(str::to_owned))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_question_goes_to_the_user_and_back() {
        let server = AskpassServer::start(|| {}).unwrap();
        let env = server.env(PathBuf::from("fasttick"));
        let (address, token) = (env.address.clone(), env.token.clone());
        let asker = std::thread::spawn(move || ask(&address, &token, "pi@robot's password:", 7));
        let prompt = server.wait_prompt(Duration::from_secs(5)).unwrap();
        assert_eq!(prompt.text, "pi@robot's password:");
        assert_eq!(prompt.kind(), PromptKind::Secret);
        prompt.answer(Some("raspberry".into()));
        assert_eq!(asker.join().unwrap().unwrap().as_deref(), Some("raspberry"));

        // Cancel.
        let (address, token) = (env.address.clone(), env.token.clone());
        let asker = std::thread::spawn(move || {
            ask(
                &address,
                &token,
                "Are you sure you want to continue connecting (yes/no/[fingerprint])?",
                7,
            )
        });
        let prompt = server.wait_prompt(Duration::from_secs(5)).unwrap();
        assert_eq!(prompt.kind(), PromptKind::Confirm);
        drop(prompt);
        assert_eq!(asker.join().unwrap().unwrap(), None);

        // A wrong token gets no answer and no dialog.
        assert_eq!(ask(&env.address, "wrong", "x", 7).unwrap(), None);
        assert!(server.wait_prompt(Duration::from_millis(200)).is_none());
    }

    #[test]
    fn a_remembered_answer_serves_new_connections_until_it_is_wrong() {
        let server = AskpassServer::start(|| {}).unwrap();
        let env = server.env(PathBuf::from("fasttick"));
        server.remember("pw:", "secret");
        // Two ssh processes (new connections): no dialog.
        for asker in [100, 101] {
            assert_eq!(
                ask(&env.address, &env.token, "pw:", asker)
                    .unwrap()
                    .as_deref(),
                Some("secret")
            );
        }
        // The same ssh asks again: the password was wrong, so the user is
        // asked, and the wrong answer is forgotten.
        let (address, token) = (env.address.clone(), env.token.clone());
        let asker = std::thread::spawn(move || ask(&address, &token, "pw:", 101));
        let prompt = server.wait_prompt(Duration::from_secs(5)).unwrap();
        prompt.answer(Some("right".into()));
        assert_eq!(asker.join().unwrap().unwrap().as_deref(), Some("right"));
        let (address, token) = (env.address.clone(), env.token.clone());
        let asker = std::thread::spawn(move || ask(&address, &token, "pw:", 102));
        let prompt = server
            .wait_prompt(Duration::from_secs(5))
            .expect("asked again");
        prompt.answer(None);
        assert_eq!(asker.join().unwrap().unwrap(), None);
    }

    #[test]
    fn tokens_are_different() {
        let a = random_token();
        assert_eq!(a.len(), 32);
        assert_ne!(a, random_token());
    }
}
