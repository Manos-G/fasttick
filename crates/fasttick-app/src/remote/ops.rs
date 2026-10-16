//! The operations on a remote computer. Each one runs one POSIX shell
//! script with [`super::ssh::run`]. All paths go into the scripts through
//! [`shell_quote`], never as raw text.

use std::collections::BTreeSet;

use fasttick_core::bt::BtFile;
use fasttick_core::project::ProjectFile;

use super::ssh::{self, SshConfig, SshError};
use super::{NO_FILE, RemoteHost, posix_join, posix_parent, shell_quote};

/// The largest tree, model or project file that FastTick copies.
pub const MAX_FILE: u64 = 64 * 1024 * 1024;

/// The most files of one folder that FastTick copies with an opened file.
pub const MAX_FOLDER_FILES: usize = 200;

/// The most rounds of include search when FastTick opens a file.
const MAX_INCLUDE_ROUNDS: usize = 16;

/// An entry of a remote folder.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Entry {
    /// The name.
    pub name: String,
    /// True for a folder.
    pub is_dir: bool,
}

/// The content of a remote folder.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Listing {
    /// The absolute path of the folder. A link in the path stays a link, so
    /// that the paths of the files are the ones that the user chose.
    pub dir: String,
    /// The folders and files, folders first, then by name. Hidden entries
    /// (names that start with `.`) are not in the list.
    pub entries: Vec<Entry>,
}

/// Lists a remote folder. An empty `dir` is the home folder of the user.
pub fn list_dir(config: &SshConfig, host: &RemoteHost, dir: &str) -> Result<Listing, SshError> {
    let cd = if dir.trim().is_empty() {
        "cd || exit 3".to_owned()
    } else {
        format!("cd -- {} || exit 3", shell_quote(dir))
    };
    let script = format!(
        "{cd}\npwd\nfor f in *; do\n  [ -e \"$f\" ] || continue\n  \
         if [ -d \"$f\" ]; then printf 'd\\t%s\\n' \"$f\"; else printf 'f\\t%s\\n' \"$f\"; fi\ndone\n"
    );
    let output = ssh::run(config, host, &script, b"").map_err(|err| match err {
        SshError::Command { code: Some(3), .. } => SshError::Other(format!(
            "The folder {dir:?} does not exist, or you cannot open it."
        )),
        err => err,
    })?;
    parse_listing(&String::from_utf8_lossy(&output.stdout))
}

/// Reads the output of the list script.
fn parse_listing(text: &str) -> Result<Listing, SshError> {
    let mut lines = text.lines();
    let dir = lines
        .next()
        .filter(|d| d.starts_with('/'))
        .ok_or_else(|| SshError::Other("The computer sent no folder name.".into()))?
        .to_owned();
    let mut entries: Vec<Entry> = lines
        .filter_map(|line| {
            let (kind, name) = line.split_once('\t')?;
            (!name.is_empty()).then(|| Entry {
                name: name.to_owned(),
                is_dir: kind == "d",
            })
        })
        .collect();
    entries.sort_by(|a, b| {
        b.is_dir
            .cmp(&a.is_dir)
            .then_with(|| a.name.to_lowercase().cmp(&b.name.to_lowercase()))
    });
    Ok(Listing { dir, entries })
}

/// Why a file was not copied.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum FetchProblem {
    /// The file does not exist or cannot be read.
    Missing,
    /// The file is larger than the limit (its size).
    TooLarge(u64),
}

/// A remote file that FastTick read.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Fetched {
    /// The remote path.
    pub path: String,
    /// The content, or why there is none.
    pub content: Result<Vec<u8>, FetchProblem>,
    /// The `cksum` of the file ([`NO_FILE`] if it is missing).
    pub base: String,
}

/// Reads remote files (one connection for all). Each file is first copied
/// to a temporary file on the remote computer, so that its size, its
/// checksum and its content belong to the same version.
pub fn fetch(
    config: &SshConfig,
    host: &RemoteHost,
    paths: &[String],
    max_size: u64,
) -> Result<Vec<Fetched>, SshError> {
    if paths.is_empty() {
        return Ok(Vec::new());
    }
    let list: Vec<String> = paths.iter().map(|p| shell_quote(p)).collect();
    let script = format!(
        "t=$(mktemp) || exit 6\ntrap 'rm -f \"$t\"' EXIT\n\
         for p in {}; do\n  \
         if [ -f \"$p\" ] && cp -- \"$p\" \"$t\" 2>/dev/null; then\n    \
         s=$(wc -c < \"$t\" | tr -d ' ')\n    \
         if [ \"$s\" -gt {max_size} ]; then printf 'B\\n%s\\n' \"$s\"; continue; fi\n    \
         c=$(cksum < \"$t\")\n    \
         printf 'F\\n%s\\n%s\\n' \"$s\" \"$c\"\n    \
         cat \"$t\"\n  \
         else printf 'M\\n'; fi\n\
         done\n",
        list.join(" ")
    );
    let output = ssh::run(config, host, &script, b"")?;
    parse_fetch(paths, &output.stdout)
}

/// Reads the output of the fetch script.
fn parse_fetch(paths: &[String], mut data: &[u8]) -> Result<Vec<Fetched>, SshError> {
    let bad = || SshError::Other("The computer sent data that FastTick cannot read.".into());
    let line = |data: &mut &[u8]| -> Result<String, SshError> {
        let end = data.iter().position(|b| *b == b'\n').ok_or_else(bad)?;
        let text = String::from_utf8_lossy(&data[..end]).trim().to_owned();
        *data = &data[end + 1..];
        Ok(text)
    };
    let mut out = Vec::with_capacity(paths.len());
    for path in paths {
        let kind = line(&mut data)?;
        let fetched = match kind.as_str() {
            "F" => {
                let size: usize = line(&mut data)?.parse().map_err(|_| bad())?;
                let base = line(&mut data)?;
                if data.len() < size {
                    return Err(bad());
                }
                let content = data[..size].to_vec();
                data = &data[size..];
                Fetched {
                    path: path.clone(),
                    content: Ok(content),
                    base,
                }
            }
            "B" => Fetched {
                path: path.clone(),
                content: Err(FetchProblem::TooLarge(
                    line(&mut data)?.parse().map_err(|_| bad())?,
                )),
                base: NO_FILE.to_owned(),
            },
            "M" => Fetched {
                path: path.clone(),
                content: Err(FetchProblem::Missing),
                base: NO_FILE.to_owned(),
            },
            _ => return Err(bad()),
        };
        out.push(fetched);
    }
    Ok(out)
}

/// The checksums (`cksum`) of remote files, in the order of `paths`:
/// [`NO_FILE`] for a file that does not exist. Nothing is copied.
pub fn checksums(
    config: &SshConfig,
    host: &RemoteHost,
    paths: &[String],
) -> Result<Vec<String>, SshError> {
    if paths.is_empty() {
        return Ok(Vec::new());
    }
    let list: Vec<String> = paths.iter().map(|p| shell_quote(p)).collect();
    let script = format!(
        "for p in {}; do\n  \
         c={no_file}\n  \
         if [ -f \"$p\" ]; then c=$(cksum < \"$p\") || c={no_file}; fi\n  \
         printf '%s\\n' \"$c\"\n\
         done\n",
        list.join(" "),
        no_file = shell_quote(NO_FILE),
    );
    let output = ssh::run(config, host, &script, b"")?;
    let text = String::from_utf8_lossy(&output.stdout);
    let sums: Vec<String> = text.lines().map(|l| l.trim().to_owned()).collect();
    if sums.len() != paths.len() {
        return Err(SshError::Other(
            "The computer sent an answer that FastTick cannot read.".into(),
        ));
    }
    Ok(sums)
}

/// Copies one remote file (for example a large log) to a local file.
pub fn fetch_to_file(
    config: &SshConfig,
    host: &RemoteHost,
    path: &str,
    local: &std::path::Path,
) -> Result<(), SshError> {
    let script = format!("exec cat -- {}", shell_quote(path));
    ssh::run_to_file(config, host, &script, local)
}

/// The result of an upload.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Upload {
    /// The file is on the remote computer; this is its new `cksum`.
    Done {
        /// The new checksum.
        base: String,
    },
    /// The remote file changed after the last download or upload; nothing
    /// was written.
    Conflict {
        /// The checksum of the remote file now ([`NO_FILE`] if it is gone).
        current: String,
    },
}

/// Writes a remote file. With `expected`, the remote file must still have
/// that checksum ([`NO_FILE`]: it must not exist), else nothing is written
/// and the result is [`Upload::Conflict`]. The new content goes to a
/// temporary file in the same folder, which then replaces the file (a
/// reader never sees a half-written file). The file keeps its permissions
/// (GNU and BSD `stat`);
/// a link stays a link (the file it points to changes).
pub fn upload(
    config: &SshConfig,
    host: &RemoteHost,
    path: &str,
    content: &[u8],
    expected: Option<&str>,
) -> Result<Upload, SshError> {
    let expected = expected.map(shell_quote).unwrap_or_else(|| "''".to_owned());
    let script = format!(
        "p={path}\ne={expected}\n\
         if [ -L \"$p\" ]; then p=$(readlink -f -- \"$p\") || exit 7; fi\n\
         if [ -n \"$e\" ]; then\n  \
         if [ -f \"$p\" ]; then c=$(cksum < \"$p\"); else c={no_file}; fi\n  \
         if [ \"$c\" != \"$e\" ]; then printf 'CONFLICT\\n%s\\n' \"$c\"; cat > /dev/null; exit 0; fi\n\
         fi\n\
         d=$(dirname -- \"$p\"); b=$(basename -- \"$p\")\n\
         mkdir -p -- \"$d\" || exit 4\n\
         t=\"$d/.$b.fasttick-$$.tmp\"\n\
         cat > \"$t\" || {{ rm -f -- \"$t\"; exit 4; }}\n\
         if [ -f \"$p\" ]; then m=$(stat -c %a -- \"$p\" 2>/dev/null || stat -f %Lp -- \"$p\" 2>/dev/null) && chmod \"$m\" \"$t\" 2>/dev/null; fi\n\
         mv -f -- \"$t\" \"$p\" || {{ rm -f -- \"$t\"; exit 5; }}\n\
         printf 'OK\\n'\n\
         cksum < \"$p\"\n",
        path = shell_quote(path),
        no_file = shell_quote(NO_FILE),
    );
    let output = ssh::run(config, host, &script, content).map_err(|err| match err {
        SshError::Command {
            code: Some(4 | 5),
            message,
        } => SshError::Other(format!("FastTick cannot write {path:?}: {message}")),
        err => err,
    })?;
    let text = String::from_utf8_lossy(&output.stdout);
    let mut lines = text.lines();
    match (lines.next(), lines.next()) {
        (Some("OK"), Some(base)) => Ok(Upload::Done {
            base: base.trim().to_owned(),
        }),
        (Some("CONFLICT"), Some(current)) => Ok(Upload::Conflict {
            current: current.trim().to_owned(),
        }),
        _ => Err(SshError::Other(
            "The computer sent an answer that FastTick cannot read.".into(),
        )),
    }
}

/// Runs a command of the user on the remote computer (for example a
/// restart of the robot program). Returns its output.
pub fn run_command(
    config: &SshConfig,
    host: &RemoteHost,
    command: &str,
) -> Result<String, SshError> {
    let output = ssh::run(config, host, &format!("{command}\n"), b"")?;
    let mut text = String::from_utf8_lossy(&output.stdout).into_owned();
    if !output.stderr.trim().is_empty() {
        text.push_str(&output.stderr);
    }
    Ok(text.trim().to_owned())
}

/// The file extensions that FastTick copies with an opened file: tree and
/// model files, and projects.
fn is_tree_file(name: &str) -> bool {
    let lower = name.to_ascii_lowercase();
    lower.ends_with(".xml") || lower.ends_with(".ftproj")
}

/// The remote files that a file needs: the files it includes (an XML file)
/// or the files of the project (a `.ftproj` file). Includes with
/// `ros_pkg` are not followed (the package folders are on the robot).
fn needed_files(path: &str, content: &[u8]) -> Vec<String> {
    let dir = posix_parent(path);
    let text = String::from_utf8_lossy(content);
    if path.to_ascii_lowercase().ends_with(".ftproj") {
        return ProjectFile::from_json(&text)
            .map(|project| {
                project
                    .files
                    .iter()
                    .chain(&project.model_files)
                    .map(|file| posix_join(&dir, file))
                    .collect()
            })
            .unwrap_or_default();
    }
    match BtFile::parse(&text, None) {
        Ok(file) => file
            .index()
            .includes
            .iter()
            .filter(|include| include.ros_pkg.is_none())
            .filter_map(|include| include.path.as_deref())
            .map(|include| posix_join(&dir, include))
            .collect(),
        Err(_) => Vec::new(),
    }
}

/// Reads a remote file with the files that it needs: the other tree files
/// of its folder (so that FastTick can offer them for missing SubTrees),
/// the files that it includes (also in other folders, and the files that
/// they include), or the files of a project. The opened file is first in
/// the result. Files that are missing or too large are in the result with
/// their problem.
pub fn collect(
    config: &SshConfig,
    host: &RemoteHost,
    main: &str,
) -> Result<Vec<Fetched>, SshError> {
    let main = super::posix_normalize(main);
    let dir = posix_parent(&main);
    let listing = list_dir(config, host, &dir)?;
    let mut wanted: Vec<String> = vec![main.clone()];
    wanted.extend(
        listing
            .entries
            .iter()
            .filter(|e| !e.is_dir && is_tree_file(&e.name))
            .map(|e| posix_join(&listing.dir, &e.name))
            .filter(|path| *path != main)
            .take(MAX_FOLDER_FILES),
    );
    let mut seen: BTreeSet<String> = wanted.iter().cloned().collect();
    let mut out: Vec<Fetched> = Vec::new();
    for _ in 0..MAX_INCLUDE_ROUNDS {
        if wanted.is_empty() {
            break;
        }
        let fetched = fetch(config, host, &wanted, MAX_FILE)?;
        wanted = Vec::new();
        for file in &fetched {
            if let Ok(content) = &file.content {
                for needed in needed_files(&file.path, content) {
                    if seen.insert(needed.clone()) {
                        wanted.push(needed);
                    }
                }
            }
        }
        out.extend(fetched);
    }
    match out.first().map(|f| &f.content) {
        Some(Ok(_)) => Ok(out),
        Some(Err(FetchProblem::TooLarge(size))) => Err(SshError::Other(format!(
            "{main} is too large ({} MiB).",
            size / (1024 * 1024)
        ))),
        _ => Err(SshError::Other(format!(
            "{main} does not exist on the computer, or you cannot read it."
        ))),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn listings_parse_with_folders_first() {
        let listing =
            parse_listing("/home/pi\nf\tmain.xml\nd\ttrees\nf\tA.xml\nd\tBuild\n").unwrap();
        assert_eq!(listing.dir, "/home/pi");
        let names: Vec<&str> = listing.entries.iter().map(|e| e.name.as_str()).collect();
        assert_eq!(names, ["Build", "trees", "A.xml", "main.xml"]);
        assert!(parse_listing("").is_err());
    }

    #[test]
    fn fetch_output_parses() {
        let paths = [
            "/a.xml".to_owned(),
            "/b.xml".to_owned(),
            "/c.xml".to_owned(),
        ];
        // The content follows its size line directly, without a newline.
        let data = b"F\n5\n123 5\nhelloM\nB\n99999999\n";
        let fetched = parse_fetch(&paths, data).unwrap();
        assert_eq!(fetched[0].content.as_deref(), Ok(&b"hello"[..]));
        assert_eq!(fetched[0].base, "123 5");
        assert_eq!(fetched[1].content, Err(FetchProblem::Missing));
        assert_eq!(fetched[2].content, Err(FetchProblem::TooLarge(99_999_999)));
        // A short content is an error, not a panic.
        assert!(parse_fetch(&paths[..1], b"F\n10\n1 10\nabc").is_err());
    }

    #[test]
    fn needed_files_follow_includes_and_projects() {
        let xml = br#"<root BTCPP_format="4">
            <include path="sub/a.xml"/>
            <include path="/opt/trees/b.xml"/>
            <include ros_pkg="pkg" path="c.xml"/>
        </root>"#;
        assert_eq!(
            needed_files("/home/pi/trees/main.xml", xml),
            ["/home/pi/trees/sub/a.xml", "/opt/trees/b.xml"]
        );
        let project = br#"{"version": 1, "name": "p", "files": ["main.xml", "../lib/x.xml"],
            "model_files": ["models.xml"]}"#;
        assert_eq!(
            needed_files("/home/pi/trees/p.ftproj", project),
            [
                "/home/pi/trees/main.xml",
                "/home/pi/lib/x.xml",
                "/home/pi/trees/models.xml"
            ]
        );
        assert!(needed_files("/x.xml", b"not xml").is_empty());
    }
}
