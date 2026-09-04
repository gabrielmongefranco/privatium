// Project:  Privatium™  |  File: crates/privatium-core/src/log/writer.rs
// Authors:  Gabriel Mongefranco (@gabrielmongefranco)
// Created:  2026-09-01  |  Modified: 2026-09-05
// Summary:  The single writer of one data/<slug>/log/<dev>.jsonl (AGENTS.md 2). Appends
//           puts, tombstones, and all-or-nothing batches, with `seq` gapless per
//           spec/protocol.md §4.1 and the clock read here rather than taken from a caller.

use std::fs;
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
///
/// **A failed append closes the writer** (`§4.1`). When a write or a flush fails, some of
/// the bytes may have reached the file and some not, and `seq` was not advanced; a
/// writer that carried on could mint that `seq` a second time or append after a torn
/// line. So the writer refuses every append until [`AppLog`](super::AppLog) has re-read
/// the file and told it where the file now ends ([`reconcile`](Self::reconcile)) — which
/// happens on the next append, and succeeds whenever the file ends on a line boundary.
#[derive(Debug)]
pub struct Writer {
    sink: Box<dyn Sink>,
    path: PathBuf,
    app: String,
    dev: String,
    seq: u64,
    durability: Durability,
    /// Why the last append failed, while it stands.
    poisoned: Option<String>,
}

/// What the writer needs of its file, so a test can hand it one that fails part-way.
pub(crate) trait Sink: std::fmt::Debug + Send + Sync {
    fn write_all(&mut self, bytes: &[u8]) -> std::io::Result<()>;
    fn sync_all(&self) -> std::io::Result<()>;
}

impl Sink for fs::File {
    fn write_all(&mut self, bytes: &[u8]) -> std::io::Result<()> {
        std::io::Write::write_all(self, bytes)
    }

    fn sync_all(&self) -> std::io::Result<()> {
        fs::File::sync_all(self)
    }
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
        sync_parent(&path, durability)?;

        Ok(Self::assembled(
            Box::new(file),
            path,
            app,
            dev,
            0,
            durability,
        ))
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

        let existed = path.exists();
        let file = fs::OpenOptions::new()
            .create(true)
            .append(true)
            .open(&path)
            .map_err(io_at(&path))?;
        if !existed {
            sync_parent(&path, durability)?;
        }

        Ok(Self::assembled(
            Box::new(file),
            path,
            app,
            dev,
            seq,
            durability,
        ))
    }

    fn assembled(
        sink: Box<dyn Sink>,
        path: PathBuf,
        app: &str,
        dev: &NodeId,
        seq: u64,
        durability: Durability,
    ) -> Self {
        Self {
            sink,
            path,
            app: app.to_owned(),
            dev: dev.to_string(),
            seq,
            durability,
            poisoned: None,
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
        // The counter is ticked on a copy and taken only once the line is on disk, as a
        // batch does: a line that did not land consumed no `lam`.
        let mut staged = *lam;
        let line = serialize(
            seq,
            staged.tick(),
            &ts,
            &self.dev,
            &self.app,
            Op::Put,
            tbl,
            id,
            None,
            Some(d),
        )?;
        self.commit(&line)?;
        self.seq = seq;
        *lam = staged;
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
        let mut staged = *lam;
        let line = serialize::<()>(
            seq,
            staged.tick(),
            &ts,
            &self.dev,
            &self.app,
            Op::Del,
            tbl,
            id,
            None,
            None,
        )?;
        self.commit(&line)?;
        self.seq = seq;
        *lam = staged;
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
    /// **How all-or-nothing is kept on a file that has to stay `echo`-appendable.** One
    /// `write_all` followed by one `fsync` is what reaches the disk, and a crash between
    /// the two can leave a byte prefix. A prefix that ends mid-line is
    /// [`Error::PartialLine`], reported by byte offset and never repaired. A prefix that
    /// ends on a line boundary is the case a plain line-oriented file cannot see, so the
    /// batch says its own length: its first line carries `"batch": n` (`§4.1`), and every
    /// reader — the materializer, the snapshot writer, the data API — skips a batch that
    /// has fewer lines than it announced ([`batch::incomplete`](super::batch::incomplete)).
    /// The lines stay in the file, the writer continues after them, and the audit says so
    /// once. No length prefix, no checksum footer, no temp-file rename: each would break
    /// `AGENTS.md` invariant 1.
    pub fn batch<F>(&mut self, lam: &mut Lamport, build: F) -> Result<Vec<Vec<u8>>>
    where
        F: FnOnce(&mut Batch<'_>) -> Result<()>,
    {
        let (seq, staged, lines) = {
            let mut batch = Batch {
                app: &self.app,
                dev: &self.dev,
                ts: now(),
                seq: self.seq,
                lam: *lam,
                staged: Vec::new(),
            };
            build(&mut batch)?;
            (batch.seq, batch.lam, batch.lines()?)
        };

        if lines.is_empty() {
            return Ok(lines);
        }

        let mut buf = Vec::with_capacity(lines.iter().map(|line| line.len() + 1).sum());
        for line in &lines {
            buf.extend_from_slice(line);
            buf.push(b'\n');
        }
        self.commit(&buf)?;
        self.seq = seq;
        *lam = staged;
        Ok(lines)
    }

    /// One `write_all`, then the durability policy. A failure of either poisons the
    /// writer: the bytes may be on disk in whole or in part, and nothing may be appended
    /// after them until the file has been re-read.
    fn commit(&mut self, bytes: &[u8]) -> Result<()> {
        if let Some(reason) = &self.poisoned {
            return Err(Error::WriterPoisoned {
                path: self.path.clone(),
                reason: reason.clone(),
            });
        }
        if let Err(error) = self.sink.write_all(bytes) {
            self.poisoned = Some(format!("write failed: {error}"));
            return Err(io_at(&self.path)(error));
        }
        if self.durability == Durability::Sync {
            // The file's data and metadata. The directory entry was synced once, when
            // the file was created (`sync_parent`); an append changes no entry.
            if let Err(error) = self.sink.sync_all() {
                self.poisoned = Some(format!("flush failed: {error}"));
                return Err(io_at(&self.path)(error));
            }
        }
        Ok(())
    }

    /// The `seq` of the last event written.
    #[must_use]
    pub fn seq(&self) -> u64 {
        self.seq
    }

    /// Why the writer refuses to append, if it does: the last commit failed and the
    /// file has not been re-read since.
    #[must_use]
    pub fn poisoned(&self) -> Option<&str> {
        self.poisoned.as_deref()
    }

    /// Continue from `seq` when the file holds more than this writer wrote — a line
    /// appended by hand while the node ran. Never moves backwards.
    pub(crate) fn resume(&mut self, seq: u64) {
        self.seq = self.seq.max(seq);
    }

    /// The file has been re-read to its end and ends on a line boundary: continue from
    /// the `seq` it holds, and take appends again. What a failed commit left is now
    /// either a whole line the scan counted or nothing.
    pub(crate) fn reconcile(&mut self, seq: u64) {
        self.resume(seq);
        self.poisoned = None;
    }

    /// Poison the writer as a failed commit would, for a test of the recovery path.
    #[cfg(test)]
    pub(crate) fn poison_for_test(&mut self, reason: &str) {
        self.poisoned = Some(reason.to_owned());
    }

    /// A writer over any sink, for the unit tests of the failure path.
    #[cfg(test)]
    pub(crate) fn over(sink: Box<dyn Sink>, path: PathBuf, app: &str, dev: &NodeId) -> Self {
        Self::assembled(sink, path, app, dev, 0, Durability::Sync)
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
/// `seq` and `lam`, so a batch is contiguous in both. Each `d` is serialized as it is
/// staged, so the caller's type is gone by the time the lines are built — and the lines
/// are built only once the batch is closed, because the first of them has to say how many
/// there are (`§4.1`).
#[derive(Debug)]
pub struct Batch<'a> {
    app: &'a str,
    dev: &'a str,
    ts: String,
    seq: u64,
    lam: Lamport,
    staged: Vec<Staged>,
}

/// One event of a batch, everything but the marker decided.
#[derive(Debug)]
struct Staged {
    seq: u64,
    lam: u64,
    op: Op,
    tbl: String,
    id: String,
    /// `d` as JSON text, exactly as the caller's value serializes.
    d: Option<String>,
}

impl Batch<'_> {
    /// Stage one `put`.
    pub fn put<D: Serialize>(&mut self, tbl: &str, id: &str, d: &D) -> Result<()> {
        let d = serde_json::to_string(d)?;
        self.stage(Op::Put, tbl, id, Some(d));
        Ok(())
    }

    /// Stage one tombstone.
    pub fn del(&mut self, tbl: &str, id: &str) -> Result<()> {
        self.stage(Op::Del, tbl, id, None);
        Ok(())
    }

    fn stage(&mut self, op: Op, tbl: &str, id: &str, d: Option<String>) {
        self.seq += 1;
        self.staged.push(Staged {
            seq: self.seq,
            lam: self.lam.tick(),
            op,
            tbl: tbl.to_owned(),
            id: id.to_owned(),
            d,
        });
    }

    /// The instant every event in this batch carries.
    #[must_use]
    pub fn ts(&self) -> &str {
        &self.ts
    }

    /// Every staged line without its newline, in order — the first carrying the marker
    /// when there are two or more (`§4.1`).
    fn lines(&self) -> Result<Vec<Vec<u8>>> {
        let count = u64::try_from(self.staged.len()).unwrap_or(u64::MAX);
        let mut lines = Vec::with_capacity(self.staged.len());
        for (index, staged) in self.staged.iter().enumerate() {
            let d = match &staged.d {
                Some(text) => Some(serde_json::value::RawValue::from_string(text.clone())?),
                None => None,
            };
            let batch = (index == 0 && count >= 2).then_some(count);
            let mut line = serialize(
                staged.seq,
                staged.lam,
                &self.ts,
                self.dev,
                self.app,
                staged.op,
                &staged.tbl,
                &staged.id,
                batch,
                d.as_ref(),
            )?;
            line.pop();
            lines.push(line);
        }
        Ok(lines)
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
    batch: Option<u64>,
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
        batch,
        d,
    })?;
    // §4.1: `\n` terminated, 0x0A, never \r\n — on Windows too, which is why the file is
    // opened in binary mode (Rust has no text mode) and the byte is written here rather
    // than by `writeln!`.
    line.push(b'\n');
    Ok(line)
}

/// A log file that has just come into existence has to survive a power cut as a name
/// as well as as bytes: its directory entry is flushed where the platform can
/// (`crate::durable`), under the same policy the appends follow.
fn sync_parent(path: &Path, durability: Durability) -> Result<()> {
    if durability == Durability::Sync
        && let Some(dir) = path.parent()
    {
        crate::durable::sync_dir(dir).map_err(io_at(dir))?;
    }
    Ok(())
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

    /// A sink that takes some of each write and then fails, or whose flush fails: what a
    /// full disk or a dying device does to an append.
    #[derive(Debug)]
    struct Failing {
        /// Bytes accepted before the write fails; `None` accepts every write.
        accept: Option<usize>,
        sync_fails: bool,
        landed: Vec<u8>,
    }

    impl Sink for Failing {
        fn write_all(&mut self, bytes: &[u8]) -> std::io::Result<()> {
            match self.accept {
                Some(n) if n < bytes.len() => {
                    self.landed.extend_from_slice(&bytes[..n]);
                    Err(std::io::Error::other("no space left on device"))
                }
                _ => {
                    self.landed.extend_from_slice(bytes);
                    Ok(())
                }
            }
        }

        fn sync_all(&self) -> std::io::Result<()> {
            if self.sync_fails {
                Err(std::io::Error::other("input/output error"))
            } else {
                Ok(())
            }
        }
    }

    fn writer_over(sink: Failing) -> (Writer, NodeId) {
        let id = an_id();
        let path = PathBuf::from(format!("root/data/hello/log/{id}.jsonl"));
        (Writer::over(Box::new(sink), path, "hello", &id), id)
    }

    /// `§4.1`: a write that lands part of a line poisons the writer — `seq` stays where
    /// it was and every later append is refused, naming the reason — until the file has
    /// been re-read. A batch behaves the same, with the Lamport counter untouched too.
    #[test]
    fn a_write_that_fails_part_way_closes_the_writer() {
        let (mut writer, _) = writer_over(Failing {
            accept: Some(10),
            sync_fails: false,
            landed: Vec::new(),
        });
        let mut lam = Lamport::new(0);
        let error = writer
            .put(&mut lam, "note", "a", &serde_json::json!({}))
            .unwrap_err();
        assert!(matches!(error, Error::Io { .. }), "{error}");
        assert_eq!(writer.seq(), 0, "seq did not advance over a failed write");
        assert_eq!(lam.get(), 0, "a line that did not land consumed no lam");
        assert!(writer.poisoned().unwrap().contains("write failed"));

        let again = writer
            .put(&mut lam, "note", "b", &serde_json::json!({}))
            .unwrap_err();
        assert!(matches!(again, Error::WriterPoisoned { .. }), "{again}");
        let batch = writer
            .batch(&mut lam, |batch| {
                batch.put("note", "c", &serde_json::json!({}))
            })
            .unwrap_err();
        assert!(matches!(batch, Error::WriterPoisoned { .. }), "{batch}");
        assert_eq!(writer.seq(), 0);

        // Told where the file ends, it takes appends again from there.
        writer.reconcile(0);
        assert!(writer.poisoned().is_none());
    }

    /// A flush that fails poisons the writer too: the bytes were handed to the OS and
    /// may or may not be on disk, and the next append must not assume either.
    #[test]
    fn a_flush_that_fails_closes_the_writer() {
        let (mut writer, _) = writer_over(Failing {
            accept: None,
            sync_fails: true,
            landed: Vec::new(),
        });
        let mut lam = Lamport::new(0);
        let error = writer
            .put(&mut lam, "note", "a", &serde_json::json!({}))
            .unwrap_err();
        assert!(matches!(error, Error::Io { .. }), "{error}");
        assert!(writer.poisoned().unwrap().contains("flush failed"));
        assert_eq!(writer.seq(), 0);
        assert!(matches!(
            writer.del(&mut lam, "note", "a").unwrap_err(),
            Error::WriterPoisoned { .. }
        ));
    }
}
