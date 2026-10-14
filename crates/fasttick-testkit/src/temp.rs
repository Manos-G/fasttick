//! Temporary directories for tests.

use std::fs;
use std::io;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{SystemTime, UNIX_EPOCH};

/// A new, empty directory under [`std::env::temp_dir`]. Drop removes it
/// with all its contents.
#[derive(Debug)]
pub struct TempDir {
    path: PathBuf,
    keep: bool,
}

impl TempDir {
    /// Makes a new directory with a unique name that contains `label`.
    ///
    /// Characters of `label` that are not ASCII letters, digits, `-` or `_`
    /// become `_`.
    pub fn new(label: &str) -> io::Result<TempDir> {
        static COUNTER: AtomicU64 = AtomicU64::new(0);

        let label: String = label
            .chars()
            .take(40)
            .map(|c| {
                if c.is_ascii_alphanumeric() || c == '-' || c == '_' {
                    c
                } else {
                    '_'
                }
            })
            .collect();
        let nanos = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map(|time| time.subsec_nanos())
            .unwrap_or(0);
        let base = std::env::temp_dir();
        loop {
            let count = COUNTER.fetch_add(1, Ordering::Relaxed);
            let name = format!(
                "fasttick-{label}-{}-{count}-{nanos:08x}",
                std::process::id()
            );
            let path = base.join(name);
            match fs::create_dir(&path) {
                Ok(()) => return Ok(TempDir { path, keep: false }),
                Err(error) if error.kind() == io::ErrorKind::AlreadyExists => continue,
                Err(error) => return Err(error),
            }
        }
    }

    /// The path of the directory.
    pub fn path(&self) -> &Path {
        &self.path
    }

    /// Gives `<directory>/<relative>`.
    pub fn join(&self, relative: impl AsRef<Path>) -> PathBuf {
        self.path.join(relative)
    }

    /// Keeps the directory after drop (useful to look at a failed test).
    /// Gives its path.
    pub fn keep(mut self) -> PathBuf {
        self.keep = true;
        self.path.clone()
    }
}

impl Drop for TempDir {
    fn drop(&mut self) {
        if !self.keep {
            let _ = fs::remove_dir_all(&self.path);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn creates_unique_directories_and_removes_them() {
        let first = TempDir::new("unit").unwrap();
        let second = TempDir::new("unit").unwrap();
        assert_ne!(first.path(), second.path());
        assert!(first.path().is_dir());
        fs::write(first.join("file.txt"), b"data").unwrap();
        fs::create_dir(first.join("sub")).unwrap();
        let path = first.path().to_path_buf();
        drop(first);
        assert!(!path.exists());
    }

    #[test]
    fn label_is_sanitized() {
        let dir = TempDir::new("a/b c..").unwrap();
        let name = dir.path().file_name().unwrap().to_str().unwrap().to_owned();
        assert!(name.starts_with("fasttick-a_b_c__-"), "{name}");
        assert_eq!(dir.path().parent(), Some(std::env::temp_dir().as_path()));
    }

    #[test]
    fn keep_leaves_the_directory() {
        let dir = TempDir::new("keep").unwrap();
        let path = dir.keep();
        assert!(path.is_dir());
        fs::remove_dir_all(&path).unwrap();
    }
}
