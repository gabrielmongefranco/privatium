// Project:  Privatium™  |  File: crates/privatium-core/src/log/writer.rs
// Authors:  Gabriel Mongefranco (@gabrielmongefranco)
// Created:  2026-09-01  |  Modified: 2026-09-01
// Summary:  The single writer of one data/<slug>/log/<dev>.jsonl (AGENTS.md 2). Appends
//           puts, tombstones, and all-or-nothing batches, with `seq` gapless per
//           spec/protocol.md §4.1 and the clock read here rather than taken from a caller.

use std::fs;
use std::io::Write as _;
use std::path::{Path, PathBuf};

use serde::Serialize;

use crate::identity::NodeId;
use crate::log::envelope::{Envelope, Op};
use crate::log::{Lamport, now};
use crate::{Error, Result, io_at};

/// How hard an append tries to be on disk before it returns.
///
/// **Not a `config.toml` key.** `spec/` defines `[node]` and `[lua]`, and
/// `docs/plans/phase-1.md §1` makes inventing a config key a signal that the spec is wrong
/// rather than licence to add one. The plan asks for the fsync policy to be configurable and
/// to default to sync-on-append; a constructor argument is configurable, and it does not
/// widen a surface nobody has numbers for. When M10 produces those numbers, an owner-facing
/// knob can be specified and added — in that order.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub enum Durability {
    /// `fsync` after every append. Correctness over throughput, and the default.
    #[default]
    Sync,
    /// Leave it to the OS. For tests and for benchmarks that would otherwise measure the
    /// disk; a power cut can lose the tail.
    Os,
}

/// The single writer of one `data/<slug>/log/<dev>.jsonl`.
///
/// `AGENTS.md` 2: a device appends only to its own log, forever. That is enforced here
/// rather than trusted — both constructors check the path against `(app, dev)` and refuse a
/// file that is not this node's own, which is what makes
/// `test_spec_3_1_never_writes_other_device_log` a refusal rather than a convention.
///
/// The writer owns `seq` and not `lam`: `§4.3`'s counter is per **app**, and in Phase 3 a
/// sync receiver folds another device's events into it without ever touching this writer.
/// So [`Lamport`] arrives as an argument, from [`AppLog`](super::AppLog), which owns it.
#[derive(Debug)]
pub struct Writer {
    file: fs::File,
    path: PathBuf,
    app: String,
    dev: String,
    seq: u64,
    durability: Durability,
}

impl Writer {
    /// Open a **new** log file, failing if one already exists.
    ///
    /// Deliberately incapable of appending to an existing stream, and it stays that way now
    /// that [`open`](Self::open) exists beside it. A caller that means "attach to whatever
    /// is there" should say so; a caller that means "this must be the first line of a new
    /// log" gets a guarantee rather than a hope.
    ///
    /// `seq` starts at zero and is incremented before the first write, so the first event is
    /// `seq: 1` — `§4.1` ("starts at 1") and the worked example in
    /// `spec/data-dictionary.md §6`.
    pub fn create(path: PathBuf, app: &str, dev: &NodeId, durability: Durability) -> Result<Self> {
        check_is_ours(&path, app, dev)?;
        if path.exists() {
            return Err(Error::LogExists { path });
        }

        let file = fs::OpenOptions::new()
            .create_new(true)
            .append(true)
            .open(&path)
            .map_err(io_at(&path))?;

        Ok(Self::assembled(file, path, app, dev, 0, durability))
    }

    /// Attach to an existing log, resuming from `seq`.
    ///
    /// `seq` comes from [`recover`](super::reader::recover), which read every segment of
    /// this device's stream — not from a count of lines and not from the last line alone. A
    /// log that was appended to by hand (`apps/hello/README.md` blesses `echo >>`) is
    /// exactly the case that distinguishes the three, and the writer has to continue from
    /// what is actually in the file or it will emit a `seq` the file already contains.
    ///
    /// The file is created if it is absent, which is the case after `local/state.jsonl`
    /// survives a `data/` that did not.
    pub fn open(
        path: PathBuf,
        app: &str,
        dev: &NodeId,
        seq: u64,
        durability: Durability,
    ) -> Result<Self> {
        check_is_ours(&path, app, dev)?;

        let file = fs::OpenOptions::new()
            .create(true)
            .append(true)
            .open(&path)
            .map_err(io_at(&path))?;

        Ok(Self::assembled(file, path, app, dev, seq, durability))
    }

    fn assembled(
        file: fs::File,
        path: PathBuf,
        app: &str,
        dev: &NodeId,
        seq: u64,
        durability: Durability,
    ) -> Self {
        Self {
            file,
            path,
            app: app.to_owned(),
            dev: dev.to_string(),
            seq,
            durability,
        }
    }

    /// Append one `put`.
    ///
    /// `ts` is read from the clock here rather than taken as an argument. A caller-supplied
    /// timestamp on the local write path is a way to put a lie into a file that is
    /// append-only forever and can only be corrected by another append — and once the writer
    /// owns the clock, `§4.4`'s "more than 24 hours in the future relative to its own clock"
    /// is unreachable from this direction by construction. The check belongs where a `ts`
    /// this node did not stamp enters it, which is the reader.
    ///
    /// A caller that needs several events to share one instant wants [`batch`](Self::batch),
    /// which is also the only way to get them written atomically.
    pub fn put<D: Serialize>(
        &mut self,
        lam: &mut Lamport,
        tbl: &str,
        id: &str,
        d: &D,
    ) -> Result<u64> {
        let ts = now();
        let seq = self.seq + 1;
        let line = serialize(
            seq,
            lam.tick(),
            &ts,
            &self.dev,
            &self.app,
            Op::Put,
            tbl,
            id,
            Some(d),
        )?;
        self.commit(&line)?;
        self.seq = seq;
        Ok(seq)
    }

    /// Append one tombstone (`§4.6`).
    ///
    /// Tombstones are permanent, are never garbage collected in `pv/1`, and the `id` is
    /// never reused. There is no hard delete: `§4.6` says the supported way to destroy data
    /// irrecoverably is to destroy `data/`.
    pub fn del(&mut self, lam: &mut Lamport, tbl: &str, id: &str) -> Result<u64> {
        let ts = now();
        let seq = self.seq + 1;
        let line = serialize::<()>(
            seq,
            lam.tick(),
            &ts,
            &self.dev,
            &self.app,
            Op::Del,
            tbl,
            id,
            None,
        )?;
        self.commit(&line)?;
        self.seq = seq;
        Ok(seq)
    }

    /// Append several events atomically, under one `ts` and contiguous `seq`. Returns the
    /// lines exactly as they went to the file, one per event and without the newline —
    /// what the data API's stream forwards, byte for byte (`spec/protocol.md §4.2`).
    ///
    /// The closure builds the batch; nothing reaches the file until it returns `Ok`, and an
    /// `Err` leaves `seq`, the Lamport counter, and the file untouched. This is the shape
    /// `spec/lua-api.md §3.3` gives `pv.batch(function(tx) ... end)`, so M7's binding is a
    /// wrapper rather than a second implementation.
    ///
    /// **The atomicity ceiling, stated rather than overclaimed.** One `write_all` followed by
    /// one `fsync` is the most an append-only plain-JSONL file can offer. A crash can still
    /// leave a byte prefix on disk — but a prefix almost always ends mid-line, and a
    /// mid-line tail is *detectable*: [`Lines`](super::reader::Lines) reports it as
    /// [`Error::PartialLine`] naming the byte offset, and nothing truncates it. A length
    /// prefix, a checksum footer, or a temp-file rename would each buy true atomicity, and
    /// each would break `AGENTS.md` invariant 1 — the live tail has to stay appendable by
    /// `echo`.
    pub fn batch<F>(&mut self, lam: &mut Lamport, build: F) -> Result<Vec<Vec<u8>>>
    where
        F: FnOnce(&mut Batch<'_>) -> Result<()>,
    {
        let (buf, seq, staged, lines) = {
            let mut batch = Batch {
                app: &self.app,
                dev: &self.dev,
                ts: now(),
                seq: self.seq,
                lam: *lam,
                buf: Vec::new(),
                lines: Vec::new(),
            };
            build(&mut batch)?;
            (batch.buf, batch.seq, batch.lam, batch.lines)
        };

        if lines.is_empty() {
            return Ok(lines);
        }

        self.commit(&buf)?;
        self.seq = seq;
        *lam = staged;
        Ok(lines)
    }

    /// One `write_all`, then the durability policy.
    fn commit(&mut self, bytes: &[u8]) -> Result<()> {
        self.file.write_all(bytes).map_err(io_at(&self.path))?;
        if self.durability == Durability::Sync {
            // The file's data and metadata. The *directory entry* is deliberately not
            // synced: Windows has no equivalent, M2's bar is that a lost tail be
            // detectable rather than impossible, and M4 is where tree-level durability
            // actually has to be argued.
            self.file.sync_all().map_err(io_at(&self.path))?;
        }
        Ok(())
    }

    /// The `seq` of the last event written.
    #[must_use]
    pub fn seq(&self) -> u64 {
        self.seq
    }

    /// Continue from `seq` when the file holds more than this writer wrote — a line
    /// appended by hand while the node ran. Never moves backwards.
    pub(crate) fn resume(&mut self, seq: u64) {
        self.seq = self.seq.max(seq);
    }

    /// The file being appended to.
    #[must_use]
    pub fn path(&self) -> &Path {
        &self.path
    }
}

/// A batch under construction. Nothing here has reached the file yet.
///
/// Every event in the batch shares one `ts` — they describe one moment — and takes the next
/// `seq` and `lam`, so a batch is contiguous in both.
#[derive(Debug)]
pub struct Batch<'a> {
    app: &'a str,
    dev: &'a str,
    ts: String,
    seq: u64,
    lam: Lamport,
    buf: Vec<u8>,
    /// Each staged line without its newline, in order — handed back once written.
    lines: Vec<Vec<u8>>,
}

impl Batch<'_> {
    /// Stage one `put`.
    pub fn put<D: Serialize>(&mut self, tbl: &str, id: &str, d: &D) -> Result<()> {
        self.seq += 1;
        let line = serialize(
            self.seq,
            self.lam.tick(),
            &self.ts,
            self.dev,
            self.app,
            Op::Put,
            tbl,
            id,
            Some(d),
        )?;
        self.stage(line);
        Ok(())
    }

    /// Stage one tombstone.
    pub fn del(&mut self, tbl: &str, id: &str) -> Result<()> {
        self.seq += 1;
        let line = serialize::<()>(
            self.seq,
            self.lam.tick(),
            &self.ts,
            self.dev,
            self.app,
            Op::Del,
            tbl,
            id,
            None,
        )?;
        self.stage(line);
        Ok(())
    }

    fn stage(&mut self, mut line: Vec<u8>) {
        self.buf.extend_from_slice(&line);
        line.pop();
        self.lines.push(line);
    }

    /// The instant every event in this batch carries.
    #[must_use]
    pub fn ts(&self) -> &str {
        &self.ts
    }
}

/// One event, as the bytes that go on the wire and into the file.
#[allow(clippy::too_many_arguments)]
fn serialize<D: Serialize>(
    seq: u64,
    lam: u64,
    ts: &str,
    dev: &str,
    app: &str,
    op: Op,
    tbl: &str,
    id: &str,
    d: Option<&D>,
) -> Result<Vec<u8>> {
    // §4.1: `d` MUST be absent when `op` is `del`, and a `put` is the row's value, so it
    // cannot be absent either. The type cannot say "present if and only if put", so the
    // pairing is asserted at the two places that build one.
    debug_assert_eq!(
        op == Op::Put,
        d.is_some(),
        "§4.1: `d` is present exactly when `op` is `put`"
    );

    let mut line = serde_json::to_vec(&Envelope {
        seq,
        lam,
        ts,
        dev,
        app,
        op,
        tbl,
        id,
        d,
    })?;
    // §4.1: `\n` terminated, 0x0A, never \r\n — on Windows too, which is why the file is
    // opened in binary mode (Rust has no text mode) and the byte is written here rather
    // than by `writeln!`.
    line.push(b'\n');
    Ok(line)
}

/// `§4.1`: `dev` MUST equal the log filename and `app` MUST equal the containing directory.
///
/// Both are checked against the path rather than assumed, so `AGENTS.md` 2 — one writer per
/// log file, forever — is a refusal a test can trigger rather than a comment. The one legal
/// exception is `§10.2`'s sync receiver, which writes `log/<origin-dev>.jsonl`; it does not
/// exist until Phase 3 and will not come through this type.
fn check_is_ours(path: &Path, app: &str, dev: &NodeId) -> Result<()> {
    let filename = path
        .file_name()
        .and_then(|name| name.to_str())
        .unwrap_or_default();
    let expected = format!("{dev}.jsonl");

    // The rolled segments of §3.2 are `<dev>.<n>.jsonl` and are equally ours. `pv/1` rolls
    // nothing, so this accepts them for the reader's sake and does not create one.
    let stem_is_ours = filename == expected
        || (filename.starts_with(&format!("{dev}."))
            && filename.ends_with(".jsonl")
            && filename.matches('.').count() == 2);

    let dir_is_app = path
        .parent()
        .and_then(Path::parent)
        .and_then(Path::file_name)
        .and_then(|name| name.to_str())
        == Some(app);

    if stem_is_ours && dir_is_app {
        return Ok(());
    }

    Err(Error::LogNotOurs {
        path: path.to_path_buf(),
        app: app.to_owned(),
        dev: dev.to_string(),
    })
}

// AGENTS.md, Style: unwrap() is permitted in tests. The crate-level deny reaches unit
// tests inside src/, so each one opts out where it is declared.
#[allow(clippy::unwrap_used, clippy::expect_used)]
#[cfg(test)]
mod tests {
    use super::*;

    fn an_id() -> NodeId {
        NodeId::derive(&ed25519_dalek::SigningKey::from_bytes(&[7u8; 32]).verifying_key())
    }

    #[test]
    fn our_own_log_and_our_own_rolled_segments_are_accepted() {
        let id = an_id();
        let dir = PathBuf::from("root/data/hello/log");
        assert!(check_is_ours(&dir.join(format!("{id}.jsonl")), "hello", &id).is_ok());
        assert!(check_is_ours(&dir.join(format!("{id}.2.jsonl")), "hello", &id).is_ok());
    }

    /// `AGENTS.md` 2. A writer aimed at another device's log is refused, not trusted.
    #[test]
    fn another_devices_log_is_refused() {
        let id = an_id();
        let path = PathBuf::from("root/data/hello/log/k7m2q9xf.jsonl");
        assert!(check_is_ours(&path, "hello", &id).is_err());
    }

    /// `§4.1`: `app` MUST equal the containing directory.
    #[test]
    fn a_log_under_the_wrong_app_is_refused() {
        let id = an_id();
        let path = PathBuf::from(format!("root/data/animals/log/{id}.jsonl"));
        assert!(check_is_ours(&path, "hello", &id).is_err());
    }
}
