// Project:  Privatium™  |  File: crates/privatium-core/src/store/mod.rs
// Authors:  Gabriel Mongefranco (@gabrielmongefranco)
// Created:  2026-09-01  |  Modified: 2026-09-05
// Summary:  One app's cache/<slug>.sqlite: the framework's connection that materializes it
//           from the log or from a snapshot (spec/protocol.md §5.3), the read-only sandboxed
//           connection app SQL gets (spec/app-contract.md §7), the watermark that notices a
//           log someone appended to by hand, and the record of which restore tier built the
//           tables.

use std::collections::BTreeMap;
use std::fs;
use std::path::{Path, PathBuf};
use std::time::Duration;

use rusqlite::Connection;
use serde::Serialize;
use thiserror::Error;

use crate::config::Paths;

mod csv;
pub mod decimal;
mod events;
pub mod materialize;
pub mod normalize;
pub mod params;
pub mod restore;
pub mod sandbox;
pub mod schema;
pub mod snapshot;
pub mod validate;

pub use decimal::Decimal;
pub use params::Params;
pub use restore::{Restored, SkipReason, Skipped, Tier};
pub use schema::{Column, Kind, Schema, Table, View};
pub use snapshot::{
    LogRetention, Manifest, ManifestTable, Pruned, Retention, Snapshot, SnapshotError, SnapshotId,
    SnapshotJob, SnapshotPolicy, TableCheck, Verification,
};
pub use validate::{Violation, validate};

use events::Event;

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
/// table the framework writes into `cache/_sys.sqlite` rather than an event in the
/// replicated `_sys` log. Disposable, like everything in `cache/`.
const HEALTH_TABLE: &str = "pv_health";

/// How long the framework's connection waits for a reader before a write fails.
const BUSY: Duration = Duration::from_secs(5);

/// Anything that can go wrong maintaining a cache database.
#[derive(Debug, Error)]
pub enum StoreError {
    /// SQLite refused.
    #[error("sqlite: {0}")]
    Sql(#[source] rusqlite::Error),

    /// A `schema.sql` is not something that can be materialized, or a log directory could
    /// not be read.
    #[error("schema.sql: {problem}")]
    Schema {
        /// What is wrong with it.
        problem: String,
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

    /// Bytes across every segment — `v_health`'s `log_bytes`.
    #[must_use]
    pub fn log_bytes(&self) -> u64 {
        self.segments.values().sum()
    }
}

/// One app's materialized cache.
///
/// **Two connections, two jobs, one file.** The framework's connection ([`conn`](Self::conn))
/// reads the log and writes the tables. App SQL runs on a separate connection
/// ([`app_conn`](Self::app_conn)) opened read-only under the authorizer of
/// `spec/app-contract.md §7`; SQLite's locking lets it read while the framework writes,
/// so there is no privileged window to open and close — a rebuild, a restore and a
/// snapshot are ordinary calls on the store at any moment.
#[derive(Debug)]
pub struct Store {
    slug: String,
    path: PathBuf,
    log_dir: PathBuf,
    snap_dir: PathBuf,
    conn: Connection,
    schema: Schema,
    /// Whether this is `_sys`, which carries the framework's health table and view.
    is_sys: bool,
    /// `cache/_sys.sqlite`, attached as `sys` on every app connection
    /// (`spec/data-dictionary.md §4`). `None` for `_sys` itself.
    sys_path: Option<PathBuf>,
    watermark: Materialized,
    /// Whether `cache/<slug>.sqlite` did not exist when this store opened it, and no
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
    /// Open — or create — `cache/<slug>.sqlite` with the framework's connection.
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
    /// database for nothing.
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
        let conn = Connection::open(&path).map_err(StoreError::Sql)?;
        conn.busy_timeout(BUSY).map_err(StoreError::Sql)?;
        decimal::register(&conn).map_err(StoreError::Sql)?;
        // A view may read `$name` (`params`); on the framework's own connection every
        // placeholder is NULL, which is all a rebuild or a snapshot ever needs of one.
        params::register(&conn).map_err(StoreError::Sql)?;

        let is_sys = slug == crate::sys::SLUG;
        Ok(Self {
            slug: slug.to_owned(),
            path,
            log_dir: paths.app_log_dir(slug),
            snap_dir: paths.app_snap_dir(slug),
            conn,
            schema,
            is_sys,
            sys_path: (!is_sys).then(|| paths.app_cache_db(crate::sys::SLUG)),
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
        let events = self.read_log(cutoff)?;
        self.replay_events(&events)
    }

    /// The replay over an already-staged log, in one transaction.
    pub(crate) fn replay_events(&mut self, events: &[Event]) -> Result<(), StoreError> {
        self.conn
            .execute_batch("BEGIN IMMEDIATE")
            .map_err(StoreError::Sql)?;
        let built = materialize::replay(&self.conn, &self.schema.tables, events)
            .and_then(|()| materialize::rebuild_tombstones(&self.conn, events))
            .and_then(|()| self.create_views());
        match built {
            Ok(()) => self.conn.execute_batch("COMMIT").map_err(StoreError::Sql)?,
            Err(error) => {
                let _ = self.conn.execute_batch("ROLLBACK");
                return Err(error);
            }
        }
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
    /// since the tables were built, or nothing has been built yet. A stat, no SQL.
    pub fn is_stale(&self) -> Result<bool, StoreError> {
        let current = self.take_inputs()?;
        Ok(!current.same_inputs(&self.watermark) || self.watermark.schema_hash.is_empty())
    }

    /// Apply one event this node just appended, without replaying the log.
    ///
    /// See [`materialize::apply`] for why overwriting blindly is correct and for the exact
    /// moment in Phase 3 when it stops being.
    pub fn apply<D: Serialize>(
        &mut self,
        tbl: &str,
        id: &str,
        d: Option<&D>,
    ) -> Result<(), StoreError> {
        let text = match d {
            Some(value) => {
                Some(
                    serde_json::to_string(value).map_err(|source| StoreError::Schema {
                        problem: source.to_string(),
                    })?,
                )
            }
            None => None,
        };
        materialize::apply(&self.conn, self.schema.table(tbl), tbl, id, text.as_deref())?;

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
                rusqlite::params![tbl, id],
                |row| row.get(0),
            )
            .map_err(StoreError::Sql)?;
        Ok(found > 0)
    }

    /// A connection for app SQL (`spec/app-contract.md §7`): read-only at the file,
    /// `query_only` at the connection, and an authorizer that refuses every write, every
    /// `PRAGMA`, `ATTACH` and extension loading — with `cache/_sys.sqlite` attached
    /// read-only as `sys` first (`spec/data-dictionary.md §4`). Fresh each time; drop it
    /// when the request is done.
    pub fn app_conn(&self) -> Result<Connection, StoreError> {
        self.app_conn_bound().map(|(conn, _)| conn)
    }

    /// [`app_conn`](Self::app_conn) together with the table its `$name` placeholders
    /// read from (`spec/data-api.md §1`) — what `/api/q/<view>` binds before it queries.
    pub fn app_conn_bound(&self) -> Result<(Connection, Params), StoreError> {
        sandbox::open_readonly(&self.path, self.sys_path.as_deref()).map_err(StoreError::Sql)
    }

    /// The framework's connection, for its own reads and writes.
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

    /// `cache/<slug>.sqlite`.
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

    /// Every sane event of this app, read from the log once (`events::read_log`).
    pub(crate) fn read_log(&self, cutoff: &str) -> Result<Vec<Event>, StoreError> {
        events::read_log(&self.log_dir, &self.slug, cutoff)
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

    /// Every view `schema.sql` declared, recreated from the author's own statement, plus
    /// the framework's health view for `_sys`.
    pub(crate) fn create_views(&self) -> Result<(), StoreError> {
        for view in &self.schema.views {
            self.conn
                .execute_batch(&format!(
                    "DROP VIEW IF EXISTS {};\n{};",
                    materialize::quote_ident(&view.name),
                    view.sql.trim_end_matches(';')
                ))
                .map_err(StoreError::Sql)?;
        }
        if self.is_sys {
            self.ensure_health()?;
        }
        Ok(())
    }

    /// `v_health` (`spec/data-dictionary.md §4`) and the table behind it.
    ///
    /// Created here rather than in `sys.sql`, because `Schema::parse` runs that file in an
    /// in-memory database that has no `pv_health` for a view to bind against. The table
    /// survives rebuilds (`IF NOT EXISTS`); the view is recreated with the rest.
    ///
    /// Four columns the dictionary asks for, as far as Phase 1 can answer: the restore
    /// tier in use, the last snapshot's age, the log size, and `unsynced_peers`, which is
    /// NULL until Phase 3 has peers to count.
    fn ensure_health(&self) -> Result<(), StoreError> {
        self.conn
            .execute_batch(&format!(
                "CREATE TABLE IF NOT EXISTS {HEALTH_TABLE} (
                     app_id       TEXT PRIMARY KEY,
                     restore_tier INTEGER,
                     snapshot_id  TEXT,
                     restored_at  TEXT,
                     log_bytes    INTEGER
                 );
                 DROP VIEW IF EXISTS v_health;
                 CREATE VIEW v_health AS
                 SELECT h.app_id,
                        h.restore_tier,
                        h.snapshot_id,
                        h.restored_at,
                        s.last_snapshot_at,
                        CAST(julianday('now') - julianday(s.last_snapshot_at) AS INTEGER)
                            AS snapshot_age_days,
                        h.log_bytes,
                        NULL AS unsynced_peers
                 FROM {HEALTH_TABLE} h
                 LEFT JOIN (SELECT app_id, max(created_at) AS last_snapshot_at
                            FROM sys_snapshot GROUP BY app_id) s
                   ON h.app_id = s.app_id;"
            ))
            .map_err(StoreError::Sql)
    }

    /// Record one app's restore facts in `v_health`. Only the `_sys` store has the table;
    /// on any other this is a no-op.
    ///
    /// A replace, which is `docs/plans/phase-1.md §2.3`: this is a table in `cache/`, not a
    /// log, and `AGENTS.md` invariant 3 does not reach it.
    pub fn note_health(
        &self,
        app: &str,
        tier: Tier,
        snapshot: Option<&str>,
        restored_at: &str,
        log_bytes: u64,
    ) -> Result<(), StoreError> {
        if !self.is_sys {
            return Ok(());
        }
        self.ensure_health()?;
        self.conn
            .execute(
                &format!("INSERT OR REPLACE INTO {HEALTH_TABLE} VALUES (?, ?, ?, ?, ?)"),
                rusqlite::params![
                    app,
                    i32::from(tier.as_u8()),
                    snapshot,
                    restored_at,
                    i64::try_from(log_bytes).unwrap_or(i64::MAX)
                ],
            )
            .map_err(StoreError::Sql)?;
        Ok(())
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
    /// That is only a fact about a database that still exists. If `cache/<slug>.sqlite`
    /// was absent when this store opened, `Connection::open` has just created an empty
    /// one, and the recorded segments match the untouched logs exactly — so
    /// [`refresh`](Self::refresh) would find nothing changed and leave the owner with a
    /// database that has no tables. Ignoring the record here, rather than teaching
    /// `refresh` a second reason to rebuild, keeps `refresh`'s contract simple: watermark
    /// equals disk means the tables are current, and a fresh file has no watermark. A
    /// no-op costs one restore, which is what deleting `cache/` is documented to cost
    /// (`docs/backup-and-restore.md`) — and which a snapshot makes cheap.
    pub fn restore_watermark(&mut self, recorded: Materialized) {
        if self.fresh {
            return;
        }
        self.watermark = recorded;
    }
}

/// `§4.4`'s horizon, as a `ts` the reader can compare against.
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
