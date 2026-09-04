// Project:  Privatium™  |  File: crates/privatium-core/src/lock.rs
// Authors:  Gabriel Mongefranco (@gabrielmongefranco)
// Created:  2026-09-05  |  Modified: 2026-09-05
// Summary:  The one-process rule of spec/protocol.md §3.1: whoever has a data root open
//           holds an exclusive OS lock on local/lock, and a second process is refused
//           rather than let mint `seq` beside the first. A node, `snapshot`, `restore`
//           and anything else that opens a Node takes it; it is released when the handle
//           closes, so a crash leaves nothing stale behind.

use std::fs;
use std::path::{Path, PathBuf};

use crate::config::Paths;
use crate::{Error, Result, io_at};

/// The file the lock is taken on, under `local/` because it is node-local by
/// definition (`spec/protocol.md §3`).
pub const LOCK_FILE: &str = "lock";

/// An exclusive lock on a data root, held for as long as this value lives.
///
/// `File::try_lock` is `flock(2)` on Unix and `LockFileEx` on Windows — an advisory
/// lock either way, which is enough: every writer of a Privatium root goes through
/// [`Node`](crate::Node), and `Node` will not open without one of these. Two opens in one
/// process are refused too, since each takes its own handle.
#[derive(Debug)]
pub struct DataLock {
    file: fs::File,
    path: PathBuf,
    paths: Paths,
}

impl DataLock {
    /// Take the lock for `paths`, creating `local/` and the file if they are absent.
    ///
    /// [`Error::Locked`] names the file when another process — or another open of the
    /// same root in this one — holds it.
    pub fn acquire(paths: Paths) -> Result<Self> {
        let dir = paths.local_dir();
        fs::create_dir_all(&dir).map_err(io_at(&dir))?;
        let path = paths.local_lock();
        let file = fs::OpenOptions::new()
            .create(true)
            .read(true)
            .write(true)
            .truncate(false)
            .open(&path)
            .map_err(io_at(&path))?;
        match file.try_lock() {
            Ok(()) => {}
            Err(fs::TryLockError::WouldBlock) => return Err(Error::Locked { path }),
            Err(fs::TryLockError::Error(source)) => return Err(io_at(&path)(source)),
        }
        Ok(Self { file, path, paths })
    }

    /// The root this lock was taken for.
    #[must_use]
    pub fn paths(&self) -> &Paths {
        &self.paths
    }

    /// `local/lock`.
    #[must_use]
    pub fn path(&self) -> &Path {
        &self.path
    }
}

impl Drop for DataLock {
    fn drop(&mut self) {
        // Closing the handle releases the lock on every platform; unlocking first makes
        // the release explicit and immediate rather than a property of drop order.
        let _ = self.file.unlock();
    }
}

// AGENTS.md, Style: unwrap() is permitted in tests. The crate-level deny reaches unit
// tests inside src/, so each one opts out where it is declared.
#[allow(clippy::unwrap_used, clippy::expect_used)]
#[cfg(test)]
mod tests {
    use super::*;

    /// `§3.1`: a second open of one root is refused while the first stands, in this
    /// process as in another, and allowed again once the first is released.
    #[test]
    fn a_root_is_held_by_one_lock_at_a_time() {
        let dir = tempfile::tempdir().unwrap();
        let paths = Paths::rooted(dir.path());
        let first = DataLock::acquire(paths.clone()).unwrap();
        assert!(first.path().ends_with("local/lock") || first.path().ends_with("local\\lock"));

        let second = DataLock::acquire(paths.clone());
        assert!(
            matches!(second, Err(Error::Locked { .. })),
            "a second lock was granted: {second:?}"
        );

        drop(first);
        assert!(DataLock::acquire(paths).is_ok());
    }
}
