//! File helpers.

use std::fs::{self, OpenOptions};
use std::io::{self, Write};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};

/// Writes a file safely: the function writes a temporary file in the same
/// directory, flushes it to the disk, and then renames it to the target
/// name. A reader never sees a half-written file, and an error leaves the
/// old file as it was.
///
/// If the target is a symbolic link, the function writes the file that the
/// link points to. The new file gets the permissions of the old file.
pub(crate) fn write_atomic(path: &Path, bytes: &[u8]) -> io::Result<()> {
    static COUNTER: AtomicU64 = AtomicU64::new(0);
    let target = resolve_link(path);
    let file_name = target
        .file_name()
        .ok_or_else(|| io::Error::new(io::ErrorKind::InvalidInput, "the path has no file name"))?;
    let dir = match target.parent() {
        Some(p) if !p.as_os_str().is_empty() => p.to_path_buf(),
        _ => PathBuf::from("."),
    };
    let tmp = dir.join(format!(
        ".{}.{}-{}.tmp",
        file_name.to_string_lossy(),
        std::process::id(),
        COUNTER.fetch_add(1, Ordering::Relaxed)
    ));
    let result = (|| {
        let mut file = OpenOptions::new().write(true).create_new(true).open(&tmp)?;
        file.write_all(bytes)?;
        file.sync_all()?;
        drop(file);
        if let Ok(meta) = fs::metadata(&target) {
            fs::set_permissions(&tmp, meta.permissions())?;
        }
        fs::rename(&tmp, &target)
    })();
    if result.is_err() {
        let _ = fs::remove_file(&tmp);
    }
    result
}

/// The file that a symbolic link points to, or the path itself.
fn resolve_link(path: &Path) -> PathBuf {
    match fs::symlink_metadata(path) {
        Ok(meta) if meta.file_type().is_symlink() => {
            fs::canonicalize(path).unwrap_or_else(|_| path.to_path_buf())
        }
        _ => path.to_path_buf(),
    }
}

/// Makes a path absolute (relative to the current directory) and removes
/// `.` and `..` parts without access to the file system.
pub(crate) fn normalize(path: &Path) -> PathBuf {
    use std::path::Component;
    let absolute = if path.is_absolute() {
        path.to_path_buf()
    } else {
        std::env::current_dir()
            .map(|cwd| cwd.join(path))
            .unwrap_or_else(|_| path.to_path_buf())
    };
    let mut out = PathBuf::new();
    for component in absolute.components() {
        match component {
            Component::CurDir => {}
            Component::ParentDir => {
                // `..` at the root stays at the root.
                if !matches!(
                    out.components().next_back(),
                    None | Some(Component::RootDir | Component::Prefix(_))
                ) {
                    out.pop();
                }
            }
            other => out.push(other.as_os_str()),
        }
    }
    out
}

/// Makes a path absolute and removes `.` and `..` parts, but only if that
/// does not change the file that the path names.
///
/// The operating system resolves `..` after it follows a symbolic link:
/// `a/link/../f.xml` is `f.xml` in the parent of the link target, not
/// `a/f.xml`. BT.CPP gives the joined path to the operating system without
/// normalization. So the normalized path is used only if its directory and
/// the directory of the original path are the same directory (or if
/// neither exists). Else the result is the absolute path without changes.
pub(crate) fn clean_path(path: &Path) -> PathBuf {
    let absolute = if path.is_absolute() {
        path.to_path_buf()
    } else {
        std::env::current_dir()
            .map(|cwd| cwd.join(path))
            .unwrap_or_else(|_| path.to_path_buf())
    };
    let normalized = normalize(&absolute);
    if normalized == absolute {
        return normalized;
    }
    let dir = |p: &Path| p.parent().map(fs::canonicalize);
    match (dir(&absolute), dir(&normalized)) {
        (Some(Ok(a)), Some(Ok(b))) if a == b => normalized,
        (Some(Err(_)) | None, Some(Err(_)) | None) => normalized,
        _ => absolute,
    }
}

/// The path of `target` relative to the directory `base`, with `/` as the
/// separator. Both paths are normalized first. If the paths have no common
/// root (for example two Windows drives), the result is the absolute
/// target path.
pub(crate) fn relative_path(base: &Path, target: &Path) -> String {
    use std::path::Component;
    let base = normalize(base);
    let target = normalize(target);
    let b: Vec<Component> = base.components().collect();
    let t: Vec<Component> = target.components().collect();
    let common = b.iter().zip(&t).take_while(|(x, y)| x == y).count();
    if common == 0 {
        return target.to_string_lossy().replace('\\', "/");
    }
    let mut parts: Vec<String> = Vec::new();
    for _ in common..b.len() {
        parts.push("..".to_owned());
    }
    for c in &t[common..] {
        parts.push(c.as_os_str().to_string_lossy().into_owned());
    }
    if parts.is_empty() {
        ".".to_owned()
    } else {
        parts.join("/")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn normalize_paths() {
        assert_eq!(
            normalize(Path::new("/a/b/../c/./d")),
            PathBuf::from("/a/c/d")
        );
        assert_eq!(normalize(Path::new("/../a")), PathBuf::from("/a"));
        assert!(normalize(Path::new("x/y")).is_absolute());
    }

    #[cfg(unix)]
    #[test]
    fn clean_path_keeps_the_meaning_of_links() {
        let dir = tempfile::tempdir().unwrap();
        let d = dir.path().canonicalize().unwrap();
        // d/a/link -> d/x/y; the file is d/x/b/f.xml.
        fs::create_dir_all(d.join("a")).unwrap();
        fs::create_dir_all(d.join("x/y")).unwrap();
        fs::create_dir_all(d.join("x/b")).unwrap();
        fs::write(d.join("x/b/f.xml"), "<root/>").unwrap();
        std::os::unix::fs::symlink(d.join("x/y"), d.join("a/link")).unwrap();
        let through_link = d.join("a/link/../b/f.xml");
        // The OS resolves ".." after the link.
        assert!(through_link.is_file());
        assert_eq!(clean_path(&through_link), through_link);
        // Without links, the path is normalized.
        assert_eq!(clean_path(&d.join("x/y/../b/./f.xml")), d.join("x/b/f.xml"));
        // A missing directory in the original path: the OS cannot open it,
        // so the path stays as it is (the load fails, as in BT.CPP).
        assert_eq!(
            clean_path(&d.join("none/../z.xml")),
            d.join("none/../z.xml")
        );
        // Neither directory exists: the path is normalized.
        assert_eq!(
            clean_path(&d.join("none/../other/z.xml")),
            d.join("other/z.xml")
        );
        assert_eq!(clean_path(&d.join("x/b/f.xml")), d.join("x/b/f.xml"));
        assert!(clean_path(Path::new("rel/../f.xml")).is_absolute());
    }

    #[test]
    fn relative_paths() {
        assert_eq!(
            relative_path(Path::new("/p/proj"), Path::new("/p/proj/trees/a.xml")),
            "trees/a.xml"
        );
        assert_eq!(
            relative_path(Path::new("/p/proj"), Path::new("/p/other/a.xml")),
            "../other/a.xml"
        );
        assert_eq!(
            relative_path(Path::new("/p/proj/"), Path::new("/p/proj")),
            "."
        );
        assert_eq!(relative_path(Path::new("/"), Path::new("/a/b")), "a/b");
        assert_eq!(
            relative_path(Path::new("/a/b/c"), Path::new("/x")),
            "../../../x"
        );
    }

    #[test]
    fn atomic_write() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("f.xml");
        write_atomic(&path, b"one").unwrap();
        assert_eq!(fs::read(&path).unwrap(), b"one");
        write_atomic(&path, b"two").unwrap();
        assert_eq!(fs::read(&path).unwrap(), b"two");
        // No temporary files stay.
        let names: Vec<_> = fs::read_dir(dir.path())
            .unwrap()
            .map(|e| e.unwrap().file_name())
            .collect();
        assert_eq!(names.len(), 1);
        // A missing directory is an error.
        assert!(write_atomic(&dir.path().join("missing/f.xml"), b"x").is_err());
        assert!(write_atomic(Path::new("/"), b"x").is_err());
    }

    #[cfg(unix)]
    #[test]
    fn atomic_write_keeps_link_and_permissions() {
        use std::os::unix::fs::PermissionsExt;
        let dir = tempfile::tempdir().unwrap();
        let real = dir.path().join("real.xml");
        fs::write(&real, "old").unwrap();
        fs::set_permissions(&real, fs::Permissions::from_mode(0o640)).unwrap();
        let link = dir.path().join("link.xml");
        std::os::unix::fs::symlink(&real, &link).unwrap();
        write_atomic(&link, b"new").unwrap();
        assert!(
            fs::symlink_metadata(&link)
                .unwrap()
                .file_type()
                .is_symlink()
        );
        assert_eq!(fs::read(&real).unwrap(), b"new");
        assert_eq!(
            fs::metadata(&real).unwrap().permissions().mode() & 0o777,
            0o640
        );
    }
}
