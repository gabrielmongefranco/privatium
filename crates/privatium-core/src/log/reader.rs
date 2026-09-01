// Project:  Privatium™  |  File: crates/privatium-core/src/log/reader.rs
// Authors:  Gabriel Mongefranco (@gabrielmongefranco)
// Created:  2026-09-01  |  Modified: 2026-09-01
// Summary:  Reading an app's log: the segment list of spec/protocol.md §3.2, a line
//           iterator per segment, and the one startup scan that recovers `seq` and the
//           Lamport counter and applies §4.4's clock hygiene.
//
//           This is NOT the materialization path. M3 points DuckDB's read_json() at
//           data/<slug>/log/*.jsonl directly (docs/plans/phase-1.md, M3). What lives here
//           exists for recovery now and for §10 sync in Phase 3, and it is deliberately
//           incurious about anything those two do not need.

use std::collections::BTreeMap;
use std::fs;
use std::io::{BufRead, BufReader};
use std::path::{Path, PathBuf};

use crate::identity::NodeId;
use crate::log::Lamport;
use crate::log::envelope::Meta;
use crate::{Error, Result, io_at};

/// The extension every log segment carries. Plain JSONL, always, for the live tail.
const EXTENSION: &str = "jsonl";

/// `§4.4` — an event more than this far in the future is rejected on ingest.
const MAX_FUTURE_SECS: i64 = 24 * 60 * 60;

/// `§4.4` — the node SHOULD warn when its own clock appears to have moved backwards more
/// than this.
const MAX_BACKWARDS_SECS: i64 = 60;

/// One file of one device's log stream.
///
/// `<dev>.jsonl` is index 1 and `<dev>.<n>.jsonl` is index `n`, with `n` starting at 2
/// (`§3.2`). Ordering by that number rather than by filename is the whole point: lexically,
/// `<dev>.10.jsonl` sorts before `<dev>.2.jsonl`, and a reader that concatenated them in
/// that order would hand `§4.5` a stream that is not in `seq` order.
///
/// `dev` is a `String`, not a [`NodeId`]. `§2.1` says other nodes' IDs are opaque, and this
/// type does not validate the shape of one it did not derive.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Segment {
    path: PathBuf,
    dev: String,
    index: u32,
}

impl Segment {
    /// The device whose log this segment belongs to.
    #[must_use]
    pub fn dev(&self) -> &str {
        &self.dev
    }

    /// The rotation index. 1 for the live tail, 2 and up for sealed segments.
    #[must_use]
    pub fn index(&self) -> u32 {
        self.index
    }

    /// The file on disk.
    #[must_use]
    pub fn path(&self) -> &Path {
        &self.path
    }

    /// Iterate this segment's lines.
    ///
    /// This method is the seam `AGENTS.md` invariant 1 reserves. Sealed historical segments
    /// MAY later be compressed or stored as Parquet, and when that happens it is this
    /// function that learns to open them — every caller above asks a segment for lines and
    /// does not know what the segment is made of. **The live tail is always plain,
    /// uncompressed JSONL**, `pv/1` seals nothing, and no compression path exists here.
    pub fn lines(&self) -> Result<Lines> {
        let file = fs::File::open(&self.path).map_err(io_at(&self.path))?;
        Ok(Lines {
            reader: BufReader::new(file),
            path: self.path.clone(),
            offset: 0,
        })
    }
}

/// One line of one segment, as bytes.
///
/// Raw, and it stays raw. `§4.2` requires unknown fields to survive verbatim, and the way
/// to guarantee that is never to build a value that could be serialized back — so this
/// carries the original bytes, and `docs/plans/phase-1.md §5`'s rule ("raw lines are
/// `String`/`&[u8]` end to end") holds by construction rather than by discipline.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Line {
    raw: Vec<u8>,
    offset: u64,
}

impl Line {
    /// The line's bytes, without its terminating newline.
    #[must_use]
    pub fn raw(&self) -> &[u8] {
        &self.raw
    }

    /// The byte offset of the line's first byte within its segment.
    #[must_use]
    pub fn offset(&self) -> u64 {
        self.offset
    }
}

/// An iterator over one segment's lines.
///
/// A trailing line with no newline yields [`Error::PartialLine`] naming the byte offset it
/// starts at. Nothing is truncated and nothing is repaired: `§3.1` forbids modifying a log
/// file, and a crash mid-append is exactly the case this has to report rather than tidy
/// away.
#[derive(Debug)]
pub struct Lines {
    reader: BufReader<fs::File>,
    path: PathBuf,
    offset: u64,
}

impl Iterator for Lines {
    type Item = Result<Line>;

    fn next(&mut self) -> Option<Self::Item> {
        loop {
            let mut raw = Vec::new();
            let read = match self.reader.read_until(b'\n', &mut raw) {
                Ok(read) => read,
                Err(error) => return Some(Err(io_at(&self.path)(error))),
            };
            if read == 0 {
                return None;
            }

            let start = self.offset;
            self.offset += read as u64;

            if raw.last() != Some(&b'\n') {
                return Some(Err(Error::PartialLine {
                    path: self.path.clone(),
                    offset: start,
                }));
            }
            raw.pop();

            // A stray blank line is a curiosity, not an outage. §4.1 forbids a reader from
            // repairing a log, and skipping whitespace is the reading equivalent of leaving
            // it alone. A `\r` before the newline lands here too: the writer never emits one
            // (§4.1 — 0x0A, never \r\n), but a file edited on Windows may carry one, and
            // JSON counts it as whitespace anyway.
            if raw.iter().all(u8::is_ascii_whitespace) {
                continue;
            }

            return Some(Ok(Line { raw, offset: start }));
        }
    }
}

/// Every segment of one app's log, across every device.
///
/// `§3.2`: `log/<device-id>*.jsonl` is one logical stream per device. Across devices there
/// is no single order and none is invented here — `§4.5` groups by `id` and orders by
/// `(lam, ts, dev)`, and `seq` means nothing between two writers.
#[derive(Debug, Clone, Default)]
pub struct Reader {
    segments: Vec<Segment>,
}

impl Reader {
    /// List the segments in `data/<slug>/log/`.
    ///
    /// A directory that does not exist yet reads as no segments rather than as an error: an
    /// app that has never been written to has no log, and that is an ordinary state.
    pub fn open(log_dir: &Path) -> Result<Self> {
        let entries = match fs::read_dir(log_dir) {
            Ok(entries) => entries,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
                return Ok(Self::default());
            }
            Err(error) => return Err(io_at(log_dir)(error)),
        };

        let mut segments = Vec::new();
        for entry in entries {
            let entry = entry.map_err(io_at(log_dir))?;
            let path = entry.path();
            let Some(parsed) = path
                .file_name()
                .and_then(|name| name.to_str())
                .and_then(parse_segment_name)
                .map(|(dev, index)| (dev.to_owned(), index))
            else {
                continue;
            };
            if !entry.file_type().map_err(io_at(&path))?.is_file() {
                continue;
            }
            let (dev, index) = parsed;
            segments.push(Segment { path, dev, index });
        }

        // By device, then by rotation index — numerically, not lexically. See [`Segment`].
        segments.sort_by(|a, b| a.dev.cmp(&b.dev).then(a.index.cmp(&b.index)));
        Ok(Self { segments })
    }

    /// The segments, ordered by `(dev, rotation index)`.
    #[must_use]
    pub fn segments(&self) -> &[Segment] {
        &self.segments
    }

    /// The segments belonging to one device, in rotation order.
    pub fn segments_for<'a>(&'a self, dev: &'a str) -> impl Iterator<Item = &'a Segment> {
        self.segments.iter().filter(move |seg| seg.dev == dev)
    }

    /// Every line of every segment, in `(dev, index, file)` order.
    ///
    /// **Not sorted by `seq`.** `§3.2` calls the concatenation of one device's segments a
    /// stream "ordered by `seq`", which is a property the writer guarantees by being gapless
    /// and append-only — not a sort a reader is entitled to impose. `§4.1` is explicit that
    /// a reader MUST NOT reorder, and a hand-edited log is exactly where the difference
    /// shows up.
    pub fn lines(&self) -> impl Iterator<Item = Result<Line>> {
        self.segments.iter().flat_map(|segment| {
            let lines: Box<dyn Iterator<Item = Result<Line>>> = match segment.lines() {
                Ok(lines) => Box::new(lines),
                Err(error) => Box::new(std::iter::once(Err(error))),
            };
            lines
        })
    }
}

/// `<dev>.jsonl` maps to `(dev, 1)`; `<dev>.<n>.jsonl` to `(dev, n)` for `n >= 2`.
///
/// Anything else in `log/` is not a segment and is ignored — an editor's backup file, a
/// `.tmp` from a half-finished copy, a directory. Being incurious here is deliberate: this
/// function decides what counts as history, and a permissive answer would let a stray file
/// become part of it.
fn parse_segment_name(name: &str) -> Option<(&str, u32)> {
    let stem = name.strip_suffix(EXTENSION)?.strip_suffix('.')?;
    if stem.is_empty() {
        return None;
    }

    match stem.rsplit_once('.') {
        None => Some((stem, 1)),
        Some((dev, number)) => {
            let index: u32 = number.parse().ok()?;
            // `n` starts at 2 (§3.2). A `<dev>.1.jsonl` is not something this node wrote,
            // and accepting it would give one device two files claiming the live tail.
            if index < 2 || dev.is_empty() {
                return None;
            }
            Some((dev, index))
        }
    }
}

/// An event excluded from recovery, and why (`§4.4`).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Rejected {
    /// The segment the line is in. The line itself stays exactly where it is.
    pub segment: PathBuf,
    /// Its byte offset within that segment.
    pub offset: u64,
    /// The writer that claimed it.
    pub dev: String,
    /// Its `seq`.
    pub seq: u64,
    /// The `ts` that failed the check.
    pub ts: String,
    /// How far ahead of this node's clock it is.
    pub ahead_secs: i64,
}

/// A line that is not a `pv/1` envelope at all.
///
/// Not audited. `§4.4` requires a *clock* rejection to reach `sys_audit`; a line that does
/// not parse is `§10.2`'s "envelope parses" validation, which belongs to the sync receiver
/// in Phase 3 and can only be acted on there. Reporting it and carrying on is what a reader
/// is allowed to do (`§4.1`).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Malformed {
    /// The segment the line is in.
    pub segment: PathBuf,
    /// Its byte offset within that segment.
    pub offset: u64,
    /// What the parser said.
    pub problem: String,
}

/// This node's clock appears to have moved backwards (`§4.4`, second sentence).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Skew {
    /// The `ts` of the last event this node wrote.
    pub tail_ts: String,
    /// How far behind that this node's clock now is, in seconds.
    pub behind_secs: i64,
}

/// What one scan of an app's log recovered.
#[derive(Debug, Clone, Default)]
pub struct Recovered {
    /// The highest `seq` in this node's own segments. The next event it writes is this plus
    /// one, which is what keeps the writer gapless (`§4.1`) even over a log that was
    /// appended to by hand.
    pub own_seq: u64,
    /// The Lamport counter, folded over every event **accepted** (`§4.3`).
    pub lam: Lamport,
    /// The highest `seq` seen per device, accepted or not. See [`recover`].
    pub heads: BTreeMap<String, u64>,
    /// Events rejected by `§4.4`.
    pub rejected: Vec<Rejected>,
    /// Lines that are not envelopes.
    pub malformed: Vec<Malformed>,
    /// Set when this node's own clock looks to have gone backwards.
    pub skew: Option<Skew>,
}

/// Scan every segment, recovering `seq` and the Lamport counter and applying `§4.4`.
///
/// **Why a full scan.** `local/state.jsonl` caches `lam` and the per-device heads, and the
/// obvious optimization is to resume from a byte offset. That is not what `known_lam` and
/// `known_heads` are for. They exist so the counter stays monotonic when the *files* move
/// backwards — a log restored from an older copy, a `snap/` rolled back — which a scan alone
/// cannot notice, and so that a line already reported once is not reported again on every
/// restart. Phase 1 logs are small and a scan is cheap; when that stops being true, the
/// thing to add is a byte cursor, not a second meaning for these two.
///
/// **On `heads`.** A head advances past a rejected line. That reads wrong for a moment and
/// is right: `seq` is a position in a file, `lam` is causal order. Refusing to acknowledge a
/// rejected line's *position* would make the writer emit a `seq` the file already contains —
/// a duplicate, which is worse than the bad line — and would re-report the same rejection on
/// every start. Its `lam` is what stays excluded, and that is the half `§4.4` is about.
pub(crate) fn recover(
    reader: &Reader,
    own: &NodeId,
    known_lam: u64,
    known_heads: &BTreeMap<String, u64>,
    now: jiff::Timestamp,
) -> Result<Recovered> {
    let mut out = Recovered {
        lam: Lamport::new(known_lam),
        ..Recovered::default()
    };
    let mut own_tail_ts: Option<String> = None;

    for segment in reader.segments() {
        for line in segment.lines()? {
            let line = line?;
            let meta: Meta<'_> = match serde_json::from_slice(line.raw()) {
                Ok(meta) => meta,
                Err(error) => {
                    out.malformed.push(Malformed {
                        segment: segment.path().to_path_buf(),
                        offset: line.offset(),
                        problem: error.to_string(),
                    });
                    continue;
                }
            };

            let dev = meta.dev.into_owned();
            let head = out.heads.entry(dev.clone()).or_default();
            *head = (*head).max(meta.seq);
            let is_ours = dev == own.as_str();
            if is_ours {
                out.own_seq = out.own_seq.max(meta.seq);
            }

            // §4.4. A `ts` this node cannot parse is not a clock problem, so the event is
            // accepted and its `ts` simply carries no information. Rejecting it would be
            // gap rejection by another name, which §4.1 forbids a reader.
            let ahead = match meta.ts.parse::<jiff::Timestamp>() {
                Ok(stamped) => stamped.duration_since(now).as_secs(),
                Err(_) => 0,
            };

            if ahead > MAX_FUTURE_SECS {
                let already_reported = known_heads.get(&dev).is_some_and(|seen| meta.seq <= *seen);
                if !already_reported {
                    out.rejected.push(Rejected {
                        segment: segment.path().to_path_buf(),
                        offset: line.offset(),
                        dev,
                        seq: meta.seq,
                        ts: meta.ts.into_owned(),
                        ahead_secs: ahead,
                    });
                }
                // Excluded from the Lamport fold, and from the tail this node dates itself
                // against. That exclusion is the whole of what "rejected" means here.
                continue;
            }

            out.lam.observe(meta.lam);
            if is_ours {
                own_tail_ts = Some(meta.ts.into_owned());
            }
        }
    }

    out.skew = own_tail_ts.and_then(|tail_ts| {
        let stamped = tail_ts.parse::<jiff::Timestamp>().ok()?;
        let behind = stamped.duration_since(now).as_secs();
        (behind > MAX_BACKWARDS_SECS).then_some(Skew {
            tail_ts,
            behind_secs: behind,
        })
    });

    Ok(out)
}

// AGENTS.md, Style: unwrap() is permitted in tests. The crate-level deny reaches unit
// tests inside src/, so each one opts out where it is declared.
#[allow(clippy::unwrap_used, clippy::expect_used)]
#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_live_tail_is_index_one_and_rolled_files_start_at_two() {
        assert_eq!(parse_segment_name("k7m2q9xf.jsonl"), Some(("k7m2q9xf", 1)));
        assert_eq!(
            parse_segment_name("k7m2q9xf.2.jsonl"),
            Some(("k7m2q9xf", 2))
        );
        assert_eq!(
            parse_segment_name("k7m2q9xf.10.jsonl"),
            Some(("k7m2q9xf", 10))
        );
    }

    /// §3.2 starts `n` at 2, so a second file claiming index 1 is not a segment.
    #[test]
    fn a_file_claiming_index_one_or_zero_is_not_a_segment() {
        assert_eq!(parse_segment_name("k7m2q9xf.1.jsonl"), None);
        assert_eq!(parse_segment_name("k7m2q9xf.0.jsonl"), None);
    }

    #[test]
    fn anything_that_is_not_a_segment_is_ignored() {
        assert_eq!(parse_segment_name("k7m2q9xf.jsonl.tmp"), None);
        assert_eq!(parse_segment_name("k7m2q9xf.bak.jsonl"), None);
        assert_eq!(parse_segment_name("README.md"), None);
        assert_eq!(parse_segment_name(".jsonl"), None);
    }

    /// Lexical ordering puts `.10` before `.2`. That is the bug the numeric index exists to
    /// prevent, so it is pinned rather than assumed.
    #[test]
    fn segments_order_numerically_not_lexically() {
        let mut segments: Vec<Segment> = ["dev.10.jsonl", "dev.2.jsonl", "dev.jsonl"]
            .into_iter()
            .map(|name| {
                let (dev, index) = parse_segment_name(name).unwrap();
                Segment {
                    path: PathBuf::from(name),
                    dev: dev.to_owned(),
                    index,
                }
            })
            .collect();
        segments.sort_by(|a, b| a.dev.cmp(&b.dev).then(a.index.cmp(&b.index)));

        let order: Vec<u32> = segments.iter().map(Segment::index).collect();
        assert_eq!(order, vec![1, 2, 10]);
    }
}
