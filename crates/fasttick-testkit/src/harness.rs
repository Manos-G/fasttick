//! Build of the C++ test harness with CMake.

use std::ffi::OsString;
use std::fmt;
use std::fs::{self, File};
use std::io;
use std::path::{Path, PathBuf};
use std::process::{Command, Output, Stdio};
use std::sync::OnceLock;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use crate::prefix::{self, REQUIRE_ENV};

/// Number of output lines that a build error keeps.
const LOG_TAIL_LINES: usize = 60;

/// The built test harness.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Harness {
    /// The BT.CPP install prefix that the harness links.
    pub prefix: PathBuf,
    /// The CMake build directory.
    pub build_dir: PathBuf,
    /// Path of the `fasttick_executor` program. It is a copy in
    /// `<build_dir>/bin/<stamp>/` that never changes, also when another
    /// process builds the harness again.
    pub executor_path: PathBuf,
    /// Path of the `fasttick_dump_models` program (a copy, as
    /// [`Harness::executor_path`]).
    pub dump_models_path: PathBuf,
    /// The directory with the sample trees (`test_harness/trees`).
    pub trees_dir: PathBuf,
    /// The library directory of the prefix. The executor needs it in
    /// `LD_LIBRARY_PATH`.
    pub lib_dir: PathBuf,
}

impl Harness {
    /// Gives the path of a sample tree, relative to [`Harness::trees_dir`].
    pub fn tree(&self, relative: impl AsRef<Path>) -> PathBuf {
        self.trees_dir.join(relative)
    }

    /// Makes a command for a harness program. The command has
    /// `LD_LIBRARY_PATH` set to [`Harness::lib_dir`] (followed by the old
    /// value) and stdin connected to null.
    pub fn command(&self, program: impl AsRef<Path>) -> Command {
        let mut command = Command::new(program.as_ref());
        command
            .env(
                "LD_LIBRARY_PATH",
                ld_library_path(&self.lib_dir, std::env::var_os("LD_LIBRARY_PATH")),
            )
            .stdin(Stdio::null());
        command
    }

    /// Runs `fasttick_dump_models` and gives its stdout.
    ///
    /// With `include_builtin`, the output also has the built-in BT.CPP nodes.
    pub fn dump_models(&self, include_builtin: bool) -> io::Result<String> {
        let mut command = self.command(&self.dump_models_path);
        if include_builtin {
            command.arg("--include-builtin");
        }
        let output = command.output()?;
        if !output.status.success() {
            return Err(io::Error::other(format!(
                "fasttick_dump_models failed ({}): {}",
                output.status,
                String::from_utf8_lossy(&output.stderr)
            )));
        }
        String::from_utf8(output.stdout)
            .map_err(|error| io::Error::new(io::ErrorKind::InvalidData, error))
    }
}

/// Why the harness is not available.
#[derive(Debug, Clone, PartialEq, Eq)]
#[non_exhaustive]
pub enum SkipReason {
    /// No standard prefix contains BT.CPP 4.x, and `FASTTICK_BTCPP_PREFIX`
    /// is not set. Each entry is a prefix and the reason why it is not valid.
    PrefixNotFound {
        /// The prefixes that the search checked.
        searched: Vec<(PathBuf, String)>,
    },
    /// `FASTTICK_BTCPP_PREFIX` names a directory that is not a BT.CPP 4.x
    /// install prefix.
    InvalidPrefix {
        /// The prefix from the environment variable.
        prefix: PathBuf,
        /// Why the prefix is not valid.
        reason: String,
    },
    /// The `cmake` program cannot run.
    CmakeNotFound {
        /// The error from the start of `cmake --version`.
        error: String,
    },
    /// The CMake configure step failed (for example no C++ compiler, or
    /// `find_package(behaviortree_cpp)` failed).
    ConfigureFailed {
        /// The BT.CPP prefix.
        prefix: PathBuf,
        /// The last lines of the CMake output.
        log: String,
    },
    /// The build failed. BT.CPP is present, so this is a defect in the
    /// harness or an incompatible BT.CPP version.
    BuildFailed {
        /// The BT.CPP prefix.
        prefix: PathBuf,
        /// The last lines of the build output.
        log: String,
    },
    /// A file-system operation failed.
    Io {
        /// What the harness tried to do.
        context: String,
        /// The error text.
        error: String,
    },
}

impl SkipReason {
    /// True if a test must fail instead of a skip.
    ///
    /// - A build failure is always fatal: BT.CPP is present, so the harness
    ///   code or the BT.CPP version has a problem.
    /// - A configure failure is fatal if `FASTTICK_BTCPP_PREFIX` is set: the
    ///   user asked for this prefix, so the harness must work with it.
    /// - All reasons are fatal if `FASTTICK_REQUIRE_HARNESS=1` is set.
    pub fn is_fatal(&self) -> bool {
        let explicit_prefix =
            std::env::var_os(prefix::PREFIX_ENV).is_some_and(|value| !value.is_empty());
        self.is_fatal_with(explicit_prefix, require_env_is_set())
    }

    fn is_fatal_with(&self, explicit_prefix: bool, required: bool) -> bool {
        match self {
            SkipReason::BuildFailed { .. } => true,
            SkipReason::ConfigureFailed { .. } => explicit_prefix || required,
            _ => required,
        }
    }
}

impl fmt::Display for SkipReason {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            SkipReason::PrefixNotFound { searched } => {
                write!(f, "no BT.CPP 4.x install found")?;
                for (prefix, reason) in searched {
                    write!(f, "; {}: {reason}", prefix.display())?;
                }
                Ok(())
            }
            SkipReason::InvalidPrefix { prefix, reason } => write!(
                f,
                "{} = {} is not a BT.CPP 4.x prefix: {reason}",
                prefix::PREFIX_ENV,
                prefix.display()
            ),
            SkipReason::CmakeNotFound { error } => write!(f, "cannot run cmake: {error}"),
            SkipReason::ConfigureFailed { prefix, log } => write!(
                f,
                "CMake configure failed for prefix {}:\n{log}",
                prefix.display()
            ),
            SkipReason::BuildFailed { prefix, log } => write!(
                f,
                "build of test_harness failed for prefix {}:\n{log}",
                prefix.display()
            ),
            SkipReason::Io { context, error } => write!(f, "{context}: {error}"),
        }
    }
}

impl std::error::Error for SkipReason {}

/// Gives the text that [`require_harness!`](crate::require_harness) writes
/// when it skips a test.
pub fn skip_message(reason: &SkipReason) -> String {
    format!("SKIPPED: BT.CPP 4.x not found (set FASTTICK_BTCPP_PREFIX) — {reason}")
}

/// Gives the root directory of the FastTick workspace.
pub fn workspace_root() -> PathBuf {
    let manifest_dir = Path::new(env!("CARGO_MANIFEST_DIR"));
    manifest_dir
        .ancestors()
        .nth(2)
        .unwrap_or(manifest_dir)
        .to_path_buf()
}

/// Gives the path of a file in `tests/fixtures/harness/`.
///
/// The directory holds real samples from the harness: `custom_models.xml`,
/// `all_models.xml` and `main.btlog`. Unit tests of other crates can read
/// them without BT.CPP.
pub fn fixture_path(name: impl AsRef<Path>) -> PathBuf {
    workspace_root()
        .join("tests")
        .join("fixtures")
        .join("harness")
        .join(name)
}

/// Builds the harness (once for each process) and gives it.
///
/// The first call configures and builds `test_harness/` with CMake into
/// `<workspace>/target/test_harness/<prefix-hash>`. A lock file in the build
/// directory makes sure that only one process builds at a time. Then the
/// programs are copied to `<build dir>/bin/<stamp>/`. These copies never
/// change, so a new build by another process does not disturb this process.
/// Later calls give the same result without a new build.
pub fn harness() -> Result<&'static Harness, SkipReason> {
    static HARNESS: OnceLock<Result<Harness, SkipReason>> = OnceLock::new();
    HARNESS
        .get_or_init(build_harness)
        .as_ref()
        .map_err(Clone::clone)
}

fn require_env_is_set() -> bool {
    std::env::var_os(REQUIRE_ENV).is_some_and(|value| value == "1")
}

fn build_harness() -> Result<Harness, SkipReason> {
    let prefix = prefix::find_btcpp_prefix()?;
    let root = workspace_root();
    let source_dir = root.join("test_harness");
    let build_dir = root
        .join("target")
        .join("test_harness")
        .join(prefix_hash(&prefix));

    if let Err(error) = Command::new("cmake")
        .arg("--version")
        .stdin(Stdio::null())
        .output()
    {
        return Err(SkipReason::CmakeNotFound {
            error: error.to_string(),
        });
    }

    fs::create_dir_all(&build_dir).map_err(|error| SkipReason::Io {
        context: format!("cannot create {}", build_dir.display()),
        error: error.to_string(),
    })?;

    // The lock serializes builds of several test processes (for example
    // `cargo test` of several crates at the same time). The lock ends when
    // `lock_file` is dropped.
    let lock_path = build_dir.join(".fasttick-build.lock");
    let lock_file = File::options()
        .create(true)
        .truncate(false)
        .write(true)
        .open(&lock_path)
        .map_err(|error| SkipReason::Io {
            context: format!("cannot open {}", lock_path.display()),
            error: error.to_string(),
        })?;
    lock_file.lock().map_err(|error| SkipReason::Io {
        context: format!("cannot lock {}", lock_path.display()),
        error: error.to_string(),
    })?;

    let mut configure = Command::new("cmake");
    configure
        .arg("-S")
        .arg(&source_dir)
        .arg("-B")
        .arg(&build_dir)
        .arg(define("CMAKE_PREFIX_PATH", prefix.as_os_str()))
        .arg("-DCMAKE_BUILD_TYPE=RelWithDebInfo");
    let output = run_logged(&mut configure).map_err(|error| SkipReason::Io {
        context: "cannot run cmake".to_owned(),
        error: error.to_string(),
    })?;
    if !output.status.success() {
        return Err(SkipReason::ConfigureFailed {
            prefix,
            log: tail_lines(&combined_output(&output), LOG_TAIL_LINES),
        });
    }

    let jobs = std::thread::available_parallelism()
        .map(|n| n.get().min(8))
        .unwrap_or(2);
    let mut build = Command::new("cmake");
    build
        .arg("--build")
        .arg(&build_dir)
        .arg("--parallel")
        .arg(jobs.to_string());
    let output = run_logged(&mut build).map_err(|error| SkipReason::Io {
        context: "cannot run cmake --build".to_owned(),
        error: error.to_string(),
    })?;
    if !output.status.success() {
        return Err(SkipReason::BuildFailed {
            prefix,
            log: tail_lines(&combined_output(&output), LOG_TAIL_LINES),
        });
    }
    for program in PROGRAMS {
        let path = build_dir.join(program);
        if !path.is_file() {
            return Err(SkipReason::BuildFailed {
                prefix,
                log: format!("the build did not make {}", path.display()),
            });
        }
    }
    let snapshot = snapshot_programs(&build_dir, &PROGRAMS).map_err(|error| SkipReason::Io {
        context: format!("cannot copy the programs of {}", build_dir.display()),
        error: error.to_string(),
    })?;
    drop(lock_file);

    Ok(Harness {
        lib_dir: prefix::lib_dir(&prefix),
        executor_path: snapshot.join(PROGRAMS[0]),
        dump_models_path: snapshot.join(PROGRAMS[1]),
        trees_dir: source_dir.join("trees"),
        build_dir,
        prefix,
    })
}

/// The programs that the build makes: the executor, then the model dump.
const PROGRAMS: [&str; 2] = ["fasttick_executor", "fasttick_dump_models"];

/// Directory (in the build directory) that holds the program copies.
const SNAPSHOT_DIR: &str = "bin";

/// Name of the file that records the last use of a program copy.
const LAST_USED_FILE: &str = ".last-used";

/// A program copy that no process used for this time is removed.
const SNAPSHOT_MAX_AGE: Duration = Duration::from_secs(2 * 3600);

/// Copies the built programs to `<build_dir>/bin/<stamp>/` and gives that
/// directory.
///
/// Several test processes share one build directory. If a process builds
/// again (for example after a change of the C++ sources), the linker
/// rewrites the programs in the build directory. A process that starts a
/// program at that moment could get a partly written file. The copies
/// never change: a new build gives a new stamp (from the size and the
/// modification time of the programs) and thus a new directory. The caller
/// must hold the build lock.
pub(crate) fn snapshot_programs(build_dir: &Path, programs: &[&str]) -> io::Result<PathBuf> {
    let mut stamp = Vec::new();
    for program in programs {
        let metadata = fs::metadata(build_dir.join(program))?;
        let modified = metadata
            .modified()?
            .duration_since(UNIX_EPOCH)
            .unwrap_or_default();
        stamp.extend_from_slice(program.as_bytes());
        stamp.push(0);
        stamp.extend_from_slice(&metadata.len().to_le_bytes());
        stamp.extend_from_slice(&modified.as_nanos().to_le_bytes());
    }
    let root = build_dir.join(SNAPSHOT_DIR);
    let target = root.join(format!("{:016x}", fnv1a64(&stamp)));
    if !programs
        .iter()
        .all(|program| target.join(program).is_file())
    {
        let staging = root.join(format!(".staging-{}", std::process::id()));
        // Leftovers of an earlier, interrupted copy.
        let _ = fs::remove_dir_all(&staging);
        let _ = fs::remove_dir_all(&target);
        fs::create_dir_all(&staging)?;
        for program in programs {
            fs::copy(build_dir.join(program), staging.join(program))?;
        }
        fs::rename(&staging, &target)?;
    }
    // Mark the copy as used now, so that no other process removes it.
    fs::write(target.join(LAST_USED_FILE), b"")?;
    remove_old_snapshots(&root, &target, SNAPSHOT_MAX_AGE);
    Ok(target)
}

/// Removes the program copies (and interrupted copies) in `root` that were
/// not used for `max_age`, except `keep`. Errors are ignored: a copy that
/// stays is not a problem.
pub(crate) fn remove_old_snapshots(root: &Path, keep: &Path, max_age: Duration) {
    let Ok(entries) = fs::read_dir(root) else {
        return;
    };
    let now = SystemTime::now();
    for entry in entries.filter_map(Result::ok) {
        let path = entry.path();
        if path == keep || !path.is_dir() {
            continue;
        }
        let last_used = fs::metadata(path.join(LAST_USED_FILE))
            .or_else(|_| fs::metadata(&path))
            .and_then(|metadata| metadata.modified());
        let age = last_used
            .ok()
            .and_then(|time| now.duration_since(time).ok())
            .unwrap_or_default();
        if age > max_age {
            let _ = fs::remove_dir_all(&path);
        }
    }
}

fn define(name: &str, value: &std::ffi::OsStr) -> OsString {
    let mut text = OsString::from(format!("-D{name}="));
    text.push(value);
    text
}

fn run_logged(command: &mut Command) -> io::Result<Output> {
    command.stdin(Stdio::null()).output()
}

fn combined_output(output: &Output) -> String {
    let mut text = String::from_utf8_lossy(&output.stdout).into_owned();
    text.push_str(&String::from_utf8_lossy(&output.stderr));
    text
}

/// Gives the last `count` lines of a text.
pub(crate) fn tail_lines(text: &str, count: usize) -> String {
    let lines: Vec<&str> = text.lines().collect();
    let start = lines.len().saturating_sub(count);
    lines[start..].join("\n")
}

/// Gives a stable 16-digit hex name for a prefix (FNV-1a, 64 bit).
pub(crate) fn prefix_hash(prefix: &Path) -> String {
    format!("{:016x}", fnv1a64(prefix.to_string_lossy().as_bytes()))
}

/// The 64-bit FNV-1a hash.
pub(crate) fn fnv1a64(bytes: &[u8]) -> u64 {
    const OFFSET_BASIS: u64 = 0xcbf2_9ce4_8422_2325;
    const PRIME: u64 = 0x0000_0100_0000_01b3;
    bytes.iter().fold(OFFSET_BASIS, |hash, byte| {
        (hash ^ u64::from(*byte)).wrapping_mul(PRIME)
    })
}

/// Puts `lib_dir` in front of an existing `LD_LIBRARY_PATH` value.
pub(crate) fn ld_library_path(lib_dir: &Path, existing: Option<OsString>) -> OsString {
    let mut paths = vec![lib_dir.to_path_buf()];
    if let Some(existing) = existing.filter(|value| !value.is_empty()) {
        paths.extend(std::env::split_paths(&existing).filter(|path| path != lib_dir));
    }
    // join_paths fails only if a path contains the separator. Then the
    // library directory alone is the best value.
    std::env::join_paths(&paths).unwrap_or_else(|_| lib_dir.as_os_str().to_owned())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn fnv1a64_matches_reference_values() {
        // Reference values of the FNV-1a 64-bit hash.
        assert_eq!(fnv1a64(b""), 0xcbf2_9ce4_8422_2325);
        assert_eq!(fnv1a64(b"a"), 0xaf63_dc4c_8601_ec8c);
        assert_eq!(fnv1a64(b"foobar"), 0x8594_4171_f739_67e8);
    }

    #[test]
    fn prefix_hash_is_stable_and_distinct() {
        let a = prefix_hash(Path::new("/opt/ros/jazzy"));
        let b = prefix_hash(Path::new("/usr/local"));
        assert_eq!(a.len(), 16);
        assert!(a.chars().all(|c| c.is_ascii_hexdigit()));
        assert_eq!(a, prefix_hash(Path::new("/opt/ros/jazzy")));
        assert_ne!(a, b);
    }

    #[test]
    fn tail_lines_keeps_the_end() {
        assert_eq!(tail_lines("a\nb\nc\n", 2), "b\nc");
        assert_eq!(tail_lines("a\nb", 5), "a\nb");
        assert_eq!(tail_lines("", 3), "");
        assert_eq!(tail_lines("a\nb\nc", 0), "");
    }

    #[test]
    fn ld_library_path_puts_the_lib_dir_first() {
        let lib = Path::new("/prefix/lib");
        assert_eq!(ld_library_path(lib, None), OsString::from("/prefix/lib"));
        assert_eq!(
            ld_library_path(lib, Some(OsString::new())),
            OsString::from("/prefix/lib")
        );
        assert_eq!(
            ld_library_path(lib, Some(OsString::from("/a:/b"))),
            OsString::from("/prefix/lib:/a:/b")
        );
        // No duplicate entry.
        assert_eq!(
            ld_library_path(lib, Some(OsString::from("/a:/prefix/lib"))),
            OsString::from("/prefix/lib:/a")
        );
    }

    #[test]
    fn define_builds_a_cmake_cache_argument() {
        assert_eq!(
            define("CMAKE_PREFIX_PATH", std::ffi::OsStr::new("/x y")),
            OsString::from("-DCMAKE_PREFIX_PATH=/x y")
        );
    }

    #[test]
    fn workspace_root_contains_the_harness_sources() {
        let root = workspace_root();
        assert!(root.join("Cargo.toml").is_file());
        assert!(root.join("test_harness").join("CMakeLists.txt").is_file());
        assert!(fixture_path("main.btlog").starts_with(root.join("tests")));
    }

    #[test]
    fn skip_reasons_explain_the_problem() {
        let not_found = SkipReason::PrefixNotFound {
            searched: vec![(PathBuf::from("/usr"), "no config".to_owned())],
        };
        let text = not_found.to_string();
        assert!(text.contains("/usr: no config"), "{text}");
        let message = skip_message(&not_found);
        assert!(
            message.starts_with("SKIPPED: BT.CPP 4.x not found (set FASTTICK_BTCPP_PREFIX) — "),
            "{message}"
        );

        let invalid = SkipReason::InvalidPrefix {
            prefix: PathBuf::from("/bad"),
            reason: "missing".to_owned(),
        };
        assert!(invalid.to_string().contains("FASTTICK_BTCPP_PREFIX = /bad"));

        let build = SkipReason::BuildFailed {
            prefix: PathBuf::from("/p"),
            log: "error: x".to_owned(),
        };
        assert!(build.is_fatal());
        assert!(build.to_string().contains("error: x"));
    }

    #[test]
    fn snapshot_copies_are_stable_and_old_ones_are_removed() {
        let temp = crate::TempDir::new("snapshot").unwrap();
        let build = temp.path();
        fs::write(build.join("a"), b"first a").unwrap();
        fs::write(build.join("b"), b"first b").unwrap();

        let first = snapshot_programs(build, &["a", "b"]).unwrap();
        assert!(first.starts_with(build.join(SNAPSHOT_DIR)));
        assert_eq!(fs::read(first.join("a")).unwrap(), b"first a");
        assert_eq!(fs::read(first.join("b")).unwrap(), b"first b");
        // The same build gives the same copy.
        assert_eq!(snapshot_programs(build, &["a", "b"]).unwrap(), first);

        // A new build gives a new copy. The old copy does not change.
        fs::write(build.join("a"), b"second build of a").unwrap();
        let second = snapshot_programs(build, &["a", "b"]).unwrap();
        assert_ne!(second, first);
        assert_eq!(fs::read(second.join("a")).unwrap(), b"second build of a");
        assert_eq!(fs::read(first.join("a")).unwrap(), b"first a");

        // A recent copy stays; an old one goes. The kept one always stays.
        let root = build.join(SNAPSHOT_DIR);
        fs::create_dir_all(root.join(".staging-1")).unwrap();
        remove_old_snapshots(&root, &second, Duration::from_secs(3600));
        assert!(first.is_dir());
        remove_old_snapshots(&root, &second, Duration::ZERO);
        std::thread::sleep(Duration::from_millis(20));
        remove_old_snapshots(&root, &second, Duration::ZERO);
        assert!(!first.exists());
        assert!(!root.join(".staging-1").exists());
        assert!(second.join("a").is_file());

        // A missing program is an error, not a panic.
        assert!(snapshot_programs(build, &["missing"]).is_err());
        // A missing root directory is not a problem for the clean-up.
        remove_old_snapshots(&build.join("none"), &second, Duration::ZERO);
    }

    #[test]
    fn fatal_rules() {
        let build = SkipReason::BuildFailed {
            prefix: PathBuf::from("/p"),
            log: String::new(),
        };
        let configure = SkipReason::ConfigureFailed {
            prefix: PathBuf::from("/p"),
            log: String::new(),
        };
        let not_found = SkipReason::PrefixNotFound { searched: vec![] };
        let cmake = SkipReason::CmakeNotFound {
            error: "x".to_owned(),
        };

        assert!(build.is_fatal_with(false, false));
        assert!(!configure.is_fatal_with(false, false));
        assert!(configure.is_fatal_with(true, false));
        assert!(configure.is_fatal_with(false, true));
        assert!(!not_found.is_fatal_with(true, false));
        assert!(not_found.is_fatal_with(false, true));
        assert!(!cmake.is_fatal_with(false, false));
        assert!(cmake.is_fatal_with(false, true));
    }
}
