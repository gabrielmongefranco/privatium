// Project:  Privatium™  |  File: crates/privatium-core/tests/common/mod.rs
// Authors:  Gabriel Mongefranco (@gabrielmongefranco)
// Created:  2026-09-02  |  Modified: 2026-09-02
// Summary:  What tests/store.rs and tests/snapshot.rs share: a node plus one app store,
//           the event line of spec/protocol.md §4.1 spelled by hand, `echo >>`, and the
//           digests the §2.5 comparisons are made with.

// AGENTS.md, Style: unwrap() is permitted in tests. Each test binary uses a different
// subset of these helpers, so the unused ones are not a finding.
#![allow(clippy::unwrap_used, clippy::expect_used, dead_code)]

use std::fs;
use std::io::Write as _;
use std::path::{Path, PathBuf};

use privatium_core::Node;
use privatium_core::local::State;
use privatium_core::log::{AppLog, Durability};
use privatium_core::store::{self, Restored, Snapshot, Store};

/// An ordinary app, so these tests exercise the path an app author gets rather than a
/// special case reserved for `_sys`.
pub const APP: &str = "hello";

/// `apps/hello/schema.sql`, near enough — one table, one column.
pub const HELLO_DDL: &str = "CREATE TABLE profile (
     id           VARCHAR PRIMARY KEY,
     display_name VARCHAR NOT NULL
 );";

/// A schema exercising every row of `spec/data-dictionary.md §2.1`.
pub const TYPED_DDL: &str = "CREATE TABLE thing (
     id           VARCHAR PRIMARY KEY,
     name         VARCHAR,
     copay_amount DECIMAL(18,2),
     count        BIGINT,
     ok           BOOLEAN,
     filled_on    DATE,
     seen_at      TIMESTAMPTZ,
     tags         VARCHAR[]
 );";

/// A node plus a store over one app, which is what M5 will assemble for real.
pub struct Fixture {
    pub root: tempfile::TempDir,
    pub node: Node,
    pub store: Store,
    pub dev: String,
}

impl Fixture {
    pub fn open(ddl: &str) -> Self {
        let root = tempfile::tempdir().unwrap();
        Self::open_in(root, ddl)
    }

    /// Open against an existing root and build the tables by the full replay (tier 3).
    pub fn open_in(root: tempfile::TempDir, ddl: &str) -> Self {
        let mut fixture = Self::open_untouched(root, ddl);
        fixture.store.materialize(&store::cutoff_now()).unwrap();
        fixture
    }

    /// Open against an existing root without building anything yet.
    pub fn open_untouched(root: tempfile::TempDir, ddl: &str) -> Self {
        let node = Node::open(root.path()).unwrap();
        let dev = node.id().as_str().to_owned();
        // The app's log directory has to exist before the store globs it.
        let state = State::load(&node.paths().local_state()).unwrap();
        let (_log, _) = AppLog::open(node.paths(), APP, node.id(), Durability::Os, &state).unwrap();
        let store = Store::open(node.paths(), APP, ddl).unwrap();
        Self {
            root,
            node,
            store,
            dev,
        }
    }

    /// Reopen the store the way a restart would, keeping the same data root, and build
    /// the tables by the full replay.
    ///
    /// The store is dropped explicitly and first: DuckDB holds an exclusive lock on
    /// `cache/<slug>.duckdb`, so opening the replacement before releasing it fails with
    /// "being used by another process".
    pub fn reopen(self, ddl: &str) -> Self {
        Self::open_in(self.release(), ddl)
    }

    /// Reopen and build the tables by `spec/protocol.md §5.3`'s three tiers instead.
    pub fn reopen_restoring(self, ddl: &str) -> (Self, Restored) {
        let mut fixture = Self::open_untouched(self.release(), ddl);
        let restored = fixture.restore();
        (fixture, restored)
    }

    /// Drop the node and the store, keeping the data root.
    pub fn release(self) -> tempfile::TempDir {
        let Fixture {
            root, node, store, ..
        } = self;
        drop(store);
        drop(node);
        root
    }

    pub fn log_path(&self) -> PathBuf {
        self.node.paths().app_log(APP, self.node.id())
    }

    pub fn snap_dir(&self) -> PathBuf {
        self.node.paths().app_snap_dir(APP)
    }

    pub fn append(&self, line: &str) {
        hand_append(&self.log_path(), line, "\n");
    }

    pub fn rematerialize(&mut self) {
        self.store.materialize(&store::cutoff_now()).unwrap();
    }

    /// The three-tier read.
    pub fn restore(&mut self) -> Restored {
        self.store.restore(&store::cutoff_now()).unwrap()
    }

    /// A snapshot at `now`, by this node.
    pub fn snapshot(&self, now: jiff::Timestamp) -> Snapshot {
        self.store.snapshot(self.node.id(), now).unwrap()
    }

    /// One column of one row, as a string. `<NULL>` where the row exists but the value is
    /// NULL; `<MISSING>` where there is no row at all.
    pub fn cell(&self, table: &str, id: &str, column: &str) -> String {
        let sql = format!(
            "SELECT coalesce(CAST(\"{column}\" AS VARCHAR), '<NULL>') FROM \"{table}\" WHERE id = ?"
        );
        self.store
            .conn()
            .query_row(&sql, duckdb::params![id], |row| row.get::<_, String>(0))
            .unwrap_or_else(|_| "<MISSING>".to_owned())
    }

    pub fn count(&self, table: &str) -> i64 {
        self.store
            .conn()
            .query_row(&format!("SELECT count(*) FROM \"{table}\""), [], |row| {
                row.get(0)
            })
            .unwrap()
    }

    /// A deterministic fingerprint of the whole tombstone set, `(tbl, id)` pairs in order.
    pub fn tombstone_digest(&self) -> String {
        self.store
            .conn()
            .query_row(
                "SELECT coalesce(md5(string_agg(tbl || ':' || id, '|' ORDER BY tbl, id)), 'empty')
                 FROM pv._tombstone",
                [],
                |row| row.get::<_, String>(0),
            )
            .unwrap()
    }

    /// A deterministic fingerprint of a table's whole contents.
    pub fn digest(&self, table: &str) -> String {
        self.store
            .conn()
            .query_row(
                &format!(
                    "SELECT coalesce(md5(string_agg(t::VARCHAR, '|' ORDER BY t.id)), 'empty')
                     FROM \"{table}\" t"
                ),
                [],
                |row| row.get::<_, String>(0),
            )
            .unwrap()
    }

    /// The table's digest and the tombstone digest together — what every `§2.5`
    /// comparison is made on.
    pub fn digests(&self, table: &str) -> (String, String) {
        (self.digest(table), self.tombstone_digest())
    }
}

/// Append a line by hand, exactly as `apps/hello/README.md` blesses `echo >>`.
pub fn hand_append(path: &Path, line: &str, terminator: &str) {
    let mut file = fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(path)
        .unwrap();
    file.write_all(line.as_bytes()).unwrap();
    file.write_all(terminator.as_bytes()).unwrap();
}

/// One event line, spelled the way `spec/protocol.md §4.1` spells one.
pub fn event(
    seq: u64,
    lam: u64,
    ts: &str,
    dev: &str,
    tbl: &str,
    id: &str,
    d: Option<&str>,
) -> String {
    match d {
        Some(d) => format!(
            r#"{{"seq":{seq},"lam":{lam},"ts":"{ts}","dev":"{dev}","app":"{APP}","op":"put","tbl":"{tbl}","id":"{id}","d":{d}}}"#
        ),
        // §4.1: `d` MUST be absent on a del — not null, not `{}`.
        None => format!(
            r#"{{"seq":{seq},"lam":{lam},"ts":"{ts}","dev":"{dev}","app":"{APP}","op":"del","tbl":"{tbl}","id":"{id}"}}"#
        ),
    }
}

/// An RFC 3339 UTC timestamp offset from now, to the millisecond (`§4.1`).
pub fn ts_offset_secs(seconds: i64) -> String {
    privatium_core::log::format_ts(
        jiff::Timestamp::now() + jiff::SignedDuration::from_secs(seconds),
    )
}

/// A fixed instant.
pub fn at(rfc3339: &str) -> jiff::Timestamp {
    rfc3339.parse().unwrap()
}

/// Corrupt a file on disk: invert one byte in the middle. Real bytes, so the SHA-256 path
/// of `spec/protocol.md §5.3` is what notices, not a mock.
pub fn flip_byte(path: &Path) {
    let mut bytes = fs::read(path).unwrap();
    assert!(!bytes.is_empty(), "{}: nothing to flip", path.display());
    let middle = bytes.len() / 2;
    bytes[middle] ^= 0xFF;
    fs::write(path, bytes).unwrap();
}
