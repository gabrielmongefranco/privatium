// Project:  Privatium™  |  File: crates/privatium-core/tests/store.rs
// Authors:  Gabriel Mongefranco (@gabrielmongefranco)
// Created:  2026-09-01  |  Modified: 2026-09-02
// Summary:  Materialization against spec/protocol.md §4.5 and §4.6 — last-write-wins at row
//           granularity, tombstones, the §4.4 horizon, the §2.1 encodings, a cache that can
//           be deleted, a log anyone may append to by hand, and the §2.5 property that the
//           incremental apply, the full replay, and a restore from a snapshot all agree.

// AGENTS.md, Style: unwrap() is permitted in tests, and a test that hides a failure
// behind `?` is worse than one that panics with a line number.
#![allow(clippy::unwrap_used, clippy::expect_used)]

mod common;

use std::fs;

use common::{APP, Fixture, HELLO_DDL, TYPED_DDL, event, flip_byte, hand_append, ts_offset_secs};
use privatium_core::Node;
use privatium_core::local::State;
use privatium_core::log::{AppLog, Durability};
use privatium_core::store::{self, Store, Tier};

// ---------------------------------------------------------------------------------------
// §4.5 — replay and merge
// ---------------------------------------------------------------------------------------

/// `spec/protocol.md §4.5` step 3 — order by `(lam, ts, dev)` ascending, take the last.
///
/// Three writers on one `id`, arranged so that each key in turn is the one that decides:
/// a higher `lam` beats a lower one whatever its `ts`; equal `lam` falls to `ts`; equal
/// `lam` and `ts` fall to `dev`, which `§4.5` says is a deterministic tie-break carrying no
/// meaning. A materializer that ordered by `ts` first would pass the first case and fail
/// the second.
#[test]
fn test_spec_4_5_lww_by_lam_ts_dev() {
    let fixture = Fixture::open(HELLO_DDL);
    let early = ts_offset_secs(-600);
    let late = ts_offset_secs(-60);

    // `lam` decides, even though the loser is later in wall-clock time.
    fixture.append(&event(
        1,
        9,
        &late,
        "aaaaaaaa",
        "profile",
        "by-lam",
        Some(r#"{"display_name":"loser"}"#),
    ));
    fixture.append(&event(
        2,
        10,
        &early,
        "aaaaaaaa",
        "profile",
        "by-lam",
        Some(r#"{"display_name":"winner"}"#),
    ));

    // `lam` ties, so `ts` decides.
    fixture.append(&event(
        3,
        5,
        &early,
        "aaaaaaaa",
        "profile",
        "by-ts",
        Some(r#"{"display_name":"loser"}"#),
    ));
    fixture.append(&event(
        4,
        5,
        &late,
        "aaaaaaaa",
        "profile",
        "by-ts",
        Some(r#"{"display_name":"winner"}"#),
    ));

    // `lam` and `ts` both tie, so `dev` decides — lexicographically, highest wins.
    fixture.append(&event(
        5,
        7,
        &early,
        "aaaaaaaa",
        "profile",
        "by-dev",
        Some(r#"{"display_name":"loser"}"#),
    ));
    let other = fixture.log_path().with_file_name("zzzzzzzz.jsonl");
    hand_append(
        &other,
        &event(
            1,
            7,
            &early,
            "zzzzzzzz",
            "profile",
            "by-dev",
            Some(r#"{"display_name":"winner"}"#),
        ),
        "\n",
    );

    let mut fixture = fixture;
    fixture.rematerialize();

    assert_eq!(fixture.cell("profile", "by-lam", "display_name"), "winner");
    assert_eq!(fixture.cell("profile", "by-ts", "display_name"), "winner");
    assert_eq!(fixture.cell("profile", "by-dev", "display_name"), "winner");
}

/// `§4.5` — last-write-wins is at **row** granularity, not field granularity.
///
/// The second event omits a column the first supplied. The row is replaced by the later
/// `d`, so the omitted column is NULL; it is not merged forward. An app needing field-level
/// merge must model each field as its own row, and this is the assertion that keeps that
/// true.
#[test]
fn test_spec_4_5_row_granularity_not_field() {
    let ddl = "CREATE TABLE profile (
        id           VARCHAR PRIMARY KEY,
        display_name VARCHAR,
        nickname     VARCHAR
    );";
    let mut fixture = Fixture::open(ddl);
    let ts = ts_offset_secs(-60);

    fixture.append(&event(
        1,
        1,
        &ts,
        &fixture.dev.clone(),
        "profile",
        "r",
        Some(r#"{"display_name":"Gabriel","nickname":"Gabe"}"#),
    ));
    fixture.append(&event(
        2,
        2,
        &ts,
        &fixture.dev.clone(),
        "profile",
        "r",
        Some(r#"{"display_name":"Gabriel"}"#),
    ));
    fixture.rematerialize();

    assert_eq!(fixture.cell("profile", "r", "display_name"), "Gabriel");
    assert_eq!(
        fixture.cell("profile", "r", "nickname"),
        "<NULL>",
        "§4.5 is row-granularity; the earlier field must not survive the later event"
    );
}

// ---------------------------------------------------------------------------------------
// §4.6 — deletion
// ---------------------------------------------------------------------------------------

/// `§4.6` — `op: "del"` writes a tombstone and the row does not exist.
#[test]
fn test_spec_4_6_tombstone_removes_row() {
    let mut fixture = Fixture::open(HELLO_DDL);
    let ts = ts_offset_secs(-60);
    let dev = fixture.dev.clone();

    fixture.append(&event(
        1,
        1,
        &ts,
        &dev,
        "profile",
        "gone",
        Some(r#"{"display_name":"Gabriel"}"#),
    ));
    fixture.append(&event(
        2,
        2,
        &ts,
        &dev,
        "profile",
        "stays",
        Some(r#"{"display_name":"Ada"}"#),
    ));
    fixture.append(&event(3, 3, &ts, &dev, "profile", "gone", None));
    fixture.rematerialize();

    assert_eq!(fixture.cell("profile", "gone", "display_name"), "<MISSING>");
    assert_eq!(fixture.cell("profile", "stays", "display_name"), "Ada");
    assert_eq!(fixture.count("profile"), 1);

    // The tombstone is remembered, which is what the §4.6 rule below is enforced from.
    assert!(fixture.store.is_tombstoned("profile", "gone").unwrap());
    assert!(!fixture.store.is_tombstoned("profile", "stays").unwrap());
}

/// `§4.6` — a deleted `id` MUST NOT be reused, narrowed to what it protects.
///
/// The rule as written forbids a reference app: `apps/animals` deletes and recreates its
/// `'cursor'` singleton every round (`app.lua:113`, `:129` against `:51`, `:62`), and
/// `§4.1` explicitly blesses that key. So `§4.6` is about **minted** ids — a ULID that
/// belonged to one row must not become the key of a different one — and this PR amends the
/// spec to say so.
///
/// Three halves, and the split matters:
///
/// 1. The materializer **reports** the tombstone. That is the fact M9's data API refuses
///    on, since `spec/data-api.md §2` already restricts client-supplied ids to ULIDs, and
///    a browser is the only caller `§4.1` does not trust to choose a row key.
/// 2. A caller-supplied stable key may be re-asserted after a tombstone; the row returns.
/// 3. A hand-appended `put` after a `del` materializes per `§4.5`, because the materializer
///    follows `§4.5` and does not invent a rule of its own. Enforcement belongs on the
///    write path: `§4.4`'s clock hygiene is the only filter a reader is required to apply,
///    and `§4.1`'s mercy for a `seq` gap is the same principle.
#[test]
fn test_spec_4_6_deleted_id_not_reusable() {
    let mut fixture = Fixture::open(HELLO_DDL);
    let ts = ts_offset_secs(-60);
    let dev = fixture.dev.clone();
    let ulid = "01J9YQ2W7C8XKF3M0N5RTVB6ZP";

    // 1. A minted ULID, deleted. The tombstone is the fact M9 will refuse on.
    fixture.append(&event(
        1,
        1,
        &ts,
        &dev,
        "profile",
        ulid,
        Some(r#"{"display_name":"Gabriel"}"#),
    ));
    fixture.append(&event(2, 2, &ts, &dev, "profile", ulid, None));
    fixture.rematerialize();
    assert!(
        fixture.store.is_tombstoned("profile", ulid).unwrap(),
        "a deleted ULID must be reportable, or nothing can enforce §4.6"
    );

    // 2. A caller-supplied stable key. `apps/animals` does this every round, and it must
    //    keep working: the row comes back.
    fixture.append(&event(
        3,
        3,
        &ts,
        &dev,
        "profile",
        "cursor",
        Some(r#"{"display_name":"round one"}"#),
    ));
    fixture.append(&event(4, 4, &ts, &dev, "profile", "cursor", None));
    fixture.append(&event(
        5,
        5,
        &ts,
        &dev,
        "profile",
        "cursor",
        Some(r#"{"display_name":"round two"}"#),
    ));
    fixture.rematerialize();
    assert_eq!(
        fixture.cell("profile", "cursor", "display_name"),
        "round two"
    );
    assert!(
        !fixture.store.is_tombstoned("profile", "cursor").unwrap(),
        "a re-asserted key is no longer tombstoned"
    );

    // 3. §4.5 decides what a log says, even where §4.6 says the writer should not have.
    fixture.append(&event(
        6,
        6,
        &ts,
        &dev,
        "profile",
        ulid,
        Some(r#"{"display_name":"resurrected"}"#),
    ));
    fixture.rematerialize();
    assert_eq!(
        fixture.cell("profile", ulid, "display_name"),
        "resurrected",
        "the materializer follows §4.5; §4.6 is enforced on the write path"
    );
}

// ---------------------------------------------------------------------------------------
// §3.1 — the cache is disposable
// ---------------------------------------------------------------------------------------

/// `§3.1` and `§13`'s first conformance line — deleting `cache/` loses zero data.
///
/// `local/` goes too, because `§3` says it is not required for restore and `AGENTS.md`
/// says never to sync it: a node restored from a correct backup has neither. What comes
/// back must be identical, not merely similar, so the comparison is a digest of the whole
/// table rather than a row count.
#[test]
fn test_spec_3_1_delete_cache_loses_nothing() {
    let mut fixture = Fixture::open(HELLO_DDL);
    let ts = ts_offset_secs(-60);
    let dev = fixture.dev.clone();

    fixture.append(&event(
        1,
        1,
        &ts,
        &dev,
        "profile",
        "a",
        Some(r#"{"display_name":"Gabriel"}"#),
    ));
    fixture.append(&event(
        2,
        2,
        &ts,
        &dev,
        "profile",
        "b",
        Some(r#"{"display_name":"Ada"}"#),
    ));
    fixture.append(&event(
        3,
        3,
        &ts,
        &dev,
        "profile",
        "a",
        Some(r#"{"display_name":"Amended"}"#),
    ));
    fixture.append(&event(4, 4, &ts, &dev, "profile", "b", None));
    fixture.rematerialize();

    let before = fixture.digest("profile");
    let rows = fixture.count("profile");
    assert_eq!(rows, 1);

    // Release DuckDB's lock on the cache without dropping the `TempDir`, which would take
    // the whole data root — including the logs this test is about — with it.
    let root = fixture.release();

    fs::remove_dir_all(root.path().join("cache")).unwrap();
    fs::remove_dir_all(root.path().join("local")).unwrap();
    assert!(!root.path().join("cache").exists());
    assert!(
        root.path().join("data").join(APP).join("log").exists(),
        "the log must survive"
    );

    let node = Node::open(root.path()).unwrap();
    let state = State::load(&node.paths().local_state()).unwrap();
    let (_log, _) = AppLog::open(node.paths(), APP, node.id(), Durability::Os, &state).unwrap();
    let mut store = Store::open(node.paths(), APP, HELLO_DDL).unwrap();
    store.materialize(&store::cutoff_now()).unwrap();

    let after: String = store
        .conn()
        .query_row(
            "SELECT coalesce(md5(string_agg(t::VARCHAR, '|' ORDER BY t.id)), 'empty') FROM profile t",
            [],
            |row| row.get(0),
        )
        .unwrap();
    assert_eq!(
        after, before,
        "the rebuilt table differs from the one deleted"
    );
}

/// `§3.1` again, with `local/` **kept** — which is what an owner actually does, since `§3`
/// calls `cache/` "fully disposable" and says nothing about the two going together.
///
/// The trap: `local/state.jsonl` still records the watermark the deleted tables were
/// built from, the logs have not moved, so a store that adopted that watermark would see
/// nothing to rebuild and leave a database with schemas and no tables. Both halves are
/// checked — `_sys` through `Node::open`, which restores the watermark itself, and an app
/// store through the same two calls M5's loader will make.
#[test]
fn test_spec_3_1_delete_cache_only_still_materializes() {
    let mut fixture = Fixture::open(HELLO_DDL);
    let ts = ts_offset_secs(-60);
    let dev = fixture.dev.clone();

    fixture.append(&event(
        1,
        1,
        &ts,
        &dev,
        "profile",
        "a",
        Some(r#"{"display_name":"Gabriel"}"#),
    ));
    fixture.append(&event(
        2,
        2,
        &ts,
        &dev,
        "profile",
        "b",
        Some(r#"{"display_name":"Ada"}"#),
    ));
    fixture.append(&event(3, 3, &ts, &dev, "profile", "b", None));
    fixture.rematerialize();
    let before = fixture.digest("profile");
    assert!(fixture.store.is_tombstoned("profile", "b").unwrap());

    // Record the watermark the way a running node does, so the reopen below has one to
    // restore. Without this line the test could not reproduce the bug.
    let mut state = State::load(&fixture.node.paths().local_state()).unwrap();
    fixture.store.save_to(&mut state);
    state.flush().unwrap();

    let root = fixture.release();

    fs::remove_dir_all(root.path().join("cache")).unwrap();
    assert!(
        root.path().join("local").join("state.jsonl").is_file(),
        "local/ must survive: the point is that only cache/ went"
    );

    // `_sys`: `Node::open` restores its own watermark and refreshes.
    let node = Node::open(root.path()).unwrap();
    for table in ["sys_device", "sys_node"] {
        let rows: i64 = node
            .store()
            .conn()
            .query_row(&format!("SELECT count(*) FROM sys.{table}"), [], |row| {
                row.get(0)
            })
            .unwrap_or_else(|error| {
                panic!("sys.{table} is missing after cache/ was deleted: {error}")
            });
        assert_eq!(rows, 1, "sys.{table}");
    }

    // The app store, restored from the same `local/state.jsonl`.
    let state = State::load(&node.paths().local_state()).unwrap();
    let (_log, _) = AppLog::open(node.paths(), APP, node.id(), Durability::Os, &state).unwrap();
    let mut store = Store::open(node.paths(), APP, HELLO_DDL).unwrap();
    store.restore_watermark(state.get(APP).unwrap().materialized.clone());
    assert!(
        store.refresh(&store::cutoff_now()).unwrap(),
        "refresh trusted a watermark for tables that no longer exist"
    );

    let after: String = store
        .conn()
        .query_row(
            "SELECT coalesce(md5(string_agg(t::VARCHAR, '|' ORDER BY t.id)), 'empty') FROM profile t",
            [],
            |row| row.get(0),
        )
        .unwrap();
    assert_eq!(
        after, before,
        "the rebuilt table differs from the one deleted"
    );
    assert!(
        store.is_tombstoned("profile", "b").unwrap(),
        "pv._tombstone must exist and hold the same set"
    );
}

// ---------------------------------------------------------------------------------------
// §2.1 — the JSON encodings
// ---------------------------------------------------------------------------------------

/// `spec/data-dictionary.md §2.1` — `DECIMAL` crosses as a **string**, and arrives exact.
///
/// This is the reason DuckDB was chosen over SQLite (`docs/decisions/0001 §3`). The second
/// half is the one that would rot silently: a client that wrongly sent a JSON *number* must
/// still land exactly, because `json_extract_string` hands back the number's own text and
/// `VARCHAR → DECIMAL` parses it rather than routing it through a double.
#[test]
fn test_decimal_arrives_as_string() {
    let mut fixture = Fixture::open(TYPED_DDL);
    let ts = ts_offset_secs(-60);
    let dev = fixture.dev.clone();

    fixture.append(&event(
        1,
        1,
        &ts,
        &dev,
        "thing",
        "s",
        Some(r#"{"copay_amount":"12.34"}"#),
    ));
    fixture.append(&event(
        2,
        2,
        &ts,
        &dev,
        "thing",
        "n",
        Some(r#"{"copay_amount":12.34}"#),
    ));
    fixture.append(&event(
        3,
        3,
        &ts,
        &dev,
        "thing",
        "big",
        Some(r#"{"copay_amount":"99999999999.99"}"#),
    ));
    fixture.rematerialize();

    assert_eq!(fixture.cell("thing", "s", "copay_amount"), "12.34");
    assert_eq!(
        fixture.cell("thing", "n", "copay_amount"),
        "12.34",
        "a JSON number must not round-trip through a double"
    );
    assert_eq!(
        fixture.cell("thing", "big", "copay_amount"),
        "99999999999.99"
    );

    // Exact arithmetic, which a float would report as 0.30000000000000004.
    let sum: String = fixture
        .store
        .conn()
        .query_row(
            "SELECT CAST(sum(copay_amount) AS VARCHAR) FROM thing WHERE id IN ('s','n')",
            [],
            |row| row.get(0),
        )
        .unwrap();
    assert_eq!(sum, "24.68");

    // And the declared type really is DECIMAL, not something inferred.
    let ty: String = fixture
        .store
        .conn()
        .query_row(
            "SELECT data_type FROM duckdb_columns()
             WHERE table_name = 'thing' AND column_name = 'copay_amount'",
            [],
            |row| row.get(0),
        )
        .unwrap();
    assert_eq!(ty, "DECIMAL(18,2)");
}

/// `spec/data-dictionary.md §2.1` — one assertion per row of the encoding table.
#[test]
fn test_spec_2_1_json_encoding_round_trip() {
    let mut fixture = Fixture::open(TYPED_DDL);
    let ts = ts_offset_secs(-60);
    let dev = fixture.dev.clone();

    let d = r#"{"name":"Gabriel","copay_amount":"12.34","count":"9007199254740993",
                "ok":true,"filled_on":"2026-08-28","seen_at":"2026-08-28T14:03:11.412Z",
                "tags":["a","b"]}"#
        .replace('\n', "")
        .replace("                ", "");
    fixture.append(&event(1, 1, &ts, &dev, "thing", "x", Some(&d)));
    // Every column absent — §2.1 makes an omitted key equivalent to null.
    fixture.append(&event(2, 2, &ts, &dev, "thing", "empty", Some("{}")));
    fixture.rematerialize();

    assert_eq!(fixture.cell("thing", "x", "name"), "Gabriel");
    assert_eq!(fixture.cell("thing", "x", "copay_amount"), "12.34");
    // 2^53 + 1: a parser that took JSON numbers as doubles would return ...992.
    assert_eq!(fixture.cell("thing", "x", "count"), "9007199254740993");
    assert_eq!(fixture.cell("thing", "x", "ok"), "true");
    assert_eq!(fixture.cell("thing", "x", "filled_on"), "2026-08-28");
    assert!(
        fixture
            .cell("thing", "x", "seen_at")
            .starts_with("2026-08-28")
    );
    assert_eq!(fixture.cell("thing", "x", "tags"), "[a, b]");

    for column in [
        "name",
        "copay_amount",
        "count",
        "ok",
        "filled_on",
        "seen_at",
        "tags",
    ] {
        assert_eq!(
            fixture.cell("thing", "empty", column),
            "<NULL>",
            "an omitted key must be NULL, not a default"
        );
    }
}

// ---------------------------------------------------------------------------------------
// §4.2 and §4.4 — what a log that anyone may append to contains
// ---------------------------------------------------------------------------------------

/// `§4.2` and `§4.5`'s projection paragraph — a `pv/2` line materializes its known columns
/// and keeps everything else in the log.
///
/// Preservation is a property of the file, which is never rewritten; projection simply does
/// not read what no column matches. Both halves are asserted, because passing one and
/// failing the other is exactly the bug `§4.5` warns about.
#[test]
fn test_spec_4_2_unknown_fields_do_not_break_materialization() {
    let mut fixture = Fixture::open(HELLO_DDL);
    let ts = ts_offset_secs(-60);
    let dev = fixture.dev.clone();

    let future_line = format!(
        r#"{{"seq":1,"lam":1,"ts":"{ts}","dev":"{dev}","app":"{APP}","op":"put","tbl":"profile","id":"a","d":{{"display_name":"Gabriel","mood":"curious"}},"origin":"pv/2","trace":[1,2,3]}}"#
    );
    fixture.append(&future_line);
    let before = fs::read(fixture.log_path()).unwrap();
    fixture.rematerialize();

    assert_eq!(fixture.cell("profile", "a", "display_name"), "Gabriel");
    assert_eq!(fixture.count("profile"), 1);
    assert_eq!(
        fs::read(fixture.log_path()).unwrap(),
        before,
        "materializing rewrote the log"
    );
}

/// `§4.4` — an event more than 24 hours ahead does not win its row.
///
/// M2 excludes such an event from the Lamport fold, but the line is still in the file and
/// `read_json()` sees it. If it materialized it would own the row permanently, and a
/// rejection that only withholds a counter increment is not a rejection.
///
/// The second half is the mercy M2's reader also grants: a `ts` this node cannot parse
/// carries no information and is **accepted**, because dropping it would be gap rejection
/// by another name and `§4.1` forbids a reader that.
#[test]
fn test_spec_4_4_future_event_does_not_win_the_row() {
    let mut fixture = Fixture::open(HELLO_DDL);
    let dev = fixture.dev.clone();
    let ts = ts_offset_secs(-60);
    let far_future = ts_offset_secs(48 * 60 * 60);

    fixture.append(&event(
        1,
        1,
        &ts,
        &dev,
        "profile",
        "r",
        Some(r#"{"display_name":"honest"}"#),
    ));
    fixture.append(&event(
        2,
        9999,
        &far_future,
        &dev,
        "profile",
        "r",
        Some(r#"{"display_name":"FUTURE"}"#),
    ));
    fixture.append(&event(
        3,
        3,
        "not-a-timestamp",
        &dev,
        "profile",
        "odd",
        Some(r#"{"display_name":"unparseable ts"}"#),
    ));
    fixture.rematerialize();

    assert_eq!(
        fixture.cell("profile", "r", "display_name"),
        "honest",
        "a §4.4-rejected event took the row despite its huge lam"
    );
    assert_eq!(
        fixture.cell("profile", "odd", "display_name"),
        "unparseable ts",
        "an unparseable ts is accepted, exactly as M2's reader accepts it"
    );

    // An event just inside the horizon is ordinary and must materialize.
    fixture.append(&event(
        4,
        10,
        &ts_offset_secs(60 * 60),
        &dev,
        "profile",
        "soon",
        Some(r#"{"display_name":"an hour ahead"}"#),
    ));
    fixture.rematerialize();
    assert_eq!(
        fixture.cell("profile", "soon", "display_name"),
        "an hour ahead"
    );
}

/// `§4.4` — materializing does not re-audit what M2 already reported once.
///
/// A log cannot be edited to remove the offending line, so a node that reported it on every
/// materialization would append to `sys_audit` forever.
#[test]
fn test_a_future_event_is_not_audited_twice_by_materializing() {
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
            r#"{{"seq":3,"lam":9999,"ts":"{}","dev":"{dev}","app":"_sys","op":"put","tbl":"sys_setting","id":"x","d":{{}}}}"#,
            ts_offset_secs(48 * 60 * 60)
        ),
        "\n",
    );

    // Two opens. The first audits the rejection; the second must not.
    let _ = Node::open(root.path()).unwrap();
    let node = Node::open(root.path()).unwrap();

    let audits: i64 = node
        .store()
        .conn()
        .query_row("SELECT count(*) FROM sys.sys_audit", [], |row| row.get(0))
        .unwrap();
    assert_eq!(audits, 1, "the same rejection was audited twice");
}

// ---------------------------------------------------------------------------------------
// The `echo >>` acceptance property
// ---------------------------------------------------------------------------------------

/// `apps/hello/README.md` — append an event by hand, then reload the page.
///
/// Reload, not restart: the store has to notice a log file that grew behind it, which is
/// what `Store::refresh` is for.
///
/// Run twice, and the second time is the one that matters. PowerShell's `>>` terminates
/// lines with `0d 0a`, so a Windows owner following the README writes a `\r` the writer
/// never emits. M2's reader tolerates it because JSON treats it as whitespace; whether
/// DuckDB's `read_json(format = 'newline_delimited')` does was unverified until here.
#[test]
fn test_hand_appended_line_appears() {
    for (label, terminator) in [("lf", "\n"), ("crlf", "\r\n")] {
        let mut fixture = Fixture::open(HELLO_DDL);
        let ts = ts_offset_secs(-60);
        let dev = fixture.dev.clone();

        hand_append(
            &fixture.log_path(),
            &event(
                1,
                1,
                &ts,
                &dev,
                "profile",
                "the-ulid",
                Some(r#"{"display_name":"Gabriel"}"#),
            ),
            terminator,
        );
        assert!(fixture.store.refresh(&store::cutoff_now()).unwrap());
        assert_eq!(
            fixture.cell("profile", "the-ulid", "display_name"),
            "Gabriel",
            "{label}: the first hand-appended line never appeared"
        );

        // The README's own example: the next unused seq, amending the same id.
        hand_append(
            &fixture.log_path(),
            &event(
                2,
                2,
                &ts,
                &dev,
                "profile",
                "the-ulid",
                Some(r#"{"display_name":"Someone Else"}"#),
            ),
            terminator,
        );
        assert!(
            fixture.store.refresh(&store::cutoff_now()).unwrap(),
            "{label}: refresh did not notice the log growing"
        );
        assert_eq!(
            fixture.cell("profile", "the-ulid", "display_name"),
            "Someone Else",
            "{label}: the amendment never appeared"
        );
        assert_eq!(
            fixture.count("profile"),
            1,
            "{label}: an amendment made a second row"
        );

        // And a refresh with nothing new does not rebuild.
        assert!(
            !fixture.store.refresh(&store::cutoff_now()).unwrap(),
            "{label}: refresh rebuilt with nothing to do"
        );

        // The CR is still in the file: §3.1 forbids repairing a log.
        if terminator == "\r\n" {
            let raw = fs::read(fixture.log_path()).unwrap();
            assert!(raw.contains(&b'\r'), "the hand-written CR was removed");
        }
    }
}

// ---------------------------------------------------------------------------------------
// §2.5 — the incremental path is an optimization, and the replay is the definition
// ---------------------------------------------------------------------------------------

/// `docs/plans/phase-1.md §2.5` — an incremental apply must produce identical table
/// contents to a full replay of the same log, and so must a restore from a snapshot plus
/// the log tail (`spec/protocol.md §5.3`), at every tier.
///
/// A pseudo-random stream per fixed seed: puts and dels over a small id space, so rows
/// are amended and resurrected rather than merely accumulated, spread over a declared
/// table **and one `schema.sql` does not declare**, because `spec/data-api.md §2` accepts
/// such writes and the tombstone set has to agree about them too. Values are chosen to
/// hurt the CSV tier — commas, quotes, newlines, empty strings, NULLs, lists of them.
///
/// Halfway through the stream a snapshot is taken from the log; the rest of the stream is
/// the tail. Then, in order: the incremental tables, a full replay, a tier-1 restore, a
/// tier-2 restore with the Parquet file corrupted on disk, and a tier-3 restore with the
/// CSV corrupted too — every one compared by digest against the replay, the table's
/// contents and the whole of `pv._tombstone`. If any of them ever differ, that path is
/// wrong — the replay is the definition.
///
/// Every run is handed the **same** cutoff. Reading the clock twice would let a test fail
/// because time passed rather than because two paths disagree.
///
/// Four fixed seeds by default; `PRIVATIUM_PROPERTY_SEEDS=<n>` runs `n` derived seeds so
/// "enough iterations to trust it" is a number in a PR rather than whoever last ran it.
#[test]
fn test_incremental_matches_full_replay() {
    let seeds: Vec<u64> = match std::env::var("PRIVATIUM_PROPERTY_SEEDS") {
        Ok(count) => {
            let count: u64 = count.parse().expect("PRIVATIUM_PROPERTY_SEEDS is a number");
            (0..count)
                .map(|i| 0x2026_0901_u64.wrapping_add(i.wrapping_mul(0x9E37_79B9_7F4A_7C15)))
                .collect()
        }
        Err(_) => vec![0x2026_0901, 0x2026_0902, 0x0BAD_5EED, 0xDEAD_BEEF],
    };
    for seed in &seeds {
        incremental_matches_full_replay(*seed);
    }
    println!("§2.5 property held over {} seed(s)", seeds.len());
}

fn incremental_matches_full_replay(seed: u64) {
    let mut fixture = Fixture::open(TYPED_DDL);
    let now = jiff::Timestamp::now();
    let cutoff = store::cutoff_from(now);
    let dev = fixture.dev.clone();
    let ts = ts_offset_secs(-60);

    // A tiny xorshift, so the stream is identical on every platform and every run.
    let mut seed = seed;
    let mut next = move || {
        seed ^= seed << 13;
        seed ^= seed >> 7;
        seed ^= seed << 17;
        seed
    };

    let mut snapshot = None;
    for n in 1..200u64 {
        if n == 100 {
            snapshot = Some(fixture.snapshot(now));
        }
        let roll = next();
        let id = format!("id-{}", roll % 12);
        let put = roll % 4 != 0;
        // `ghost` is not in `TYPED_DDL`. Its events reach the log and the tombstone set
        // and nothing else.
        let tbl = if (roll >> 32) % 3 == 0 {
            "ghost"
        } else {
            "thing"
        };
        let name = match roll % 5 {
            0 => String::new(),
            1 => format!("n{n}, \"quoted\"\nsecond line"),
            2 => format!("n{n} 'apostrophe' [bracket]"),
            _ => format!("n{n}"),
        };
        let mut d = serde_json::json!({
            "name": name,
            "copay_amount": format!("{}.{:02}", roll % 1000, roll % 100),
            "count": (roll % 9_007_199_254_740_993_u64).to_string(),
            "ok": roll % 2 == 0,
            "tags": [format!("t{}", roll % 5), "a,b", "", "q't"],
        });
        if roll % 7 == 0 {
            // §2.1: an omitted key is NULL.
            d.as_object_mut().unwrap().remove("name");
            d.as_object_mut().unwrap().remove("tags");
        }
        let d = serde_json::to_string(&d).unwrap();

        fixture.append(&event(n, n, &ts, &dev, tbl, &id, put.then_some(d.as_str())));
        if put {
            let value: serde_json::Value = serde_json::from_str(&d).unwrap();
            fixture.store.apply(tbl, &id, Some(&value)).unwrap();
        } else {
            fixture
                .store
                .apply::<serde_json::Value>(tbl, &id, None)
                .unwrap();
        }
    }
    let snapshot = snapshot.unwrap();
    assert_eq!(snapshot.manifest.hi_lam, 99, "seed {seed:#x}");

    let incremental = fixture.digests("thing");
    let incremental_rows = fixture.count("thing");

    // Now the definition: a full replay of the very same log.
    fixture.store.materialize(&cutoff).unwrap();
    let replay = fixture.digests("thing");
    assert_eq!(
        fixture.count("thing"),
        incremental_rows,
        "seed {seed:#x}: row counts diverged"
    );
    assert_eq!(
        replay, incremental,
        "seed {seed:#x}: the incremental path and the full replay disagree (table, tombstones); §4.5 says the replay is right"
    );

    // Tier 1: Parquet plus the tail written after the snapshot.
    let restored = fixture.store.restore(&cutoff).unwrap();
    assert_eq!(restored.tier, Tier::Parquet, "seed {seed:#x}: {restored:?}");
    assert_eq!(
        restored.snapshot.as_deref(),
        Some(snapshot.id.to_string().as_str())
    );
    assert_eq!(
        fixture.digests("thing"),
        replay,
        "seed {seed:#x}: tier 1 and the full replay disagree"
    );

    // Tier 2: the Parquet file is corrupted on disk, so CSV plus schema.sql plus the tail.
    flip_byte(&snapshot.dir.join("thing.parquet"));
    let restored = fixture.store.restore(&cutoff).unwrap();
    assert_eq!(restored.tier, Tier::Csv, "seed {seed:#x}: {restored:?}");
    assert_eq!(
        fixture.digests("thing"),
        replay,
        "seed {seed:#x}: tier 2 and the full replay disagree"
    );

    // Tier 3: both files gone bad; the replay, reported as unexpected.
    flip_byte(&snapshot.dir.join("thing.csv"));
    let restored = fixture.store.restore(&cutoff).unwrap();
    assert_eq!(restored.tier, Tier::Replay, "seed {seed:#x}: {restored:?}");
    assert!(restored.unexpected(), "seed {seed:#x}: {restored:?}");
    assert_eq!(
        fixture.digests("thing"),
        replay,
        "seed {seed:#x}: tier 3 is the replay"
    );

    // The undeclared table left tombstones and nothing else.
    assert_ne!(replay.1, "empty", "seed {seed:#x}: no tombstones at all");
    let ghost_tables: i64 = fixture
        .store
        .conn()
        .query_row(
            "SELECT count(*) FROM duckdb_tables() WHERE table_name = 'ghost'",
            [],
            |row| row.get(0),
        )
        .unwrap();
    assert_eq!(
        ghost_tables, 0,
        "seed {seed:#x}: an undeclared table was created"
    );
}

/// `spec/protocol.md §4.6` with `spec/data-api.md §2` — a `del` in an app with no
/// `schema.sql` is still a tombstone, and `is_tombstoned` reports it.
///
/// This is what M9's data API has to consult to refuse a client-supplied ULID naming a
/// deleted row, and `apps/sketch` is exactly such an app. Both paths are checked: the
/// replay, for a hand-appended `del`, and the incremental apply, for one this node wrote.
#[test]
fn test_spec_4_6_tombstone_reportable_without_schema() {
    let mut fixture = Fixture::open("");
    let ts = ts_offset_secs(-60);
    let dev = fixture.dev.clone();

    fixture.append(&event(
        1,
        1,
        &ts,
        &dev,
        "stroke",
        "s1",
        Some(r#"{"points":[[0,0],[4,9]]}"#),
    ));
    fixture.append(&event(2, 2, &ts, &dev, "stroke", "s1", None));
    fixture.rematerialize();
    assert!(
        fixture.store.is_tombstoned("stroke", "s1").unwrap(),
        "a schema-less app's del must be reportable"
    );
    assert!(!fixture.store.is_tombstoned("stroke", "s2").unwrap());

    // The incremental path, for an event this node wrote.
    fixture
        .store
        .apply::<serde_json::Value>("stroke", "s2", None)
        .unwrap();
    assert!(fixture.store.is_tombstoned("stroke", "s2").unwrap());

    // And a re-asserted key clears it, on both paths alike.
    let value = serde_json::json!({"points": []});
    fixture.store.apply("stroke", "s2", Some(&value)).unwrap();
    assert!(!fixture.store.is_tombstoned("stroke", "s2").unwrap());

    // Still no table: the app has no schema.
    assert!(fixture.store.schema().tables.is_empty());
}

// ---------------------------------------------------------------------------------------
// spec/app-contract.md §7 — the sandbox
// ---------------------------------------------------------------------------------------

/// `spec/app-contract.md §7` — after sealing, app SQL cannot reach the filesystem.
///
/// The privilege boundary is in **time**, not in the handle: DuckDB makes
/// `enable_external_access` and `lock_configuration` `GLOBAL_ONLY`, so the seal covers
/// every connection on the instance, the framework's included. That is stronger than §7
/// asks and is the only shape the engine allows — which is why the last assertions here,
/// that the privileged connection is caught too, are a feature rather than a defect.
#[test]
fn test_spec_app_contract_7_sealed_connection_cannot_read_files() {
    let mut fixture = Fixture::open(HELLO_DDL);
    let key = fixture
        .node
        .paths()
        .node_key()
        .display()
        .to_string()
        .replace('\\', "/");

    assert!(
        fixture.store.app_conn().is_err(),
        "an unsealed store has no sandboxed handle"
    );
    fixture.store.seal().unwrap();
    assert!(fixture.store.is_sealed());

    let app = fixture.store.app_conn().unwrap();

    // Reading the table it was made for still works.
    let rows: i64 = app
        .query_row("SELECT count(*) FROM profile", [], |row| row.get(0))
        .unwrap();
    assert_eq!(rows, 0);

    // Everything that reaches the filesystem does not. `identity/node.key` is the file
    // §7 names as the reason this exists.
    for sql in [
        format!("SELECT * FROM read_json('{key}')"),
        format!("SELECT * FROM read_csv('{key}')"),
        format!("SELECT * FROM read_parquet('{key}')"),
        "COPY (SELECT 1) TO 'leak.csv'".to_owned(),
        "COPY (SELECT 1) TO 'leak.parquet' (FORMAT PARQUET)".to_owned(),
        "INSTALL httpfs".to_owned(),
        "ATTACH 'other.duckdb'".to_owned(),
    ] {
        assert!(
            app.execute_batch(&sql).is_err(),
            "app SQL was allowed to run: {sql}"
        );
    }

    // And the sandbox cannot be lifted, which is what `lock_configuration` last buys.
    assert!(
        app.execute_batch("SET enable_external_access = true")
            .is_err()
    );

    // The seal is instance-wide, so rematerializing, restoring and snapshotting now need
    // a fresh store rather than this one. Reported rather than silently producing an
    // empty table or an empty snapshot.
    assert!(fixture.store.materialize(&store::cutoff_now()).is_err());
    assert!(fixture.store.restore(&store::cutoff_now()).is_err());
    assert!(fixture.store.restore_dry_run(&store::cutoff_now()).is_err());
    assert!(
        fixture
            .store
            .snapshot(fixture.node.id(), jiff::Timestamp::now())
            .is_err()
    );
}

// ---------------------------------------------------------------------------------------
// schema.sql
// ---------------------------------------------------------------------------------------

/// An app whose log directory holds no segment yet still opens — schema-less or not.
///
/// `read_json()` refuses a glob that matches no file, and the tombstone set is now built
/// for every app rather than only those with declared tables, so this is the case that
/// would fail first: a `sketch` that nobody has drawn on. The declared-table half is the
/// same guard, checked because it is the same statement shape.
#[test]
fn test_an_app_with_no_log_yet_still_opens() {
    let root = tempfile::tempdir().unwrap();
    let node = Node::open(root.path()).unwrap();
    assert!(!node.paths().app_log_dir("sketch").exists());

    for (slug, ddl) in [("sketch", ""), ("blank", HELLO_DDL)] {
        let mut store = Store::open(node.paths(), slug, ddl).unwrap();
        store
            .materialize(&store::cutoff_now())
            .unwrap_or_else(|error| panic!("{slug}: {error}"));
        assert!(!store.is_tombstoned("stroke", "s1").unwrap());
        if !ddl.is_empty() {
            let rows: i64 = store
                .conn()
                .query_row("SELECT count(*) FROM profile", [], |row| row.get(0))
                .unwrap();
            assert_eq!(rows, 0, "{slug}: a table from no log must be empty");
        }
    }
}

/// `spec/app-contract.md §4.5` and `§5.3` — an app with no `schema.sql` has no tables and
/// still has its log. This is `apps/sketch`.
#[test]
fn test_schemaless_app_materializes_no_tables() {
    let mut fixture = Fixture::open("");
    let ts = ts_offset_secs(-60);
    let dev = fixture.dev.clone();

    fixture.append(&event(
        1,
        1,
        &ts,
        &dev,
        "stroke",
        "s1",
        Some(r#"{"points":[[0,0],[4,9]]}"#),
    ));
    fixture.rematerialize();

    assert!(fixture.store.schema().tables.is_empty());
    let tables: i64 = fixture
        .store
        .conn()
        .query_row(
            "SELECT count(*) FROM duckdb_tables() WHERE schema_name = 'main'",
            [],
            |row| row.get(0),
        )
        .unwrap();
    assert_eq!(tables, 0, "a schema-less app created a table");

    // The event is still in the log, which is the whole point of §5.3.
    let raw = fs::read_to_string(fixture.log_path()).unwrap();
    assert!(raw.contains("\"tbl\":\"stroke\""));
}

/// `spec/app-contract.md §4.5` — changing `schema.sql` rematerializes from the logs, and a
/// new column is NULL for events that predate it.
#[test]
fn test_a_changed_schema_rematerializes_and_new_columns_are_null() {
    let mut fixture = Fixture::open(HELLO_DDL);
    let ts = ts_offset_secs(-60);
    let dev = fixture.dev.clone();
    fixture.append(&event(
        1,
        1,
        &ts,
        &dev,
        "profile",
        "a",
        Some(r#"{"display_name":"Gabriel"}"#),
    ));
    fixture.rematerialize();
    assert_eq!(fixture.cell("profile", "a", "display_name"), "Gabriel");

    let widened = "CREATE TABLE profile (
        id           VARCHAR PRIMARY KEY,
        display_name VARCHAR NOT NULL,
        nickname     VARCHAR
    );";
    let fixture = fixture.reopen(widened);

    assert_eq!(fixture.cell("profile", "a", "display_name"), "Gabriel");
    assert_eq!(
        fixture.cell("profile", "a", "nickname"),
        "<NULL>",
        "a column added later must be NULL for events that predate it"
    );
}

/// `spec/app-contract.md §5` and `spec/data-api.md §1` — a `CREATE VIEW` in `schema.sql`
/// resolves, and survives being materialized more than once.
///
/// Both halves are the point. Views are re-created from DuckDB's own rendering of them, so
/// this pins that the rendering can be re-executed: if it could not, the second
/// materialization would fail with "view already exists" rather than quietly doing nothing,
/// and every rematerialize — a schema change, a restore, a hand-appended line — would break.
#[test]
fn test_a_view_resolves_and_survives_rematerializing() {
    let ddl = "CREATE TABLE node (
        id   VARCHAR PRIMARY KEY,
        kind VARCHAR,
        name VARCHAR
    );
    CREATE VIEW v_animals AS SELECT id, name FROM node WHERE kind = 'a';";

    let mut fixture = Fixture::open(ddl);
    let ts = ts_offset_secs(-60);
    let dev = fixture.dev.clone();

    fixture.append(&event(
        1,
        1,
        &ts,
        &dev,
        "node",
        "n1",
        Some(r#"{"kind":"a","name":"wombat"}"#),
    ));
    fixture.append(&event(
        2,
        2,
        &ts,
        &dev,
        "node",
        "n2",
        Some(r#"{"kind":"q","name":"does it swim?"}"#),
    ));
    fixture.rematerialize();

    let animals: i64 = fixture
        .store
        .conn()
        .query_row("SELECT count(*) FROM v_animals", [], |row| row.get(0))
        .unwrap();
    assert_eq!(
        animals, 1,
        "the view did not resolve against the materialized table"
    );

    // Again. This is the assertion that would fail if the view SQL could not be re-run.
    fixture.rematerialize();
    fixture.rematerialize();
    let animals: i64 = fixture
        .store
        .conn()
        .query_row("SELECT count(*) FROM v_animals", [], |row| row.get(0))
        .unwrap();
    assert_eq!(animals, 1);
}

/// The `sys` schema of `spec/data-dictionary.md §1` and `§3`, materialized by `Node::open`.
///
/// Step 4 of `docs/plans/phase-1.md §2.6`, checked through the two rows the bootstrap
/// wrote: they are events like any other, and they arrive in `sys` by the same §4.5 replay
/// an app's table gets.
#[test]
fn test_sys_materializes_the_bootstrap_rows() {
    let root = tempfile::tempdir().unwrap();
    let node = Node::open(root.path()).unwrap();

    let kind: String = node
        .store()
        .conn()
        .query_row(
            "SELECT kind FROM sys.sys_device WHERE id = ?",
            duckdb::params![node.id().as_str()],
            |row| row.get(0),
        )
        .unwrap();
    assert_eq!(kind, "node");

    let replica: bool = node
        .store()
        .conn()
        .query_row(
            "SELECT replica FROM sys.sys_device WHERE id = ?",
            duckdb::params![node.id().as_str()],
            |row| row.get(0),
        )
        .unwrap();
    assert!(replica, "spec/protocol.md §1: nodes are always replicas");

    let protocol: String = node
        .store()
        .conn()
        .query_row("SELECT protocol FROM sys.sys_node", [], |row| row.get(0))
        .unwrap();
    assert_eq!(protocol, "pv/1");

    // §1 of the data dictionary: `_sys` materializes into the schema `sys`, so nothing of
    // it lands in `main`.
    let stray: i64 = node
        .store()
        .conn()
        .query_row(
            "SELECT count(*) FROM duckdb_tables() WHERE schema_name = 'main'",
            [],
            |row| row.get(0),
        )
        .unwrap();
    assert_eq!(stray, 0);

    // §4's views resolve, `v_health` included: a first run is the replay, with no snapshot.
    let devices: i64 = node
        .store()
        .conn()
        .query_row("SELECT count(*) FROM sys.v_device_active", [], |row| {
            row.get(0)
        })
        .unwrap();
    assert_eq!(devices, 1);
    let (tier, snapshot): (i32, Option<String>) = node
        .store()
        .conn()
        .query_row(
            "SELECT restore_tier, snapshot_id FROM sys.v_health WHERE app_id = '_sys'",
            [],
            |row| Ok((row.get(0)?, row.get(1)?)),
        )
        .unwrap();
    assert_eq!(tier, 3);
    assert_eq!(snapshot, None);
    assert_eq!(node.restore_tier("_sys"), Some(Tier::Replay));
}
