// Project:  Privatium™  |  File: crates/privatium-core/tests/log.rs
// Authors:  Gabriel Mongefranco (@gabrielmongefranco)
// Created:  2026-09-01  |  Modified: 2026-09-05
// Summary:  The event log against spec/protocol.md §4 — the envelope, gapless `seq`, a
//           reader that tolerates a gap, the Lamport clock across a restart, §4.4 clock
//           hygiene, and what a batch can promise on a file that must stay appendable by
//           `echo`.

// AGENTS.md, Style: unwrap() is permitted in tests, and a test that hides a failure
// behind `?` is worse than one that panics with a line number.
#![allow(clippy::unwrap_used, clippy::expect_used)]

use std::fs;
use std::io::Write as _;
use std::path::{Path, PathBuf};

use privatium_core::local::State;
use privatium_core::log::{AppLog, Durability, Reader, Recovered, Writer};
use privatium_core::{Error, Node};
use serde_json::Value;

/// An ordinary app, so these tests exercise the same path an app author would rather than
/// a special case reserved for `_sys`.
const APP: &str = "hello";

/// A node, plus one app log opened against it.
///
/// Opening and dropping one of these is a restart: the writer's `seq` and the app's Lamport
/// counter come back from the files and from `local/state.jsonl`, exactly as they would
/// after the process died.
#[derive(Debug)]
struct Session {
    node: Node,
    state: State,
    log: AppLog,
    recovered: Recovered,
}

impl Session {
    fn open(root: &Path) -> Self {
        Self::try_open(root).unwrap()
    }

    fn try_open(root: &Path) -> privatium_core::Result<Self> {
        let node = Node::open(root)?;
        let state = State::load(&node.paths().local_state())?;
        let (log, recovered) = AppLog::open(node.paths(), APP, node.id(), Durability::Os, &state)?;
        Ok(Session {
            node,
            state,
            log,
            recovered,
        })
    }

    /// Persist what this session learned, the way `Node::flush` does for `_sys`.
    fn close(mut self) {
        self.log.save_to(&mut self.state);
        self.state.flush().unwrap();
    }

    fn dev(&self) -> String {
        self.node.id().as_str().to_owned()
    }

    fn log_path(&self) -> PathBuf {
        self.log.path().to_path_buf()
    }

    /// The app log's lines, parsed.
    fn events(&self) -> Vec<Value> {
        events_in(&self.log_path())
    }
}

fn events_in(path: &Path) -> Vec<Value> {
    fs::read_to_string(path)
        .unwrap()
        .lines()
        .filter(|line| !line.trim().is_empty())
        .map(|line| serde_json::from_str(line).unwrap())
        .collect()
}

/// `data/_sys/log/<dev>.jsonl`, parsed.
fn sys_events(node: &Node) -> Vec<Value> {
    events_in(&node.paths().app_log("_sys", node.id()))
}

/// Append a line by hand, exactly as `apps/hello/README.md` blesses `echo >>`.
///
/// Opened in append mode and terminated with a bare `0x0A`, because a test that wrote
/// `\r\n` on Windows would be testing something else.
fn hand_append(path: &Path, line: &str) {
    let mut file = fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(path)
        .unwrap();
    file.write_all(line.as_bytes()).unwrap();
    file.write_all(b"\n").unwrap();
}

/// An RFC 3339 UTC timestamp offset from now, to the millisecond (`§4.1`).
fn ts_offset_secs(seconds: i64) -> String {
    let stamped = jiff::Timestamp::now() + jiff::SignedDuration::from_secs(seconds);
    jiff::fmt::temporal::DateTimePrinter::new()
        .precision(Some(3))
        .timestamp_to_string(&stamped)
}

// ---------------------------------------------------------------------------------------
// §4.1 — the envelope
// ---------------------------------------------------------------------------------------

/// `spec/protocol.md §4.1` — every required field, correctly typed, and `d` present on a
/// `put` and **absent** on a `del`.
///
/// The tombstone half is the one M1 could not have: its envelope struct had `op` as a
/// constant and `d` as a required field, so "MUST be absent when `op` is `del`" had nothing
/// to be true of.
#[test]
fn test_spec_4_1_envelope_shape() {
    let root = tempfile::tempdir().unwrap();
    let mut session = Session::open(root.path());

    session
        .log
        .put(
            "profile",
            "01J9YQ2W7C8XKF3M0N5RTVB6ZP",
            &serde_json::json!({"display_name": "Gabriel"}),
        )
        .unwrap();
    session
        .log
        .del("profile", "01J9YQ2W7C8XKF3M0N5RTVB6ZP")
        .unwrap();

    let events = session.events();
    assert_eq!(events.len(), 2);

    for event in &events {
        assert!(event["seq"].is_u64(), "seq is not an integer: {event}");
        assert!(event["lam"].is_u64(), "lam is not an integer: {event}");
        // §4.1: `dev` MUST equal the log filename, `app` MUST equal the containing
        // directory. Both are read off the path rather than restated, so this fails if the
        // writer ever stamps something the layout does not agree with.
        assert_eq!(event["dev"].as_str().unwrap(), session.dev());
        assert_eq!(event["app"].as_str().unwrap(), APP);
        assert_eq!(event["tbl"], "profile");
        assert_eq!(event["id"], "01J9YQ2W7C8XKF3M0N5RTVB6ZP");

        let ts = event["ts"].as_str().unwrap();
        assert_eq!(ts.len(), "2026-08-28T14:03:11.412Z".len(), "{ts}");
        assert!(ts.ends_with('Z'), "{ts}");
        assert!(ts.parse::<jiff::Timestamp>().is_ok(), "{ts}");
    }

    assert_eq!(events[0]["op"], "put");
    assert!(events[0]["d"].is_object(), "a put must carry `d`");

    assert_eq!(events[1]["op"], "del");
    assert!(
        events[1].get("d").is_none(),
        "§4.1: `d` MUST be absent when `op` is `del`, not null and not empty"
    );

    let filename = session
        .log_path()
        .file_name()
        .unwrap()
        .to_string_lossy()
        .into_owned();
    assert_eq!(filename, format!("{}.jsonl", session.dev()));
}

/// `§4.1` — a writer MUST emit `seq` gapless. Across a restart, which is where a writer
/// that recovered its counter from a line count or from thin air would go wrong.
#[test]
fn test_spec_4_1_seq_gapless_on_write() {
    let root = tempfile::tempdir().unwrap();

    let mut session = Session::open(root.path());
    for n in 0..5 {
        session
            .log
            .put("note", &format!("id-{n}"), &serde_json::json!({"n": n}))
            .unwrap();
    }
    session.close();

    let mut session = Session::open(root.path());
    assert_eq!(session.log.seq(), 5, "seq did not survive the restart");
    for n in 5..10 {
        session
            .log
            .put("note", &format!("id-{n}"), &serde_json::json!({"n": n}))
            .unwrap();
    }

    let seqs: Vec<u64> = session
        .events()
        .iter()
        .map(|event| event["seq"].as_u64().unwrap())
        .collect();
    assert_eq!(seqs, (1..=10).collect::<Vec<u64>>());
}

/// `§4.1` — a reader MUST NOT reject, reorder, or repair a `seq` gap in a local log file.
///
/// Gap rejection belongs to sync (`§10.2`), where the missing range can actually be
/// requested, and it arrives in Phase 3. Here the gap is simply history: the reader yields
/// all three lines and the writer continues from what is in the file, so the events it adds
/// are gapless from the head without the hole ever being touched.
#[test]
fn test_spec_4_1_reader_tolerates_gap() {
    let root = tempfile::tempdir().unwrap();

    let dev = {
        let session = Session::open(root.path());
        let dev = session.dev();
        session.close();
        dev
    };

    // A log with 1, 2, 5 — as a hand-edit or a partial restore would leave it.
    let path = root
        .path()
        .join("data")
        .join(APP)
        .join("log")
        .join(format!("{dev}.jsonl"));
    fs::write(&path, b"").unwrap();
    for seq in [1u64, 2, 5] {
        hand_append(
            &path,
            &format!(
                r#"{{"seq":{seq},"lam":{seq},"ts":"{}","dev":"{dev}","app":"{APP}","op":"put","tbl":"note","id":"id-{seq}","d":{{}}}}"#,
                ts_offset_secs(-60)
            ),
        );
    }
    let before = fs::read(&path).unwrap();

    let mut session = Session::open(root.path());

    // The reader hands back all three, and says nothing about the hole.
    let read: Vec<_> = session
        .log
        .reader()
        .unwrap()
        .lines()
        .map(std::result::Result::unwrap)
        .collect();
    assert_eq!(read.len(), 3, "the reader dropped a line over a seq gap");
    assert!(session.recovered.rejected.is_empty());
    assert!(session.recovered.malformed.is_empty());

    // The writer continues from the head, not from the count.
    session
        .log
        .put("note", "id-next", &serde_json::json!({}))
        .unwrap();
    let seqs: Vec<u64> = session
        .events()
        .iter()
        .map(|event| event["seq"].as_u64().unwrap())
        .collect();
    assert_eq!(
        seqs,
        vec![1, 2, 5, 6],
        "the writer did not resume from the head"
    );

    // And nothing repaired the hole on the way past it.
    assert_eq!(&fs::read(&path).unwrap()[..before.len()], &before[..]);
}

// ---------------------------------------------------------------------------------------
// §4.2 — forward compatibility
// ---------------------------------------------------------------------------------------

/// `§4.2` — a reader MUST accept and preserve unknown top-level fields and unknown keys
/// inside `d`, byte for byte.
///
/// This is the mechanism by which a `pv/1` node and a `pv/2` node share a log without
/// either losing information, so the assertion is on the bytes and not on a re-parse: a
/// reader that round-tripped the line through a `Value` and back would pass a semantic
/// comparison and fail this one.
#[test]
fn test_spec_4_2_unknown_fields_preserved() {
    let root = tempfile::tempdir().unwrap();

    let (dev, path) = {
        let session = Session::open(root.path());
        let pair = (session.dev(), session.log_path());
        session.close();
        pair
    };

    let future_line = format!(
        r#"{{"seq":1,"lam":1,"ts":"{}","dev":"{dev}","app":"{APP}","op":"put","tbl":"note","id":"a","d":{{"body":"hi","mood":"curious"}},"origin":"pv/2","trace":[1,2,3]}}"#,
        ts_offset_secs(-60)
    );
    fs::write(&path, b"").unwrap();
    hand_append(&path, &future_line);
    let before = fs::read(&path).unwrap();

    let mut session = Session::open(root.path());

    // The reader hands the line back exactly as it found it.
    let raw = session
        .log
        .reader()
        .unwrap()
        .lines()
        .next()
        .unwrap()
        .unwrap();
    assert_eq!(raw.raw(), future_line.as_bytes());

    // And appending after it does not disturb it.
    session
        .log
        .put("note", "b", &serde_json::json!({"body": "there"}))
        .unwrap();
    let after = fs::read(&path).unwrap();
    assert_eq!(
        &after[..before.len()],
        &before[..],
        "an unknown-field line was rewritten"
    );

    let events = session.events();
    assert_eq!(events[0]["origin"], "pv/2");
    assert_eq!(events[0]["trace"], serde_json::json!([1, 2, 3]));
    assert_eq!(events[0]["d"]["mood"], "curious");
}

// ---------------------------------------------------------------------------------------
// §4.3 — the Lamport clock
// ---------------------------------------------------------------------------------------

/// `§4.3` — `lam = max(lam_local, lam_max_seen) + 1`, over every way of writing an event.
#[test]
fn test_spec_4_3_lamport_monotonic() {
    let root = tempfile::tempdir().unwrap();
    let mut session = Session::open(root.path());

    session
        .log
        .put("note", "a", &serde_json::json!({}))
        .unwrap();
    session.log.del("note", "a").unwrap();
    session
        .log
        .batch(|batch| {
            batch.put("note", "b", &serde_json::json!({}))?;
            batch.put("note", "c", &serde_json::json!({}))?;
            batch.del("note", "b")
        })
        .unwrap();

    let lams: Vec<u64> = session
        .events()
        .iter()
        .map(|event| event["lam"].as_u64().unwrap())
        .collect();
    assert_eq!(lams, vec![1, 2, 3, 4, 5]);
    assert_eq!(session.log.lam(), 5);
    session.close();

    // `lam_max_seen` is the other half of the max, and it is what a hand-appended line — or,
    // from Phase 3, a peer's event — supplies. The next write must land past it.
    let path = root.path().join("data").join(APP).join("log");
    let dev = Session::open(root.path()).dev();
    hand_append(
        &path.join(format!("{dev}.jsonl")),
        &format!(
            r#"{{"seq":6,"lam":500,"ts":"{}","dev":"{dev}","app":"{APP}","op":"put","tbl":"note","id":"d","d":{{}}}}"#,
            ts_offset_secs(-60)
        ),
    );

    let mut session = Session::open(root.path());
    assert_eq!(
        session.log.lam(),
        500,
        "a seen `lam` did not move the counter"
    );
    session
        .log
        .put("note", "e", &serde_json::json!({}))
        .unwrap();
    assert_eq!(session.log.lam(), 501);
}

/// `§4.3` and `spec/protocol.md §13` — the Lamport counter is monotonic across a restart.
///
/// Run twice, because `local/state.jsonl` must be an optimization and not the answer.
/// `§3` says `local/` is not required for restore and `AGENTS.md` says never sync it, so a
/// node whose `local/` was deleted — or restored from a backup that correctly excluded it —
/// has to come back with the same counter, re-derived from the logs.
#[test]
fn test_spec_4_3_lamport_survives_restart() {
    for delete_local in [false, true] {
        let root = tempfile::tempdir().unwrap();

        let mut session = Session::open(root.path());
        for n in 0..4 {
            session
                .log
                .put("note", &format!("id-{n}"), &serde_json::json!({"n": n}))
                .unwrap();
        }
        let before = session.log.lam();
        assert_eq!(before, 4);
        session.close();

        if delete_local {
            fs::remove_dir_all(root.path().join("local")).unwrap();
        }

        let mut session = Session::open(root.path());
        assert_eq!(
            session.log.lam(),
            before,
            "the counter did not survive a restart (local deleted: {delete_local})"
        );
        session
            .log
            .put("note", "after", &serde_json::json!({}))
            .unwrap();
        assert_eq!(session.log.lam(), before + 1);
    }
}

// ---------------------------------------------------------------------------------------
// §4.4 — clock hygiene
// ---------------------------------------------------------------------------------------

/// `§4.4` — an event whose `ts` is more than 24 hours in the future is rejected on ingest,
/// and the rejection is recorded in `sys_audit`.
///
/// "Rejected" here means excluded from the Lamport fold and audited. The line itself stays
/// exactly where it is: `§3.1` forbids modifying a log file and `§4.1` forbids repairing
/// one, so its *position* is still acknowledged — which is what keeps the writer gapless and
/// what stops the same bad line being reported again on every start.
#[test]
fn test_spec_4_4_future_ts_rejected() {
    let root = tempfile::tempdir().unwrap();

    let (dev, sys_log) = {
        let node = Node::open(root.path()).unwrap();
        let pair = (
            node.id().as_str().to_owned(),
            node.paths().app_log("_sys", node.id()),
        );
        // Identity founding wrote four events; nothing has gone wrong yet.
        assert_eq!(node.sys_log().lam(), 4);
        pair
    };

    hand_append(
        &sys_log,
        &format!(
            r#"{{"seq":5,"lam":9999,"ts":"{}","dev":"{dev}","app":"_sys","op":"put","tbl":"sys_setting","id":"x","d":{{}}}}"#,
            ts_offset_secs(48 * 60 * 60)
        ),
    );
    let with_bad_line = fs::read(&sys_log).unwrap();

    let node = Node::open(root.path()).unwrap();

    // The bogus `lam` did not pull the counter forward. What did move it is the audit row
    // this open wrote, which is event 6.
    assert!(
        node.sys_log().lam() < 9999,
        "a rejected event's `lam` was folded in: {}",
        node.sys_log().lam()
    );
    assert_eq!(node.sys_log().lam(), 5);

    let events = sys_events(&node);
    let audits: Vec<&Value> = events
        .iter()
        .filter(|event| event["tbl"] == "sys_audit" && event["d"]["kind"] == "event.rejected")
        .collect();
    assert_eq!(audits.len(), 1, "§4.4 requires the rejection in sys_audit");

    let row = &audits[0]["d"];
    assert_eq!(row["kind"], "event.rejected");
    assert_eq!(row["actor"], "system");
    assert_eq!(row["severity"], "warn");
    assert_eq!(row["subject"], dev.as_str());

    // §3.10 types `detail` as VARCHAR holding JSON, and §2.1 encodes VARCHAR as a string.
    // So it is a string containing JSON, not a nested object.
    let detail = row["detail"].as_str().expect("detail must be a string");
    let detail: Value = serde_json::from_str(detail).unwrap();
    assert_eq!(detail["seq"], 5);
    assert!(detail["ahead_secs"].as_i64().unwrap() > 24 * 60 * 60);
    // The file name, never the data root: sys_audit is replicated (§3.10).
    assert_eq!(detail["segment"], format!("{dev}.jsonl"));

    // The bad line was not touched.
    assert_eq!(
        &fs::read(&sys_log).unwrap()[..with_bad_line.len()],
        &with_bad_line[..]
    );
    drop(node);

    // And it is reported once, not on every start. A log cannot be edited to remove the
    // line, so a node that re-reported it would append two rows per restart forever.
    let node = Node::open(root.path()).unwrap();
    let audits = sys_events(&node)
        .iter()
        .filter(|event| event["tbl"] == "sys_audit" && event["d"]["kind"] == "event.rejected")
        .count();
    assert_eq!(audits, 1, "the same rejection was audited twice");
}

/// `§4.4`, second sentence — the node SHOULD warn when its own clock appears to have moved
/// backwards more than 60 seconds.
///
/// Simulated the only way an append-only log allows: the tail is dated ahead of the current
/// clock, which is indistinguishable from the clock having fallen behind it. Ninety seconds,
/// so it is inside the 24-hour rejection threshold and is a warning rather than a refusal.
#[test]
fn test_spec_4_4_backwards_clock_warns() {
    let root = tempfile::tempdir().unwrap();

    let (dev, sys_log) = {
        let node = Node::open(root.path()).unwrap();
        (
            node.id().as_str().to_owned(),
            node.paths().app_log("_sys", node.id()),
        )
    };

    hand_append(
        &sys_log,
        &format!(
            r#"{{"seq":5,"lam":5,"ts":"{}","dev":"{dev}","app":"_sys","op":"put","tbl":"sys_setting","id":"x","d":{{}}}}"#,
            ts_offset_secs(90)
        ),
    );

    let node = Node::open(root.path()).unwrap();
    let events = sys_events(&node);
    let audit = events
        .iter()
        .find(|event| event["tbl"] == "sys_audit" && event["d"]["kind"] == "clock.skew")
        .expect("no clock.skew row was written");

    assert_eq!(audit["d"]["kind"], "clock.skew");
    assert_eq!(audit["d"]["severity"], "warn");
    assert!(
        audit["d"].get("subject").is_none(),
        "clock.skew is about this node's own clock; §3.10's subject is a device, app, or \
         snapshot, and `actor: system` already says whose"
    );

    // Unlike a rejection, the event itself is accepted: 90 seconds is not 24 hours.
    assert_eq!(
        node.sys_log().lam(),
        6,
        "the event was folded in, then audited"
    );
}

/// A line that is not an envelope at all does not stop the reader.
///
/// Not audited, and deliberately. `§4.4` requires a *clock* rejection to reach `sys_audit`;
/// "envelope parses" is `§10.2`'s validation, which belongs to the sync receiver in Phase 3
/// and can only be acted on there. `§4.1` says a reader carries on.
#[test]
fn test_a_malformed_line_does_not_stop_the_reader() {
    let root = tempfile::tempdir().unwrap();

    let path = {
        let session = Session::open(root.path());
        let path = session.log_path();
        session.close();
        path
    };

    fs::write(&path, b"").unwrap();
    hand_append(&path, "this is not JSON");
    let session = Session::open(root.path());

    assert_eq!(session.recovered.malformed.len(), 1);
    assert_eq!(session.recovered.malformed[0].offset, 0);
    assert!(session.recovered.rejected.is_empty());
    assert_eq!(
        session.log.seq(),
        0,
        "a malformed line has no seq to recover"
    );
}

// ---------------------------------------------------------------------------------------
// §3.1 — one writer per log file
// ---------------------------------------------------------------------------------------

/// `AGENTS.md` 2 and `spec/protocol.md §3.1` — a node appends only to its own log.
///
/// The Phase 1 subset of `§13`'s conformance item, which also covers `§10.2`'s receiver and
/// therefore cannot be claimed until Phase 3. Two halves: another device's file is not
/// touched by ordinary operation, and a writer pointed at one is refused rather than
/// trusted.
#[test]
fn test_spec_3_1_never_writes_other_device_log() {
    let root = tempfile::tempdir().unwrap();

    let session = Session::open(root.path());
    let dev = session.dev();
    let log_dir = session.log.log_dir().to_path_buf();
    session.close();

    // A peer's log, as sync would leave it in Phase 3.
    let foreign_dev = "k7m2q9xf";
    let foreign = log_dir.join(format!("{foreign_dev}.jsonl"));
    let foreign_line = format!(
        r#"{{"seq":1,"lam":40,"ts":"{}","dev":"{foreign_dev}","app":"{APP}","op":"put","tbl":"note","id":"theirs","d":{{}}}}"#,
        ts_offset_secs(-60)
    );
    hand_append(&foreign, &foreign_line);
    let before = fs::read(&foreign).unwrap();

    let mut session = Session::open(root.path());
    for n in 0..3 {
        session
            .log
            .put("note", &format!("mine-{n}"), &serde_json::json!({"n": n}))
            .unwrap();
    }

    // Read, and folded into the Lamport counter — but never written to.
    assert_eq!(
        fs::read(&foreign).unwrap(),
        before,
        "a peer's log was modified"
    );
    assert_eq!(session.recovered.heads.get(foreign_dev), Some(&1));
    assert!(session.log.lam() > 40, "a peer's `lam` was not observed");
    assert_eq!(
        session.log.path().file_name().unwrap().to_string_lossy(),
        format!("{dev}.jsonl")
    );

    // And the writer refuses to be aimed at it, so this is a guarantee rather than a habit.
    let refused = Writer::open(foreign.clone(), APP, session.node.id(), 0, Durability::Os);
    assert!(
        matches!(refused, Err(Error::LogNotOurs { .. })),
        "a writer was allowed to open another device's log"
    );
    assert!(matches!(
        Writer::create(foreign, APP, session.node.id(), Durability::Os),
        Err(Error::LogNotOurs { .. })
    ));
}

// ---------------------------------------------------------------------------------------
// Batches
// ---------------------------------------------------------------------------------------

/// A batch is contiguous in `seq` and `lam`, shares one `ts`, and reaches the file as a
/// unit (`spec/lua-api.md §3.3`).
#[test]
fn test_batch_is_contiguous_and_shares_one_ts() {
    let root = tempfile::tempdir().unwrap();
    let mut session = Session::open(root.path());

    session
        .log
        .put("note", "before", &serde_json::json!({}))
        .unwrap();
    let written = session
        .log
        .batch(|batch| {
            batch.put("node", "a", &serde_json::json!({"text": "wombat"}))?;
            batch.put("node", "b", &serde_json::json!({"text": "penguin"}))?;
            batch.del("cursor", "cursor")
        })
        .unwrap();
    assert_eq!(written.len(), 3, "the lines as written come back");

    let events = session.events();
    assert_eq!(events.len(), 4);
    // What `batch` handed back is exactly what is in the file (`spec/protocol.md §4.2`).
    let disk = fs::read_to_string(session.log_path()).unwrap();
    for line in &written {
        assert!(disk.contains(std::str::from_utf8(line).unwrap()), "{disk}");
        assert!(!line.ends_with(b"\n"));
    }
    let batched = &events[1..];
    assert_eq!(
        batched
            .iter()
            .map(|e| e["seq"].as_u64().unwrap())
            .collect::<Vec<_>>(),
        vec![2, 3, 4]
    );
    assert_eq!(
        batched
            .iter()
            .map(|e| e["lam"].as_u64().unwrap())
            .collect::<Vec<_>>(),
        vec![2, 3, 4]
    );
    for event in batched {
        assert_eq!(event["ts"], batched[0]["ts"], "a batch is one moment");
    }
    assert_eq!(batched[2]["op"], "del");
    assert!(batched[2].get("d").is_none());
}

/// A batch that fails part-way writes nothing at all, and leaves `seq` and `lam` where they
/// were — `spec/lua-api.md §3.3`: "either appends every event or none".
#[test]
fn test_batch_that_fails_writes_nothing() {
    let root = tempfile::tempdir().unwrap();
    let mut session = Session::open(root.path());

    session
        .log
        .put("note", "kept", &serde_json::json!({}))
        .unwrap();
    let before = fs::read(session.log_path()).unwrap();

    let failed = session.log.batch(|batch| {
        batch.put("note", "doomed", &serde_json::json!({}))?;
        Err(Error::NoDataDir)
    });
    assert!(failed.is_err());

    assert_eq!(fs::read(session.log_path()).unwrap(), before);
    assert_eq!(session.log.seq(), 1);
    assert_eq!(session.log.lam(), 1);

    // And the next write takes the seq the abandoned batch did not.
    session
        .log
        .put("note", "next", &serde_json::json!({}))
        .unwrap();
    assert_eq!(session.log.seq(), 2);
}

/// `spec/protocol.md §4.1` — the first line of a batch of two or more carries
/// `"batch": n`, before `d`; no other line of the batch, no single event, and no line
/// appended by hand carries the key at all.
#[test]
fn test_spec_4_1_batch_marker_on_the_first_line_only() {
    let root = tempfile::tempdir().unwrap();
    let mut session = Session::open(root.path());
    session
        .log
        .put("note", "alone", &serde_json::json!({}))
        .unwrap();
    let written = session
        .log
        .batch(|batch| {
            batch.put("node", "a", &serde_json::json!({"text": "wombat"}))?;
            batch.put("node", "b", &serde_json::json!({"text": "penguin"}))?;
            batch.del("cursor", "cursor")
        })
        .unwrap();
    session
        .log
        .batch(|batch| batch.put("note", "one", &serde_json::json!({})))
        .unwrap();

    let events = session.events();
    assert_eq!(events.len(), 5);
    assert!(events[0].get("batch").is_none(), "{}", events[0]);
    assert_eq!(events[1]["batch"], 3, "{}", events[1]);
    assert!(events[2].get("batch").is_none(), "{}", events[2]);
    assert!(events[3].get("batch").is_none(), "{}", events[3]);
    assert!(
        events[4].get("batch").is_none(),
        "a batch of one is an event: {}",
        events[4]
    );
    // The key stands before `d`, so `d` stays last on the line.
    let first = std::str::from_utf8(&written[0]).unwrap();
    assert!(
        first.find("\"batch\":3").unwrap() < first.find("\"d\":").unwrap(),
        "{first}"
    );
}

/// `spec/protocol.md §4.1`, `spec/lua-api.md §3.3` — a batch that reached the disk with
/// fewer lines than it announced, cut on a line boundary, is skipped whole by every
/// reader: nothing of it materializes, the writer continues past it with the next
/// `seq`, the lines stay in the file, and `sys_audit` says so once. Cut mid-file, after
/// the writer resumed, it stays skipped and what follows is not.
#[test]
fn test_spec_4_1_incomplete_batch_is_skipped_by_replay_and_audited_once() {
    let root = tempfile::tempdir().unwrap();
    let ids = [
        "01K4B0000000000000000000A1",
        "01K4B0000000000000000000A2",
        "01K4B0000000000000000000A3",
    ];
    let path = {
        let mut node = Node::open(root.path()).unwrap();
        node.sys_log_mut()
            .batch(|batch| {
                for id in ids {
                    batch.put("sys_setting", id, &serde_json::json!({"value": "\"x\""}))?;
                }
                Ok(())
            })
            .unwrap();
        node.flush().unwrap();
        node.paths().app_log("_sys", node.id())
    };

    // Preserve the first two events of a three-event batch, after identity founding.
    let whole = fs::read_to_string(&path).unwrap();
    let lines: Vec<&str> = whole.lines().collect();
    assert_eq!(lines.len(), 7);
    assert!(lines[4].contains("\"batch\":3"), "{}", lines[4]);
    let kept = format!("{}\n", lines[..6].join("\n"));
    fs::write(&path, &kept).unwrap();

    let setting = |node: &Node, id: &str| -> Option<String> {
        node.store()
            .conn()
            .query_row("SELECT value FROM sys_setting WHERE id = ?", [id], |row| {
                row.get(0)
            })
            .ok()
    };
    let audits = |node: &Node| {
        sys_events(node)
            .into_iter()
            .filter(|e| e["tbl"] == "sys_audit" && e["d"]["kind"] == "batch.incomplete")
            .count()
    };

    let mut node = Node::open(root.path()).unwrap();
    assert!(
        setting(&node, ids[0]).is_none(),
        "a line of a short batch materialized"
    );
    assert!(setting(&node, ids[1]).is_none());
    assert_eq!(audits(&node), 1, "{:?}", sys_events(&node));
    // The lines stay — the file still begins with every byte that was there — and the
    // writer continues past them.
    assert!(fs::read_to_string(&path).unwrap().starts_with(&kept));
    let next = node
        .sys_log_mut()
        .put(
            "sys_setting",
            "01K4B0000000000000000000B1",
            &serde_json::json!({"value": "1"}),
        )
        .unwrap();
    assert_eq!(
        next, 8,
        "seq 5 and 6 are positions in the file, so the next is past the audit row at 7"
    );
    // A raw append to `_sys` is applied on the next refresh, as a request would.
    node.refresh().unwrap();
    assert_eq!(
        setting(&node, "01K4B0000000000000000000B1").as_deref(),
        Some("1")
    );
    node.flush().unwrap();
    drop(node);

    // Reopened: the short batch is still short (its `ts` is not the audit row's), still
    // skipped, and not reported again; what came after it is there.
    let node = Node::open(root.path()).unwrap();
    assert!(setting(&node, ids[0]).is_none());
    assert_eq!(
        setting(&node, "01K4B0000000000000000000B1").as_deref(),
        Some("1")
    );
    assert_eq!(audits(&node), 1);
}

/// A batch interrupted by a crash is **detectable**, which is the most an append-only,
/// `echo`-appendable JSONL file can offer for a cut that lands mid-line.
///
/// One `write_all` plus one `fsync` is the ceiling: a length prefix, a checksum footer, or a
/// temp-file rename would each buy true atomicity, and each would break `AGENTS.md`
/// invariant 1. A byte prefix that ends mid-line is reported by byte offset and never
/// repaired; one that ends on a line boundary is the batch marker's case, above.
/// Truncating the file to a byte inside the last line reproduces the first exactly, and
/// deterministically on all three platforms.
#[test]
fn test_batch_is_atomic_under_kill() {
    let root = tempfile::tempdir().unwrap();

    let path = {
        let mut session = Session::open(root.path());
        session
            .log
            .batch(|batch| {
                batch.put("note", "a", &serde_json::json!({"text": "first"}))?;
                batch.put("note", "b", &serde_json::json!({"text": "second"}))?;
                batch.put("note", "c", &serde_json::json!({"text": "third"}))
            })
            .unwrap();
        let path = session.log_path();
        session.close();
        path
    };

    let whole = fs::read(&path).unwrap();
    let last_line_starts = whole
        .iter()
        .take(whole.len() - 1)
        .enumerate()
        .filter(|(_, byte)| **byte == b'\n')
        .map(|(index, _)| index as u64 + 1)
        .next_back()
        .unwrap();

    // Cut ten bytes into the final line: what a kill during the write would leave.
    let truncated_len = last_line_starts + 10;
    fs::OpenOptions::new()
        .write(true)
        .open(&path)
        .unwrap()
        .set_len(truncated_len)
        .unwrap();

    let error = Session::try_open(root.path()).unwrap_err();
    match error {
        Error::PartialLine {
            path: named,
            offset,
        } => {
            assert_eq!(offset, last_line_starts, "the wrong byte offset was named");
            assert_eq!(named, path);
        }
        other => panic!("expected a partial line, got {other}"),
    }

    // Nothing was repaired, and nothing was truncated further: §3.1 forbids modifying a log
    // file, and that includes tidying away the damage.
    assert_eq!(fs::metadata(&path).unwrap().len(), truncated_len);

    // The complete lines before the damage are still readable, which is what makes the
    // report actionable rather than merely fatal.
    let reader = Reader::open(path.parent().unwrap()).unwrap();
    let mut complete = 0usize;
    for line in reader.lines() {
        match line {
            Ok(_) => complete += 1,
            Err(Error::PartialLine { .. }) => break,
            Err(other) => panic!("{other}"),
        }
    }
    assert_eq!(complete, 2);
}
