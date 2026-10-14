//! Search for a BehaviorTree.CPP 4.x install prefix.

use std::ffi::OsStr;
use std::fs;
use std::path::{Path, PathBuf};

use crate::harness::SkipReason;

/// Environment variable that names the BT.CPP install prefix.
pub const PREFIX_ENV: &str = "FASTTICK_BTCPP_PREFIX";

/// Environment variable. When it is `1`, a test fails if the harness is not
/// available (see [`SkipReason::is_fatal`]).
pub const REQUIRE_ENV: &str = "FASTTICK_REQUIRE_HARNESS";

/// Standard prefixes. The search uses them in this order, after
/// [`PREFIX_ENV`].
pub const DEFAULT_PREFIXES: [&str; 3] = ["/opt/ros/jazzy", "/usr/local", "/usr"];

/// Prefixes that are never accepted. ROS 2 Humble ships BT.CPP 3.x.
const REJECTED_PREFIXES: [&str; 1] = ["/opt/ros/humble"];

/// Directories (relative to the prefix) that can hold the CMake package
/// config. A plain CMake install uses `lib/cmake/<package>`
/// (BT.CPP `cmake/conan_build.cmake`). A ROS 2 (ament) install uses
/// `share/<package>/cmake`.
const CONFIG_DIRS: [&str; 2] = ["lib/cmake/behaviortree_cpp", "share/behaviortree_cpp/cmake"];

/// Names of the CMake package config file.
const CONFIG_FILES: [&str; 2] = [
    "behaviortree_cppConfig.cmake",
    "behaviortree_cpp-config.cmake",
];

/// Names of the CMake package version file. A plain install of BT.CPP 4.9.0
/// has no version file. The ament install has one.
const VERSION_FILES: [&str; 3] = [
    "behaviortree_cppConfigVersion.cmake",
    "behaviortree_cppConfig-version.cmake",
    "behaviortree_cpp-config-version.cmake",
];

/// Gives the BT.CPP 4.x install prefix, or `None`.
///
/// The search order is: the environment variable [`PREFIX_ENV`], then
/// [`DEFAULT_PREFIXES`]. See [`find_btcpp_prefix`] for the rules.
pub fn btcpp_prefix() -> Option<PathBuf> {
    find_btcpp_prefix().ok()
}

/// Finds the BT.CPP 4.x install prefix, or tells why there is none.
///
/// Rules:
///
/// - If [`PREFIX_ENV`] is set and not empty, only that prefix is checked.
///   An explicit value that is not valid gives [`SkipReason::InvalidPrefix`].
/// - Else the function checks [`DEFAULT_PREFIXES`] in order.
/// - A prefix is valid if `lib/cmake/behaviortree_cpp` or
///   `share/behaviortree_cpp/cmake` contains a package config file.
/// - `/opt/ros/humble` is never valid (it ships BT.CPP 3.x).
/// - If a package version file is present, the major version must be 4 or
///   more.
pub fn find_btcpp_prefix() -> Result<PathBuf, SkipReason> {
    let env_value = std::env::var_os(PREFIX_ENV);
    let defaults: Vec<PathBuf> = DEFAULT_PREFIXES.iter().map(PathBuf::from).collect();
    find_prefix_in(env_value.as_deref(), &defaults)
}

/// The search of [`find_btcpp_prefix`], with the inputs as arguments.
pub(crate) fn find_prefix_in(
    env_value: Option<&OsStr>,
    candidates: &[PathBuf],
) -> Result<PathBuf, SkipReason> {
    if let Some(value) = env_value.filter(|value| !value.is_empty()) {
        let prefix = PathBuf::from(value);
        let prefix = std::path::absolute(&prefix).unwrap_or(prefix);
        return match check_prefix(&prefix) {
            Ok(()) => Ok(prefix),
            Err(reason) => Err(SkipReason::InvalidPrefix { prefix, reason }),
        };
    }
    let mut searched = Vec::with_capacity(candidates.len());
    for candidate in candidates {
        match check_prefix(candidate) {
            Ok(()) => return Ok(candidate.clone()),
            Err(reason) => searched.push((candidate.clone(), reason)),
        }
    }
    Err(SkipReason::PrefixNotFound { searched })
}

/// Checks one prefix. Gives the reason if the prefix is not valid.
pub(crate) fn check_prefix(prefix: &Path) -> Result<(), String> {
    if is_rejected_prefix(prefix) {
        return Err("ROS 2 Humble ships BT.CPP 3.x; FastTick needs BT.CPP 4.x".to_owned());
    }
    if !prefix.is_dir() {
        return Err("the directory does not exist".to_owned());
    }
    let Some(config_dir) = config_dir(prefix) else {
        return Err(format!(
            "no {} in {}",
            CONFIG_FILES[0],
            CONFIG_DIRS.join(" or ")
        ));
    };
    if let Some(version) = package_version(&config_dir) {
        let major = version
            .split('.')
            .next()
            .and_then(|major| major.trim().parse::<u32>().ok());
        if let Some(major) = major.filter(|major| *major < 4) {
            return Err(format!(
                "BT.CPP version {version} (major {major}); FastTick needs 4.x"
            ));
        }
    }
    Ok(())
}

/// True for a prefix that FastTick never accepts.
pub(crate) fn is_rejected_prefix(prefix: &Path) -> bool {
    REJECTED_PREFIXES
        .iter()
        .any(|rejected| prefix.starts_with(rejected))
}

/// Gives the directory that holds the BT.CPP package config file.
pub(crate) fn config_dir(prefix: &Path) -> Option<PathBuf> {
    CONFIG_DIRS
        .iter()
        .map(|dir| prefix.join(dir))
        .find(|dir| CONFIG_FILES.iter().any(|file| dir.join(file).is_file()))
}

/// Reads `PACKAGE_VERSION` from the package version file, if there is one.
pub(crate) fn package_version(config_dir: &Path) -> Option<String> {
    VERSION_FILES
        .iter()
        .filter_map(|file| fs::read_to_string(config_dir.join(file)).ok())
        .find_map(|text| parse_package_version(&text))
}

/// Finds `set(PACKAGE_VERSION "x.y.z")` in the text of a CMake version file.
pub(crate) fn parse_package_version(text: &str) -> Option<String> {
    text.lines().find_map(|line| {
        let rest = line.trim().strip_prefix("set(")?.trim_start();
        let rest = rest.strip_prefix("PACKAGE_VERSION")?;
        // Reject PACKAGE_VERSION_COMPATIBLE and similar names.
        if !rest.starts_with(|c: char| c.is_whitespace()) {
            return None;
        }
        let value = rest.trim().strip_suffix(')')?.trim();
        let value = value.trim_matches('"').trim();
        (!value.is_empty()).then(|| value.to_owned())
    })
}

/// Gives the library directory of a prefix: the first of `lib` and `lib64`
/// that contains `libbehaviortree_cpp.so`, else `<prefix>/lib`.
pub(crate) fn lib_dir(prefix: &Path) -> PathBuf {
    ["lib", "lib64"]
        .iter()
        .map(|dir| prefix.join(dir))
        .find(|dir| has_btcpp_library(dir))
        .unwrap_or_else(|| prefix.join("lib"))
}

fn has_btcpp_library(dir: &Path) -> bool {
    let Ok(entries) = fs::read_dir(dir) else {
        return false;
    };
    entries.filter_map(Result::ok).any(|entry| {
        entry
            .file_name()
            .to_str()
            .is_some_and(|name| name.starts_with("libbehaviortree_cpp.so"))
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::TempDir;

    fn make_prefix(root: &Path, config_dir: &str, config_file: &str) -> PathBuf {
        let dir = root.join(config_dir);
        fs::create_dir_all(&dir).unwrap();
        fs::write(dir.join(config_file), "# test\n").unwrap();
        root.to_path_buf()
    }

    #[test]
    fn plain_cmake_layout_is_valid() {
        let temp = TempDir::new("prefix-plain").unwrap();
        let prefix = make_prefix(
            temp.path(),
            "lib/cmake/behaviortree_cpp",
            "behaviortree_cppConfig.cmake",
        );
        assert_eq!(check_prefix(&prefix), Ok(()));
        assert_eq!(
            config_dir(&prefix),
            Some(prefix.join("lib/cmake/behaviortree_cpp"))
        );
    }

    #[test]
    fn ament_layout_is_valid() {
        let temp = TempDir::new("prefix-ament").unwrap();
        let prefix = make_prefix(
            temp.path(),
            "share/behaviortree_cpp/cmake",
            "behaviortree_cpp-config.cmake",
        );
        assert_eq!(check_prefix(&prefix), Ok(()));
    }

    #[test]
    fn empty_or_missing_directory_is_not_valid() {
        let temp = TempDir::new("prefix-empty").unwrap();
        let error = check_prefix(temp.path()).unwrap_err();
        assert!(error.contains("no behaviortree_cppConfig.cmake"), "{error}");
        let missing = temp.path().join("does-not-exist");
        assert_eq!(
            check_prefix(&missing).unwrap_err(),
            "the directory does not exist"
        );
    }

    #[test]
    fn config_dir_without_config_file_is_not_valid() {
        let temp = TempDir::new("prefix-noconfig").unwrap();
        let prefix = make_prefix(
            temp.path(),
            "lib/cmake/behaviortree_cpp",
            "behaviortree_cppTargets.cmake",
        );
        assert!(check_prefix(&prefix).is_err());
    }

    #[test]
    fn version_3_is_rejected_and_version_4_is_accepted() {
        let temp = TempDir::new("prefix-version").unwrap();
        let prefix = make_prefix(
            temp.path(),
            "share/behaviortree_cpp/cmake",
            "behaviortree_cppConfig.cmake",
        );
        let version_file =
            prefix.join("share/behaviortree_cpp/cmake/behaviortree_cppConfig-version.cmake");

        fs::write(&version_file, "set(PACKAGE_VERSION \"3.8.6\")\n").unwrap();
        let error = check_prefix(&prefix).unwrap_err();
        assert!(error.contains("3.8.6"), "{error}");

        fs::write(&version_file, "set(PACKAGE_VERSION \"4.6.2\")\n").unwrap();
        assert_eq!(check_prefix(&prefix), Ok(()));
    }

    #[test]
    fn humble_is_always_rejected() {
        assert!(is_rejected_prefix(Path::new("/opt/ros/humble")));
        assert!(is_rejected_prefix(Path::new("/opt/ros/humble/")));
        assert!(!is_rejected_prefix(Path::new("/opt/ros/jazzy")));
        assert!(!is_rejected_prefix(Path::new("/opt/ros/humble2")));
        let error = check_prefix(Path::new("/opt/ros/humble")).unwrap_err();
        assert!(error.contains("3.x"), "{error}");
    }

    #[test]
    fn parses_package_version() {
        let text = "\
set(PACKAGE_VERSION_COMPATIBLE FALSE)
  set(PACKAGE_VERSION \"4.9.0\")
";
        assert_eq!(parse_package_version(text), Some("4.9.0".to_owned()));
        assert_eq!(
            parse_package_version("set(PACKAGE_VERSION 4.6.2)"),
            Some("4.6.2".to_owned())
        );
        assert_eq!(parse_package_version("set(PACKAGE_VERSION \"\")"), None);
        assert_eq!(
            parse_package_version("set(PACKAGE_VERSION_EXACT TRUE)"),
            None
        );
        assert_eq!(parse_package_version("message(PACKAGE_VERSION)"), None);
        assert_eq!(parse_package_version(""), None);
    }

    #[test]
    fn explicit_value_wins_and_does_not_fall_back() {
        let temp = TempDir::new("prefix-env").unwrap();
        let good = make_prefix(
            &temp.path().join("good"),
            "lib/cmake/behaviortree_cpp",
            "behaviortree_cppConfig.cmake",
        );
        let bad = temp.path().join("bad");
        fs::create_dir_all(&bad).unwrap();

        let found = find_prefix_in(Some(good.as_os_str()), std::slice::from_ref(&bad)).unwrap();
        assert_eq!(found, good);

        let error = find_prefix_in(Some(bad.as_os_str()), std::slice::from_ref(&good)).unwrap_err();
        assert!(
            matches!(&error, SkipReason::InvalidPrefix { prefix, .. } if *prefix == bad),
            "{error:?}"
        );
    }

    #[test]
    fn empty_value_uses_the_candidates_in_order() {
        let temp = TempDir::new("prefix-order").unwrap();
        let missing = temp.path().join("missing");
        let first = make_prefix(
            &temp.path().join("first"),
            "lib/cmake/behaviortree_cpp",
            "behaviortree_cppConfig.cmake",
        );
        let second = make_prefix(
            &temp.path().join("second"),
            "share/behaviortree_cpp/cmake",
            "behaviortree_cppConfig.cmake",
        );
        let candidates = [missing.clone(), first.clone(), second.clone()];
        assert_eq!(
            find_prefix_in(Some(OsStr::new("")), &candidates).unwrap(),
            first
        );
        assert_eq!(find_prefix_in(None, &candidates[2..]).unwrap(), second);

        let error = find_prefix_in(None, std::slice::from_ref(&missing)).unwrap_err();
        match error {
            SkipReason::PrefixNotFound { searched } => {
                assert_eq!(searched.len(), 1);
                assert_eq!(searched[0].0, missing);
            }
            other => panic!("unexpected reason: {other:?}"),
        }
    }

    #[test]
    fn relative_explicit_value_becomes_absolute() {
        let error = find_prefix_in(Some(OsStr::new("relative/prefix")), &[]).unwrap_err();
        match error {
            SkipReason::InvalidPrefix { prefix, .. } => assert!(prefix.is_absolute()),
            other => panic!("unexpected reason: {other:?}"),
        }
    }

    #[test]
    fn lib_dir_prefers_the_directory_with_the_library() {
        let temp = TempDir::new("prefix-lib").unwrap();
        assert_eq!(lib_dir(temp.path()), temp.path().join("lib"));
        fs::create_dir_all(temp.path().join("lib64")).unwrap();
        fs::write(temp.path().join("lib64/libbehaviortree_cpp.so"), b"").unwrap();
        assert_eq!(lib_dir(temp.path()), temp.path().join("lib64"));
        fs::create_dir_all(temp.path().join("lib")).unwrap();
        fs::write(temp.path().join("lib/libbehaviortree_cpp.so.4"), b"").unwrap();
        assert_eq!(lib_dir(temp.path()), temp.path().join("lib"));
    }
}
