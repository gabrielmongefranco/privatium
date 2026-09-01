// Project:  Privatium™  |  File: crates/privatium-core/src/log/mod.rs
// Authors:  Gabriel Mongefranco (@gabrielmongefranco)
// Created:  2026-09-01  |  Modified: 2026-09-01
// Summary:  The append-only event log (spec/protocol.md §4). M1 carries only what the
//           _sys bootstrap needs: the envelope, and a writer that can start a new log
//           and nothing else. The reader, rotation, gap tolerance, batch atomicity, and
//           clock hygiene are M2.

use std::fs;
use std::io::Write as _;
use std::path::{Path, PathBuf};

use serde::Serialize;

use crate::identity::NodeId;
use crate::{Error, Result, io_at};

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

/// A `put` event, serialized in the key order of `spec/protocol.md §4.1`.
///
/// The order is what serde emits for a struct's fields, so the declaration order below is
/// load-bearing: §4.1 says readers MUST NOT depend on key order, and also that writers
/// SHOULD emit this one, because a human grepping a log file is the point.
///
/// There is no `del` here. M1 writes no tombstones, so `op` is a constant and `d` is not
/// optional; §4.1's rule that `d` MUST be absent when `op` is `del` arrives with the
/// tombstone path in M2.
#[derive(Serialize)]
struct Put<'a, D: Serialize> {
    seq: u64,
    lam: u64,
    ts: &'a str,
    dev: &'a str,
    app: &'a str,
    op: &'static str,
    tbl: &'a str,
    id: &'a str,
    d: &'a D,
}

/// The single writer of one `data/<slug>/log/<dev>.jsonl`.
///
/// `AGENTS.md` 2: a device appends only to its own log, forever.
#[derive(Debug)]
pub struct Writer {
    file: fs::File,
    path: PathBuf,
    app: String,
    dev: String,
    seq: u64,
    lam: u64,
}

impl Writer {
    /// Open a **new** log file, failing if one already exists.
    ///
    /// This is M1's only constructor, and it is deliberately incapable of appending to an
    /// existing stream. `seq` is gapless per `(device, app)` (`§4.1`) and is recovered on
    /// startup by reading the tail — that recovery is M2's, and `open()` arrives beside
    /// this then. Until it does, a writer that could attach to an existing log would emit
    /// `seq: 1` over a stream already past it: a corrupted log rather than a caught bug.
    ///
    /// Counters start at zero and are incremented before the first write, so the first
    /// event is `seq: 1, lam: 1` — `§4.1` ("starts at 1") and the worked example in
    /// `spec/data-dictionary.md §6`.
    pub fn create(path: PathBuf, app: &str, dev: &NodeId) -> Result<Self> {
        if path.exists() {
            return Err(Error::LogExists { path });
        }

        let file = fs::OpenOptions::new()
            .create_new(true)
            .append(true)
            .open(&path)
            .map_err(io_at(&path))?;

        Ok(Self {
            file,
            path,
            app: app.to_owned(),
            dev: dev.to_string(),
            seq: 0,
            lam: 0,
        })
    }

    /// Append one `put`.
    ///
    /// `ts` is taken rather than read from the clock so that a caller writing several
    /// related events can stamp them with one instant. M2 owns clock hygiene (`§4.4`) and
    /// is where any check on this value belongs.
    pub fn put<D: Serialize>(&mut self, tbl: &str, id: &str, ts: &str, d: &D) -> Result<()> {
        self.seq += 1;
        // §4.3: lam = max(lam_local, lam_max_seen) + 1. This writer has seen nothing —
        // there is no sync in Phase 1 and the log is new — so lam_max_seen is its own
        // counter and the max collapses to an increment.
        self.lam += 1;

        let event = Put {
            seq: self.seq,
            lam: self.lam,
            ts,
            dev: &self.dev,
            app: &self.app,
            op: "put",
            tbl,
            id,
            d,
        };

        let mut line = serde_json::to_vec(&event)?;
        // §4.1: `\n` terminated, 0x0A, never \r\n — on Windows too, which is why the file
        // is opened in binary mode (Rust has no text mode) and the byte is written here
        // rather than by `writeln!`.
        line.push(b'\n');

        self.file.write_all(&line).map_err(io_at(&self.path))?;
        // Sync on append. Correctness over throughput; M2 makes the policy configurable
        // and keeps this as the default.
        self.file.sync_all().map_err(io_at(&self.path))?;
        Ok(())
    }

    /// The `seq` of the last event written, or 0 if none.
    #[must_use]
    pub fn seq(&self) -> u64 {
        self.seq
    }

    /// The Lamport counter after the last event written, or 0 if none.
    #[must_use]
    pub fn lam(&self) -> u64 {
        self.lam
    }

    /// The file being appended to.
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
}
