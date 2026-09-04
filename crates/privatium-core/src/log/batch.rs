// Project:  Privatium™  |  File: crates/privatium-core/src/log/batch.rs
// Authors:  Gabriel Mongefranco (@gabrielmongefranco)
// Created:  2026-09-05  |  Modified: 2026-09-05
// Summary:  The batch rule of spec/protocol.md §4.1: the first line of a batch of n ≥ 2
//           events carries `"batch": n`, and a reader that finds fewer than n consecutive
//           lines with that `ts` and contiguous `seq` after it — the segment ended, a line
//           with another `ts` came first, a new batch began — has an incomplete batch on
//           its hands, which a crash between the write and the disk left. Its lines are
//           not materialized, not served and not sent; nothing is truncated, and the
//           writer continues after them. Every reader of a log applies this one function.

use std::ops::Range;

/// What the rule reads of one line, in file order within one segment.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct Head<'a> {
    /// `seq`, or 0 for a line that has none (which no batch line lacks).
    pub seq: u64,
    /// `ts` as written.
    pub ts: Option<&'a str>,
    /// `batch`, on the first line of a batch of that many events.
    pub batch: Option<u64>,
}

/// The index ranges, within `heads`, of every incomplete batch — each starting at its
/// header line and covering the lines that did arrive.
///
/// A header names `n`; the lines that belong to it are the next `n - 1` with the same
/// `ts` and a `seq` one past the previous, none of them a header itself. Fewer than that
/// is incomplete. A `batch` of 0 or 1 is not a marker the writer emits and is ignored.
pub(crate) fn incomplete(heads: &[Head<'_>]) -> Vec<Range<usize>> {
    let mut out = Vec::new();
    let mut at = 0usize;
    while at < heads.len() {
        let header = &heads[at];
        let Some(expected) = header
            .batch
            .filter(|n| *n >= 2)
            .map(|n| usize::try_from(n).unwrap_or(usize::MAX))
        else {
            at += 1;
            continue;
        };
        let mut found = 1usize;
        while found < expected {
            let Some(next) = heads.get(at + found) else {
                break;
            };
            let contiguous = next.seq == header.seq.wrapping_add(found as u64);
            if next.batch.is_some() || next.ts != header.ts || !contiguous {
                break;
            }
            found += 1;
        }
        if found < expected {
            out.push(at..at + found);
        }
        at += found;
    }
    out
}

/// Whether index `i` falls inside any of `ranges`. The ranges are few and in order, so
/// a scan is fine.
pub(crate) fn covered(ranges: &[Range<usize>], i: usize) -> bool {
    ranges.iter().any(|range| range.contains(&i))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn head(seq: u64, ts: &'static str, batch: Option<u64>) -> Head<'static> {
        Head {
            seq,
            ts: Some(ts),
            batch,
        }
    }

    /// A complete batch, a single event, and a hand-appended line are all complete.
    #[test]
    fn complete_batches_and_single_lines_are_left_alone() {
        let heads = [
            head(1, "a", None),
            head(2, "b", Some(3)),
            head(3, "b", None),
            head(4, "b", None),
            head(5, "c", None),
        ];
        assert!(incomplete(&heads).is_empty());
    }

    /// The segment ended before the batch did: what a crash leaves.
    #[test]
    fn a_batch_cut_at_the_end_of_the_segment_is_incomplete() {
        let heads = [
            head(1, "a", None),
            head(2, "b", Some(3)),
            head(3, "b", None),
        ];
        assert_eq!(incomplete(&heads), vec![1..3]);
        assert!(!covered(&incomplete(&heads), 0));
        assert!(covered(&incomplete(&heads), 2));
    }

    /// The writer restarted after the crash and carried on: the batch is still short,
    /// because the lines after it carry another `ts`, and they are complete themselves.
    #[test]
    fn a_batch_cut_mid_file_is_incomplete_and_what_follows_is_not() {
        let heads = [
            head(2, "b", Some(3)),
            head(3, "b", None),
            head(4, "c", None),
            head(5, "d", Some(2)),
            head(6, "d", None),
        ];
        assert_eq!(incomplete(&heads), vec![0..2]);
    }

    /// A new header ends the previous batch, and a `seq` gap does too.
    #[test]
    fn a_new_header_or_a_seq_gap_ends_the_batch() {
        let heads = [
            head(1, "b", Some(3)),
            head(2, "b", None),
            head(3, "b", Some(2)),
            head(4, "b", None),
        ];
        assert_eq!(incomplete(&heads), vec![0..2]);
        let gap = [head(1, "b", Some(2)), head(3, "b", None)];
        assert_eq!(incomplete(&gap), vec![0..1]);
    }

    /// `batch` of 0 or 1 is not a marker.
    #[test]
    fn a_count_below_two_is_not_a_marker() {
        let heads = [head(1, "a", Some(1)), head(2, "a", Some(0))];
        assert!(incomplete(&heads).is_empty());
    }
}
