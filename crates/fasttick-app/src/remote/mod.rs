//! Files on another computer (for example a robot) over SSH.
//!
//! FastTick runs the `ssh` program of the system (see [`ssh`]), so the
//! keys, the `~/.ssh/config` file and the known-hosts check of the user
//! work as in a terminal. The remote computer needs an SSH server and a
//! POSIX shell with the usual tools (`cat`, `cksum`, `mktemp`, `mv`); a
//! Raspberry Pi with SSH on has them. Nothing is installed on it.
//!
//! When the user opens a remote file, FastTick copies it, the files that it
//! includes and the other tree files of its folder into a local cache
//! ([`RemoteCache`]) that keeps the folder structure of the remote
//! computer, and opens the local copy. The [`RemoteIndex`] records, for each
//! copy, the checksum (`cksum`) of the remote file at the last download or
//! upload. A save writes the local copy and then uploads it (see
//! [`ops::upload`]): the upload first checks that the remote file still
//! has that checksum, so FastTick never overwrites a change that somebody
//! else made on the remote computer without asking.
//!
//! Password and passphrase questions of `ssh` go to a dialog of FastTick
//! (see [`askpass`]).

pub mod askpass;
pub mod ops;
pub mod ssh;

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

/// The default SSH port.
pub const DEFAULT_SSH_PORT: u16 = 22;

/// The scheme of remote file addresses.
pub const URL_SCHEME: &str = "ssh://";

/// A computer that FastTick reaches with SSH: `user@host`, with an optional
/// port.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct RemoteHost {
    /// The user name. Without it, `ssh` uses its configured or local user
    /// name.
    pub user: Option<String>,
    /// The host name or IP address (an IPv6 address without brackets).
    pub host: String,
    /// The port. Without it, `ssh` uses its configured port (usually 22).
    pub port: Option<u16>,
}

impl RemoteHost {
    /// Parses `host`, `user@host`, `host:port`, `user@host:port` or
    /// `user@[ipv6]:port`. A name must not start with `-` (`ssh` would read
    /// it as an option) and must not contain white space or `/`.
    pub fn parse(text: &str) -> Result<RemoteHost, String> {
        let text = text.trim();
        if text.is_empty() {
            return Err("Type the address of the computer, for example pi@robot.local.".into());
        }
        let (user, rest) = match text.rsplit_once('@') {
            Some((user, rest)) => (Some(user), rest),
            None => (None, text),
        };
        let (host, port) = if let Some(inner) = rest.strip_prefix('[') {
            let (host, after) = inner
                .split_once(']')
                .ok_or_else(|| format!("The address {text:?} has no closing ']'."))?;
            match after {
                "" => (host, None),
                _ => match after.strip_prefix(':') {
                    Some(port) => (host, Some(port)),
                    None => return Err(format!("The address {text:?} is not valid.")),
                },
            }
        } else if rest.matches(':').count() == 1 {
            let (host, port) = rest.split_once(':').unwrap_or((rest, ""));
            (host, Some(port))
        } else {
            // No colon, or an IPv6 address without brackets (no port).
            (rest, None)
        };
        let port = match port {
            None => None,
            Some(port) => Some(
                port.parse::<u16>()
                    .ok()
                    .filter(|p| *p > 0)
                    .ok_or_else(|| format!("The port {port:?} is not a number from 1 to 65535."))?,
            ),
        };
        let check = |what: &str, value: &str| -> Result<(), String> {
            if value.is_empty() {
                return Err(format!("The {what} is empty."));
            }
            if value.starts_with('-') {
                return Err(format!("The {what} must not start with '-'."));
            }
            if value
                .chars()
                .any(|c| c.is_whitespace() || c.is_control() || matches!(c, '/' | '\\' | '\''))
            {
                return Err(format!(
                    "The {what} {value:?} has characters that are not allowed."
                ));
            }
            Ok(())
        };
        if let Some(user) = user {
            check("user name", user)?;
        }
        check("host name", host)?;
        Ok(RemoteHost {
            user: user.map(str::to_owned),
            host: host.to_owned(),
            port,
        })
    }

    /// The destination argument of `ssh`: `user@host` or `host`.
    pub fn destination(&self) -> String {
        match &self.user {
            Some(user) => format!("{user}@{}", self.host),
            None => self.host.clone(),
        }
    }

    /// A short name for tab titles: the host name.
    pub fn label(&self) -> &str {
        &self.host
    }

    /// The host part of an address, with brackets for an IPv6 address.
    fn host_part(&self) -> String {
        if self.host.contains(':') {
            format!("[{}]", self.host)
        } else {
            self.host.clone()
        }
    }
}

impl std::fmt::Display for RemoteHost {
    /// `user@host`, with `:port` if the port is set.
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        if let Some(user) = &self.user {
            write!(f, "{user}@")?;
        }
        match self.port {
            Some(port) => write!(f, "{}:{port}", self.host_part()),
            None if self.host.contains(':') => write!(f, "{}", self.host_part()),
            None => write!(f, "{}", self.host),
        }
    }
}

/// A file or folder on a remote computer: `ssh://user@host:port/path`.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct RemoteUrl {
    /// The computer.
    pub host: RemoteHost,
    /// The absolute path on the computer (POSIX).
    pub path: String,
}

impl RemoteUrl {
    /// A remote file.
    pub fn new(host: RemoteHost, path: impl Into<String>) -> RemoteUrl {
        RemoteUrl {
            host,
            path: posix_normalize(&path.into()),
        }
    }

    /// Parses `ssh://user@host:port/absolute/path`.
    pub fn parse(text: &str) -> Result<RemoteUrl, String> {
        let rest = text
            .trim()
            .strip_prefix(URL_SCHEME)
            .ok_or_else(|| format!("{text:?} does not start with {URL_SCHEME}."))?;
        // The path starts at the first '/' after the host (an IPv6 host is
        // in brackets and has no '/').
        let slash = rest
            .find('/')
            .ok_or_else(|| format!("{text:?} has no path after the computer name."))?;
        let host = RemoteHost::parse(&rest[..slash])?;
        Ok(RemoteUrl::new(host, &rest[slash..]))
    }

    /// The file name (the last part of the path).
    pub fn file_name(&self) -> &str {
        posix_file_name(&self.path)
    }
}

impl std::fmt::Display for RemoteUrl {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{URL_SCHEME}{}{}", self.host, self.path)
    }
}

/// True if a path or text is a remote file address (`ssh://...`).
pub fn is_remote_url(text: impl AsRef<std::ffi::OsStr>) -> bool {
    text.as_ref()
        .to_str()
        .is_some_and(|t| t.starts_with(URL_SCHEME))
}

/// Removes `.`, `..` and repeated `/` from an absolute POSIX path. A path
/// that does not start with `/` gets one.
pub fn posix_normalize(path: &str) -> String {
    let mut parts: Vec<&str> = Vec::new();
    for part in path.split('/') {
        match part {
            "" | "." => {}
            ".." => {
                parts.pop();
            }
            part => parts.push(part),
        }
    }
    format!("/{}", parts.join("/"))
}

/// The folder of a POSIX path (`/` for a file in the root folder).
pub fn posix_parent(path: &str) -> String {
    let path = posix_normalize(path);
    match path.rsplit_once('/') {
        Some(("", _)) | None => "/".to_owned(),
        Some((dir, _)) => dir.to_owned(),
    }
}

/// The last part of a POSIX path.
pub fn posix_file_name(path: &str) -> &str {
    path.rsplit('/').next().unwrap_or(path)
}

/// `rel` relative to the folder `dir`, or `rel` itself if it is absolute.
pub fn posix_join(dir: &str, rel: &str) -> String {
    if rel.starts_with('/') {
        posix_normalize(rel)
    } else {
        posix_normalize(&format!("{dir}/{rel}"))
    }
}

/// Quotes a word for a POSIX shell: `'text'`, with each `'` written as
/// `'\''`. The shell reads the result as exactly `text`.
pub fn shell_quote(text: &str) -> String {
    format!("'{}'", text.replace('\'', r"'\''"))
}

/// Encodes one part of a path for a local file name: `%`, path separators,
/// characters that Windows does not allow, and control characters become
/// `%XX`.
fn encode_component(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    for c in text.chars() {
        if c.is_control()
            || matches!(
                c,
                '%' | '/' | '\\' | ':' | '*' | '?' | '"' | '<' | '>' | '|'
            )
        {
            let mut buffer = [0u8; 4];
            for byte in c.encode_utf8(&mut buffer).bytes() {
                out.push_str(&format!("%{byte:02X}"));
            }
        } else {
            out.push(c);
        }
    }
    // "." and ".." are not file names.
    match out.as_str() {
        "." => "%2E".to_owned(),
        ".." => "%2E%2E".to_owned(),
        _ => out,
    }
}

/// Reverses [`encode_component`].
fn decode_component(text: &str) -> Option<String> {
    let bytes = text.as_bytes();
    let mut out = Vec::with_capacity(bytes.len());
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i] == b'%' {
            let hex = text.get(i + 1..i + 3)?;
            out.push(u8::from_str_radix(hex, 16).ok()?);
            i += 3;
        } else {
            out.push(bytes[i]);
            i += 1;
        }
    }
    String::from_utf8(out).ok()
}

/// The local folder with the copies of remote files. Each computer has a
/// folder (`user@host:port`, encoded); below it, the copies keep the
/// folder structure of the remote computer, so relative `<include>` paths
/// work.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RemoteCache {
    root: PathBuf,
}

impl RemoteCache {
    /// A cache in `root`.
    pub fn new(root: impl Into<PathBuf>) -> RemoteCache {
        RemoteCache { root: root.into() }
    }

    /// The default cache folder: `FASTTICK_REMOTE_CACHE` if it is set, else
    /// the folder `remote` in the data folder of FastTick.
    pub fn default_root() -> PathBuf {
        if let Some(dir) = std::env::var_os("FASTTICK_REMOTE_CACHE").filter(|d| !d.is_empty()) {
            return PathBuf::from(dir);
        }
        eframe::storage_dir("fasttick")
            .map(|dir| dir.join("remote"))
            .unwrap_or_else(|| std::env::temp_dir().join("fasttick-remote"))
    }

    /// The cache folder.
    pub fn root(&self) -> &Path {
        &self.root
    }

    /// The folder of one computer.
    fn host_dir(&self, host: &RemoteHost) -> PathBuf {
        let mut key = host.clone();
        key.port.get_or_insert(DEFAULT_SSH_PORT);
        self.root.join(encode_component(&key.to_string()))
    }

    /// The local copy of a remote file.
    pub fn local_path(&self, url: &RemoteUrl) -> PathBuf {
        let mut path = self.host_dir(&url.host);
        for part in url.path.split('/').filter(|p| !p.is_empty()) {
            path.push(encode_component(part));
        }
        path
    }

    /// The remote file of a local copy, or `None` if the path is not in
    /// the cache.
    pub fn url_of(&self, local: &Path) -> Option<RemoteUrl> {
        let rest = local.strip_prefix(&self.root).ok()?;
        let mut parts = rest.components().map(|c| c.as_os_str().to_str());
        let mut host = RemoteHost::parse(&decode_component(parts.next()??)?).ok()?;
        if host.port == Some(DEFAULT_SSH_PORT) {
            host.port = None;
        }
        let mut path = String::new();
        for part in parts {
            path.push('/');
            path.push_str(&decode_component(part?)?);
        }
        (!path.is_empty()).then(|| RemoteUrl::new(host, path))
    }

    /// True if a local path is in the cache.
    pub fn contains(&self, local: &Path) -> bool {
        local.starts_with(&self.root)
    }
}

/// The checksum text of a remote file that does not exist.
pub const NO_FILE: &str = "-";

/// What FastTick knows about one local copy of a remote file.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RemoteFile {
    /// The remote file.
    pub url: RemoteUrl,
    /// The output of `cksum` for the remote file at the last download or
    /// upload ([`NO_FILE`] if it did not exist).
    pub base: String,
    /// True if the local copy has changes that are not on the remote
    /// computer yet.
    pub pending: bool,
}

/// The local copies of remote files, saved as JSON in the cache folder, so
/// that a change that was not uploaded survives a restart.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct RemoteIndex {
    /// The copies, by local path.
    pub files: BTreeMap<PathBuf, RemoteFile>,
}

impl RemoteIndex {
    /// The file of the index in a cache folder.
    fn file(cache: &RemoteCache) -> PathBuf {
        cache.root().join("index.json")
    }

    /// Loads the index of a cache folder (empty if there is none).
    pub fn load(cache: &RemoteCache) -> RemoteIndex {
        std::fs::read_to_string(Self::file(cache))
            .ok()
            .and_then(|text| serde_json::from_str(&text).ok())
            .unwrap_or_default()
    }

    /// Saves the index.
    pub fn save(&self, cache: &RemoteCache) -> std::io::Result<()> {
        std::fs::create_dir_all(cache.root())?;
        let text = serde_json::to_string_pretty(self).map_err(std::io::Error::other)?;
        let path = Self::file(cache);
        let tmp = path.with_extension("json.tmp");
        std::fs::write(&tmp, text)?;
        std::fs::rename(tmp, path)
    }

    /// The local copies with changes that are not uploaded.
    pub fn pending(&self) -> Vec<PathBuf> {
        self.files
            .iter()
            .filter(|(_, f)| f.pending)
            .map(|(path, _)| path.clone())
            .collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn hosts_parse_and_print() {
        let host = RemoteHost::parse("pi@robot.local").unwrap();
        assert_eq!(host.user.as_deref(), Some("pi"));
        assert_eq!(host.host, "robot.local");
        assert_eq!(host.port, None);
        assert_eq!(host.destination(), "pi@robot.local");
        assert_eq!(host.to_string(), "pi@robot.local");
        let host = RemoteHost::parse("robot:2222").unwrap();
        assert_eq!((host.user, host.port), (None, Some(2222)));
        let host = RemoteHost::parse("me@[fe80::1]:22").unwrap();
        assert_eq!(host.host, "fe80::1");
        assert_eq!(host.to_string(), "me@[fe80::1]:22");
        let host = RemoteHost::parse("fe80::1").unwrap();
        assert_eq!((host.host.as_str(), host.port), ("fe80::1", None));
        for bad in [
            "",
            "-oProxyCommand=x",
            "pi@-x",
            "pi@robot:0",
            "pi@robot:99999",
            "two words",
            "pi@ro/bot",
            "it's@robot",
        ] {
            assert!(RemoteHost::parse(bad).is_err(), "{bad:?}");
        }
    }

    #[test]
    fn urls_parse_and_print() {
        let url = RemoteUrl::parse("ssh://pi@robot.local/home/pi/trees/./main.xml").unwrap();
        assert_eq!(url.host.destination(), "pi@robot.local");
        assert_eq!(url.path, "/home/pi/trees/main.xml");
        assert_eq!(url.file_name(), "main.xml");
        assert_eq!(
            url.to_string(),
            "ssh://pi@robot.local/home/pi/trees/main.xml"
        );
        let url = RemoteUrl::parse("ssh://robot:2222/a b/c.xml").unwrap();
        assert_eq!(url.to_string(), "ssh://robot:2222/a b/c.xml");
        assert!(RemoteUrl::parse("ssh://robot").is_err());
        assert!(RemoteUrl::parse("/home/pi/a.xml").is_err());
        assert!(is_remote_url("ssh://robot/a.xml"));
        assert!(!is_remote_url("/ssh://robot/a.xml"));
    }

    #[test]
    fn posix_paths() {
        assert_eq!(posix_normalize("/a//b/./c/../d"), "/a/b/d");
        assert_eq!(posix_normalize("/../a"), "/a");
        assert_eq!(posix_parent("/home/pi/main.xml"), "/home/pi");
        assert_eq!(posix_parent("/main.xml"), "/");
        assert_eq!(posix_join("/home/pi", "sub/x.xml"), "/home/pi/sub/x.xml");
        assert_eq!(posix_join("/home/pi", "../x.xml"), "/home/x.xml");
        assert_eq!(posix_join("/home/pi", "/opt/x.xml"), "/opt/x.xml");
        assert_eq!(posix_file_name("/home/pi/main.xml"), "main.xml");
    }

    #[test]
    fn quoting_survives_the_shell() {
        assert_eq!(shell_quote("a b"), "'a b'");
        assert_eq!(shell_quote("it's"), r"'it'\''s'");
        // The shell reads the quoted word back exactly.
        for text in ["plain", "it's", "$(rm -rf /)", "a\"b`c\\d", "new\nline", ""] {
            let output = std::process::Command::new("sh")
                .arg("-c")
                .arg(format!("printf '%s' {}", shell_quote(text)))
                .output()
                .unwrap();
            assert_eq!(String::from_utf8(output.stdout).unwrap(), text);
        }
    }

    #[test]
    fn cache_paths_round_trip() {
        let cache = RemoteCache::new("/cache");
        let url = RemoteUrl::parse("ssh://pi@robot.local/home/pi/my trees/a:b%.xml").unwrap();
        let local = cache.local_path(&url);
        assert_eq!(
            local,
            Path::new("/cache/pi@robot.local%3A22/home/pi/my trees/a%3Ab%25.xml")
        );
        assert_eq!(cache.url_of(&local), Some(url));
        let url = RemoteUrl::parse("ssh://robot:2222/x.xml").unwrap();
        assert_eq!(cache.url_of(&cache.local_path(&url)), Some(url));
        assert_eq!(cache.url_of(Path::new("/elsewhere/x.xml")), None);
        assert!(cache.contains(&cache.local_path(&RemoteUrl::parse("ssh://r/x").unwrap())));
    }

    #[test]
    fn the_index_round_trips() {
        let dir = tempfile::tempdir().unwrap();
        let cache = RemoteCache::new(dir.path());
        let mut index = RemoteIndex::default();
        let url = RemoteUrl::parse("ssh://pi@robot/home/pi/a.xml").unwrap();
        index.files.insert(
            cache.local_path(&url),
            RemoteFile {
                url,
                base: "123 45".into(),
                pending: true,
            },
        );
        index.save(&cache).unwrap();
        let loaded = RemoteIndex::load(&cache);
        assert_eq!(loaded, index);
        assert_eq!(loaded.pending().len(), 1);
    }
}
