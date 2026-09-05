// Project:  Privatium™  |  File: crates/privatium-core/tests/snapshot.rs
// Authors:  Gabriel Mongefranco (@gabrielmongefranco)
// Created:  2026-09-02  |  Modified: 2026-09-05
// Summary:  spec/protocol.md §5 — the snapshot id and manifest, the three-tier read with
//           real bytes flipped on disk, when a snapshot does not apply, retention that
//           never prunes the oldest, verification, the weekly policy, and where the tier
//           used is recorded.

// AGENTS.md, Style: unwrap() is permitted in tests, and a test that hides a failure
// behind `?` is worse than one that panics with a line number.
#![allow(clippy::unwrap_used, clippy::expect_used)]

mod common;

use std::collections::BTreeSet;
use std::fs;

use common::{
    APP, Fixture, HELLO_DDL, TYPED_DDL, at, event, flip_byte, hand_append, ts_offset_secs,
};
use privatium_core::local::State;
use privatium_core::log::{AppLog, Durability};
use privatium_core::store::{self, LogRetention, Retention, SkipReason, Store, Tier, snapshot};
use privatium_core::{Error, Node};

/// Three events, spelled at a fixed instant so a snapshot named for a fixed week sees them.
const TS: &str = "2026-08-28T14:03:11.412Z";

fn seed_hello(fixture: &Fixture) {
    let dev = fixture.dev.clone();
    fixture.append(&event(
        1,
        1,
        TS,
        &dev,
        "profile",
        "a",
        Some(r#"{"display_name":"Gabriel"}"#),
    ));
    fixture.append(&event(
        2,
        2,
        TS,
        &dev,
        "profile",
        "b",
        Some(r#"{"display_name":"Ada"}"#),
    ));
    fixture.append(&event(3, 3, TS, &dev, "profile", "b", None));
}

/// The tail: written after the snapshot, causally after everything in it.
fn tail_hello(fixture: &Fixture) {
    let dev = fixture.dev.clone();
    let ts = ts_offset_secs(-30);
    fixture.append(&event(
        4,
        4,
        &ts,
        &dev,
        "profile",
        "c",
        Some(r#"{"display_name":"Grace"}"#),
    ));
    fixture.append(&event(
        5,
        5,
        &ts,
        &dev,
        "profile",
        "a",
        Some(r#"{"display_name":"Amended"}"#),
    ));
    fixture.append(&event(6, 6, &ts, &dev, "profile", "c", None));
    fixture.append(&event(
        7,
        7,
        &ts,
        &dev,
        "ghost",
        "g1",
        Some(r#"{"anything":1}"#),
    ));
    fixture.append(&event(8, 8, &ts, &dev, "ghost", "g1", None));
}

/// `^\d{4}-W\d{2}-[0-9a-z]{8}-\d+$`, by hand — no regex crate in the workspace.
fn is_snapshot_id(s: &str) -> bool {
    let parts: Vec<&str> = s.split('-').collect();
    parts.len() == 4
        && parts[0].len() == 4
        && parts[0].bytes().all(|b| b.is_ascii_digit())
        && parts[1].len() == 3
        && parts[1].starts_with('W')
        && parts[1][1..].bytes().all(|b| b.is_ascii_digit())
        && parts[2].len() == 8
        && parts[2]
            .bytes()
            .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit())
        && !parts[3].is_empty()
        && parts[3].bytes().all(|b| b.is_ascii_digit())
}

fn files_in(dir: &std::path::Path) -> BTreeSet<String> {
    fs::read_dir(dir)
        .unwrap()
        .map(|e| e.unwrap().file_name().to_string_lossy().into_owned())
        .collect()
}

fn count(node: &Node, sql: &str) -> i64 {
    node.store()
        .conn()
        .query_row(sql, [], |row| row.get(0))
        .unwrap()
}

// ---------------------------------------------------------------------------------------
// §5.1 and §5.2 — layout, id, manifest
// ---------------------------------------------------------------------------------------

/// `spec/protocol.md §5.1` — `<ISO-year>-W<week>-<dev>-<hi_lam>`, and the four files.
#[test]
fn test_spec_5_1_snapshot_id_format() {
    let fixture = Fixture::open(HELLO_DDL);
    seed_hello(&fixture);

    let snapshot = fixture.snapshot(at("2026-08-30T03:00:00Z"));
    let id = snapshot.id.to_string();
    assert!(is_snapshot_id(&id), "{id}");
    assert_eq!(id, format!("2026-W35-{}-3", fixture.dev));
    assert_eq!(snapshot.dir, fixture.snap_dir().join(&id));

    let expected: BTreeSet<String> = [
        "MANIFEST.json",
        "schema.sql",
        "profile.sqlite",
        "profile.csv",
    ]
    .into_iter()
    .map(str::to_owned)
    .collect();
    assert_eq!(files_in(&snapshot.dir), expected, "§5.1's layout, exactly");
    assert_eq!(
        files_in(&fixture.snap_dir()),
        BTreeSet::from([id.clone()]),
        "no .part left behind"
    );

    // Two digits, zero-padded, at the year boundary.
    let early = fixture.snapshot(at("2026-01-02T00:00:00Z"));
    assert!(
        early.id.to_string().starts_with("2026-W01-"),
        "{}",
        early.id
    );

    // The same id again replaces the directory rather than failing or duplicating.
    let again = fixture.snapshot(at("2026-08-30T04:00:00Z"));
    assert_eq!(again.id, snapshot.id);
    assert_eq!(files_in(&fixture.snap_dir()).len(), 2);
    assert!(snapshot::verify(&again.dir).unwrap().ok());
}

/// `spec/protocol.md §5.2` — the manifest, key for key, and nothing added.
#[test]
fn test_spec_5_2_manifest_shape() {
    let fixture = Fixture::open(HELLO_DDL);
    seed_hello(&fixture);
    let snapshot = fixture.snapshot(at("2026-08-30T03:00:00.000Z"));

    let text = fs::read_to_string(snapshot.dir.join("MANIFEST.json")).unwrap();
    let json: serde_json::Value = serde_json::from_str(&text).unwrap();

    let keys: BTreeSet<&str> = json
        .as_object()
        .unwrap()
        .keys()
        .map(String::as_str)
        .collect();
    let expected: BTreeSet<&str> = [
        "v",
        "snapshot_id",
        "app",
        "created",
        "hi_lam",
        "hi_seq",
        "engine",
        "tables",
    ]
    .into_iter()
    .collect();
    assert_eq!(keys, expected, "§5.2 names these keys and no others");

    // In the example's order, checked on the text since a parsed object forgets it.
    let mut cursor = 0;
    for key in [
        "\"v\"",
        "\"snapshot_id\"",
        "\"app\"",
        "\"created\"",
        "\"hi_lam\"",
        "\"hi_seq\"",
        "\"engine\"",
        "\"tables\"",
    ] {
        let found = text[cursor..]
            .find(key)
            .unwrap_or_else(|| panic!("{key} missing or out of order"));
        cursor += found + key.len();
    }

    assert_eq!(json["v"], 1);
    assert_eq!(json["snapshot_id"], snapshot.id.to_string());
    assert_eq!(json["app"], APP);
    assert_eq!(json["created"], "2026-08-30T03:00:00.000Z");
    assert_eq!(json["hi_lam"], 3);
    assert_eq!(
        json["hi_seq"],
        serde_json::json!({ fixture.dev.clone(): 3 })
    );

    let engine = json["engine"].as_str().unwrap();
    let version = engine
        .strip_prefix("sqlite ")
        .unwrap_or_else(|| panic!("{engine}"));
    assert!(
        version.starts_with(|c: char| c.is_ascii_digit()),
        "{engine}: no leading v"
    );

    let tables = json["tables"].as_array().unwrap();
    assert_eq!(tables.len(), 1);
    let table_keys: BTreeSet<&str> = tables[0]
        .as_object()
        .unwrap()
        .keys()
        .map(String::as_str)
        .collect();
    assert_eq!(
        table_keys,
        ["name", "rows", "sqlite_sha256", "csv_sha256"]
            .into_iter()
            .collect()
    );
    assert_eq!(tables[0]["name"], "profile");
    assert_eq!(tables[0]["rows"], 1, "b was deleted; a remains");
    for key in ["sqlite_sha256", "csv_sha256"] {
        let sha = tables[0][key].as_str().unwrap();
        assert_eq!(sha.len(), 64, "{key}");
        assert!(
            sha.bytes()
                .all(|b| b.is_ascii_hexdigit() && !b.is_ascii_uppercase()),
            "{key}: {sha}"
        );
    }

    // schema.sql: the storage types with the declared type beside each, the key and
    // nothing else — the table a tier loads into.
    let ddl = fs::read_to_string(snapshot.dir.join("schema.sql")).unwrap();
    assert!(
        ddl.contains(
            "CREATE TABLE \"profile\" (id TEXT PRIMARY KEY, \"display_name\" TEXT /* VARCHAR */);"
        ),
        "{ddl}"
    );
    assert!(!ddl.contains("NOT NULL"), "{ddl}");
}

/// A snapshot of an app nobody has written to is legal and empty; so is one of a
/// schema-less app, which has no tables to export.
#[test]
fn test_an_empty_or_schemaless_app_snapshots_cleanly() {
    let fixture = Fixture::open(HELLO_DDL);
    let snapshot = fixture.snapshot(at("2026-08-30T03:00:00Z"));
    assert_eq!(snapshot.manifest.hi_lam, 0);
    assert!(snapshot.manifest.hi_seq.is_empty());
    assert_eq!(snapshot.manifest.tables[0].rows, 0);
    assert!(snapshot::verify(&snapshot.dir).unwrap().ok());

    let sketch = Fixture::open("");
    let snapshot = sketch.snapshot(at("2026-08-30T03:00:00Z"));
    assert!(snapshot.manifest.tables.is_empty());
    assert_eq!(
        files_in(&snapshot.dir),
        ["MANIFEST.json", "schema.sql"]
            .into_iter()
            .map(str::to_owned)
            .collect()
    );
}

// ---------------------------------------------------------------------------------------
// §5.3 — the three tiers
// ---------------------------------------------------------------------------------------

/// `spec/protocol.md §5.3` tier 1 — the snapshot's SQLite file plus the log tail equals
/// the full replay, tombstones included, across a restart.
#[test]
fn test_spec_5_3_tier1_sqlite() {
    let fixture = Fixture::open(HELLO_DDL);
    seed_hello(&fixture);
    let snapshot = fixture.snapshot(jiff::Timestamp::now());
    tail_hello(&fixture);

    let (mut fixture, restored) = fixture.reopen_restoring(HELLO_DDL);
    assert_eq!(restored.tier, Tier::Sqlite, "{restored:?}");
    assert_eq!(
        restored.snapshot.as_deref(),
        Some(snapshot.id.to_string().as_str())
    );
    assert!(restored.skipped.is_empty(), "{restored:?}");
    assert!(!restored.unexpected());
    assert_eq!(fixture.store.restore_tier(), Some(Tier::Sqlite));

    let restored_digests = fixture.digests("profile");
    assert_eq!(
        fixture.cell("profile", "a", "display_name"),
        "Amended",
        "the tail's amendment"
    );
    assert_eq!(
        fixture.cell("profile", "b", "display_name"),
        "<MISSING>",
        "deleted before the snapshot"
    );
    assert_eq!(
        fixture.cell("profile", "c", "display_name"),
        "<MISSING>",
        "put and deleted in the tail"
    );
    assert!(
        fixture.store.is_tombstoned("ghost", "g1").unwrap(),
        "an undeclared table's tombstone"
    );

    fixture.rematerialize();
    assert_eq!(
        fixture.digests("profile"),
        restored_digests,
        "tier 1 and the replay disagree"
    );
}

/// `§5.3` tier 2 — the SQLite file with a real byte flipped fails its SHA-256 and CSV
/// plus `schema.sql` plus the tail takes over, with the same result.
#[test]
fn test_spec_5_3_tier2_on_sqlite_corruption() {
    let fixture = Fixture::open(HELLO_DDL);
    seed_hello(&fixture);
    let snapshot = fixture.snapshot(jiff::Timestamp::now());
    tail_hello(&fixture);
    flip_byte(&snapshot.dir.join("profile.sqlite"));

    // The dry run predicts it without touching a table.
    let plan = fixture.store.restore_dry_run(&store::cutoff_now()).unwrap();
    assert_eq!(plan.tier, Tier::Csv, "{plan:?}");

    let (mut fixture, restored) = fixture.reopen_restoring(HELLO_DDL);
    assert_eq!(restored.tier, Tier::Csv, "{restored:?}");
    assert_eq!(restored.skipped.len(), 1);
    assert_eq!(restored.skipped[0].tier, Tier::Sqlite);
    assert!(
        matches!(
            &restored.skipped[0].reason,
            SkipReason::ChecksumMismatch { table, file } if table == "profile" && file == "profile.sqlite"
        ),
        "{restored:?}"
    );
    assert!(!restored.unexpected());

    let restored_digests = fixture.digests("profile");
    fixture.rematerialize();
    assert_eq!(
        fixture.digests("profile"),
        restored_digests,
        "tier 2 and the replay disagree"
    );
}

/// `§5.3` tier 3 — both files bad, so the full replay, and reported as an unexpected fall
/// through (`spec/cli.md §7`).
#[test]
fn test_spec_5_3_tier3_on_csv_corruption() {
    let fixture = Fixture::open(HELLO_DDL);
    seed_hello(&fixture);
    let snapshot = fixture.snapshot(jiff::Timestamp::now());
    tail_hello(&fixture);
    flip_byte(&snapshot.dir.join("profile.sqlite"));
    flip_byte(&snapshot.dir.join("profile.csv"));

    let (fixture, restored) = fixture.reopen_restoring(HELLO_DDL);
    assert_eq!(restored.tier, Tier::Replay, "{restored:?}");
    assert!(restored.unexpected(), "{restored:?}");
    assert_eq!(restored.skipped.len(), 2);
    assert!(
        restored
            .skipped
            .iter()
            .all(|s| matches!(s.reason, SkipReason::ChecksumMismatch { .. }))
    );
    assert_eq!(fixture.cell("profile", "a", "display_name"), "Amended");
    assert_eq!(fixture.count("profile"), 1);
}

/// `spec/app-contract.md §4.5` — a snapshot of an older `schema.sql` does not apply;
/// the replay rebuilds with the new column NULL, and nothing calls that a failure.
#[test]
fn test_spec_5_3_stale_schema_falls_to_replay() {
    let fixture = Fixture::open(HELLO_DDL);
    seed_hello(&fixture);
    fixture.snapshot(jiff::Timestamp::now());

    let widened = "CREATE TABLE profile (
        id           VARCHAR PRIMARY KEY,
        display_name VARCHAR NOT NULL,
        nickname     VARCHAR
    );";
    let (fixture, restored) = fixture.reopen_restoring(widened);
    assert_eq!(restored.tier, Tier::Replay, "{restored:?}");
    assert!(restored.snapshot.is_some());
    assert!(
        restored
            .skipped
            .iter()
            .all(|s| s.reason == SkipReason::SchemaChanged),
        "{restored:?}"
    );
    assert!(!restored.unexpected());
    assert_eq!(fixture.cell("profile", "a", "nickname"), "<NULL>");
}

/// `§5.3`'s first applicability condition — an event the snapshot never saw whose `lam` is
/// not above `hi_lam` (`§4.1`'s cross-device case, or a hand-written line) cannot be
/// merged against snapshot rows that carry no `(lam, ts, dev)`. The replay is the answer,
/// and it is `§4.5`'s answer.
#[test]
fn test_spec_5_3_non_causal_tail_falls_to_replay() {
    let fixture = Fixture::open(HELLO_DDL);
    seed_hello(&fixture);
    fixture.snapshot(jiff::Timestamp::now());

    // seq 4 is past hi_seq, but lam 2 is not past hi_lam 3.
    let dev = fixture.dev.clone();
    fixture.append(&event(
        4,
        2,
        &ts_offset_secs(-30),
        &dev,
        "profile",
        "a",
        Some(r#"{"display_name":"older lam, newer seq"}"#),
    ));

    let (mut fixture, restored) = fixture.reopen_restoring(HELLO_DDL);
    assert_eq!(restored.tier, Tier::Replay, "{restored:?}");
    assert!(
        restored
            .skipped
            .iter()
            .all(|s| s.reason == SkipReason::TailNotCausal { events: 1 }),
        "{restored:?}"
    );
    assert!(!restored.unexpected());
    // §4.5: lam 2 beats lam 1 for `a`, whatever a snapshot thought.
    assert_eq!(
        fixture.cell("profile", "a", "display_name"),
        "older lam, newer seq"
    );
    let digests = fixture.digests("profile");
    fixture.rematerialize();
    assert_eq!(fixture.digests("profile"), digests);
}

/// `§5.3`'s second condition and `§5`'s "snapshots carry no authority" — a log replaced by
/// an older copy is replayed as it is; the snapshot does not resurrect what the log lost.
#[test]
fn test_spec_5_3_log_behind_snapshot_falls_to_replay() {
    let fixture = Fixture::open(HELLO_DDL);
    seed_hello(&fixture);
    fixture.snapshot(jiff::Timestamp::now());

    // An owner restores an older copy of the log: two lines instead of three.
    let path = fixture.log_path();
    let text = fs::read_to_string(&path).unwrap();
    let older: String = text.lines().take(2).map(|l| format!("{l}\n")).collect();
    fs::write(&path, older).unwrap();

    let (fixture, restored) = fixture.reopen_restoring(HELLO_DDL);
    assert_eq!(restored.tier, Tier::Replay, "{restored:?}");
    let dev = fixture.dev.clone();
    assert!(
        restored.skipped.iter().all(|s| s.reason
            == SkipReason::LogBehindSnapshot {
                dev: dev.clone(),
                have: 2,
                claimed: 3
            }),
        "{restored:?}"
    );
    assert!(!restored.unexpected());
    assert_eq!(
        fixture.cell("profile", "b", "display_name"),
        "Ada",
        "the third line's del is gone with the log"
    );
}

/// `spec/data-dictionary.md §2.1` through tier 2 — every type survives CSV, including the
/// values a CSV writer gets wrong: commas and quotes inside list elements, an empty string
/// that is not a NULL, a newline inside a value.
#[test]
fn test_spec_2_1_typed_columns_survive_csv() {
    let fixture = Fixture::open(TYPED_DDL);
    let dev = fixture.dev.clone();
    let ts = ts_offset_secs(-60);
    let full = serde_json::json!({
        "name": "line one\nline \"two\", with comma",
        "copay_amount": "12.34",
        "count": "9007199254740993",
        "ok": true,
        "filled_on": "2026-08-28",
        "seen_at": "2026-08-28T14:03:11.412Z",
        "tags": ["a,b", "q't", "[br]", "", "NULL", "sp ace"],
    });
    fixture.append(&event(
        1,
        1,
        &ts,
        &dev,
        "thing",
        "full",
        Some(&full.to_string()),
    ));
    fixture.append(&event(
        2,
        2,
        &ts,
        &dev,
        "thing",
        "empty",
        Some(r#"{"name":"","tags":[]}"#),
    ));
    fixture.append(&event(3, 3, &ts, &dev, "thing", "nulls", Some("{}")));
    let snapshot = fixture.snapshot(jiff::Timestamp::now());
    flip_byte(&snapshot.dir.join("thing.sqlite"));

    let (mut fixture, restored) = fixture.reopen_restoring(TYPED_DDL);
    assert_eq!(restored.tier, Tier::Csv, "{restored:?}");

    assert_eq!(
        fixture.cell("thing", "full", "name"),
        "line one\nline \"two\", with comma"
    );
    assert_eq!(fixture.cell("thing", "full", "copay_amount"), "12.34");
    assert_eq!(fixture.cell("thing", "full", "count"), "9007199254740993");
    assert_eq!(fixture.cell("thing", "full", "ok"), "1");
    assert_eq!(fixture.cell("thing", "full", "filled_on"), "2026-08-28");
    assert_eq!(
        fixture.cell("thing", "full", "seen_at"),
        "2026-08-28T14:03:11.412Z"
    );
    assert_eq!(
        fixture.cell("thing", "full", "tags"),
        r#"["a,b","q't","[br]","","NULL","sp ace"]"#
    );
    assert_eq!(
        fixture.cell("thing", "empty", "name"),
        "",
        "an empty string is not a NULL"
    );
    assert_eq!(fixture.cell("thing", "empty", "tags"), "[]");
    assert_eq!(fixture.cell("thing", "nulls", "name"), "<NULL>");
    assert_eq!(fixture.cell("thing", "nulls", "tags"), "<NULL>");

    let digests = fixture.digests("thing");
    fixture.rematerialize();
    assert_eq!(
        fixture.digests("thing"),
        digests,
        "tier 2 and the replay disagree"
    );
}

// ---------------------------------------------------------------------------------------
// §5.4 — retention
// ---------------------------------------------------------------------------------------

/// `spec/protocol.md §5.4` — expired snapshots go, the oldest never does, and snapshot
/// retention may not exceed log retention.
#[test]
fn test_spec_5_4_never_prunes_oldest() {
    let dir = tempfile::tempdir().unwrap();
    let ids = [
        "2024-W01-aaaaaaaa-1",
        "2024-W10-aaaaaaaa-2",
        "2025-W01-aaaaaaaa-3",
        "2026-W35-aaaaaaaa-4",
    ];
    for id in ids {
        fs::create_dir_all(dir.path().join(id)).unwrap();
    }
    // Things that are not snapshots are left alone.
    fs::create_dir_all(dir.path().join("2024-W02-aaaaaaaa-9.part")).unwrap();
    fs::write(dir.path().join("README.txt"), "not a snapshot").unwrap();

    let now = at("2026-09-02T00:00:00Z");
    let pruned = snapshot::prune(dir.path(), now, &Retention::default()).unwrap();
    let names = |ids: &[store::SnapshotId]| ids.iter().map(ToString::to_string).collect::<Vec<_>>();
    assert_eq!(
        names(&pruned.removed),
        vec!["2024-W10-aaaaaaaa-2", "2025-W01-aaaaaaaa-3"]
    );
    assert_eq!(
        names(&pruned.kept),
        vec!["2024-W01-aaaaaaaa-1", "2026-W35-aaaaaaaa-4"]
    );
    assert!(
        dir.path().join("2024-W01-aaaaaaaa-1").is_dir(),
        "the oldest must survive"
    );
    assert!(!dir.path().join("2025-W01-aaaaaaaa-3").exists());
    assert!(dir.path().join("2024-W02-aaaaaaaa-9.part").is_dir());
    assert!(dir.path().join("README.txt").is_file());

    // A second run finds nothing to do, and the oldest still stands whatever the setting.
    let again = snapshot::prune(
        dir.path(),
        now,
        &Retention {
            snapshot_days: 0,
            log: LogRetention::Forever,
        },
    )
    .unwrap();
    assert_eq!(names(&again.removed), vec!["2026-W35-aaaaaaaa-4"]);
    assert_eq!(names(&again.kept), vec!["2024-W01-aaaaaaaa-1"]);
    let last = snapshot::prune(
        dir.path(),
        now,
        &Retention {
            snapshot_days: 0,
            log: LogRetention::Forever,
        },
    )
    .unwrap();
    assert!(last.removed.is_empty());
    assert_eq!(names(&last.kept), vec!["2024-W01-aaaaaaaa-1"]);

    // The assertion `pv/1` can never trip, tripped.
    let error = snapshot::prune(
        dir.path(),
        now,
        &Retention {
            snapshot_days: 365,
            log: LogRetention::Days(30),
        },
    )
    .unwrap_err();
    assert!(error.to_string().contains("§5.4"), "{error}");
    assert!(
        dir.path().join("2024-W01-aaaaaaaa-1").is_dir(),
        "a refused prune deletes nothing"
    );
}

// ---------------------------------------------------------------------------------------
// Where the tier lives
// ---------------------------------------------------------------------------------------

/// `§5.3` "MUST record which tier succeeded" — on the store, in `local/state.jsonl`, in
/// `v_health`, through `Node::restore_tier`, and as bounded `sys_audit` rows.
#[test]
fn test_restore_reports_tier_used() {
    let root = tempfile::tempdir().unwrap();
    let mut node = Node::open(root.path()).unwrap();
    let snapshot = node.snapshot("_sys").unwrap();

    // Tier 1: the store, the local record, the view, the accessor.
    let restored = node.restore("_sys").unwrap();
    assert_eq!(restored.tier, Tier::Sqlite, "{restored:?}");
    assert_eq!(node.restore_tier("_sys"), Some(Tier::Sqlite));
    let state = State::load(&node.paths().local_state()).unwrap();
    let record = state
        .get("_sys")
        .unwrap()
        .materialized
        .restore
        .clone()
        .unwrap();
    assert_eq!(record.tier, Tier::Sqlite);
    assert_eq!(
        record.snapshot.as_deref(),
        Some(snapshot.id.to_string().as_str())
    );
    let (tier, snapshot_id, age, log_bytes): (i32, Option<String>, Option<i64>, i64) = node
        .store()
        .conn()
        .query_row(
            "SELECT restore_tier, snapshot_id, snapshot_age_days, log_bytes FROM v_health WHERE app_id = '_sys'",
            [],
            |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?, row.get(3)?)),
        )
        .unwrap();
    assert_eq!(tier, 1);
    assert_eq!(
        snapshot_id.as_deref(),
        Some(snapshot.id.to_string().as_str())
    );
    assert_eq!(age, Some(0), "the snapshot was taken just now");
    assert!(log_bytes > 0);
    assert_eq!(
        count(
            &node,
            "SELECT count(*) FROM sys_audit WHERE kind LIKE 'restore.%'"
        ),
        0,
        "tier 1 is not audited"
    );

    // Tier 2: audited once, however many times it recurs.
    flip_byte(&snapshot.dir.join("sys_device.sqlite"));
    assert_eq!(node.restore("_sys").unwrap().tier, Tier::Csv);
    assert_eq!(node.restore("_sys").unwrap().tier, Tier::Csv);
    assert_eq!(node.restore_tier("_sys"), Some(Tier::Csv));
    assert_eq!(
        count(
            &node,
            "SELECT count(*) FROM sys_audit WHERE kind = 'restore.tier2' AND severity = 'warn'"
        ),
        1
    );
    assert_eq!(
        count(
            &node,
            "SELECT restore_tier FROM v_health WHERE app_id = '_sys'"
        ),
        2
    );

    // Tier 3, unexpectedly: an alert (§3.10), once.
    flip_byte(&snapshot.dir.join("sys_device.csv"));
    let restored = node.restore("_sys").unwrap();
    assert_eq!(restored.tier, Tier::Replay);
    assert!(restored.unexpected(), "{restored:?}");
    assert_eq!(node.restore("_sys").unwrap().tier, Tier::Replay);
    let (alerts, subject): (i64, String) = node
        .store()
        .conn()
        .query_row(
            "SELECT count(*), min(subject) FROM sys_audit WHERE kind = 'restore.tier3' AND severity = 'alert'",
            [],
            |row| Ok((row.get(0)?, row.get(1)?)),
        )
        .unwrap();
    assert_eq!(alerts, 1);
    assert_eq!(
        subject,
        snapshot.id.to_string(),
        "§3.10: the subject is the snapshot"
    );
    assert_eq!(
        count(
            &node,
            "SELECT restore_tier FROM v_health WHERE app_id = '_sys'"
        ),
        3
    );

    // And a restart carries the record over without rebuilding.
    drop(node);
    let node = Node::open(root.path()).unwrap();
    assert_eq!(node.restore_tier("_sys"), Some(Tier::Replay));
    assert_eq!(
        count(
            &node,
            "SELECT count(*) FROM sys_audit WHERE kind LIKE 'restore.%'"
        ),
        2,
        "a restart re-audited nothing"
    );

    // Nothing is loaded for any other app yet, and that is an error rather than a shrug.
    assert!(matches!(
        Node::open(tempfile::tempdir().unwrap().path())
            .unwrap()
            .snapshot("hello"),
        Err(Error::AppNotLoaded { .. })
    ));
}

/// `cache/` deleted, a snapshot present: the reopen is a tier-1 restore — for `_sys`
/// through `Node::open`, and for an app store through the loader's two calls — and it
/// leaves `cache/` holding exactly the database, no `.wal` and no temp directory.
#[test]
fn test_reopen_restores_from_tier1_after_cache_deleted() {
    // `_sys`.
    let root = tempfile::tempdir().unwrap();
    {
        let mut node = Node::open(root.path()).unwrap();
        node.snapshot("_sys").unwrap();
    }
    fs::remove_dir_all(root.path().join("cache")).unwrap();
    let node = Node::open(root.path()).unwrap();
    let restored = node.store().restored().unwrap();
    assert_eq!(restored.tier, Tier::Sqlite, "{restored:?}");
    assert_eq!(count(&node, "SELECT count(*) FROM sys_device"), 1);
    assert_eq!(
        count(&node, "SELECT count(*) FROM sys_snapshot"),
        1,
        "the tail carried the snapshot's own row"
    );
    assert_eq!(
        count(
            &node,
            "SELECT count(*) FROM sys_audit WHERE kind = 'restore.tier3'"
        ),
        0,
        "not rebuilt from scratch"
    );
    assert_eq!(
        files_in(&root.path().join("cache")),
        BTreeSet::from(["_sys.sqlite".to_owned()])
    );
    drop(node);

    // An app store, with `local/` kept so the recorded watermark is the trap it was in M3.
    let mut fixture = Fixture::open_in(root, HELLO_DDL);
    seed_hello(&fixture);
    fixture.rematerialize();
    let before = fixture.digests("profile");
    fixture.snapshot(jiff::Timestamp::now());
    let mut state = State::load(&fixture.node.paths().local_state()).unwrap();
    fixture.store.save_to(&mut state);
    state.flush().unwrap();
    let root = fixture.release();
    fs::remove_dir_all(root.path().join("cache")).unwrap();

    let node = Node::open(root.path()).unwrap();
    let state = State::load(&node.paths().local_state()).unwrap();
    let (_log, _) = AppLog::open(node.paths(), APP, node.id(), Durability::Os, &state).unwrap();
    let mut store = Store::open(node.paths(), APP, HELLO_DDL).unwrap();
    store.restore_watermark(state.get(APP).unwrap().materialized.clone());
    assert!(
        store.refresh(&store::cutoff_now()).unwrap(),
        "a fresh cache must rebuild"
    );
    assert_eq!(store.restored().unwrap().tier, Tier::Sqlite);
    let fixture = Fixture {
        root,
        node,
        store,
        dev: String::new(),
    };
    assert_eq!(fixture.digests("profile"), before);
    assert_eq!(
        files_in(&fixture.root.path().join("cache")),
        BTreeSet::from(["_sys.sqlite".to_owned(), "hello.sqlite".to_owned()])
    );
}

/// `docs/backup-and-restore.md §3` — a node started against logs with no snapshot and no
/// cache says "I rebuilt from scratch" once, as the `restore.tier3` alert; a first run,
/// whose only events it wrote itself, does not.
#[test]
fn test_rebuilt_from_scratch_is_alerted_once() {
    let root = tempfile::tempdir().unwrap();
    let node = Node::open(root.path()).unwrap();
    assert_eq!(
        count(
            &node,
            "SELECT count(*) FROM sys_audit WHERE severity = 'alert'"
        ),
        0,
        "a first run alerts nothing"
    );
    drop(node);

    fs::remove_dir_all(root.path().join("cache")).unwrap();
    fs::remove_dir_all(root.path().join("local")).unwrap();
    // The alert is itself an event, so the tables were refreshed once more after it and
    // `restored()` describes that second, ordinary rebuild; the audit row is the evidence.
    let node = Node::open(root.path()).unwrap();
    assert_eq!(
        count(
            &node,
            "SELECT count(*) FROM sys_audit WHERE kind = 'restore.tier3' AND severity = 'alert'"
        ),
        1
    );
    drop(node);

    let node = Node::open(root.path()).unwrap();
    assert_eq!(
        count(
            &node,
            "SELECT count(*) FROM sys_audit WHERE kind = 'restore.tier3'"
        ),
        1,
        "alerted again on an ordinary restart"
    );
}

// ---------------------------------------------------------------------------------------
// §3.9, cli §7, §3.6 — the index row, verify, the policy, maintenance
// ---------------------------------------------------------------------------------------

/// `spec/data-dictionary.md §3.9` — a snapshot is indexed by an event like any other,
/// with `§2.1`'s encodings, and materializes into `sys_snapshot`.
#[test]
fn test_spec_3_9_snapshot_row_is_an_event() {
    let root = tempfile::tempdir().unwrap();
    let mut node = Node::open(root.path()).unwrap();
    let snapshot = node.snapshot("_sys").unwrap();

    let raw = fs::read_to_string(node.paths().app_log("_sys", node.id())).unwrap();
    let lines: Vec<serde_json::Value> = raw
        .lines()
        .map(|l| serde_json::from_str(l).unwrap())
        .collect();
    assert_eq!(
        lines.len(),
        6,
        "identity founding, then the index row and its audit"
    );
    let row = &lines[4];
    assert_eq!(row["tbl"], "sys_snapshot");
    assert_eq!(row["id"], snapshot.id.to_string());
    assert_eq!(row["d"]["app_id"], "_sys");
    assert_eq!(row["d"]["hi_lam"], "4", "§2.1: BIGINT as a string");
    assert_eq!(row["d"]["bytes"], snapshot.bytes.to_string());
    assert_eq!(row["d"]["created_by"], node.id().as_str());
    assert!(
        row["d"]["row_counts"]
            .as_str()
            .unwrap()
            .contains("\"sys_device\":1")
    );
    assert!(row["d"].get("verified_at").is_none());
    assert_eq!(lines[5]["tbl"], "sys_audit");
    assert_eq!(lines[5]["d"]["kind"], "snapshot.created");
    assert_eq!(lines[5]["d"]["subject"], snapshot.id.to_string());
    assert_eq!(lines[4]["ts"], lines[5]["ts"], "one batch, one instant");

    assert!(node.refresh().unwrap());
    let (hi_lam, created_by): (i64, String) = node
        .store()
        .conn()
        .query_row("SELECT hi_lam, created_by FROM sys_snapshot", [], |row| {
            Ok((row.get(0)?, row.get(1)?))
        })
        .unwrap();
    assert_eq!(hi_lam, 4);
    assert_eq!(created_by, node.id().as_str());
}

/// `spec/cli.md §7` `--verify` — checksums recomputed against the manifest; a flipped byte
/// is named, and a clean pass re-asserts the row with `verified_at`.
#[test]
fn test_spec_cli_7_verify_detects_a_flipped_byte() {
    let root = tempfile::tempdir().unwrap();
    let mut node = Node::open(root.path()).unwrap();
    let snapshot = node.snapshot("_sys").unwrap();

    let clean = node.verify_snapshot("_sys", &snapshot.id).unwrap();
    assert!(clean.ok());
    assert_eq!(clean.tables.len(), snapshot.manifest.tables.len());
    node.refresh().unwrap();
    assert_eq!(
        count(
            &node,
            "SELECT count(*) FROM sys_snapshot WHERE verified_at IS NOT NULL"
        ),
        1
    );

    flip_byte(&snapshot.dir.join("sys_app.csv"));
    let dirty = node.verify_snapshot("_sys", &snapshot.id).unwrap();
    assert!(!dirty.ok());
    let bad: Vec<&str> = dirty
        .tables
        .iter()
        .filter(|t| !(t.sqlite_ok && t.csv_ok))
        .map(|t| t.name.as_str())
        .collect();
    assert_eq!(bad, vec!["sys_app"]);
    let check = dirty.tables.iter().find(|t| t.name == "sys_app").unwrap();
    assert!(check.sqlite_ok && !check.csv_ok);

    // A missing file is a mismatch too, not a crash.
    fs::remove_file(snapshot.dir.join("sys_app.sqlite")).unwrap();
    assert!(!node.verify_snapshot("_sys", &snapshot.id).unwrap().ok());
}

/// `spec/data-dictionary.md §3.6` — the three `snapshot.*` keys come from `sys_setting`,
/// JSON-encoded scalars, with the dictionary's defaults.
#[test]
fn test_spec_3_6_snapshot_policy_reads_sys_setting() {
    let root = tempfile::tempdir().unwrap();
    let node = Node::open(root.path()).unwrap();
    let policy = node.snapshot_policy().unwrap();
    assert_eq!(
        (
            policy.retention_days,
            policy.interval_days,
            policy.min_events
        ),
        (365, 7, 100)
    );
    let dev = node.id().as_str().to_owned();
    let log = node.paths().app_log("_sys", node.id());
    drop(node);

    let ts = ts_offset_secs(-10);
    for (seq, key, value) in [
        (3, "snapshot.retention_days", "30"),
        (4, "snapshot.interval_days", "\"1\""),
        (5, "snapshot.min_events", "5"),
    ] {
        let d = serde_json::json!({ "value": value, "updated_at": ts }).to_string();
        hand_append(
            &log,
            &format!(
                r#"{{"seq":{seq},"lam":{seq},"ts":"{ts}","dev":"{dev}","app":"_sys","op":"put","tbl":"sys_setting","id":"{key}","d":{d}}}"#
            ),
            "\n",
        );
    }
    let node = Node::open(root.path()).unwrap();
    let policy = node.snapshot_policy().unwrap();
    assert_eq!(
        (
            policy.retention_days,
            policy.interval_days,
            policy.min_events
        ),
        (30, 1, 5)
    );
    assert_eq!(policy.retention().snapshot_days, 30);
}

/// The weekly schedule as API — due by interval or by event count, whichever first, with
/// retention applied after. The timer is M6's; the command is M11's.
#[test]
fn test_weekly_snapshot_is_due_by_interval_or_events() {
    let root = tempfile::tempdir().unwrap();
    let mut node = Node::open(root.path()).unwrap();
    let now = jiff::Timestamp::now();

    let first = node.maintain("_sys", now).unwrap();
    let snapshot = first
        .snapshot
        .expect("no snapshot yet, and events exist: due");
    assert!(first.pruned.removed.is_empty());

    let same_week = node
        .maintain("_sys", now + jiff::SignedDuration::from_hours(1))
        .unwrap();
    assert!(same_week.snapshot.is_none(), "nothing due an hour later");

    let later = node
        .maintain("_sys", now + jiff::SignedDuration::from_hours(24 * 8))
        .unwrap();
    let next = later.snapshot.expect("eight days later: due by interval");
    assert_ne!(next.id, snapshot.id);
    assert!(
        next.manifest.hi_lam > snapshot.manifest.hi_lam,
        "the second saw the first's index row"
    );

    assert!(matches!(
        node.maintain("hello", now),
        Err(Error::AppNotLoaded { .. })
    ));
}

/// `spec/protocol.md §5` — a snapshot is decided at one moment and read and written at
/// another: the job taken from the store names the log as it stood, a length per
/// segment, and what lands in the log afterwards — a request served while the node's
/// lock was released — is neither in its files nor in its marks. The same steps through
/// the node record the row once written, and a `.part` never survives.
#[test]
fn test_spec_5_snapshot_job_describes_the_moment_it_was_read() {
    let mut fixture = Fixture::open(HELLO_DDL);
    seed_hello(&fixture);
    let job = fixture
        .store
        .snapshot_job(fixture.node.id(), at("2026-08-30T03:00:00Z"))
        .unwrap();
    let id: snapshot::SnapshotId = format!("2026-W35-{}-3", fixture.dev).parse().unwrap();
    assert_eq!(job.slug(), APP);
    assert_eq!(job.segments().len(), 1);
    assert_eq!(
        job.segments()[0].1,
        fs::metadata(fixture.log_path()).unwrap().len(),
        "the job holds the log's length now"
    );
    // The log grows while the job is unwritten: `a` is amended, `c` comes and goes.
    tail_hello(&fixture);
    fixture.rematerialize();
    assert_eq!(
        fixture.cell("profile", "a", "display_name"),
        "Amended",
        "the tail is in the tables"
    );

    let snapshot = job.write().unwrap();
    assert_eq!(snapshot.id, id);
    assert_eq!(snapshot.manifest.hi_lam, 3);
    assert_eq!(snapshot.manifest.hi_seq[&fixture.dev], 3);
    assert_eq!(snapshot.manifest.tables[0].rows, 1, "a; b was deleted");
    let csv = fs::read_to_string(snapshot.dir.join("profile.csv")).unwrap();
    assert!(
        csv.contains("Gabriel") && !csv.contains("Amended"),
        "the files describe the moment the job was read: {csv}"
    );
    assert!(snapshot.dir.join(snapshot::MANIFEST_FILE).is_file());
    assert!(
        !fixture.snap_dir().join(format!("{id}.part")).exists(),
        "the part directory was renamed away"
    );

    // Through the node: decide and read, write, record.
    let now = jiff::Timestamp::now();
    let job = fixture
        .node
        .snapshot_due("_sys", now)
        .unwrap()
        .expect("events and no snapshot: due");
    let written = job.write().unwrap();
    fixture.node.record_snapshot(&written).unwrap();
    // The row is an event in `_sys`; the tables see it on the next refresh.
    fixture.node.refresh().unwrap();
    assert!(common::sys_row(&fixture.node, "sys_snapshot", &written.id.to_string()).is_some());
    assert!(
        fixture.node.snapshot_due("_sys", now).unwrap().is_none(),
        "nothing due a moment later"
    );
}

/// `spec/protocol.md §5` — the log grows while a snapshot is being read and written:
/// the job runs on another thread with no lock at all, the appends go on beside it,
/// and the snapshot describes the log as it stood when the job was taken.
#[test]
fn test_spec_5_snapshot_writes_while_the_log_grows() {
    let fixture = Fixture::open(HELLO_DDL);
    seed_hello(&fixture);
    let job = fixture
        .store
        .snapshot_job(fixture.node.id(), at("2026-08-30T03:00:00Z"))
        .unwrap();
    let writer = std::thread::spawn(move || job.write());
    let dev = fixture.dev.clone();
    let ts = ts_offset_secs(-10);
    for n in 4..=200u64 {
        fixture.append(&event(
            n,
            n,
            &ts,
            &dev,
            "profile",
            &format!("row-{n}"),
            Some(r#"{"display_name":"later"}"#),
        ));
    }
    let snapshot = writer.join().unwrap().unwrap();
    assert_eq!(snapshot.manifest.hi_seq[&fixture.dev], 3);
    assert_eq!(snapshot.manifest.hi_lam, 3);
    assert_eq!(snapshot.manifest.tables[0].rows, 1);
    assert_eq!(
        fs::read_to_string(fixture.log_path())
            .unwrap()
            .lines()
            .count(),
        200,
        "every append landed while the snapshot was written"
    );
    assert!(snapshot::verify(&snapshot.dir).unwrap().ok());
}

/// `spec/app-contract.md §7` — a snapshot is written while an app's read-only connection
/// is open on the same cache, and the connection sees nothing of it: the file is written
/// from the log, not from the tables, and the store needs no window to do it.
#[test]
fn test_spec_app_contract_7_snapshot_needs_no_window() {
    let mut fixture = Fixture::open(HELLO_DDL);
    seed_hello(&fixture);
    fixture.rematerialize();
    let app = fixture.store.app_conn().unwrap();
    let before: i64 = app
        .query_row("SELECT count(*) FROM profile", [], |row| row.get(0))
        .unwrap();
    let snapshot = fixture.snapshot(jiff::Timestamp::now());
    assert_eq!(snapshot.manifest.tables[0].rows, 1);
    let after: i64 = app
        .query_row("SELECT count(*) FROM profile", [], |row| row.get(0))
        .unwrap();
    assert_eq!(before, after);
    // The per-table file is a database of its own that any SQLite tool opens.
    let file = rusqlite::Connection::open_with_flags(
        snapshot.dir.join("profile.sqlite"),
        rusqlite::OpenFlags::SQLITE_OPEN_READ_ONLY,
    )
    .unwrap();
    let name: String = file
        .query_row(
            "SELECT display_name FROM profile WHERE id = 'a'",
            [],
            |row| row.get(0),
        )
        .unwrap();
    assert_eq!(name, "Gabriel");
}
