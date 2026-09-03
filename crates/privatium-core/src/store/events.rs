// Project:  Privatium™  |  File: crates/privatium-core/src/store/events.rs
// Authors:  Gabriel Mongefranco (@gabrielmongefranco)
// Created:  2026-09-03  |  Modified: 2026-09-03
// Summary:  The staged log: every sane event of one app, read from data/<slug>/log/*.jsonl
//           once, and spec/protocol.md §4.5's ranking over it. The materializer, the three
//           restore tiers and the snapshot writer all work from this one reading, which is
//           what keeps docs/plans/phase-1.md §2.5's equality structural. Reading only:
//           nothing here writes or forwards a line (§4.2).

use std::collections::BTreeMap;
use std::fs;
use std::path::Path;

use serde::Deserialize;
use serde_json::value::RawValue;

use crate::store::StoreError;

/// `op` (`spec/protocol.md §4.1`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Op {
    Put,
    Del,
    /// A value `pv/1` does not know. It takes part in the ranking — `§4.2` says a later
    /// version's line is still a line — and, having won, neither inserts nor tombstones.
    Other,
}

/// One sane event, as `§4.5` needs it.
#[derive(Debug, Clone)]
pub(crate) struct Event {
    pub seq: u64,
    pub lam: u64,
    /// As written. Compared as text, which for RFC 3339 UTC is time order.
    pub ts: Option<String>,
    pub dev: String,
    pub op: Op,
    pub tbl: String,
    pub id: String,
    /// The raw JSON text of `d`, untouched, so a number keeps its own digits
    /// (`spec/data-dictionary.md §2.1`).
    pub d: Option<String>,
}

impl Event {
    /// `§4.5` step 3: `(lam, ts, dev)` ascending, and the last wins. `seq` is a final
    /// tie-break so two lines from one device that agree on all three still order the
    /// same way every time.
    fn rank(&self) -> (u64, Option<&str>, &str, u64) {
        (self.lam, self.ts.as_deref(), &self.dev, self.seq)
    }
}

/// The envelope as a line spells it. Everything optional, because a line anyone may have
/// appended by hand is a line; `sane` below decides what qualifies.
#[derive(Deserialize)]
struct Line<'a> {
    seq: Option<u64>,
    lam: Option<u64>,
    ts: Option<String>,
    dev: Option<String>,
    app: Option<String>,
    op: Option<String>,
    tbl: Option<String>,
    id: Option<String>,
    #[serde(borrow)]
    d: Option<&'a RawValue>,
}

/// Every sane event of `app` under `log_dir`, in file order.
///
/// **What disqualifies a line.** Two families, two reasons. A line that is not an envelope
/// — unparseable, or missing `seq`, `lam`, `tbl` or `id` — has no place in a causal ordering
/// and is skipped; `§4.2`'s unknown *fields* are kept by never being read. And `§4.4`: an
/// event more than the horizon ahead of this node's clock must not win a row permanently,
/// so a `ts` past `cutoff` is skipped too — with the same mercy M2's reader grants, that a
/// `ts` this node cannot parse carries no information and is accepted, because rejecting it
/// would be gap rejection by another name and `§4.1` forbids a reader that.
///
/// No audit row is written from here. M2's `recover()` reports each rejection once.
pub(crate) fn read_log(log_dir: &Path, app: &str, cutoff: &str) -> Result<Vec<Event>, StoreError> {
    let horizon: Option<jiff::Timestamp> = cutoff.parse().ok();
    let mut segments: Vec<_> = match fs::read_dir(log_dir) {
        Ok(entries) => entries
            .filter_map(Result::ok)
            .map(|entry| entry.path())
            .filter(|path| path.extension().is_some_and(|ext| ext == "jsonl"))
            .collect(),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(Vec::new()),
        Err(error) => {
            return Err(StoreError::Schema {
                problem: format!("{}: {error}", log_dir.display()),
            });
        }
    };
    segments.sort();

    let mut events = Vec::new();
    for segment in segments {
        let text = fs::read_to_string(&segment).map_err(|error| StoreError::Schema {
            problem: format!("{}: {error}", segment.display()),
        })?;
        for raw in text.lines() {
            let raw = raw.trim_end_matches('\r');
            if raw.trim().is_empty() {
                continue;
            }
            let Ok(line) = serde_json::from_str::<Line<'_>>(raw) else {
                continue;
            };
            if let Some(event) = sane(line, app, horizon) {
                events.push(event);
            }
        }
    }
    Ok(events)
}

fn sane(line: Line<'_>, app: &str, horizon: Option<jiff::Timestamp>) -> Option<Event> {
    if line.app.as_deref() != Some(app) {
        return None;
    }
    let (seq, lam, tbl, id) = (line.seq?, line.lam?, line.tbl?, line.id?);
    if let (Some(ts), Some(horizon)) = (line.ts.as_deref(), horizon)
        && let Ok(at) = ts.parse::<jiff::Timestamp>()
        && at > horizon
    {
        return None;
    }
    let op = match line.op.as_deref() {
        Some("put") => Op::Put,
        Some("del") => Op::Del,
        _ => Op::Other,
    };
    Some(Event {
        seq,
        lam,
        ts: line.ts,
        dev: line.dev.unwrap_or_default(),
        op,
        tbl,
        id,
        d: line.d.map(|d| d.get().to_owned()),
    })
}

/// `§4.5` steps 2 and 3 over every table at once: the winning event per `(tbl, id)`.
pub(crate) fn winners(events: &[Event]) -> BTreeMap<(&str, &str), &Event> {
    let mut out: BTreeMap<(&str, &str), &Event> = BTreeMap::new();
    for event in events {
        let key = (event.tbl.as_str(), event.id.as_str());
        match out.get(&key) {
            Some(current) if current.rank() >= event.rank() => {}
            _ => {
                out.insert(key, event);
            }
        }
    }
    out
}

/// The highest `lam` among `events` — `hi_lam` (`spec/protocol.md §5.2`) — or 0.
pub(crate) fn hi_lam(events: &[Event]) -> u64 {
    events.iter().map(|e| e.lam).max().unwrap_or(0)
}

/// The highest `seq` per device — `hi_seq`. A line with no `dev` counts for no device.
pub(crate) fn hi_seq(events: &[Event]) -> BTreeMap<String, u64> {
    let mut out: BTreeMap<String, u64> = BTreeMap::new();
    for event in events {
        if event.dev.is_empty() {
            continue;
        }
        let head = out.entry(event.dev.clone()).or_default();
        *head = (*head).max(event.seq);
    }
    out
}

// AGENTS.md, Style: unwrap() is permitted in tests. The crate-level deny reaches unit
// tests inside src/, so each one opts out where it is declared.
#[allow(clippy::unwrap_used, clippy::expect_used)]
#[cfg(test)]
mod tests {
    use super::*;

    fn event(seq: u64, lam: u64, ts: &str, dev: &str, op: Op, id: &str) -> Event {
        Event {
            seq,
            lam,
            ts: Some(ts.to_owned()),
            dev: dev.to_owned(),
            op,
            tbl: "t".to_owned(),
            id: id.to_owned(),
            d: None,
        }
    }

    /// `§4.5` step 3, one key at a time.
    #[test]
    fn the_winner_is_by_lam_then_ts_then_dev() {
        let events = vec![
            event(1, 9, "2026-01-02T00:00:00.000Z", "a", Op::Put, "by-lam"),
            event(2, 10, "2026-01-01T00:00:00.000Z", "a", Op::Del, "by-lam"),
            event(3, 5, "2026-01-01T00:00:00.000Z", "a", Op::Put, "by-ts"),
            event(4, 5, "2026-01-02T00:00:00.000Z", "a", Op::Del, "by-ts"),
            event(5, 7, "2026-01-01T00:00:00.000Z", "a", Op::Put, "by-dev"),
            event(1, 7, "2026-01-01T00:00:00.000Z", "z", Op::Del, "by-dev"),
        ];
        let winners = winners(&events);
        assert_eq!(winners[&("t", "by-lam")].op, Op::Del);
        assert_eq!(winners[&("t", "by-ts")].op, Op::Del);
        assert_eq!(winners[&("t", "by-dev")].op, Op::Del);
        assert_eq!(hi_lam(&events), 10);
        assert_eq!(hi_seq(&events)["a"], 5);
        assert_eq!(hi_seq(&events)["z"], 1);
    }

    /// The reader: unknown fields kept by not being read, a line that is not an envelope
    /// skipped, `\r` tolerated, `§4.4` applied, and a number's digits kept verbatim.
    #[test]
    fn reading_a_log_anyone_may_append_to() {
        let dir = tempfile::tempdir().unwrap();
        let log = dir.path().join("log");
        fs::create_dir_all(&log).unwrap();
        fs::write(
            log.join("aaaaaaaa.jsonl"),
            concat!(
                r#"{"seq":1,"lam":1,"ts":"2026-01-01T00:00:00.000Z","dev":"aaaaaaaa","app":"hello","op":"put","tbl":"profile","id":"a","d":{"n":12.340,"origin":"pv/2"},"trace":[1]}"#,
                "\r\n",
                "not json\n",
                r#"{"seq":2,"lam":2,"ts":"2999-01-01T00:00:00.000Z","dev":"aaaaaaaa","app":"hello","op":"put","tbl":"profile","id":"future","d":{}}"#,
                "\n",
                r#"{"seq":3,"lam":3,"ts":"not a timestamp","dev":"aaaaaaaa","app":"hello","op":"del","tbl":"profile","id":"odd"}"#,
                "\n",
                r#"{"seq":4,"lam":4,"ts":"2026-01-01T00:00:00.000Z","dev":"aaaaaaaa","app":"other","op":"put","tbl":"profile","id":"x","d":{}}"#,
                "\n",
                r#"{"lam":5,"ts":"2026-01-01T00:00:00.000Z","dev":"aaaaaaaa","app":"hello","op":"put","tbl":"profile","id":"noseq","d":{}}"#,
                "\n",
            ),
        )
        .unwrap();
        let events = read_log(&log, "hello", "2026-06-01T00:00:00.000Z").unwrap();
        let ids: Vec<&str> = events.iter().map(|e| e.id.as_str()).collect();
        assert_eq!(ids, ["a", "odd"]);
        assert_eq!(
            events[0].d.as_deref(),
            Some(r#"{"n":12.340,"origin":"pv/2"}"#),
            "the digits are the line's own"
        );
        assert_eq!(events[1].op, Op::Del);
        assert!(
            read_log(
                &dir.path().join("absent"),
                "hello",
                "2026-06-01T00:00:00.000Z"
            )
            .unwrap()
            .is_empty()
        );
    }
}
