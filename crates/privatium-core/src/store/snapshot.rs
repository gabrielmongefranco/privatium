// Project:  Privatium™  |  File: crates/privatium-core/src/store/snapshot.rs
// Authors:  Gabriel Mongefranco (@gabrielmongefranco)
// Created:  2026-09-02  |  Modified: 2026-09-05
// Summary:  spec/protocol.md §5.1, §5.2 and §5.4 — the snapshot id, MANIFEST.json, the
//           writer that produces one snapshot directory from the log (a SQLite file and a
//           CSV per table), checksum verification (spec/cli.md §7), retention, and the
//           weekly policy of spec/data-dictionary.md §3.6.

use std::collections::BTreeMap;
use std::fmt;
use std::fs;
use std::path::{Path, PathBuf};
use std::str::FromStr;

use jiff::Timestamp;
use jiff::civil::{Date, ISOWeekDate, Weekday};
use jiff::tz::TimeZone;
use rusqlite::Connection;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use thiserror::Error;

use crate::identity::NodeId;
use crate::store::events::{self, Op, winners};
use crate::store::schema::{ID_COLUMN, Kind};
use crate::store::{Schema, Store, StoreError, csv, materialize};

/// `MANIFEST.json` (`§5.2`).
pub const MANIFEST_FILE: &str = "MANIFEST.json";

/// `schema.sql` (`§5.1`) — `CREATE TABLE` statements with the storage types.
pub const SCHEMA_FILE: &str = "schema.sql";

/// The one manifest version `pv/1` writes and reads.
pub const MANIFEST_VERSION: u32 = 1;

/// A snapshot directory being written. Renamed to its id only once every file and the
/// manifest are on disk, so a crash mid-write leaves a `.part` that nothing reads rather
/// than a snapshot whose checksums fail.
const PART_SUFFIX: &str = ".part";

/// Anything that can go wrong with a snapshot directory short of the engine refusing.
#[derive(Debug, Error)]
pub enum SnapshotError {
    /// A file or directory could not be read, written, or removed.
    #[error("{path}: {source}")]
    Io {
        /// The path involved.
        path: PathBuf,
        /// What the OS said.
        #[source]
        source: std::io::Error,
    },

    /// `MANIFEST.json` is missing, unparseable, or describes a different snapshot.
    #[error("{path}: {problem}")]
    Manifest {
        /// The manifest.
        path: PathBuf,
        /// What is wrong with it.
        problem: String,
    },

    /// A string that is not `<ISO-year>-W<week>-<dev>-<hi_lam>` (`§5.1`).
    #[error("{0:?}: not a snapshot id (spec/protocol.md §5.1)")]
    BadId(String),

    /// `§5.4`: snapshot retention MUST NOT exceed log retention.
    #[error(
        "snapshot retention of {snapshot_days} days exceeds log retention of {log_days} days \
         (spec/protocol.md §5.4)"
    )]
    RetentionExceedsLog {
        /// The snapshot retention asked for.
        snapshot_days: u32,
        /// The log retention it exceeds.
        log_days: u32,
    },

    /// An ISO week that does not name a date.
    #[error("{0}: not an ISO week")]
    Week(String),
}

impl SnapshotError {
    fn io(path: &Path) -> impl FnOnce(std::io::Error) -> Self {
        let path = path.to_path_buf();
        move |source| Self::Io { path, source }
    }
}

/// `<ISO-year>-W<week>-<dev>-<hi_lam>` (`spec/protocol.md §5.1`).
///
/// The directory name is the whole identity: `§5.1` says the read predicate is derivable
/// from a directory listing alone, and `§5.4`'s pruner works from the same listing. Field
/// order here is the sort order — by week, then by how much log the snapshot saw, then by
/// writer — which is what "newest" means below.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct SnapshotId {
    year: i16,
    week: u8,
    hi_lam: u64,
    dev: String,
}

impl SnapshotId {
    /// The id of a snapshot taken at `now` by `dev`, having materialized up to `hi_lam`.
    #[must_use]
    pub fn new(now: Timestamp, dev: &NodeId, hi_lam: u64) -> Self {
        let week = now.to_zoned(TimeZone::UTC).date().iso_week_date();
        Self {
            year: week.year(),
            // `ISOWeekDate::week` is 1..=53; the cast cannot truncate.
            week: week.week().unsigned_abs(),
            hi_lam,
            dev: dev.to_string(),
        }
    }

    /// The ISO week-numbering year.
    #[must_use]
    pub fn year(&self) -> i16 {
        self.year
    }

    /// The ISO week, 1 to 53.
    #[must_use]
    pub fn week(&self) -> u8 {
        self.week
    }

    /// The node that wrote it.
    #[must_use]
    pub fn dev(&self) -> &str {
        &self.dev
    }

    /// The highest `lam` it materialized (`§5.1`).
    #[must_use]
    pub fn hi_lam(&self) -> u64 {
        self.hi_lam
    }

    /// The Monday of the snapshot's ISO week — the date `§5.4`'s retention counts from.
    ///
    /// From the name, not from the manifest's `created`: retention has to work over a
    /// directory whose manifest is unreadable, and a week's precision is what a
    /// week-numbered id promises.
    pub fn week_monday(&self) -> Result<Date, SnapshotError> {
        let week = i8::try_from(self.week).map_err(|_| SnapshotError::Week(self.to_string()))?;
        ISOWeekDate::new(self.year, week, Weekday::Monday)
            .map(ISOWeekDate::date)
            .map_err(|_| SnapshotError::Week(self.to_string()))
    }

    /// Whole days from the snapshot's week to `now`.
    pub fn age_days(&self, now: Timestamp) -> Result<i64, SnapshotError> {
        let today = now.to_zoned(TimeZone::UTC).date();
        let monday = self.week_monday()?;
        let span = today
            .since(monday)
            .map_err(|_| SnapshotError::Week(self.to_string()))?;
        Ok(i64::from(span.get_days()))
    }
}

impl fmt::Display for SnapshotId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        // Two-digit week, zero-padded, as ISO 8601 spells one (`§5.1`).
        write!(
            f,
            "{:04}-W{:02}-{}-{}",
            self.year, self.week, self.dev, self.hi_lam
        )
    }
}

impl FromStr for SnapshotId {
    type Err = SnapshotError;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        let bad = || SnapshotError::BadId(s.to_owned());
        let (year, rest) = s.split_once('-').ok_or_else(bad)?;
        let (week, rest) = rest.split_once('-').ok_or_else(bad)?;
        let (dev, hi_lam) = rest.rsplit_once('-').ok_or_else(bad)?;
        let week = week.strip_prefix('W').ok_or_else(bad)?;

        if year.len() != 4 || week.len() != 2 || dev.is_empty() || dev.contains('-') {
            return Err(bad());
        }
        let year: i16 = year.parse().map_err(|_| bad())?;
        let week: u8 = week.parse().map_err(|_| bad())?;
        if !(1..=53).contains(&week) {
            return Err(bad());
        }
        let hi_lam: u64 = hi_lam.parse().map_err(|_| bad())?;

        Ok(Self {
            year,
            week,
            hi_lam,
            dev: dev.to_owned(),
        })
    }
}

/// `MANIFEST.json`, exactly the shape of `spec/protocol.md §5.2` and nothing more.
///
/// Field order is the example's, because serde emits struct fields in declaration order
/// and a human diffing two manifests deserves the same shape every time. Reading does not
/// deny unknown fields — a `pv/2` manifest with one more key is still readable — but
/// nothing here writes one.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Manifest {
    /// Manifest version. Always [`MANIFEST_VERSION`].
    pub v: u32,
    /// The directory name.
    pub snapshot_id: String,
    /// The app slug.
    pub app: String,
    /// When it was written, as a `ts` (`§4.1`).
    pub created: String,
    /// The highest `lam` materialized.
    pub hi_lam: u64,
    /// The highest `seq` materialized per device.
    pub hi_seq: BTreeMap<String, u64>,
    /// `sqlite <version>` — the engine that wrote the SQLite files, as it reports itself.
    pub engine: String,
    /// One entry per declared table.
    pub tables: Vec<ManifestTable>,
}

/// One table's entry in the manifest.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ManifestTable {
    /// The table name, also the stem of both files.
    pub name: String,
    /// Row count.
    pub rows: u64,
    /// SHA-256 of `<name>.sqlite`, lowercase hex.
    pub sqlite_sha256: String,
    /// SHA-256 of `<name>.csv`, lowercase hex.
    pub csv_sha256: String,
}

impl Manifest {
    /// `row_counts` for `sys_snapshot` (`spec/data-dictionary.md §3.9`): a JSON object,
    /// table → count, as text — the column is `VARCHAR` holding JSON, like `sys_audit`'s
    /// `detail`.
    #[must_use]
    pub fn row_counts_json(&self) -> String {
        let counts: BTreeMap<&str, u64> = self
            .tables
            .iter()
            .map(|t| (t.name.as_str(), t.rows))
            .collect();
        serde_json::to_string(&counts).unwrap_or_else(|_| "{}".to_owned())
    }
}

/// A snapshot that was just written, or read back.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Snapshot {
    /// Its id.
    pub id: SnapshotId,
    /// `data/<slug>/snap/<id>/`.
    pub dir: PathBuf,
    /// Its manifest.
    pub manifest: Manifest,
    /// Total size of every file in the directory — `sys_snapshot.bytes`.
    pub bytes: u64,
}

/// The snapshot ids in `data/<slug>/snap/`, oldest first.
///
/// Anything that is not a directory named like `§5.1` is ignored: a `.part` still being
/// written, a stray file, an editor's backup. Being incurious here is deliberate, for the
/// same reason the log reader is — this decides what counts as a snapshot, and a permissive
/// answer would let a half-written directory become one.
pub fn list(snap_dir: &Path) -> Result<Vec<SnapshotId>, SnapshotError> {
    let entries = match fs::read_dir(snap_dir) {
        Ok(entries) => entries,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(Vec::new()),
        Err(error) => return Err(SnapshotError::io(snap_dir)(error)),
    };

    let mut ids = Vec::new();
    for entry in entries {
        let entry = entry.map_err(SnapshotError::io(snap_dir))?;
        if !entry.path().is_dir() {
            continue;
        }
        if let Ok(id) = entry.file_name().to_string_lossy().parse::<SnapshotId>() {
            ids.push(id);
        }
    }
    ids.sort();
    Ok(ids)
}

/// The newest snapshot, by `(year, week, hi_lam, dev)`.
pub fn newest(snap_dir: &Path) -> Result<Option<SnapshotId>, SnapshotError> {
    Ok(list(snap_dir)?.pop())
}

/// Read and parse `<dir>/MANIFEST.json`.
pub fn read_manifest(dir: &Path) -> Result<Manifest, SnapshotError> {
    let path = dir.join(MANIFEST_FILE);
    let text = fs::read_to_string(&path).map_err(SnapshotError::io(&path))?;
    serde_json::from_str(&text).map_err(|error| SnapshotError::Manifest {
        path,
        problem: error.to_string(),
    })
}

impl Snapshot {
    /// Read a snapshot directory back: its manifest, its id, and its size on disk.
    pub fn read(dir: &Path) -> Result<Self, SnapshotError> {
        let manifest = read_manifest(dir)?;
        let id: SnapshotId = manifest.snapshot_id.parse()?;
        Ok(Self {
            id,
            dir: dir.to_path_buf(),
            bytes: dir_bytes(dir)?,
            manifest,
        })
    }
}

/// `schema.sql` as a snapshot carries it (`§5.1`): one `CREATE TABLE` per declared table,
/// `id` first, the storage type of every column with its declared type beside it, and no
/// constraint but the key.
///
/// Deterministic — tables in name order, no date — because a restore compares this text
/// against the file to decide whether the snapshot still describes the app's schema
/// (`spec/app-contract.md §4.5`: a changed `schema.sql` rematerializes from the logs).
#[must_use]
pub fn render_ddl(schema: &Schema) -> String {
    let mut out = String::from(
        "-- Privatium snapshot schema (spec/protocol.md §5.1). The storage type of each column\n\
         -- with the declared type beside it; no constraint but the key, so a table loaded from\n\
         -- this equals a replay. DECIMAL columns are text under the `decimal` collation.\n",
    );
    for table in &schema.tables {
        out.push('\n');
        out.push_str(&materialize::create_table_sql(table));
        out.push('\n');
    }
    out
}

/// SHA-256 of a file, lowercase hex.
pub(crate) fn sha256_file(path: &Path) -> Result<String, SnapshotError> {
    let bytes = fs::read(path).map_err(SnapshotError::io(path))?;
    Ok(hex(&Sha256::digest(&bytes)))
}

fn hex(bytes: &[u8]) -> String {
    use std::fmt::Write as _;
    let mut out = String::with_capacity(bytes.len() * 2);
    for byte in bytes {
        // Writing to a String cannot fail; the result is discarded rather than unwrapped.
        let _ = write!(out, "{byte:02x}");
    }
    out
}

/// The name of one table's SQLite file.
pub(crate) fn sqlite_file(table: &str) -> String {
    format!("{table}.sqlite")
}

/// The name of one table's CSV file.
pub(crate) fn csv_file(table: &str) -> String {
    format!("{table}.csv")
}

/// The result of recomputing a snapshot's checksums (`spec/cli.md §7`, `--verify`).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Verification {
    /// The snapshot checked.
    pub id: SnapshotId,
    /// One entry per table in the manifest.
    pub tables: Vec<TableCheck>,
}

/// One table's two files against their recorded checksums.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TableCheck {
    /// The table.
    pub name: String,
    /// Whether `<name>.sqlite` hashes to `sqlite_sha256`. A missing file is a mismatch.
    pub sqlite_ok: bool,
    /// Whether `<name>.csv` hashes to `csv_sha256`.
    pub csv_ok: bool,
}

impl Verification {
    /// Whether every file matched.
    #[must_use]
    pub fn ok(&self) -> bool {
        self.tables.iter().all(|t| t.sqlite_ok && t.csv_ok)
    }
}

/// Recompute every checksum in `<dir>/MANIFEST.json` against the files beside it.
pub fn verify(dir: &Path) -> Result<Verification, SnapshotError> {
    let manifest = read_manifest(dir)?;
    let id: SnapshotId = manifest.snapshot_id.parse()?;
    let mut tables = Vec::with_capacity(manifest.tables.len());
    for table in &manifest.tables {
        let hashes_to = |file: &str, expected: &str| {
            sha256_file(&dir.join(file)).is_ok_and(|actual| actual == expected)
        };
        tables.push(TableCheck {
            name: table.name.clone(),
            sqlite_ok: hashes_to(&sqlite_file(&table.name), &table.sqlite_sha256),
            csv_ok: hashes_to(&csv_file(&table.name), &table.csv_sha256),
        });
    }
    Ok(Verification { id, tables })
}

/// How long log files are kept.
///
/// `pv/1` never deletes a log (`spec/protocol.md §4.6`, `§14`), so [`Forever`](Self::Forever)
/// is the only value anything in this crate produces. [`Days`](Self::Days) exists so that
/// `§5.4`'s assertion has something to compare against the day a compaction feature
/// appears — which is the point of the assertion.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LogRetention {
    /// Logs are never deleted. Always, in `pv/1`.
    Forever,
    /// Logs older than this may be compacted. Not a `pv/1` value.
    Days(u32),
}

/// `§5.4`'s two numbers.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Retention {
    /// Snapshots older than this are pruned. Default 365.
    pub snapshot_days: u32,
    /// What the logs promise, which the snapshots must not outlast.
    pub log: LogRetention,
}

impl Default for Retention {
    fn default() -> Self {
        Self {
            snapshot_days: SnapshotPolicy::default().retention_days,
            log: LogRetention::Forever,
        }
    }
}

/// What one pruning run did.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Pruned {
    /// Deleted, oldest first.
    pub removed: Vec<SnapshotId>,
    /// Still on disk, oldest first.
    pub kept: Vec<SnapshotId>,
}

/// Delete snapshots older than the retention (`spec/protocol.md §5.4`).
///
/// Two rules, both MUSTs. The oldest snapshot is never deleted, whatever its age —
/// `docs/backup-and-restore.md §5` promises the owner the same. And snapshot retention
/// must not exceed log retention, which cannot fail in `pv/1` and is checked anyway so
/// that a future log compaction cannot leave a gap between the last log line kept and the
/// oldest snapshot that could replay it.
///
/// Age is counted from the Monday of the snapshot's ISO week, off the name alone.
pub fn prune(
    snap_dir: &Path,
    now: Timestamp,
    retention: &Retention,
) -> Result<Pruned, SnapshotError> {
    if let LogRetention::Days(log_days) = retention.log
        && retention.snapshot_days > log_days
    {
        return Err(SnapshotError::RetentionExceedsLog {
            snapshot_days: retention.snapshot_days,
            log_days,
        });
    }

    let ids = list(snap_dir)?;
    let mut pruned = Pruned::default();
    for (index, id) in ids.into_iter().enumerate() {
        let expired = id.age_days(now)? > i64::from(retention.snapshot_days);
        if index == 0 || !expired {
            pruned.kept.push(id);
            continue;
        }
        let dir = snap_dir.join(id.to_string());
        fs::remove_dir_all(&dir).map_err(SnapshotError::io(&dir))?;
        pruned.removed.push(id);
    }
    Ok(pruned)
}

/// The three `snapshot.*` settings of `spec/data-dictionary.md §3.6`, with their defaults.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct SnapshotPolicy {
    /// `snapshot.interval_days`.
    pub interval_days: u32,
    /// `snapshot.min_events` — also snapshot after this many events, whichever first.
    pub min_events: u64,
    /// `snapshot.retention_days` (`§5.4`).
    pub retention_days: u32,
}

impl Default for SnapshotPolicy {
    fn default() -> Self {
        Self {
            interval_days: 7,
            min_events: 100,
            retention_days: 365,
        }
    }
}

impl SnapshotPolicy {
    /// The retention this policy asks for, against `pv/1`'s logs.
    #[must_use]
    pub fn retention(&self) -> Retention {
        Retention {
            snapshot_days: self.retention_days,
            log: LogRetention::Forever,
        }
    }
}

/// Whether a snapshot is due under `policy`.
///
/// `newest` is the most recent snapshot's manifest, if any; `heads` is the highest `seq`
/// per device the log now holds. Due when there is no snapshot and there are events; when
/// the newest is at least `interval_days` old; or when at least `min_events` events have
/// been written since it. With no events at all nothing is due — an app nobody has written
/// to has nothing to snapshot.
pub fn due(
    newest: Option<&Manifest>,
    heads: &BTreeMap<String, u64>,
    now: Timestamp,
    policy: &SnapshotPolicy,
) -> Result<bool, SnapshotError> {
    let Some(manifest) = newest else {
        return Ok(heads.values().any(|seq| *seq > 0));
    };
    let id: SnapshotId = manifest.snapshot_id.parse()?;
    if id.age_days(now)? >= i64::from(policy.interval_days) {
        return Ok(true);
    }
    let since: u64 = heads
        .iter()
        .map(|(dev, seq)| seq.saturating_sub(manifest.hi_seq.get(dev).copied().unwrap_or(0)))
        .sum();
    Ok(since >= policy.min_events)
}

/// A snapshot decided and read but not yet written.
///
/// Everything [`write`](Self::write) needs, taken from the store in one reading of the
/// log — so the files describe one moment — and nothing of the store itself, so the
/// writing, which is the slow part, can run with no lock held while requests go on
/// appending. The id is fixed at [`Store::snapshot_job`]: it names the log state that
/// was read, whatever lands afterwards.
#[derive(Debug)]
pub struct SnapshotJob {
    id: SnapshotId,
    slug: String,
    snap_dir: PathBuf,
    schema: Schema,
    engine: String,
    created: String,
    hi_lam: u64,
    hi_seq: BTreeMap<String, u64>,
    events: Vec<events::Event>,
}

impl SnapshotJob {
    /// The id the snapshot will have.
    #[must_use]
    pub fn id(&self) -> &SnapshotId {
        &self.id
    }

    /// The app.
    #[must_use]
    pub fn slug(&self) -> &str {
        &self.slug
    }

    /// Write the snapshot directory (`spec/protocol.md §5.1`, `§5.2`).
    ///
    /// Into `<id>.part` first — every file synced, the manifest last, the directory
    /// flushed — then one rename, so a crash leaves a directory [`list`] ignores rather
    /// than a snapshot whose manifest names files that never made it. An existing
    /// directory with the same id is replaced: the id names a log state, and a second
    /// snapshot of the same state — after a `schema.sql` change, say — is the one that
    /// should survive.
    pub fn write(self) -> Result<Snapshot, StoreError> {
        let winners = winners(&self.events);
        let part = self.snap_dir.join(format!("{}{PART_SUFFIX}", self.id));
        remove_if_present(&part)?;
        fs::create_dir_all(&part).map_err(SnapshotError::io(&part))?;

        let mut tables = Vec::with_capacity(self.schema.tables.len());
        for table in &self.schema.tables {
            let rows: Vec<Vec<rusqlite::types::Value>> = winners
                .iter()
                .filter(|((tbl, _), event)| *tbl == table.name && event.op == Op::Put)
                .map(|((_, id), event)| materialize::project(table, id, event.d.as_deref()))
                .collect();

            let sqlite = part.join(sqlite_file(&table.name));
            write_sqlite(&sqlite, table, &rows)?;
            let csv_path = part.join(csv_file(&table.name));
            let mut header = vec![ID_COLUMN];
            header.extend(table.columns.iter().map(|c| c.name.as_str()));
            csv::write(
                &csv_path,
                &header,
                rows.iter().map(|row| {
                    row.iter()
                        .zip(
                            std::iter::once(Kind::Text).chain(table.columns.iter().map(|c| c.kind)),
                        )
                        .map(|(value, kind)| csv_text(value, kind))
                        .collect()
                }),
            )
            .map_err(SnapshotError::io(&csv_path))?;
            sync_file(&csv_path)?;

            tables.push(ManifestTable {
                name: table.name.clone(),
                rows: u64::try_from(rows.len()).unwrap_or_default(),
                sqlite_sha256: sha256_file(&sqlite)?,
                csv_sha256: sha256_file(&csv_path)?,
            });
        }

        let schema_path = part.join(SCHEMA_FILE);
        crate::durable::write_synced(&schema_path, render_ddl(&self.schema).as_bytes())
            .map_err(SnapshotError::io(&schema_path))?;

        let manifest = Manifest {
            v: MANIFEST_VERSION,
            snapshot_id: self.id.to_string(),
            app: self.slug.clone(),
            created: self.created.clone(),
            hi_lam: self.hi_lam,
            hi_seq: self.hi_seq.clone(),
            engine: self.engine.clone(),
            tables,
        };
        // Last, so a directory that has a manifest has everything the manifest names.
        let manifest_path = part.join(MANIFEST_FILE);
        let mut text =
            serde_json::to_string_pretty(&manifest).map_err(|error| SnapshotError::Manifest {
                path: manifest_path.clone(),
                problem: error.to_string(),
            })?;
        text.push('\n');
        crate::durable::write_synced(&manifest_path, text.as_bytes())
            .map_err(SnapshotError::io(&manifest_path))?;

        let bytes = dir_bytes(&part)?;
        let dir = self.snap_dir.join(self.id.to_string());
        remove_if_present(&dir)?;
        fs::rename(&part, &dir).map_err(SnapshotError::io(&dir))?;
        crate::durable::sync_dir(&self.snap_dir).map_err(SnapshotError::io(&self.snap_dir))?;

        Ok(Snapshot {
            id: self.id,
            dir,
            manifest,
            bytes,
        })
    }
}

/// The writer (`spec/protocol.md §5.1`, `§5.2`).
impl Store {
    /// Read what a snapshot of this app at `now`, by `dev`, will hold — the log, once —
    /// and hand it back as a job that writes the files without the store.
    ///
    /// **From the log, not from the cache tables.** The log is read once and `hi_lam`,
    /// `hi_seq` and every table's `§4.5` winners come from that one reading, so the
    /// manifest describes exactly the rows in the files. Copying the cache tables and
    /// then reading the log for the marks would race a hand `echo` between the two. The
    /// reading is the fast part and belongs under whatever lock keeps the log still; the
    /// writing is [`SnapshotJob::write`] and needs no lock at all.
    pub fn snapshot_job(&self, dev: &NodeId, now: Timestamp) -> Result<SnapshotJob, StoreError> {
        let cutoff = crate::store::cutoff_from(now);
        let events = self.read_log(&cutoff)?;
        let hi_lam = events::hi_lam(&events);
        Ok(SnapshotJob {
            id: SnapshotId::new(now, dev, hi_lam),
            slug: self.slug().to_owned(),
            snap_dir: self.snap_dir().to_path_buf(),
            schema: self.schema().clone(),
            engine: engine_string(self.conn())?,
            created: crate::log::format_ts(now),
            hi_lam,
            hi_seq: events::hi_seq(&events),
            events,
        })
    }

    /// Write one snapshot of this app from its log, at `now`, by `dev`:
    /// [`snapshot_job`](Self::snapshot_job) and [`SnapshotJob::write`] in one call, for a
    /// caller that holds no lock anyone is waiting on.
    pub fn snapshot(&self, dev: &NodeId, now: Timestamp) -> Result<Snapshot, StoreError> {
        self.snapshot_job(dev, now)?.write()
    }
}

/// Flush one file the manifest is about to name.
fn sync_file(path: &Path) -> Result<(), SnapshotError> {
    crate::durable::sync_file(path).map_err(SnapshotError::io(path))
}

/// One table's SQLite file: the same `CREATE TABLE` the cache uses, its rows in `id`
/// order, in a database of its own that any SQLite tool opens.
fn write_sqlite(
    path: &Path,
    table: &crate::store::schema::Table,
    rows: &[Vec<rusqlite::types::Value>],
) -> Result<(), StoreError> {
    let conn = Connection::open(path).map_err(StoreError::Sql)?;
    crate::store::decimal::register(&conn).map_err(StoreError::Sql)?;
    conn.execute_batch(&materialize::create_table_sql(table))
        .map_err(StoreError::Sql)?;
    let tx = conn.unchecked_transaction().map_err(StoreError::Sql)?;
    {
        let mut insert = tx
            .prepare(&materialize::insert_sql(table))
            .map_err(StoreError::Sql)?;
        for row in rows {
            insert
                .execute(rusqlite::params_from_iter(row.iter()))
                .map_err(StoreError::Sql)?;
        }
    }
    tx.commit().map_err(StoreError::Sql)?;
    // Written once and never again: compact it, so the file is the rows and nothing else.
    conn.execute_batch("VACUUM").map_err(StoreError::Sql)?;
    Ok(())
}

/// A typed value as its CSV text: booleans as `true`/`false` for a person's benefit,
/// integers and decimals as their digits, NULL as none.
fn csv_text(value: &rusqlite::types::Value, kind: Kind) -> Option<String> {
    use rusqlite::types::Value;
    match (value, kind) {
        (Value::Null, _) => None,
        (Value::Integer(i), Kind::Boolean) => Some((*i != 0).to_string()),
        (Value::Integer(i), _) => Some(i.to_string()),
        (Value::Text(t), _) => Some(t.clone()),
        (Value::Real(r), _) => Some(r.to_string()),
        (Value::Blob(_), _) => None,
    }
}

/// `sqlite <version>` (`§5.2`), from the engine itself.
fn engine_string(conn: &Connection) -> Result<String, StoreError> {
    let version: String = conn
        .query_row("SELECT sqlite_version()", [], |row| row.get(0))
        .map_err(StoreError::Sql)?;
    Ok(format!("sqlite {version}"))
}

fn remove_if_present(dir: &Path) -> Result<(), SnapshotError> {
    match fs::remove_dir_all(dir) {
        Ok(()) => Ok(()),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Err(error) => Err(SnapshotError::io(dir)(error)),
    }
}

/// Total size of the files directly inside `dir`.
fn dir_bytes(dir: &Path) -> Result<u64, SnapshotError> {
    let mut total = 0;
    for entry in fs::read_dir(dir).map_err(SnapshotError::io(dir))? {
        let entry = entry.map_err(SnapshotError::io(dir))?;
        let meta = entry.metadata().map_err(SnapshotError::io(&entry.path()))?;
        if meta.is_file() {
            total += meta.len();
        }
    }
    Ok(total)
}

// AGENTS.md, Style: unwrap() is permitted in tests. The crate-level deny reaches unit
// tests inside src/, so each one opts out where it is declared.
#[allow(clippy::unwrap_used, clippy::expect_used)]
#[cfg(test)]
mod tests {
    use super::*;

    fn dev() -> NodeId {
        NodeId::derive(&ed25519_dalek::SigningKey::from_bytes(&[7u8; 32]).verifying_key())
    }

    fn at(s: &str) -> Timestamp {
        s.parse().unwrap()
    }

    /// `§5.1`'s example, and the two week boundaries a naive `year-week` gets wrong.
    #[test]
    fn the_id_is_year_week_dev_hi_lam() {
        let id = SnapshotId::new(at("2026-08-30T03:00:00Z"), &dev(), 8830);
        assert_eq!(id.to_string(), format!("2026-W35-{}-8830", dev()));
        assert_eq!(id.to_string().parse::<SnapshotId>().unwrap(), id);

        // 1 January 2027 is a Friday and belongs to ISO week 2026-W53.
        let last = SnapshotId::new(at("2027-01-01T12:00:00Z"), &dev(), 1);
        assert!(last.to_string().starts_with("2026-W53-"), "{last}");
        // 29 December 2025 is a Monday and opens ISO week 2026-W01.
        let first = SnapshotId::new(at("2025-12-29T12:00:00Z"), &dev(), 1);
        assert!(first.to_string().starts_with("2026-W01-"), "{first}");
    }

    #[test]
    fn anything_else_is_not_an_id() {
        for bad in [
            "2026-W35-k7m2q9xf",
            "2026-35-k7m2q9xf-8830",
            "2026-W5-k7m2q9xf-8830",
            "2026-W54-k7m2q9xf-8830",
            "2026-W35--8830",
            "2026-W35-k7m2q9xf-8830.part",
            "README.md",
        ] {
            assert!(bad.parse::<SnapshotId>().is_err(), "{bad}");
        }
    }

    /// Newest is by week, then by how much log was seen.
    #[test]
    fn ids_order_by_week_then_hi_lam() {
        let mut ids: Vec<SnapshotId> = [
            "2026-W35-k7m2q9xf-9000",
            "2026-W36-k7m2q9xf-100",
            "2026-W35-k7m2q9xf-8830",
        ]
        .iter()
        .map(|s| s.parse().unwrap())
        .collect();
        ids.sort();
        let order: Vec<String> = ids.iter().map(ToString::to_string).collect();
        assert_eq!(
            order,
            vec![
                "2026-W35-k7m2q9xf-8830",
                "2026-W35-k7m2q9xf-9000",
                "2026-W36-k7m2q9xf-100"
            ]
        );
    }

    #[test]
    fn age_counts_from_the_monday_of_the_week() {
        let id: SnapshotId = "2026-W35-k7m2q9xf-8830".parse().unwrap();
        assert_eq!(id.week_monday().unwrap().to_string(), "2026-08-24");
        assert_eq!(id.age_days(at("2026-08-30T00:00:00Z")).unwrap(), 6);
        assert_eq!(id.age_days(at("2027-08-24T00:00:00Z")).unwrap(), 365);
    }

    /// `§5.2`, key for key. Serialization must add nothing.
    #[test]
    fn the_manifest_has_exactly_the_spec_keys_in_order() {
        let manifest = Manifest {
            v: 1,
            snapshot_id: "2026-W35-k7m2q9xf-8830".into(),
            app: "hello".into(),
            created: "2026-08-30T03:00:00.000Z".into(),
            hi_lam: 8830,
            hi_seq: BTreeMap::from([("k7m2q9xf".to_owned(), 1041)]),
            engine: "sqlite 3.53.2".into(),
            tables: vec![ManifestTable {
                name: "profile".into(),
                rows: 1,
                sqlite_sha256: "a".into(),
                csv_sha256: "b".into(),
            }],
        };
        let json = serde_json::to_string(&manifest).unwrap();
        assert_eq!(
            json,
            r#"{"v":1,"snapshot_id":"2026-W35-k7m2q9xf-8830","app":"hello","created":"2026-08-30T03:00:00.000Z","hi_lam":8830,"hi_seq":{"k7m2q9xf":1041},"engine":"sqlite 3.53.2","tables":[{"name":"profile","rows":1,"sqlite_sha256":"a","csv_sha256":"b"}]}"#
        );
        assert_eq!(manifest.row_counts_json(), r#"{"profile":1}"#);
    }

    /// `§5.4`'s assertion, exercised with the value `pv/1` can never produce.
    #[test]
    fn retention_may_not_exceed_log_retention() {
        let dir = tempfile::tempdir().unwrap();
        let retention = Retention {
            snapshot_days: 365,
            log: LogRetention::Days(30),
        };
        let error = prune(dir.path(), at("2026-09-02T00:00:00Z"), &retention).unwrap_err();
        assert!(
            matches!(error, SnapshotError::RetentionExceedsLog { .. }),
            "{error}"
        );
        assert!(
            prune(
                dir.path(),
                at("2026-09-02T00:00:00Z"),
                &Retention::default()
            )
            .is_ok()
        );
    }

    /// `§3.6`: interval or event count, whichever first; nothing to snapshot means not due.
    #[test]
    fn due_by_interval_or_by_events_and_never_for_an_empty_log() {
        let policy = SnapshotPolicy::default();
        let now = at("2026-09-02T00:00:00Z");
        assert!(!due(None, &BTreeMap::new(), now, &policy).unwrap());
        assert!(due(None, &BTreeMap::from([("k".to_owned(), 1)]), now, &policy).unwrap());

        let manifest = Manifest {
            v: 1,
            snapshot_id: "2026-W36-k7m2q9xf-10".into(),
            app: "hello".into(),
            created: String::new(),
            hi_lam: 10,
            hi_seq: BTreeMap::from([("k7m2q9xf".to_owned(), 10)]),
            engine: String::new(),
            tables: Vec::new(),
        };
        let heads = BTreeMap::from([("k7m2q9xf".to_owned(), 50)]);
        assert!(!due(Some(&manifest), &heads, now, &policy).unwrap());
        let busy = BTreeMap::from([("k7m2q9xf".to_owned(), 110)]);
        assert!(due(Some(&manifest), &busy, now, &policy).unwrap());
        assert!(due(Some(&manifest), &heads, at("2026-09-08T00:00:00Z"), &policy).unwrap());
    }
}
