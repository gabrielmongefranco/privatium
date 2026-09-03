// Project:  Privatium™  |  File: crates/privatium-core/src/local.rs
// Authors:  Gabriel Mongefranco (@gabrielmongefranco)
// Created:  2026-09-01  |  Modified: 2026-09-03
// Summary:  local/state.jsonl — the node-local state of spec/protocol.md §3. Never synced,
//           never backed up, and never required for restore. In M2 it holds one record per
//           app: the Lamport counter and the highest `seq` seen per device.

use std::collections::BTreeMap;
use std::fs;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

use crate::{Result, io_at};

/// What this node knows about one app's log without having to read it.
///
/// Everything here is **recoverable from the logs**, and that is the point: nothing about
/// correctness may depend on this file. `spec/protocol.md §3` says `local/` is not required
/// for restore, `AGENTS.md` says never sync it, and `docs/backup-and-restore.md` tells
/// owners to copy `data/` and nothing else. A node whose `local/` has been deleted must come
/// back with the same Lamport counter, and
/// `test_spec_4_3_lamport_survives_restart` checks exactly that, twice.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Record {
    /// The app slug this record is about.
    pub app: String,
    /// The Lamport counter as of the last flush (`§4.3`).
    pub lam: u64,
    /// The highest `seq` seen per device, whether the event was accepted or not.
    ///
    /// Two jobs. It tells a later start which lines it has already looked at, so a
    /// `§4.4` rejection is reported once rather than on every restart; and in Phase 3 it is
    /// what a sync receiver compares a peer's heads against (`§10.1`).
    #[serde(default)]
    pub heads: BTreeMap<String, u64>,
    /// What the app's `cache/<slug>.sqlite` was last built from (M3).
    ///
    /// The `schema.sql` hash and a length per log segment. Two jobs, both of them
    /// "notice that the tables are stale": a changed hash is `spec/app-contract.md §4.5`'s
    /// rematerialize-on-schema-change, and a changed length is a line someone appended by
    /// hand, which `apps/hello/README.md` blesses and expects to see on the next page load
    /// rather than the next restart.
    ///
    /// It goes here rather than in a file of its own because `spec/protocol.md §3` shows
    /// `local/` holding `state.jsonl` and nothing else, and because it is a cache in
    /// exactly the way the rest of this record is: lose it and the next start
    /// rematerializes, which costs work and no data.
    ///
    ///
    /// M4 adds which restore tier built the tables and from which snapshot
    /// (`store::RestoreRecord`). Node-local for the same reason as the rest: a tier is a
    /// fact about this node's cache, and copying it to another machine would be a lie.
    ///
    /// `#[serde(default)]` so a `state.jsonl` written by M2 still loads.
    #[serde(default)]
    pub materialized: crate::store::Materialized,
    /// When the record was written. For a human reading the file; nothing parses it.
    pub at: String,
}

/// `local/state.jsonl`, loaded.
///
/// One JSON object per line, last record wins per app — which is what the extension means.
/// Rewritten whole on [`flush`](Self::flush) rather than appended to: the file holds one
/// line per app, a full rewrite compacts it for free, and `AGENTS.md` invariant 3's
/// append-only rule governs **log files** under `data/`, not this.
#[derive(Debug, Clone)]
pub struct State {
    path: PathBuf,
    records: BTreeMap<String, Record>,
    dirty: bool,
}

impl State {
    /// Read `local/state.jsonl`, or start empty if it is absent.
    ///
    /// A line that does not parse is skipped rather than fatal. This is a cache: refusing to
    /// start because a disposable file got scrambled would turn a shrug into an outage, and
    /// the logs hold everything this file remembers anyway.
    pub fn load(path: &Path) -> Result<Self> {
        let mut state = Self {
            path: path.to_path_buf(),
            records: BTreeMap::new(),
            dirty: false,
        };

        let text = match fs::read_to_string(path) {
            Ok(text) => text,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(state),
            Err(error) => return Err(io_at(path)(error)),
        };

        let mut lines = 0usize;
        for line in text.lines().filter(|line| !line.trim().is_empty()) {
            lines += 1;
            if let Ok(record) = serde_json::from_str::<Record>(line) {
                state.records.insert(record.app.clone(), record);
            }
        }

        // More lines than records means an older build appended rather than rewrote, or a
        // record was dropped. Marking it dirty makes the next flush compact the file.
        state.dirty = lines != state.records.len();
        Ok(state)
    }

    /// What is known about one app, if anything.
    #[must_use]
    pub fn get(&self, app: &str) -> Option<&Record> {
        self.records.get(app)
    }

    /// Record what is now known about one app's log.
    ///
    /// Leaves the materialization watermark alone: the log counter and the cache's state
    /// are set by different callers at different moments, and folding them into one write
    /// would mean whichever ran second erased the other.
    pub fn set(&mut self, app: &str, lam: u64, heads: BTreeMap<String, u64>) {
        self.update(app, |record| {
            record.lam = lam;
            record.heads = heads;
        });
    }

    /// Record what one app's `cache/<slug>.sqlite` was built from (M3).
    pub fn set_materialized(&mut self, app: &str, materialized: crate::store::Materialized) {
        self.update(app, |record| record.materialized = materialized);
    }

    /// Amend one app's record in place, marking the file dirty only if something changed.
    ///
    /// The comparison deliberately ignores `at`. It is a human-readable breadcrumb that
    /// nothing parses, and including it would make every call a change — which would
    /// rewrite `local/state.jsonl` on every request once M6 calls `refresh` on the read
    /// path.
    fn update<F: FnOnce(&mut Record)>(&mut self, app: &str, amend: F) {
        let existing = self.records.get(app);
        let mut amended = existing.cloned().unwrap_or_else(|| Record {
            app: app.to_owned(),
            ..Record::default()
        });
        amend(&mut amended);

        // Compare with `at` held equal, so only the facts count.
        amended
            .at
            .clone_from(&existing.map(|r| r.at.clone()).unwrap_or_default());
        if existing == Some(&amended) {
            return;
        }

        amended.at = crate::log::now();
        self.records.insert(app.to_owned(), amended);
        self.dirty = true;
    }

    /// Write the file, if anything changed.
    ///
    /// Temp file then rename, so a crash mid-write leaves the previous state rather than
    /// half of this one. The temp file is not fsynced first: a `local/` that loses its last
    /// update is a `local/` that makes the next start do a little more work, which is the
    /// worst thing that can happen to a cache.
    pub fn flush(&mut self) -> Result<()> {
        if !self.dirty {
            return Ok(());
        }

        let mut out = String::new();
        for record in self.records.values() {
            out.push_str(&serde_json::to_string(record)?);
            out.push('\n');
        }

        let temp = self.path.with_extension("jsonl.tmp");
        fs::write(&temp, out.as_bytes()).map_err(io_at(&temp))?;
        fs::rename(&temp, &self.path).map_err(io_at(&self.path))?;

        self.dirty = false;
        Ok(())
    }

    /// The file this state came from.
    #[must_use]
    pub fn path(&self) -> &Path {
        &self.path
    }
}

// AGENTS.md, Style: unwrap() is permitted in tests. The crate-level deny reaches unit
// tests inside src/, so each one opts out where it is declared.
#[allow(clippy::unwrap_used, clippy::expect_used)]
#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn an_absent_file_loads_as_empty_state() {
        let dir = tempfile::tempdir().unwrap();
        let state = State::load(&dir.path().join("state.jsonl")).unwrap();
        assert!(state.get("_sys").is_none());
    }

    #[test]
    fn a_record_round_trips_through_the_file() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("state.jsonl");

        let mut state = State::load(&path).unwrap();
        state.set("_sys", 42, BTreeMap::from([("as3nn9tm".to_owned(), 7)]));
        state.flush().unwrap();

        let reloaded = State::load(&path).unwrap();
        let record = reloaded.get("_sys").unwrap();
        assert_eq!(record.lam, 42);
        assert_eq!(record.heads.get("as3nn9tm"), Some(&7));
    }

    /// Last record wins, and the rewrite compacts. A file that has accumulated several
    /// records for one app must load as one and be written back as one.
    #[test]
    fn duplicate_records_collapse_and_the_file_is_compacted() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("state.jsonl");
        fs::write(
            &path,
            "{\"app\":\"_sys\",\"lam\":1,\"at\":\"x\"}\n{\"app\":\"_sys\",\"lam\":9,\"at\":\"y\"}\n",
        )
        .unwrap();

        let mut state = State::load(&path).unwrap();
        assert_eq!(state.get("_sys").unwrap().lam, 9);

        state.flush().unwrap();
        assert_eq!(fs::read_to_string(&path).unwrap().lines().count(), 1);
    }

    /// A scrambled cache is a shrug, not an outage.
    #[test]
    fn a_line_that_does_not_parse_is_skipped() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("state.jsonl");
        fs::write(
            &path,
            "not json at all\n{\"app\":\"hello\",\"lam\":3,\"at\":\"z\"}\n",
        )
        .unwrap();

        let state = State::load(&path).unwrap();
        assert_eq!(state.get("hello").unwrap().lam, 3);
    }

    #[test]
    fn flushing_unchanged_state_writes_nothing() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("state.jsonl");

        let mut state = State::load(&path).unwrap();
        state.flush().unwrap();
        assert!(!path.exists(), "an empty, unchanged state created a file");
    }
}
