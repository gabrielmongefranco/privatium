// Project:  Privatium™  |  File: crates/privatium-core/src/durable.rs
// Authors:  Gabriel Mongefranco (@gabrielmongefranco)
// Created:  2026-09-05  |  Modified: 2026-09-05
// Summary:  The last step of making a file exist: after a new file's bytes are on disk,
//           its directory entry has to be too, or a power cut can lose a file whose
//           contents were flushed. Unix filesystems ask for an fsync of the directory;
//           Windows has no such call, and NTFS journals its metadata itself.

use std::fs;
use std::io::{self, Write as _};
use std::path::Path;

/// Flush the directory entry of a file just created, renamed into, or appended to under
/// `dir`.
///
/// On Unix this opens the directory and calls `fsync` on it, which is what makes a new
/// name durable on ext4, XFS, APFS and their relatives. On Windows a directory cannot be
/// flushed this way — `FlushFileBuffers` wants a handle with write access, which a
/// directory does not grant — and NTFS writes its metadata through a journal, so the
/// call is a no-op there and says so rather than pretending.
pub(crate) fn sync_dir(dir: &Path) -> io::Result<()> {
    #[cfg(unix)]
    {
        fs::File::open(dir)?.sync_all()
    }
    #[cfg(not(unix))]
    {
        let _ = dir;
        Ok(())
    }
}

/// Write a whole file and make it durable: the bytes, then the file's metadata, then the
/// directory entry. For files a manifest will name — a snapshot's tables and CSVs — and
/// anything else that must not be found empty after a crash.
pub(crate) fn write_synced(path: &Path, bytes: &[u8]) -> io::Result<()> {
    let mut file = fs::OpenOptions::new()
        .write(true)
        .create(true)
        .truncate(true)
        .open(path)?;
    file.write_all(bytes)?;
    file.sync_all()?;
    drop(file);
    if let Some(dir) = path.parent() {
        sync_dir(dir)?;
    }
    Ok(())
}

/// Flush a file that was written by someone else's handle — through one opened for
/// writing, which is what `FlushFileBuffers` requires on Windows; a read-only handle is
/// refused there with "access denied".
pub(crate) fn sync_file(path: &Path) -> io::Result<()> {
    fs::OpenOptions::new().write(true).open(path)?.sync_all()
}

// AGENTS.md, Style: unwrap() is permitted in tests. The crate-level deny reaches unit
// tests inside src/, so each one opts out where it is declared.
#[allow(clippy::unwrap_used, clippy::expect_used)]
#[cfg(test)]
mod tests {
    use super::*;

    /// Both calls succeed on every platform CI runs, which is the whole contract a
    /// test can hold them to: durability itself needs a power cut.
    #[test]
    fn syncing_a_directory_and_writing_a_file_succeed() {
        let dir = tempfile::tempdir().expect("tempdir");
        sync_dir(dir.path()).expect("sync_dir");
        let file = dir.path().join("a.txt");
        write_synced(&file, b"x").expect("write_synced");
        assert_eq!(fs::read(&file).expect("read"), b"x");
        write_synced(&file, b"yz").expect("write_synced replaces");
        assert_eq!(fs::read(&file).expect("read"), b"yz");
        sync_file(&file).expect("sync_file");
    }
}
