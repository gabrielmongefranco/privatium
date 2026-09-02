// Project:  Privatium™  |  File: crates/privatium-core/src/store/mod.rs
// Authors:  Gabriel Mongefranco (@gabrielmongefranco)
// Created:  2026-09-01  |  Modified: 2026-09-02
// Summary:  One app's cache/<slug>.duckdb: the privileged connection that materializes it
//           from the log or from a snapshot (spec/protocol.md §5.3), the seal that turns it
//           into spec/app-contract.md §7's sandbox, the watermark that notices a log someone
//           appended to by hand, and the record of which restore tier built the tables.

use std::collections::BTreeMap;
use std::fs;
use std::path::{Path, PathBuf};

use duckdb::Connection;
use serde::Serialize;
use thiserror::Error;

use crate::config::Paths;

pub mod materialize;
pub mod restore;
pub mod schema;
pub mod snapshot;

pub use restore::{Restored, SkipReason, Skipped, Tier};
pub use schema::{Column, Schema, Table, View};
pub use snapshot::{
    LogRetention, Manifest, ManifestTable, Pruned, Retention, Snapshot, SnapshotError, SnapshotId,
    SnapshotPolicy, TableCheck, Verification,
};

use materialize::Source;

/// The `sys` schema `_sys` materializes into (`spec/data-dictionary.md §1`, `§3`).
pub const SYS_SCHEMA: &str = "sys";

/// The framework's own `schema.sql`, for `_sys`.
///
/// `_sys` is an app (`docs/plans/phase-1.md §2.6`) and gets no special machinery — only a
/// DDL that lives in the binary rather than in a folder, because it has no folder.
pub const SYS_DDL: &str = include_str!("sys.sql");

/// `§4.4`'s horizon: an event more than this far ahead is not materialized.
const MAX_FUTURE_HOURS: i64 = 24;

/// The framework's per-app health facts, in the `_sys` cache (`spec/data-dictionary.md §4`).
///
/// Node-local by nature — a restore tier is a fact about **this** node's cache — so it is a
/// table the framework writes into `cache/_sys.duckdb` rather than an event in the
/// replicated `_sys` log. Disposable, like everything in `cache/`.
const HEALTH_TABLE: &str = "pv.health";

/// Anything that can go wrong maintaining a cache database.
#[derive(Debug, Error)]
pub enum StoreError {
    /// DuckDB refused.
    #[error("duckdb: {0}")]
    Duck(#[source] duckdb::Error),

    /// A `schema.sql` is not something that can be materialized.
    #[error("schema.sql: {problem}")]
    Schema {
        /// What is wrong with it.
        problem: String,
    },

    /// An operation needing the filesystem ran after the store was sealed.
    #[error(
        "{slug}: the store is sealed; rematerializing and snapshotting need a privileged \
         instance (spec/app-contract.md §7)"
    )]
    Sealed {
        /// The app.
        slug: String,
    },

    /// A snapshot directory could not be written, read, or pruned (`spec/protocol.md §5`).
    #[error(transparent)]
    Snapshot(#[from] SnapshotError),
}

/// Which tier built the tables, and from which snapshot (`spec/protocol.md §5.3`, "MUST
/// record which tier succeeded").
///
/// Node-local: it describes this node's cache and would be actively wrong on another
/// machine (`spec/data-dictionary.md §1`), which is why it lives in `local/state.jsonl`
/// through [`Materialized`] and is never an event.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct RestoreRecord {
    /// The tier.
    pub tier: Tier,
    /// The newest snapshot at the time, whether or not the tier used it.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub snapshot: Option<String>,
    /// When, as a `ts`.
    pub at: String,
}

/// What was recorded about a materialization, so a later run can tell it is still valid.
///
/// Lives inside `local::Record` and therefore inside `local/state.jsonl` — the file
/// `spec/protocol.md §3` already names. No new file appears in `local/`, which is the same
/// rule `docs/plans/phase-1.md §2.2` applied to the CSRF secret.
#[derive(Debug, Clone, Default, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct Materialized {
    /// The `schema.sql` hash the tables were built from.
    ///
    /// `spec/app-contract.md §4.5`: changing `schema.sql` rematerializes from the logs.
    #[serde(default)]
    pub schema_hash: String,
    /// Segment file name → its length in bytes at the time of materialization.
    ///
    /// A hand-appended line changes a length, which is how `apps/hello/README.md`'s
    /// `echo >>` becomes visible without a restart.
    #[serde(default)]
    pub segments: BTreeMap<String, u64>,
    /// Which restore tier built the tables (M4). `None` in a record written before M4.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub restore: Option<RestoreRecord>,
}

impl Materialized {
    /// Whether the tables were built from the same schema and the same log bytes.
    ///
    /// The restore record is deliberately not compared: it says how the tables were built,
    /// not from what, and [`Store::refresh`] asks only the second question.
    #[must_use]
    pub fn same_inputs(&self, other: &Self) -> bool {
        self.schema_hash == other.schema_hash && self.segments == other.segments
    }

    /// Bytes across every segment — `sys.v_health`'s `log_bytes`.
    #[must_use]
    pub fn log_bytes(&self) -> u64 {
        self.segments.values().sum()
    }
}

/// One app's materialized cache.
///
/// **On the two connections of `spec/app-contract.md §7`.** They cannot be two handles.
/// DuckDB makes `enable_external_access`, `autoload_known_extensions` and
/// `lock_configuration` `GLOBAL_ONLY` — they belong to the database instance — and it
/// takes an exclusive lock on the file, so a second instance cannot open the same cache
/// alongside the first. The privilege boundary is therefore in **time**: the store opens
/// privileged, materializes, and is then [`seal`](Self::seal)ed, after which nothing on it
/// — including this connection — can reach the filesystem. Rematerializing
/// ([`restore`](Self::restore), [`materialize`](Self::materialize)) and writing a snapshot
/// ([`snapshot`](Self::snapshot)) drop the store and open a fresh one.
#[derive(Debug)]
pub struct Store {
    slug: String,
    path: PathBuf,
    log_dir: PathBuf,
    snap_dir: PathBuf,
    conn: Connection,
    schema: Schema,
    target_schema: Option<&'static str>,
    sealed: bool,
    watermark: Materialized,
    /// Whether `cache/<slug>.duckdb` did not exist when this store opened it, and no
    /// rebuild has happened since.
    ///
    /// `spec/protocol.md §3` calls `cache/` fully disposable and `§3.1` requires that
    /// deleting it loses nothing — and an owner who deletes it will usually leave `local/`
    /// alone, since `§3` says nothing about the two going together. A watermark read back
    /// from `local/state.jsonl` then describes tables that were in a file which no longer
    /// exists, and adopting it would let [`refresh`](Self::refresh) conclude an empty
    /// database is current. See [`restore_watermark`](Self::restore_watermark).
    fresh: bool,
    /// What the last [`restore`](Self::restore) on this instance did.
    restored: Option<Restored>,
}

impl Store {
    /// Open — or create — `cache/<slug>.duckdb`, privileged.
    ///
    /// External access stays **on**, because materialization reads the log files through
    /// `read_json()`. Autoload and autoinstall are turned off immediately: the bundled
    /// build compiles with `DUCKDB_EXTENSION_AUTOLOAD_DEFAULT=1`, so `AGENTS.md`'s
    /// "extensions statically linked, autoload disabled" is a thing this has to do rather
    /// than a thing it gets.
    pub fn open(paths: &Paths, slug: &str, ddl: &str) -> Result<Self, StoreError> {
        let schema = if ddl.trim().is_empty() {
            Schema::empty()
        } else {
            Schema::parse(ddl)?
        };
        Self::open_with(paths, slug, schema)
    }

    /// [`open`](Self::open) with a `schema.sql` that has already been parsed.
    ///
    /// The app loader parses the schema once, early, so a broken `schema.sql` is refused
    /// before a log is opened; parsing it again here would spin up a second in-memory
    /// instance for nothing. The same call is what reopens a sealed store for its
    /// privileged window (`spec/app-contract.md §7`).
    pub fn open_with(paths: &Paths, slug: &str, schema: Schema) -> Result<Self, StoreError> {
        let path = paths.app_cache_db(slug);
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent).map_err(|source| StoreError::Schema {
                problem: format!("{}: {source}", parent.display()),
            })?;
        }

        // Before `Connection::open`, which creates the file if it is absent: afterwards
        // there is no way to tell a cache the owner deleted from one that was always here.
        let fresh = !path.is_file();
        let conn = Connection::open(&path).map_err(StoreError::Duck)?;
        conn.execute_batch(
            "SET autoinstall_known_extensions = false;
             SET autoload_known_extensions = false;",
        )
        .map_err(StoreError::Duck)?;

        let target_schema = (slug == crate::sys::SLUG).then_some(SYS_SCHEMA);
        if let Some(name) = target_schema {
            conn.execute_batch(&format!("CREATE SCHEMA IF NOT EXISTS {name};"))
                .map_err(StoreError::Duck)?;
        }
        conn.execute_batch(&format!(
            "CREATE SCHEMA IF NOT EXISTS {};",
            materialize::PV_SCHEMA
        ))
        .map_err(StoreError::Duck)?;

        Ok(Self {
            slug: slug.to_owned(),
            path,
            log_dir: paths.app_log_dir(slug),
            snap_dir: paths.app_snap_dir(slug),
            conn,
            schema,
            target_schema,
            sealed: false,
            watermark: Materialized::default(),
            fresh,
            restored: None,
        })
    }

    /// Rebuild every table from the log — `spec/protocol.md §4.5`, in full. Tier 3.
    ///
    /// This is the definition. The incremental path in [`apply`](Self::apply) and the
    /// snapshot tiers in [`restore`](Self::restore) are optimizations that must agree with
    /// it (`docs/plans/phase-1.md §2.5`), and when in doubt this is the one that is right.
    pub fn materialize(&mut self, cutoff: &str) -> Result<(), StoreError> {
        if self.sealed {
            return Err(StoreError::Sealed {
                slug: self.slug.clone(),
            });
        }

        let source = self.log_source()?;
        for table in &self.schema.tables {
            let sql = materialize::replay_sql(
                &self.qualified(&table.name),
                &self.slug,
                table,
                &source,
                cutoff,
            );
            self.conn.execute_batch(&sql).map_err(StoreError::Duck)?;
        }

        let sql = materialize::tombstone_sql(&self.slug, &source, cutoff);
        self.conn.execute_batch(&sql).map_err(StoreError::Duck)?;

        self.create_views()?;
        self.checkpoint()?;
        self.note_rebuilt(Tier::Replay, None);
        Ok(())
    }

    /// Rematerialize if anything the tables were built from has changed.
    ///
    /// `apps/hello/README.md` blesses appending an event with `echo` and then **reloading
    /// the page** — not restarting the node. So the read path has to notice a log file
    /// that grew behind it. Comparing a stat of each segment against the watermark is
    /// cheap enough to do per request and is the whole mechanism.
    ///
    /// A rebuild goes through [`restore`](Self::restore), so a snapshot is used when one
    /// applies; [`restored`](Self::restored) says which tier did the work.
    ///
    /// Returns whether it rebuilt.
    pub fn refresh(&mut self, cutoff: &str) -> Result<bool, StoreError> {
        if !self.is_stale()? {
            return Ok(false);
        }
        self.restore(cutoff)?;
        Ok(true)
    }

    /// Whether [`refresh`](Self::refresh) would rebuild: the log or `schema.sql` has moved
    /// since the tables were built, or nothing has been built yet.
    ///
    /// A stat, no DuckDB — so it is answerable on a sealed store, which is what lets the
    /// loader decide whether an app needs its privileged window reopened at all.
    pub fn is_stale(&self) -> Result<bool, StoreError> {
        let current = self.take_inputs()?;
        Ok(!current.same_inputs(&self.watermark) || self.watermark.schema_hash.is_empty())
    }

    /// Apply one event this node just appended, without replaying the log.
    ///
    /// See [`materialize::apply_sql`] for why overwriting blindly is correct and for the
    /// exact moment in Phase 3 when it stops being.
    pub fn apply<D: Serialize>(
        &mut self,
        tbl: &str,
        id: &str,
        d: Option<&D>,
    ) -> Result<(), StoreError> {
        // The tombstone set first, and for **every** table, declared or not. It mirrors
        // `materialize::tombstone_sql`, whose scope is every `tbl` in the log because
        // `spec/data-api.md §2` accepts writes to a schema-less app and `spec/protocol.md
        // §4.6` still has to be enforceable there. Doing it before the schema lookup is
        // what keeps an undeclared table's `del` reportable through `is_tombstoned`.
        //
        // Delete before insert, so a second `del` on the same id does not add a second
        // row. The replay produces one row per *currently* tombstoned id, and
        // `docs/plans/phase-1.md §2.5` requires this path to match it — an accumulating
        // set would still answer `is_tombstoned` correctly and would still be wrong,
        // which is exactly the sort of drift the property test exists to catch.
        self.conn
            .execute(
                &format!(
                    "DELETE FROM {} WHERE tbl = ? AND id = ?",
                    materialize::TOMBSTONE_TABLE
                ),
                duckdb::params![tbl, id],
            )
            .map_err(StoreError::Duck)?;
        if d.is_none() {
            self.conn
                .execute(
                    &format!(
                        "INSERT INTO {} (tbl, id) VALUES (?, ?)",
                        materialize::TOMBSTONE_TABLE
                    ),
                    duckdb::params![tbl, id],
                )
                .map_err(StoreError::Duck)?;
        }

        // The table itself, only if `schema.sql` declares one. An event for a table it
        // does not is ordinary: a schema-less app has none at all
        // (`spec/app-contract.md §5.3`), and a column added later does not retroactively
        // make old events invalid.
        if let Some(table) = self.schema.table(tbl).cloned() {
            let target = self.qualified(&table.name);
            let (delete, insert) = materialize::apply_sql(&target, &table);
            self.conn
                .execute(&delete, duckdb::params![id])
                .map_err(StoreError::Duck)?;
            if let Some(value) = d {
                let json = serde_json::to_string(value).map_err(|source| StoreError::Schema {
                    problem: source.to_string(),
                })?;
                self.conn
                    .execute(&insert, duckdb::params![id, json])
                    .map_err(StoreError::Duck)?;
            }
        }

        // The inputs move; how the tables were originally built does not.
        let inputs = self.take_inputs()?;
        self.watermark.schema_hash = inputs.schema_hash;
        self.watermark.segments = inputs.segments;
        Ok(())
    }

    /// Whether `(tbl, id)`'s winning event is a tombstone (`spec/protocol.md §4.6`).
    ///
    /// `§4.6` forbids reusing a **minted** id — a ULID that belonged to one row must not
    /// become the key of a different one. It does not forbid re-asserting a
    /// caller-supplied stable key: `sys_node` and `sys_device` are keyed by Node ID, and
    /// `apps/animals` deletes and recreates its `'cursor'` singleton every round, both
    /// blessed by `§4.1`. So this reports the fact and does not decide policy. M9's data
    /// API — the only caller that accepts an id from something untrusted, and which
    /// `spec/data-api.md §2` already restricts to ULIDs — is where the refusal lives.
    pub fn is_tombstoned(&self, tbl: &str, id: &str) -> Result<bool, StoreError> {
        let found: i64 = self
            .conn
            .query_row(
                &format!(
                    "SELECT count(*) FROM {} WHERE tbl = ? AND id = ?",
                    materialize::TOMBSTONE_TABLE
                ),
                duckdb::params![tbl, id],
                |row| row.get(0),
            )
            .map_err(StoreError::Duck)?;
        Ok(found > 0)
    }

    /// Apply `spec/app-contract.md §7` to this instance, permanently.
    ///
    /// `lock_configuration` is last, because it is what makes the other three
    /// unrepealable. Afterwards **no** connection on this instance can touch the
    /// filesystem — not the app's, and not this one — which is stronger than §7 asks and
    /// is the only shape DuckDB's `GLOBAL_ONLY` settings allow. Materializing again means
    /// dropping this store and opening a new one.
    pub fn seal(&mut self) -> Result<(), StoreError> {
        if self.sealed {
            return Ok(());
        }
        self.conn
            .execute_batch(
                "SET enable_external_access = false;
                 SET autoinstall_known_extensions = false;
                 SET autoload_known_extensions = false;
                 SET lock_configuration = true;",
            )
            .map_err(StoreError::Duck)?;
        self.sealed = true;
        Ok(())
    }

    /// A connection for app SQL (`spec/app-contract.md §7`).
    ///
    /// Only after [`seal`](Self::seal), because before it the connection would be
    /// privileged — the settings are instance-wide, so an unsealed store has no sandboxed
    /// handle to give out.
    pub fn app_conn(&self) -> Result<Connection, StoreError> {
        if !self.sealed {
            return Err(StoreError::Sealed {
                slug: self.slug.clone(),
            });
        }
        self.conn.try_clone().map_err(StoreError::Duck)
    }

    /// The privileged connection, for the framework's own reads.
    #[must_use]
    pub fn conn(&self) -> &Connection {
        &self.conn
    }

    /// What `schema.sql` declared.
    #[must_use]
    pub fn schema(&self) -> &Schema {
        &self.schema
    }

    /// The app.
    #[must_use]
    pub fn slug(&self) -> &str {
        &self.slug
    }

    /// Whether `§7`'s sandbox has been applied.
    #[must_use]
    pub fn is_sealed(&self) -> bool {
        self.sealed
    }

    /// `cache/<slug>.duckdb`.
    #[must_use]
    pub fn path(&self) -> &Path {
        &self.path
    }

    /// `data/<slug>/snap/`.
    #[must_use]
    pub fn snap_dir(&self) -> &Path {
        &self.snap_dir
    }

    /// What the last [`restore`](Self::restore) on this instance did, if one ran.
    #[must_use]
    pub fn restored(&self) -> Option<&Restored> {
        self.restored.as_ref()
    }

    /// Which tier built the tables now in the cache — from this instance's own restore, or
    /// from the record `local/state.jsonl` carried over a restart.
    #[must_use]
    pub fn restore_tier(&self) -> Option<Tier> {
        self.watermark.restore.as_ref().map(|r| r.tier)
    }

    /// The restore record the watermark carries.
    #[must_use]
    pub fn restore_record(&self) -> Option<&RestoreRecord> {
        self.watermark.restore.as_ref()
    }

    /// Bytes across every segment of the app's log, as of the last stat.
    pub fn log_bytes(&self) -> Result<u64, StoreError> {
        Ok(self.take_inputs()?.log_bytes())
    }

    /// Whether the cache file was absent at open and nothing has rebuilt it since.
    pub(crate) fn is_fresh(&self) -> bool {
        self.fresh
    }

    pub(crate) fn set_restored(&mut self, restored: Restored) {
        self.restored = Some(restored);
    }

    /// The tables were just rebuilt, by `tier`: take the watermark and stamp the record.
    pub(crate) fn note_rebuilt(&mut self, tier: Tier, snapshot: Option<String>) {
        let inputs = self.take_inputs().unwrap_or_default();
        self.watermark = Materialized {
            schema_hash: inputs.schema_hash,
            segments: inputs.segments,
            restore: Some(RestoreRecord {
                tier,
                snapshot,
                at: crate::log::now(),
            }),
        };
        self.fresh = false;
    }

    /// A table name qualified by the schema it materializes into.
    pub(crate) fn qualified(&self, table: &str) -> String {
        match self.target_schema {
            Some(name) => format!("{name}.{}", materialize::quote_ident(table)),
            None => materialize::quote_ident(table),
        }
    }

    /// `_sys`'s views live in `sys` too, and DuckDB's rendering names them bare.
    fn qualify_view(&self, sql: &str) -> String {
        match self.target_schema {
            Some(name) => sql.replacen(
                "CREATE OR REPLACE VIEW ",
                &format!("CREATE OR REPLACE VIEW {name}."),
                1,
            ),
            None => sql.to_owned(),
        }
    }

    /// Every view `schema.sql` declared, plus the framework's own for `_sys`.
    pub(crate) fn create_views(&self) -> Result<(), StoreError> {
        for view in &self.schema.views {
            let sql = view
                .sql
                .replacen("CREATE VIEW", "CREATE OR REPLACE VIEW", 1);
            self.conn
                .execute_batch(&self.qualify_view(&sql))
                .map_err(StoreError::Duck)?;
        }
        if self.target_schema.is_some() {
            self.ensure_health()?;
        }
        Ok(())
    }

    /// CHECKPOINT, and not only for tidiness: an uncheckpointed write leaves
    /// `cache/<slug>.duckdb.wal` beside the database, and `test_spec_3_layout_created`
    /// asserts the §3 tree exhaustively. A stray `.wal` is exactly the unannounced file in
    /// `cache/` that test exists to catch.
    pub(crate) fn checkpoint(&self) -> Result<(), StoreError> {
        self.conn
            .execute_batch("CHECKPOINT")
            .map_err(StoreError::Duck)
    }

    /// `sys.v_health` (`spec/data-dictionary.md §4`) and the table behind it.
    ///
    /// Created here rather than in `sys.sql`, because `Schema::parse` runs that file in an
    /// in-memory instance that has no `pv.health` for a view to bind against. The table
    /// survives rebuilds (`IF NOT EXISTS`); the view is recreated with the rest.
    ///
    /// Four columns the dictionary asks for, as far as Phase 1 can answer: the restore
    /// tier in use, the last snapshot's age, the log size, and `unsynced_peers`, which is
    /// NULL until Phase 3 has peers to count.
    fn ensure_health(&self) -> Result<(), StoreError> {
        self.conn
            .execute_batch(&format!(
                "CREATE TABLE IF NOT EXISTS {HEALTH_TABLE} (
                     app_id       VARCHAR,
                     restore_tier INTEGER,
                     snapshot_id  VARCHAR,
                     restored_at  TIMESTAMPTZ,
                     log_bytes    BIGINT
                 );
                 CREATE OR REPLACE VIEW {SYS_SCHEMA}.v_health AS
                 SELECT h.app_id,
                        h.restore_tier,
                        h.snapshot_id,
                        h.restored_at,
                        s.last_snapshot_at,
                        CAST(floor((epoch(CAST(now() AS TIMESTAMP)) - epoch(CAST(s.last_snapshot_at AS TIMESTAMP))) / 86400) AS BIGINT)
                            AS snapshot_age_days,
                        h.log_bytes,
                        CAST(NULL AS INTEGER) AS unsynced_peers
                 FROM {HEALTH_TABLE} h
                 LEFT JOIN (SELECT app_id, max(created_at) AS last_snapshot_at
                            FROM {SYS_SCHEMA}.sys_snapshot GROUP BY app_id) s
                   ON h.app_id = s.app_id;"
            ))
            .map_err(StoreError::Duck)
    }

    /// Record one app's restore facts in `sys.v_health`. Only the `_sys` store has the
    /// table; on any other this is a no-op.
    ///
    /// `DELETE` then `INSERT`, which is `docs/plans/phase-1.md §2.3`: this is a table in
    /// `cache/`, not a log, and `AGENTS.md` invariant 3 does not reach it.
    pub fn note_health(
        &self,
        app: &str,
        tier: Tier,
        snapshot: Option<&str>,
        restored_at: &str,
        log_bytes: u64,
    ) -> Result<(), StoreError> {
        if self.target_schema.is_none() {
            return Ok(());
        }
        self.ensure_health()?;
        self.conn
            .execute(
                &format!("DELETE FROM {HEALTH_TABLE} WHERE app_id = ?"),
                duckdb::params![app],
            )
            .map_err(StoreError::Duck)?;
        self.conn
            .execute(
                &format!("INSERT INTO {HEALTH_TABLE} VALUES (?, ?, ?, CAST(? AS TIMESTAMPTZ), ?)"),
                duckdb::params![
                    app,
                    i32::from(tier.as_u8()),
                    snapshot,
                    restored_at,
                    i64::try_from(log_bytes).unwrap_or(i64::MAX)
                ],
            )
            .map_err(StoreError::Duck)?;
        // A checkpoint, for the same reason `materialize` ends with one: this runs after
        // the rebuild's own, and an uncheckpointed insert leaves a `.wal` in `cache/`.
        self.checkpoint()
    }

    fn log_glob(&self) -> String {
        // DuckDB takes forward slashes on every platform, including Windows.
        format!("{}/*.jsonl", self.log_dir.display()).replace('\\', "/")
    }

    /// The log as a source of envelope rows — or the empty stand-in when there is no
    /// segment yet, since `read_json()` refuses a glob that matches no file.
    ///
    /// The stat is taken once, here, so every statement of one rebuild agrees about it.
    pub(crate) fn log_source(&self) -> Result<Source, StoreError> {
        let has_segments = !self.take_inputs()?.segments.is_empty();
        Ok(Source::log(&self.log_glob(), has_segments))
    }

    /// Stat every segment, so a later run can tell whether the log moved underneath it.
    fn take_inputs(&self) -> Result<Materialized, StoreError> {
        let mut segments = BTreeMap::new();
        match fs::read_dir(&self.log_dir) {
            Ok(entries) => {
                for entry in entries {
                    let entry = entry.map_err(|source| StoreError::Schema {
                        problem: format!("{}: {source}", self.log_dir.display()),
                    })?;
                    let name = entry.file_name().to_string_lossy().into_owned();
                    if !name.ends_with(".jsonl") {
                        continue;
                    }
                    let len = entry.metadata().map(|meta| meta.len()).unwrap_or_default();
                    segments.insert(name, len);
                }
            }
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
            Err(error) => {
                return Err(StoreError::Schema {
                    problem: format!("{}: {error}", self.log_dir.display()),
                });
            }
        }
        Ok(Materialized {
            schema_hash: self.schema.hash.clone(),
            segments,
            restore: None,
        })
    }

    /// Record what this store now knows into `local/state.jsonl`.
    pub fn save_to(&self, state: &mut crate::local::State) {
        state.set_materialized(&self.slug, self.watermark.clone());
    }

    /// Adopt a watermark read back from `local/state.jsonl` — unless the cache file it
    /// describes is gone.
    ///
    /// The watermark says "the tables were built from these segments at these lengths".
    /// That is only a fact about a database that still exists. If `cache/<slug>.duckdb`
    /// was absent when this store opened, `Connection::open` has just created an empty
    /// one, and the recorded segments match the untouched logs exactly — so
    /// [`refresh`](Self::refresh) would find nothing changed and leave the owner with a
    /// database that has schemas and no tables. Ignoring the record here, rather than
    /// teaching `refresh` a second reason to rebuild, keeps `refresh`'s contract simple:
    /// watermark equals disk means the tables are current, and a fresh file has no
    /// watermark. A no-op costs one restore, which is what deleting `cache/` is
    /// documented to cost (`docs/backup-and-restore.md`) — and which a snapshot makes
    /// cheap.
    pub fn restore_watermark(&mut self, recorded: Materialized) {
        if self.fresh {
            return;
        }
        self.watermark = recorded;
    }
}

/// `§4.4`'s horizon, as a `ts` the projection can compare against.
///
/// Passed into [`Store::materialize`] rather than read inside it, so that a full replay
/// and an incremental apply compared against each other cannot disagree merely because
/// time passed between them (`docs/plans/phase-1.md §2.5`).
#[must_use]
pub fn cutoff_from(now: jiff::Timestamp) -> String {
    let horizon = now + jiff::SignedDuration::from_hours(MAX_FUTURE_HOURS);
    crate::log::format_ts(horizon)
}

/// `§4.4`'s horizon relative to this node's clock, now.
#[must_use]
pub fn cutoff_now() -> String {
    cutoff_from(jiff::Timestamp::now())
}
