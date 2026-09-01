// Project:  Privatium™  |  File: crates/privatium-core/src/log/mod.rs
// Authors:  Gabriel Mongefranco (@gabrielmongefranco)
// Created:  2026-09-01  |  Modified: 2026-09-01
// Summary:  The append-only event log (spec/protocol.md §4). This module is the timestamp
//           printer, the `op` and envelope types, and AppLog — one app's log, which owns
//           the §4.3 Lamport counter and the single writer §3.1 allows this node.

use std::collections::BTreeMap;
use std::fs;
use std::path::{Path, PathBuf};

use serde::Serialize;

use crate::config::Paths;
use crate::identity::NodeId;
use crate::local::State;
use crate::{Result, io_at};

mod envelope;
mod lamport;
mod reader;
mod writer;

pub use envelope::Op;
pub use lamport::Lamport;
pub use reader::{Line, Lines, Malformed, Reader, Recovered, Rejected, Segment, Skew};
pub use writer::{Batch, Durability, Writer};

/// RFC 3339 UTC with millisecond precision and a literal `Z` (`spec/protocol.md §4.1`).
///
/// Exactly three subsecond digits, always — the default printer trims trailing zeros,
/// which would make `ts` vary in width and break the greppability §4.1 asks for.
const TIMESTAMP: jiff::fmt::temporal::DateTimePrinter =
    jiff::fmt::temporal::DateTimePrinter::new().precision(Some(3));

/// The current instant, formatted as `ts`.
#[must_use]
pub fn now() -> String {
    TIMESTAMP.timestamp_to_string(&jiff::Timestamp::now())
}

/// One app's event log: every segment on disk, the one this node writes, and the app's
/// Lamport counter.
///
/// This is the type everything above M2 appends through. It owns the [`Lamport`] counter
/// because `§4.3` makes that per **app** — in Phase 1 there is exactly one writer per app so
/// the two coincide, and in Phase 3 a sync receiver folds another device's events into the
/// same counter without going through this node's writer.
///
/// Opening one is where `seq` and `lam` are recovered. What comes back beside it is a
/// [`Recovered`], which carries anything the scan found that the owner should hear about —
/// `§4.4` rejections and clock skew. Turning those into `sys_audit` rows is
/// [`Node`](crate::Node)'s job, not this type's: `_sys` has to be open before anything can
/// be audited, and a log that reached into another log to report itself would make that
/// order impossible to see.
#[derive(Debug)]
pub struct AppLog {
    slug: String,
    dev: NodeId,
    log_dir: PathBuf,
    writer: Writer,
    lamport: Lamport,
    heads: BTreeMap<String, u64>,
}

impl AppLog {
    /// Open `data/<slug>/log/`, recovering `seq` and the Lamport counter.
    ///
    /// The log directory is created if absent, so an app's first append does not need a
    /// separate "make the app" step. `_sys` already has its directory from
    /// [`Paths::create_tree`](crate::Paths::create_tree), which has to run before any app is
    /// read (`docs/plans/phase-1.md §2.6`).
    pub fn open(
        paths: &Paths,
        slug: &str,
        dev: &NodeId,
        durability: Durability,
        state: &State,
    ) -> Result<(Self, Recovered)> {
        let log_dir = paths.app_log_dir(slug);
        fs::create_dir_all(&log_dir).map_err(io_at(&log_dir))?;

        let known = state.get(slug);
        let known_lam = known.map_or(0, |record| record.lam);
        let empty = BTreeMap::new();
        let known_heads = known.map_or(&empty, |record| &record.heads);

        let reader = Reader::open(&log_dir)?;
        let recovered =
            reader::recover(&reader, dev, known_lam, known_heads, jiff::Timestamp::now())?;

        // `create` where there is no log yet, `open` where there is. Routing everything
        // through `open` would work and would quietly retire a guarantee worth keeping:
        // `create` cannot append to an existing stream, so "this is the first line of a new
        // log" stays a promise the type makes rather than a thing the caller hopes.
        let path = paths.app_log(slug, dev);
        let writer = if path.exists() {
            Writer::open(path, slug, dev, recovered.own_seq, durability)?
        } else {
            Writer::create(path, slug, dev, durability)?
        };

        let log = Self {
            slug: slug.to_owned(),
            dev: dev.clone(),
            log_dir,
            writer,
            lamport: recovered.lam,
            heads: recovered.heads.clone(),
        };
        Ok((log, recovered))
    }

    /// Append one `put`, returning its `seq`.
    pub fn put<D: Serialize>(&mut self, tbl: &str, id: &str, d: &D) -> Result<u64> {
        let seq = self.writer.put(&mut self.lamport, tbl, id, d)?;
        self.note_own_head(seq);
        Ok(seq)
    }

    /// Append one tombstone (`§4.6`), returning its `seq`.
    pub fn del(&mut self, tbl: &str, id: &str) -> Result<u64> {
        let seq = self.writer.del(&mut self.lamport, tbl, id)?;
        self.note_own_head(seq);
        Ok(seq)
    }

    /// Append several events atomically under one `ts` and contiguous `seq`.
    ///
    /// See [`Writer::batch`] for what "atomically" can and cannot mean on a file that has to
    /// stay appendable by `echo`.
    pub fn batch<F>(&mut self, build: F) -> Result<usize>
    where
        F: FnOnce(&mut Batch<'_>) -> Result<()>,
    {
        let count = self.writer.batch(&mut self.lamport, build)?;
        let seq = self.writer.seq();
        self.note_own_head(seq);
        Ok(count)
    }

    /// A fresh view of every segment on disk.
    ///
    /// Rebuilt on each call rather than cached: another process, a restore, or a hand-run
    /// `echo` can add a line between two calls, and a stale segment list is how a reader
    /// starts quietly missing the tail.
    pub fn reader(&self) -> Result<Reader> {
        Reader::open(&self.log_dir)
    }

    /// Record what this log now knows into `local/state.jsonl`.
    pub fn save_to(&self, state: &mut State) {
        state.set(&self.slug, self.lamport.get(), self.heads.clone());
    }

    /// The app slug.
    #[must_use]
    pub fn slug(&self) -> &str {
        &self.slug
    }

    /// This node's ID — the `dev` of every event this log writes.
    #[must_use]
    pub fn dev(&self) -> &NodeId {
        &self.dev
    }

    /// The `seq` of the last event this node wrote.
    #[must_use]
    pub fn seq(&self) -> u64 {
        self.writer.seq()
    }

    /// The Lamport counter (`§4.3`).
    #[must_use]
    pub fn lam(&self) -> u64 {
        self.lamport.get()
    }

    /// The highest `seq` seen per device.
    #[must_use]
    pub fn heads(&self) -> &BTreeMap<String, u64> {
        &self.heads
    }

    /// `data/<slug>/log/`.
    #[must_use]
    pub fn log_dir(&self) -> &Path {
        &self.log_dir
    }

    /// The file this node appends to.
    #[must_use]
    pub fn path(&self) -> &Path {
        self.writer.path()
    }

    fn note_own_head(&mut self, seq: u64) {
        let head = self.heads.entry(self.dev.to_string()).or_default();
        *head = (*head).max(seq);
    }
}

// AGENTS.md, Style: unwrap() is permitted in tests. The crate-level deny reaches unit
// tests inside src/, so each one opts out where it is declared.
#[allow(clippy::unwrap_used, clippy::expect_used)]
#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ts_has_exactly_three_subsecond_digits_and_a_literal_z() {
        // Whole seconds is the case a trailing-zero-trimming printer gets wrong.
        let whole = jiff::Timestamp::from_millisecond(1_756_000_000_000).unwrap();
        assert_eq!(
            TIMESTAMP.timestamp_to_string(&whole),
            "2025-08-24T01:46:40.000Z"
        );

        let fractional = jiff::Timestamp::from_millisecond(1_756_000_000_412).unwrap();
        assert_eq!(
            TIMESTAMP.timestamp_to_string(&fractional),
            "2025-08-24T01:46:40.412Z"
        );
    }

    #[test]
    fn now_matches_the_envelope_shape() {
        let ts = now();
        assert_eq!(ts.len(), "2026-08-28T14:03:11.412Z".len(), "{ts}");
        assert!(ts.ends_with('Z'), "{ts}");
        assert_eq!(&ts[23..24], "Z", "{ts}");
    }

    /// `now()` is what the writer stamps and what `§4.4` compares against, so it has to
    /// round-trip through the parser the reader uses. A printer and a parser that disagree
    /// would make every event look like a clock problem.
    #[test]
    fn now_parses_back_as_a_timestamp() {
        let ts = now();
        assert!(ts.parse::<jiff::Timestamp>().is_ok(), "{ts}");
    }
}
