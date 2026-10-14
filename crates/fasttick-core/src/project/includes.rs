//! Resolution of `<include>` paths.

use std::ffi::{OsStr, OsString};
use std::fmt;
use std::path::{Path, PathBuf};

use crate::bt::IncludeEntry;
use crate::fsio::normalize;
use crate::xml::NodeId;

use super::FileId;

/// The name of the environment variable that lists the ROS 2 install
/// prefixes.
pub const AMENT_PREFIX_PATH: &str = "AMENT_PREFIX_PATH";

/// Why an `<include>` gives no file.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum IncludeProblem {
    /// The element has no `path` attribute. BT.CPP stops with "Invalid
    /// <include> tag: missing 'path' attribute".
    MissingPath,
    /// The `ros_pkg` package is not found. BT.CPP without ROS support stops
    /// for each `ros_pkg`; with ROS 2 it asks the ament index. FastTick
    /// searches the prefixes of `AMENT_PREFIX_PATH`.
    RosPackageNotFound {
        /// The package name.
        package: String,
        /// True if `AMENT_PREFIX_PATH` is not set.
        no_ament_prefix_path: bool,
    },
    /// The file cannot be loaded (it does not exist, or it is not valid
    /// XML).
    Load {
        /// The resolved path.
        path: PathBuf,
        /// The error message.
        message: String,
    },
    /// The include makes a cycle: the target file includes (directly or
    /// through other files) the file with this include. BT.CPP has no
    /// check for this and recurses without end.
    Cycle {
        /// The resolved path.
        path: PathBuf,
    },
}

impl fmt::Display for IncludeProblem {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            IncludeProblem::MissingPath => {
                f.write_str("the <include> element has no path attribute")
            }
            IncludeProblem::RosPackageNotFound {
                package,
                no_ament_prefix_path: true,
            } => write!(
                f,
                "cannot find the ROS package \"{package}\": {AMENT_PREFIX_PATH} is not set"
            ),
            IncludeProblem::RosPackageNotFound { package, .. } => write!(
                f,
                "cannot find the ROS package \"{package}\" in {AMENT_PREFIX_PATH}"
            ),
            IncludeProblem::Load { path, message } => {
                write!(
                    f,
                    "cannot load the included file {}: {message}",
                    path.display()
                )
            }
            IncludeProblem::Cycle { path } => write!(
                f,
                "the include of {} makes a cycle: that file includes this file again",
                path.display()
            ),
        }
    }
}

/// The result of one `<include>` element of a loaded file.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct IncludeLink {
    /// The `<include>` element.
    pub element: NodeId,
    /// The resolved, normalized path, if the path could be made.
    pub path: Option<PathBuf>,
    /// The loaded target file. It is set also for a cycle.
    pub target: Option<FileId>,
    /// The problem, if any.
    pub problem: Option<IncludeProblem>,
}

/// Finds the share directory of a ROS 2 package, like
/// `ament_index_cpp::get_package_share_directory`: the first prefix in
/// `prefix_path` that has the marker file
/// `share/ament_index/resource_index/packages/<package>` gives
/// `<prefix>/share/<package>`. If no prefix has the marker, the first prefix
/// that has the directory `share/<package>` is used.
pub fn find_package_share(package: &str, prefix_path: &OsStr) -> Option<PathBuf> {
    if package.is_empty() || package.contains(['/', '\\']) || package == ".." {
        return None;
    }
    let prefixes: Vec<PathBuf> = std::env::split_paths(prefix_path)
        .filter(|p| !p.as_os_str().is_empty())
        .collect();
    prefixes
        .iter()
        .find(|prefix| {
            prefix
                .join("share/ament_index/resource_index/packages")
                .join(package)
                .is_file()
        })
        .or_else(|| {
            prefixes
                .iter()
                .find(|prefix| prefix.join("share").join(package).is_dir())
        })
        .map(|prefix| prefix.join("share").join(package))
}

/// Resolves the path of an `<include>` like `loadDocImpl`
/// (`src/xml_parsing.cpp`):
///
/// 1. With `ros_pkg` and a relative path, the path is relative to the share
///    directory of the package. With an absolute path, BT.CPP ignores
///    `ros_pkg` (it prints a warning).
/// 2. A relative path is relative to `base_dir`: the directory of the
///    including file.
///
/// The result is absolute and normalized (`.` and `..` are removed).
pub fn resolve_include_path(
    base_dir: &Path,
    include: &IncludeEntry,
    ament_prefix_path: Option<&OsStr>,
) -> Result<PathBuf, IncludeProblem> {
    let path = include.path.as_deref().ok_or(IncludeProblem::MissingPath)?;
    let mut file_path = PathBuf::from(path);
    if let Some(package) = include.ros_pkg.as_deref()
        && !file_path.is_absolute()
    {
        let share = ament_prefix_path
            .and_then(|prefixes| find_package_share(package, prefixes))
            .ok_or_else(|| IncludeProblem::RosPackageNotFound {
                package: package.to_owned(),
                no_ament_prefix_path: ament_prefix_path.is_none(),
            })?;
        file_path = share.join(file_path);
    }
    if !file_path.is_absolute() {
        file_path = base_dir.join(file_path);
    }
    Ok(normalize(&file_path))
}

/// The value of `AMENT_PREFIX_PATH` in the environment.
pub fn ament_prefix_path_from_env() -> Option<OsString> {
    std::env::var_os(AMENT_PREFIX_PATH).filter(|v| !v.is_empty())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn include(path: Option<&str>, ros_pkg: Option<&str>) -> IncludeEntry {
        IncludeEntry {
            element: NodeId(0),
            path: path.map(str::to_owned),
            ros_pkg: ros_pkg.map(str::to_owned),
        }
    }

    #[test]
    fn relative_and_absolute_paths() {
        let base = Path::new("/project/trees");
        assert_eq!(
            resolve_include_path(base, &include(Some("sub/a.xml"), None), None),
            Ok(PathBuf::from("/project/trees/sub/a.xml"))
        );
        assert_eq!(
            resolve_include_path(base, &include(Some("../a.xml"), None), None),
            Ok(PathBuf::from("/project/a.xml"))
        );
        assert_eq!(
            resolve_include_path(base, &include(Some("/abs/a.xml"), Some("pkg")), None),
            Ok(PathBuf::from("/abs/a.xml"))
        );
        assert_eq!(
            resolve_include_path(base, &include(None, None), None),
            Err(IncludeProblem::MissingPath)
        );
        let err =
            resolve_include_path(base, &include(Some("a.xml"), Some("pkg")), None).unwrap_err();
        assert_eq!(
            err,
            IncludeProblem::RosPackageNotFound {
                package: "pkg".into(),
                no_ament_prefix_path: true
            }
        );
        assert!(err.to_string().contains("AMENT_PREFIX_PATH is not set"));
    }

    #[test]
    fn ros_packages() {
        let dir = tempfile::tempdir().unwrap();
        let first = dir.path().join("first");
        let second = dir.path().join("second");
        // "marked" has the ament marker in the second prefix only; "plain"
        // has only a share directory in the first prefix.
        std::fs::create_dir_all(first.join("share/marked")).unwrap();
        std::fs::create_dir_all(second.join("share/marked")).unwrap();
        std::fs::create_dir_all(second.join("share/ament_index/resource_index/packages")).unwrap();
        std::fs::write(
            second.join("share/ament_index/resource_index/packages/marked"),
            "",
        )
        .unwrap();
        std::fs::create_dir_all(first.join("share/plain")).unwrap();
        let prefixes = std::env::join_paths([&first, &second]).unwrap();

        assert_eq!(
            find_package_share("marked", &prefixes),
            Some(second.join("share/marked"))
        );
        assert_eq!(
            find_package_share("plain", &prefixes),
            Some(first.join("share/plain"))
        );
        assert_eq!(find_package_share("missing", &prefixes), None);
        assert_eq!(find_package_share("", &prefixes), None);
        assert_eq!(find_package_share("../x", &prefixes), None);

        let resolved = resolve_include_path(
            Path::new("/elsewhere"),
            &include(Some("trees/a.xml"), Some("marked")),
            Some(&prefixes),
        )
        .unwrap();
        assert_eq!(
            resolved,
            normalize(&second.join("share/marked/trees/a.xml"))
        );
        let err = resolve_include_path(
            Path::new("/elsewhere"),
            &include(Some("a.xml"), Some("missing")),
            Some(&prefixes),
        )
        .unwrap_err();
        assert!(matches!(
            err,
            IncludeProblem::RosPackageNotFound {
                no_ament_prefix_path: false,
                ..
            }
        ));
        assert!(err.to_string().contains("in AMENT_PREFIX_PATH"));
    }

    #[test]
    fn problem_messages() {
        let load = IncludeProblem::Load {
            path: PathBuf::from("/a.xml"),
            message: "not found".into(),
        };
        assert!(load.to_string().contains("/a.xml"));
        let cycle = IncludeProblem::Cycle {
            path: PathBuf::from("/b.xml"),
        };
        assert!(cycle.to_string().contains("cycle"));
        assert!(IncludeProblem::MissingPath.to_string().contains("path"));
    }
}
