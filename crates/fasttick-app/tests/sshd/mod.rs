//! A private SSH server for the tests of remote files.
//!
//! [`TestSshd::start`] runs the OpenSSH server of the system as the current
//! user, on a free port of 127.0.0.1, with its own host key and the test
//! keys in its authorized keys. The tests connect with the `ssh` client
//! and a configuration file of their own (`ssh -F`), so the user's SSH
//! files are not used.
//!
//! If `sshd`, `ssh` or `ssh-keygen` is not installed, the tests skip.
//! With `FASTTICK_REQUIRE_SSHD=1` (CI) they fail instead.

#![allow(dead_code)]

use std::net::{TcpListener, TcpStream};
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::time::{Duration, Instant};

use fasttick_app::remote::RemoteHost;
use fasttick_app::remote::askpass::AskpassEnv;
use fasttick_app::remote::ssh::SshConfig;

/// The host alias with a key without passphrase and accept-new host keys.
pub const HOST: &str = "fasttick-test";
/// The host alias with a key with the passphrase [`PASSPHRASE`] and
/// host key questions.
pub const HOST_ASK: &str = "fasttick-test-ask";
/// The host alias with a key that the server does not accept.
pub const HOST_BAD_KEY: &str = "fasttick-test-bad";
/// The passphrase of the key of [`HOST_ASK`].
pub const PASSPHRASE: &str = "correct horse";

/// A running SSH server. It stops when the value is dropped.
pub struct TestSshd {
    dir: tempfile::TempDir,
    child: Child,
    /// The port.
    pub port: u16,
}

fn find_program(name: &str, extra: &[&str]) -> Option<PathBuf> {
    let from_path = std::env::var_os("PATH").and_then(|paths| {
        std::env::split_paths(&paths)
            .map(|dir| dir.join(name))
            .find(|p| p.is_file())
    });
    from_path.or_else(|| extra.iter().map(PathBuf::from).find(|p| p.is_file()))
}

fn skip_or_fail(reason: &str) -> Option<TestSshd> {
    if std::env::var("FASTTICK_REQUIRE_SSHD").as_deref() == Ok("1") {
        panic!("FASTTICK_REQUIRE_SSHD=1, but {reason}");
    }
    eprintln!("skipped: {reason}");
    None
}

fn keygen(keygen: &Path, file: &Path, passphrase: &str) {
    let status = Command::new(keygen)
        .args([
            "-q",
            "-t",
            "ed25519",
            "-C",
            "fasttick-test",
            "-N",
            passphrase,
            "-f",
        ])
        .arg(file)
        .stdin(Stdio::null())
        .status()
        .expect("ssh-keygen runs");
    assert!(status.success(), "ssh-keygen failed");
}

fn user_name() -> String {
    let output = Command::new("id").arg("-un").output().expect("id runs");
    String::from_utf8_lossy(&output.stdout).trim().to_owned()
}

impl TestSshd {
    /// Starts a server, or returns `None` (the test skips) if OpenSSH is not
    /// installed.
    pub fn start() -> Option<TestSshd> {
        let Some(sshd) = find_program("sshd", &["/usr/sbin/sshd", "/usr/local/sbin/sshd"]) else {
            return skip_or_fail("sshd is not installed");
        };
        let Some(ssh_keygen) = find_program("ssh-keygen", &[]) else {
            return skip_or_fail("ssh-keygen is not installed");
        };
        if find_program("ssh", &[]).is_none() {
            return skip_or_fail("ssh is not installed");
        }
        let dir = tempfile::tempdir().unwrap();
        let path = |name: &str| dir.path().join(name);
        keygen(&ssh_keygen, &path("host_ed25519"), "");
        keygen(&ssh_keygen, &path("id_test"), "");
        keygen(&ssh_keygen, &path("id_ask"), PASSPHRASE);
        keygen(&ssh_keygen, &path("id_bad"), "");
        let mut authorized = std::fs::read_to_string(path("id_test.pub")).unwrap();
        authorized.push_str(&std::fs::read_to_string(path("id_ask.pub")).unwrap());
        std::fs::write(path("authorized_keys"), authorized).unwrap();

        let port = {
            let listener = TcpListener::bind("127.0.0.1:0").unwrap();
            listener.local_addr().unwrap().port()
        };
        std::fs::write(
            path("sshd_config"),
            format!(
                "Port {port}\nListenAddress 127.0.0.1\nHostKey {host_key}\n\
                 AuthorizedKeysFile {authorized}\nPubkeyAuthentication yes\n\
                 PasswordAuthentication no\nKbdInteractiveAuthentication no\n\
                 UsePAM no\nStrictModes no\nPermitRootLogin yes\n\
                 PidFile {pid}\nLogLevel ERROR\n",
                host_key = path("host_ed25519").display(),
                authorized = path("authorized_keys").display(),
                pid = path("sshd.pid").display(),
            ),
        )
        .unwrap();
        let user = user_name();
        let host_block = |alias: &str, key: &str, check: &str, known: &str| {
            format!(
                "Host {alias}\n  HostName 127.0.0.1\n  Port {port}\n  User {user}\n  \
                 IdentityFile {key}\n  IdentitiesOnly yes\n  IdentityAgent none\n  \
                 UserKnownHostsFile {known}\n  GlobalKnownHostsFile /dev/null\n  \
                 StrictHostKeyChecking {check}\n  LogLevel ERROR\n",
                key = path(key).display(),
                known = path(known).display(),
            )
        };
        let config = [
            host_block(HOST, "id_test", "accept-new", "known_hosts"),
            host_block(HOST_ASK, "id_ask", "ask", "known_hosts_ask"),
            host_block(HOST_BAD_KEY, "id_bad", "accept-new", "known_hosts_bad"),
        ]
        .join("\n");
        std::fs::write(path("ssh_config"), config).unwrap();

        // The server as root needs its privilege separation folder.
        if user == "root" {
            let _ = std::fs::create_dir_all("/run/sshd");
        }
        let child = Command::new(&sshd)
            .args(["-D", "-e", "-f"])
            .arg(path("sshd_config"))
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .spawn()
            .expect("sshd starts");
        let server = TestSshd { dir, child, port };
        let start = Instant::now();
        while TcpStream::connect(("127.0.0.1", port)).is_err() {
            assert!(
                start.elapsed() < Duration::from_secs(10),
                "sshd did not start on port {port}"
            );
            std::thread::sleep(Duration::from_millis(50));
        }
        Some(server)
    }

    /// A folder for test files (the "robot" files).
    pub fn files(&self) -> PathBuf {
        let dir = self.dir.path().join("robot");
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    /// The client configuration file.
    pub fn client_config(&self) -> PathBuf {
        self.dir.path().join("ssh_config")
    }

    /// A host alias of the client configuration.
    pub fn host(&self, alias: &str) -> RemoteHost {
        RemoteHost::parse(alias).unwrap()
    }

    /// An `ssh` configuration for the tests, with its own folder for shared
    /// connections (`name`), and the question dialog if `askpass` is set.
    pub fn ssh_config(&self, name: &str, askpass: Option<AskpassEnv>) -> SshConfig {
        let control = self.dir.path().join(format!("c-{name}"));
        SshConfig {
            program: PathBuf::from("ssh"),
            config_file: Some(self.client_config()),
            askpass,
            control_dir: Some(control),
        }
    }
}

impl Drop for TestSshd {
    fn drop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}
