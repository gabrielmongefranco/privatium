// Project:  Privatium™  |  File: crates/privatium-core/src/backup.rs
// Authors:  Gabriel Mongefranco (@gabrielmongefranco)
// Created:  2026-09-04  |  Modified: 2026-09-04
// Summary:  `privatium restore --from <path>` (spec/cli.md §7): bringing a backed-up data/
//           folder into this node's data root before the three-tier rebuild runs. A plan
//           first, then an apply, so `--dry-run` and the real thing read the same
//           decisions. Log files are the only delicate part — a device's log is one
//           writer's, forever — so a file is copied only when this root lacks it or holds a
//           strict prefix of it, and a divergence refuses the whole restore before a byte
//           moves. Snapshots are caches and are copied when absent. local/ and cache/ are
//           never read from a backup (spec/protocol.md §3.1).

use std::collections::BTreeSet;
use std::fs;
use std::path::{Path, PathBuf};

use thiserror::Error;

use crate::config::Paths;
use crate::store::snapshot::{MANIFEST_FILE, SnapshotId};

/// What can go wrong reading a backup or writing it in.
#[derive(Debug, Error)]
pub enum BackupError {
    /// A filesystem operation failed.
    #[error("{path}: {source}")]
    Io {
        /// The file or directory.
        path: PathBuf,
        /// What the OS said.
        source: std::io::Error,
    },

    /// `--from` names nothing this understands.
    #[error(
        "{path}: not a backup — expected a data/ folder holding <slug>/log/ and <slug>/snap/ \
         (spec/protocol.md §3), or a data root containing data/"
    )]
    NotABackup {
        /// What was given.
        path: PathBuf,
    },

    /// A log in the backup and the one here have gone different ways.
    #[error(
        "refusing to restore: {count} log file(s) diverge from this node's copy (the first is \
         {first}); nothing was written. A device's log is one writer's — resolve it by hand \
         (spec/protocol.md §3.1)"
    )]
    Diverged {
        /// How many.
        count: usize,
        /// The first, as `<slug>/log/<file>`.
        first: String,
    },
}

fn io_at(path: &Path) -> impl FnOnce(std::io::Error) -> BackupError {
    move |source| BackupError::Io {
        path: path.to_path_buf(),
        source,
    }
}

/// Why a file is copied.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Reason {
    /// This root has no such file.
    Absent,
    /// This root's copy is a strict byte prefix of the backup's — the backup is ahead.
    BackupAhead,
}

impl Reason {
    /// One phrase, for the report.
    #[must_use]
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Absent => "absent here",
            Self::BackupAhead => "backup is ahead",
        }
    }
}

/// One file or snapshot directory the plan copies.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Copy {
    /// The app.
    pub slug: String,
    /// `log/<file>` or `snap/<id>`.
    pub what: String,
    /// Where it comes from.
    pub from: PathBuf,
    /// Where it goes.
    pub to: PathBuf,
    /// Why.
    pub reason: Reason,
}

/// One item the plan leaves alone.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Skip {
    /// The app.
    pub slug: String,
    /// `log/<file>` or `snap/<id>`.
    pub what: String,
    /// Why — identical, this node ahead, present.
    pub reason: String,
}

/// One log that cannot be reconciled.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Conflict {
    /// The app.
    pub slug: String,
    /// `log/<file>`.
    pub what: String,
}

/// What a restore would copy, decided without writing anything.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Plan {
    /// The `data/` folder read.
    pub from: PathBuf,
    /// In order: logs first, then snapshots, by slug.
    pub copies: Vec<Copy>,
    /// Left alone.
    pub skipped: Vec<Skip>,
    /// Diverged logs. A plan with any of these refuses to apply.
    pub conflicts: Vec<Conflict>,
}

impl Plan {
    /// Read the backup at `from` against `paths`, for one app or every app it holds.
    ///
    /// `from` may be the `data/` folder itself or a data root containing one
    /// (`docs/backup-and-restore.md §1`).
    pub fn build(from: &Path, paths: &Paths, app: Option<&str>) -> Result<Self, BackupError> {
        let data = resolve_data_dir(from)?;
        let mut plan = Self {
            from: data.clone(),
            copies: Vec::new(),
            skipped: Vec::new(),
            conflicts: Vec::new(),
        };

        let mut slugs: Vec<String> = Vec::new();
        for entry in fs::read_dir(&data).map_err(io_at(&data))? {
            let entry = entry.map_err(io_at(&data))?;
            if !entry.path().is_dir() {
                continue;
            }
            let name = entry.file_name().to_string_lossy().into_owned();
            if app.is_some_and(|wanted| wanted != name) {
                continue;
            }
            slugs.push(name);
        }
        slugs.sort();

        for slug in &slugs {
            plan.plan_logs(slug, &data.join(slug).join("log"), &paths.app_log_dir(slug))?;
        }
        for slug in &slugs {
            plan.plan_snapshots(
                slug,
                &data.join(slug).join("snap"),
                &paths.app_snap_dir(slug),
            )?;
        }
        Ok(plan)
    }

    /// Every app the backup holds, in the plan's order.
    #[must_use]
    pub fn slugs(&self) -> BTreeSet<String> {
        self.copies
            .iter()
            .map(|c| c.slug.clone())
            .chain(self.skipped.iter().map(|s| s.slug.clone()))
            .chain(self.conflicts.iter().map(|c| c.slug.clone()))
            .collect()
    }

    /// Whether anything stops this plan from applying.
    #[must_use]
    pub fn is_applicable(&self) -> bool {
        self.conflicts.is_empty()
    }

    /// Copy everything the plan names. Refuses outright while a conflict stands, so a
    /// diverged log is never half-restored.
    pub fn apply(&self) -> Result<(), BackupError> {
        if let Some(first) = self.conflicts.first() {
            return Err(BackupError::Diverged {
                count: self.conflicts.len(),
                first: format!("{}/{}", first.slug, first.what),
            });
        }
        for copy in &self.copies {
            if copy.from.is_dir() {
                copy_dir(&copy.from, &copy.to)?;
            } else {
                if let Some(parent) = copy.to.parent() {
                    fs::create_dir_all(parent).map_err(io_at(parent))?;
                }
                fs::copy(&copy.from, &copy.to).map_err(io_at(&copy.from))?;
            }
        }
        Ok(())
    }

    fn plan_logs(&mut self, slug: &str, from: &Path, to: &Path) -> Result<(), BackupError> {
        let mut files = list_files(from, |name| name.ends_with(".jsonl"))?;
        files.sort();
        for name in files {
            let source = from.join(&name);
            let target = to.join(&name);
            let what = format!("log/{name}");
            if !target.exists() {
                self.copies.push(Copy {
                    slug: slug.to_owned(),
                    what,
                    from: source,
                    to: target,
                    reason: Reason::Absent,
                });
                continue;
            }
            let theirs = fs::read(&source).map_err(io_at(&source))?;
            let ours = fs::read(&target).map_err(io_at(&target))?;
            if theirs == ours {
                self.skipped.push(Skip {
                    slug: slug.to_owned(),
                    what,
                    reason: "identical".to_owned(),
                });
            } else if theirs.starts_with(&ours) {
                self.copies.push(Copy {
                    slug: slug.to_owned(),
                    what,
                    from: source,
                    to: target,
                    reason: Reason::BackupAhead,
                });
            } else if ours.starts_with(&theirs) {
                self.skipped.push(Skip {
                    slug: slug.to_owned(),
                    what,
                    reason: "this node is ahead".to_owned(),
                });
            } else {
                self.conflicts.push(Conflict {
                    slug: slug.to_owned(),
                    what,
                });
            }
        }
        Ok(())
    }

    fn plan_snapshots(&mut self, slug: &str, from: &Path, to: &Path) -> Result<(), BackupError> {
        let entries = match fs::read_dir(from) {
            Ok(entries) => entries,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(()),
            Err(error) => return Err(io_at(from)(error)),
        };
        let mut ids: Vec<String> = Vec::new();
        for entry in entries {
            let entry = entry.map_err(io_at(from))?;
            let name = entry.file_name().to_string_lossy().into_owned();
            let path = entry.path();
            if path.is_dir()
                && name.parse::<SnapshotId>().is_ok()
                && path.join(MANIFEST_FILE).is_file()
            {
                ids.push(name);
            }
        }
        ids.sort();
        for id in ids {
            let what = format!("snap/{id}");
            let target = to.join(&id);
            if target.exists() {
                self.skipped.push(Skip {
                    slug: slug.to_owned(),
                    what,
                    reason: "present".to_owned(),
                });
            } else {
                self.copies.push(Copy {
                    slug: slug.to_owned(),
                    what,
                    from: from.join(&id),
                    to: target,
                    reason: Reason::Absent,
                });
            }
        }
        Ok(())
    }
}

/// `from` itself when it holds `<slug>/log/` directories, else `from/data` when that does.
fn resolve_data_dir(from: &Path) -> Result<PathBuf, BackupError> {
    if holds_app_dirs(from)? {
        return Ok(from.to_path_buf());
    }
    let nested = from.join("data");
    if nested.is_dir() && holds_app_dirs(&nested)? {
        return Ok(nested);
    }
    Err(BackupError::NotABackup {
        path: from.to_path_buf(),
    })
}

/// Whether any child of `dir` looks like `data/<slug>/` — has a `log/` or `snap/`.
fn holds_app_dirs(dir: &Path) -> Result<bool, BackupError> {
    let entries = match fs::read_dir(dir) {
        Ok(entries) => entries,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(false),
        Err(error) => return Err(io_at(dir)(error)),
    };
    for entry in entries {
        let path = entry.map_err(io_at(dir))?.path();
        if path.is_dir() && (path.join("log").is_dir() || path.join("snap").is_dir()) {
            return Ok(true);
        }
    }
    Ok(false)
}

fn list_files(dir: &Path, keep: impl Fn(&str) -> bool) -> Result<Vec<String>, BackupError> {
    let entries = match fs::read_dir(dir) {
        Ok(entries) => entries,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(Vec::new()),
        Err(error) => return Err(io_at(dir)(error)),
    };
    let mut names = Vec::new();
    for entry in entries {
        let entry = entry.map_err(io_at(dir))?;
        let name = entry.file_name().to_string_lossy().into_owned();
        if entry.path().is_file() && keep(&name) {
            names.push(name);
        }
    }
    Ok(names)
}

/// Copy a snapshot directory: its files, one level, which is all `§5.1` has.
fn copy_dir(from: &Path, to: &Path) -> Result<(), BackupError> {
    fs::create_dir_all(to).map_err(io_at(to))?;
    for entry in fs::read_dir(from).map_err(io_at(from))? {
        let entry = entry.map_err(io_at(from))?;
        let path = entry.path();
        if path.is_file() {
            fs::copy(&path, to.join(entry.file_name())).map_err(io_at(&path))?;
        }
    }
    Ok(())
}

// AGENTS.md, Style: unwrap() is permitted in tests. The crate-level deny reaches unit
// tests inside src/, so each one opts out where it is declared.
#[allow(clippy::unwrap_used, clippy::expect_used)]
#[cfg(test)]
mod tests {
    use super::*;

    fn write(path: &Path, text: &str) {
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        fs::write(path, text).unwrap();
    }

    #[test]
    fn logs_are_copied_when_absent_or_ahead_and_refused_when_diverged() {
        let backup = tempfile::tempdir().unwrap();
        let root = tempfile::tempdir().unwrap();
        let paths = Paths::rooted(root.path());
        let data = backup.path().join("data");
        write(&data.join("hello/log/aaa.jsonl"), "1\n2\n3\n");
        write(&data.join("hello/log/bbb.jsonl"), "1\n");
        write(&data.join("hello/log/ccc.jsonl"), "1\n2\n");
        write(&data.join("hello/log/ddd.jsonl"), "1\nX\n");
        write(&data.join("hello/snap/2026-W35-aaa-3/MANIFEST.json"), "{}");
        write(&data.join("hello/snap/junk/MANIFEST.json"), "{}");
        // Ours: absent aaa, identical bbb, ahead ccc's prefix, diverged ddd.
        write(&paths.app_log_dir("hello").join("bbb.jsonl"), "1\n");
        write(&paths.app_log_dir("hello").join("ccc.jsonl"), "1\n2\n3\n");
        write(&paths.app_log_dir("hello").join("ddd.jsonl"), "1\n2\n");

        let plan = Plan::build(backup.path(), &paths, None).unwrap();
        assert_eq!(plan.from, data);
        let copies: Vec<(&str, Reason)> = plan
            .copies
            .iter()
            .map(|c| (c.what.as_str(), c.reason))
            .collect();
        assert_eq!(
            copies,
            [
                ("log/aaa.jsonl", Reason::Absent),
                ("snap/2026-W35-aaa-3", Reason::Absent)
            ]
        );
        let skipped: Vec<(&str, &str)> = plan
            .skipped
            .iter()
            .map(|s| (s.what.as_str(), s.reason.as_str()))
            .collect();
        assert_eq!(
            skipped,
            [
                ("log/bbb.jsonl", "identical"),
                ("log/ccc.jsonl", "this node is ahead")
            ]
        );
        assert_eq!(plan.conflicts.len(), 1);
        assert_eq!(plan.conflicts[0].what, "log/ddd.jsonl");
        assert!(!plan.is_applicable());
        let error = plan.apply().unwrap_err();
        assert!(
            matches!(error, BackupError::Diverged { count: 1, .. }),
            "{error}"
        );
        assert!(!paths.app_log_dir("hello").join("aaa.jsonl").exists());

        // Without the diverged file the plan applies, and the backup-ahead case copies.
        fs::remove_file(data.join("hello/log/ddd.jsonl")).unwrap();
        fs::write(data.join("hello/log/ccc.jsonl"), "1\n2\n3\n4\n").unwrap();
        let plan = Plan::build(&data, &paths, Some("hello")).unwrap();
        assert!(plan.is_applicable());
        plan.apply().unwrap();
        assert_eq!(
            fs::read_to_string(paths.app_log_dir("hello").join("ccc.jsonl")).unwrap(),
            "1\n2\n3\n4\n"
        );
        assert!(paths.app_log_dir("hello").join("aaa.jsonl").exists());
        assert!(
            paths
                .app_snap_dir("hello")
                .join("2026-W35-aaa-3")
                .join(MANIFEST_FILE)
                .exists()
        );
        assert!(!paths.app_snap_dir("hello").join("junk").exists());
    }

    #[test]
    fn something_that_is_not_a_backup_is_refused() {
        let dir = tempfile::tempdir().unwrap();
        let paths = Paths::rooted(dir.path().join("root"));
        let error = Plan::build(dir.path(), &paths, None).unwrap_err();
        assert!(matches!(error, BackupError::NotABackup { .. }), "{error}");
    }
}
