// Project:  Privatium™  |  File: crates/privatium-core/src/store/restore.rs
// Authors:  Gabriel Mongefranco (@gabrielmongefranco)
// Created:  2026-09-02  |  Modified: 2026-09-05
// Summary:  spec/protocol.md §5.3 — the three-tier read. The snapshot's SQLite files plus
//           the log tail, then CSV plus schema.sql plus the tail, then the full replay;
//           which tier succeeded, and why the ones before it did not.

use std::fmt;
use std::path::PathBuf;

use rusqlite::Connection;
use serde::{Deserialize, Serialize};

use crate::store::events::{self, Event};
use crate::store::snapshot::{self, Manifest, SnapshotId};
use crate::store::{Store, StoreError, csv, materialize};

/// One of `§5.3`'s three tiers.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(into = "u8", try_from = "u8")]
pub enum Tier {
    /// The snapshot's SQLite files + log tail.
    Sqlite = 1,
    /// CSV + `schema.sql` + log tail.
    Csv = 2,
    /// Full log replay from `lam` 0.
    Replay = 3,
}

impl Tier {
    /// `1`, `2`, or `3`.
    #[must_use]
    pub fn as_u8(self) -> u8 {
        self as u8
    }

    /// The source's name, lowercase.
    #[must_use]
    pub fn name(self) -> &'static str {
        match self {
            Self::Sqlite => "sqlite",
            Self::Csv => "csv",
            Self::Replay => "replay",
        }
    }
}

impl fmt::Display for Tier {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "tier {} ({})", self.as_u8(), self.name())
    }
}

impl From<Tier> for u8 {
    fn from(tier: Tier) -> Self {
        tier.as_u8()
    }
}

impl TryFrom<u8> for Tier {
    type Error = String;

    fn try_from(value: u8) -> Result<Self, Self::Error> {
        match value {
            1 => Ok(Self::Sqlite),
            2 => Ok(Self::Csv),
            3 => Ok(Self::Replay),
            other => Err(format!(
                "{other}: not a restore tier (spec/protocol.md §5.3)"
            )),
        }
    }
}

/// Why a tier was not used.
///
/// Two families. The first three and `TailNotCausal`/`LogBehindSnapshot` mean the snapshot
/// does not *apply* — the replay is the right answer and nothing is wrong with the files.
/// The rest mean the files are wrong: a snapshot that should have worked did not, which
/// `spec/cli.md §7` calls falling through unexpectedly.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(tag = "reason", rename_all = "snake_case")]
pub enum SkipReason {
    /// `data/<slug>/snap/` holds no snapshot.
    NoSnapshot,
    /// The snapshot's `schema.sql` is not the app's current schema
    /// (`spec/app-contract.md §4.5`).
    SchemaChanged,
    /// Events the snapshot did not see are not all causally after it (`§5.3`).
    TailNotCausal {
        /// How many such events.
        events: u64,
    },
    /// The log no longer holds everything `hi_seq` claims the snapshot saw (`§5`: a
    /// snapshot carries no authority).
    LogBehindSnapshot {
        /// The device.
        dev: String,
        /// The highest `seq` the log has for it.
        have: u64,
        /// The `seq` the manifest claims.
        claimed: u64,
    },
    /// `MANIFEST.json` is missing, unparseable, or names another snapshot.
    ManifestUnreadable {
        /// What was wrong.
        problem: String,
    },
    /// A file's SHA-256 does not match the manifest (`§5.3`, "SHA mismatch").
    ChecksumMismatch {
        /// The table.
        table: String,
        /// The file.
        file: String,
    },
    /// A file matched its checksum and still could not be loaded (`§5.3`, "unreadable").
    Unreadable {
        /// The table.
        table: String,
        /// What went wrong, first line.
        problem: String,
    },
}

impl SkipReason {
    /// Whether this reason means the snapshot was broken rather than inapplicable.
    #[must_use]
    pub fn is_failure(&self) -> bool {
        matches!(
            self,
            Self::ManifestUnreadable { .. }
                | Self::ChecksumMismatch { .. }
                | Self::Unreadable { .. }
        )
    }
}

impl fmt::Display for SkipReason {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::NoSnapshot => f.write_str("no snapshot"),
            Self::SchemaChanged => f.write_str("schema.sql has changed since the snapshot"),
            Self::TailNotCausal { events } => write!(
                f,
                "{events} event(s) the snapshot did not see are not causally after it"
            ),
            Self::LogBehindSnapshot { dev, have, claimed } => write!(
                f,
                "the log holds seq {have} for {dev} but the snapshot saw {claimed}"
            ),
            Self::ManifestUnreadable { problem } => write!(f, "MANIFEST.json: {problem}"),
            Self::ChecksumMismatch { table, file } => {
                write!(f, "{file}: SHA-256 does not match MANIFEST.json ({table})")
            }
            Self::Unreadable { table, problem } => write!(f, "{table}: {problem}"),
        }
    }
}

/// A tier that was not used, and why.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct Skipped {
    /// The tier.
    pub tier: Tier,
    /// The reason.
    #[serde(flatten)]
    pub reason: SkipReason,
}

/// What one restore did — `§5.3`'s "MUST record which tier succeeded", with the rest of
/// the story attached.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct Restored {
    /// The tier that built the tables.
    pub tier: Tier,
    /// The newest snapshot, whether or not it was used. `None` when there was none.
    pub snapshot: Option<String>,
    /// The tiers before `tier`, and why each was passed over.
    pub skipped: Vec<Skipped>,
    /// Tier 3 rebuilt a cache that did not exist, for an app whose log has events —
    /// `docs/backup-and-restore.md §3`'s "I rebuilt from scratch".
    pub from_scratch: bool,
}

impl Restored {
    /// `spec/cli.md §7`: fell through to tier 3 **unexpectedly** — a snapshot was there
    /// and applied, and its files were the only thing that failed.
    #[must_use]
    pub fn unexpected(&self) -> bool {
        self.tier == Tier::Replay
            && self.snapshot.is_some()
            && !self.skipped.is_empty()
            && self.skipped.iter().all(|s| s.reason.is_failure())
    }
}

/// The newest snapshot, once it has been read and found to describe this app.
struct Candidate {
    dir: PathBuf,
    manifest: Manifest,
}

/// What one pass over the tiers found, before any table is rebuilt from the replay.
struct Attempt {
    /// The tier that loaded every table, if one did.
    loaded: Option<Tier>,
    snapshot: Option<String>,
    skipped: Vec<Skipped>,
}

impl Store {
    /// Rebuild every table by `spec/protocol.md §5.3`: the snapshot's SQLite files + log
    /// tail, then CSV + `schema.sql` + log tail, then the full replay.
    ///
    /// **Unconditional** (`docs/plans/phase-1.md §2.5`): nothing in the existing cache is
    /// trusted or kept. The tiers decide where the rows come from, not whether the rebuild
    /// is partial. The tombstone set is rebuilt from the whole log on every tier, because
    /// a snapshot never carries it (`materialize::rebuild_tombstones`).
    ///
    /// The log is read once, here, and every tier and check works from that reading.
    pub fn restore(&mut self, cutoff: &str) -> Result<Restored, StoreError> {
        let events = self.read_log(cutoff)?;
        let attempt = self.attempt(&events, true)?;
        let restored = match attempt.loaded {
            Some(tier) => {
                self.finish(&events)?;
                Restored {
                    tier,
                    snapshot: attempt.snapshot,
                    skipped: attempt.skipped,
                    from_scratch: false,
                }
            }
            None => {
                let from_scratch = self.is_fresh() && self.log_bytes()? > 0;
                self.replay_events(&events)?;
                Restored {
                    tier: Tier::Replay,
                    snapshot: attempt.snapshot,
                    skipped: attempt.skipped,
                    from_scratch,
                }
            }
        };
        self.record_restore(&restored);
        Ok(restored)
    }

    /// Which tier [`restore`](Self::restore) would use, without writing a table
    /// (`spec/cli.md §7`, `--dry-run`).
    ///
    /// A prediction: the checksums are recomputed and the applicability checks run, but a
    /// file that hashes correctly and still fails to load is only discovered by loading.
    pub fn restore_dry_run(&self, cutoff: &str) -> Result<Restored, StoreError> {
        let events = self.read_log(cutoff)?;
        let attempt = self.attempt(&events, false)?;
        Ok(Restored {
            tier: attempt.loaded.unwrap_or(Tier::Replay),
            snapshot: attempt.snapshot,
            skipped: attempt.skipped,
            from_scratch: false,
        })
    }

    /// Tiers 1 and 2. With `load`, a tier that passes its checks is loaded into the tables;
    /// without, the checks are the whole of it.
    fn attempt(&self, events: &[Event], load: bool) -> Result<Attempt, StoreError> {
        let mut skipped = Vec::new();
        let mut skip_both = |reason: SkipReason| {
            skipped.push(Skipped {
                tier: Tier::Sqlite,
                reason: reason.clone(),
            });
            skipped.push(Skipped {
                tier: Tier::Csv,
                reason,
            });
        };

        let Some(id) = snapshot::newest(self.snap_dir())? else {
            skip_both(SkipReason::NoSnapshot);
            return Ok(Attempt {
                loaded: None,
                snapshot: None,
                skipped,
            });
        };
        let snapshot_name = id.to_string();
        let dir = self.snap_dir().join(&snapshot_name);

        let candidate = match self.read_candidate(&id, dir) {
            Ok(candidate) => candidate,
            Err(problem) => {
                skip_both(SkipReason::ManifestUnreadable { problem });
                return Ok(Attempt {
                    loaded: None,
                    snapshot: Some(snapshot_name),
                    skipped,
                });
            }
        };

        // (c) The snapshot describes this schema. Compared as text against the same
        // rendering the writer used; a different `schema.sql` is app-contract §4.5's full
        // rematerialization, and the replay is the only tier that can do it.
        let ddl = std::fs::read_to_string(candidate.dir.join(snapshot::SCHEMA_FILE)).ok();
        if ddl.as_deref() != Some(snapshot::render_ddl(self.schema()).as_str()) {
            skip_both(SkipReason::SchemaChanged);
            return Ok(Attempt {
                loaded: None,
                snapshot: Some(snapshot_name),
                skipped,
            });
        }

        // (a) `§5.3`'s first applicability condition: the events the snapshot did not see
        // are exactly the events with `lam > hi_lam`. A device the manifest never saw has
        // `hi_seq` 0, so all of its rows are "unseen" and every one of them had better be
        // above `hi_lam`. Any row counted here is the `§4.1` cross-device case, and no tier
        // but the replay can place it, because a snapshot row carries no `(lam, ts, dev)`.
        let manifest = &candidate.manifest;
        let non_causal = events
            .iter()
            .filter(|e| {
                let unseen = e.seq > manifest.hi_seq.get(&e.dev).copied().unwrap_or(0);
                unseen != (e.lam > manifest.hi_lam)
            })
            .count();
        if non_causal > 0 {
            skip_both(SkipReason::TailNotCausal {
                events: u64::try_from(non_causal).unwrap_or_default(),
            });
            return Ok(Attempt {
                loaded: None,
                snapshot: Some(snapshot_name),
                skipped,
            });
        }

        // (b) The log holds everything `hi_seq` claims. A snapshot carries no authority
        // (`§5`), so it never resurrects an event the log has lost.
        let heads = events::hi_seq(events);
        for (dev, claimed) in &manifest.hi_seq {
            let have = heads.get(dev).copied().unwrap_or(0);
            if have < *claimed {
                skip_both(SkipReason::LogBehindSnapshot {
                    dev: dev.clone(),
                    have,
                    claimed: *claimed,
                });
                return Ok(Attempt {
                    loaded: None,
                    snapshot: Some(snapshot_name),
                    skipped,
                });
            }
        }

        for tier in [Tier::Sqlite, Tier::Csv] {
            match self.load_tier(&candidate, tier, events, load) {
                Ok(()) => {
                    return Ok(Attempt {
                        loaded: Some(tier),
                        snapshot: Some(snapshot_name),
                        skipped,
                    });
                }
                Err(reason) => skipped.push(Skipped { tier, reason }),
            }
        }
        Ok(Attempt {
            loaded: None,
            snapshot: Some(snapshot_name),
            skipped,
        })
    }

    /// Read and sanity-check the manifest. The error is the problem, for the skip reason.
    fn read_candidate(&self, id: &SnapshotId, dir: PathBuf) -> Result<Candidate, String> {
        let manifest = snapshot::read_manifest(&dir).map_err(|error| error.to_string())?;
        if manifest.v != snapshot::MANIFEST_VERSION {
            return Err(format!(
                "manifest version {} is not {}",
                manifest.v,
                snapshot::MANIFEST_VERSION
            ));
        }
        if manifest.app != self.slug() {
            return Err(format!(
                "describes app {:?}, not {:?}",
                manifest.app,
                self.slug()
            ));
        }
        if manifest.snapshot_id != id.to_string() {
            return Err(format!(
                "names snapshot {:?} but lives in {id}",
                manifest.snapshot_id
            ));
        }
        Ok(Candidate { dir, manifest })
    }

    /// One tier over every declared table: checksums first, then — if asked — the tables
    /// created empty, each file loaded, and the tail applied, all in one transaction that
    /// is rolled back if any file turns out unreadable.
    fn load_tier(
        &self,
        candidate: &Candidate,
        tier: Tier,
        events: &[Event],
        load: bool,
    ) -> Result<(), SkipReason> {
        let mut files = Vec::with_capacity(self.schema().tables.len());
        for table in &self.schema().tables {
            let entry = candidate
                .manifest
                .tables
                .iter()
                .find(|t| t.name == table.name)
                .ok_or_else(|| SkipReason::Unreadable {
                    table: table.name.clone(),
                    problem: "not listed in MANIFEST.json".to_owned(),
                })?;
            let (file, expected) = match tier {
                Tier::Sqlite => (snapshot::sqlite_file(&table.name), &entry.sqlite_sha256),
                Tier::Csv => (snapshot::csv_file(&table.name), &entry.csv_sha256),
                Tier::Replay => return Ok(()),
            };
            let path = candidate.dir.join(&file);
            let actual = snapshot::sha256_file(&path).map_err(|error| SkipReason::Unreadable {
                table: table.name.clone(),
                problem: error.to_string(),
            })?;
            if actual != *expected {
                return Err(SkipReason::ChecksumMismatch {
                    table: table.name.clone(),
                    file,
                });
            }
            files.push((table, path));
        }
        if !load {
            return Ok(());
        }

        // Tier 1's files are attached before the transaction and detached after it: SQLite
        // refuses a `DETACH` inside a transaction, and a file left attached stays open.
        let conn = self.conn();
        let mut attached = Vec::new();
        if tier == Tier::Sqlite {
            for (index, (table, path)) in files.iter().enumerate() {
                let alias = format!("snap{index}");
                if let Err(problem) = attach(conn, &alias, path) {
                    detach_all(conn, &attached);
                    return Err(SkipReason::Unreadable {
                        table: table.name.clone(),
                        problem,
                    });
                }
                attached.push(alias);
            }
        }
        let loaded = self.load_attached(conn, tier, &files, &attached, events, candidate);
        detach_all(conn, &attached);
        loaded
    }

    /// The transactional half of a tier: create every table empty, load each file, apply
    /// the tail, commit — or roll the lot back.
    fn load_attached(
        &self,
        conn: &Connection,
        tier: Tier,
        files: &[(&crate::store::schema::Table, std::path::PathBuf)],
        attached: &[String],
        events: &[Event],
        candidate: &Candidate,
    ) -> Result<(), SkipReason> {
        let unreadable = |table: &str, problem: String| SkipReason::Unreadable {
            table: table.to_owned(),
            problem,
        };
        let tx = conn
            .unchecked_transaction()
            .map_err(|error| unreadable("", error.to_string()))?;
        materialize::create_tables(&tx, &self.schema().tables)
            .map_err(|error| unreadable("", error.to_string()))?;
        for (index, (table, path)) in files.iter().enumerate() {
            let loaded = match tier {
                Tier::Sqlite => copy_attached(&tx, table, &attached[index]),
                Tier::Csv | Tier::Replay => load_csv(&tx, table, path),
            };
            loaded.map_err(|problem| unreadable(&table.name, problem))?;
        }
        materialize::apply_tail(
            &tx,
            &self.schema().tables,
            events,
            candidate.manifest.hi_lam,
        )
        .map_err(|error| unreadable("", error.to_string()))?;
        tx.commit()
            .map_err(|error| unreadable("", error.to_string()))
    }

    /// After a tier loaded the tables: the tombstone set from the whole log, the declared
    /// indexes, and the views — the same tail `materialize` has.
    fn finish(&self, events: &[Event]) -> Result<(), StoreError> {
        materialize::rebuild_tombstones(self.conn(), events)?;
        self.create_indexes()?;
        self.create_views()
    }

    /// Restore-time bookkeeping shared by the tiers: the watermark, and what happened.
    fn record_restore(&mut self, restored: &Restored) {
        self.note_rebuilt(restored.tier, restored.snapshot.clone());
        self.set_restored(restored.clone());
    }
}

/// Tier 1: attach one table's SQLite file read-only under `alias`.
fn attach(conn: &Connection, alias: &str, path: &std::path::Path) -> Result<(), String> {
    let uri = format!(
        "file:{}?mode=ro",
        path.display()
            .to_string()
            .replace('\\', "/")
            .replace('?', "%3F")
    );
    conn.execute(
        &format!("ATTACH DATABASE ? AS {alias}"),
        rusqlite::params![uri],
    )
    .map(|_| ())
    .map_err(|e| crate::store::schema::first_line(&e.to_string()))
}

/// Detach what [`attach`] attached; nothing to report if one is already gone.
fn detach_all(conn: &Connection, aliases: &[String]) {
    for alias in aliases {
        let _ = conn.execute_batch(&format!("DETACH DATABASE {alias}"));
    }
}

/// Tier 1: copy one table out of its attached file, column by name. The target was just
/// created with the current types, so a value comes across as the file stored it and is
/// retyped by the column it lands in.
fn copy_attached(
    conn: &Connection,
    table: &crate::store::schema::Table,
    alias: &str,
) -> Result<(), String> {
    let mut columns = String::from(crate::store::schema::ID_COLUMN);
    for column in &table.columns {
        columns.push_str(", ");
        columns.push_str(&materialize::quote_ident(&column.name));
    }
    let target = materialize::quote_ident(&table.name);
    conn.execute_batch(&format!(
        "INSERT INTO main.{target} ({columns}) SELECT {columns} FROM {alias}.{target} ORDER BY {id};",
        id = crate::store::schema::ID_COLUMN
    ))
    .map_err(|e| crate::store::schema::first_line(&e.to_string()))
}

/// Tier 2: read one table's CSV, typed from `schema.sql` and never inferred.
fn load_csv(
    conn: &Connection,
    table: &crate::store::schema::Table,
    path: &std::path::Path,
) -> Result<(), String> {
    let file = csv::read(path)?;
    let mut expected = vec![crate::store::schema::ID_COLUMN.to_owned()];
    expected.extend(table.columns.iter().map(|c| c.name.clone()));
    if file.header != expected {
        return Err(format!(
            "header {:?} does not match the declared columns {:?}",
            file.header, expected
        ));
    }
    let mut insert = conn
        .prepare(&materialize::insert_sql(table))
        .map_err(|e| e.to_string())?;
    for row in &file.rows {
        let mut values = Vec::with_capacity(row.len());
        let Some(id) = row[0].as_deref() else {
            return Err("a row with no id".to_owned());
        };
        values.push(rusqlite::types::Value::Text(id.to_owned()));
        for (column, text) in table.columns.iter().zip(row[1..].iter()) {
            values.push(materialize::typed_text(column, text.as_deref()));
        }
        insert
            .execute(rusqlite::params_from_iter(values))
            .map_err(|e| crate::store::schema::first_line(&e.to_string()))?;
    }
    Ok(())
}

// AGENTS.md, Style: unwrap() is permitted in tests. The crate-level deny reaches unit
// tests inside src/, so each one opts out where it is declared.
#[allow(clippy::unwrap_used, clippy::expect_used)]
#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn tiers_round_trip_through_their_number() {
        for tier in [Tier::Sqlite, Tier::Csv, Tier::Replay] {
            assert_eq!(Tier::try_from(tier.as_u8()).unwrap(), tier);
            assert_eq!(
                serde_json::to_string(&tier).unwrap(),
                tier.as_u8().to_string()
            );
        }
        assert!(Tier::try_from(0).is_err());
        assert_eq!(Tier::Csv.to_string(), "tier 2 (csv)");
        assert_eq!(Tier::Sqlite.to_string(), "tier 1 (sqlite)");
    }

    /// `spec/cli.md §7`: only a broken snapshot is an unexpected fall-through.
    #[test]
    fn unexpected_means_the_files_failed_not_that_the_snapshot_did_not_apply() {
        let broken = Restored {
            tier: Tier::Replay,
            snapshot: Some("2026-W35-k7m2q9xf-8830".into()),
            skipped: vec![
                Skipped {
                    tier: Tier::Sqlite,
                    reason: SkipReason::ChecksumMismatch {
                        table: "profile".into(),
                        file: "profile.sqlite".into(),
                    },
                },
                Skipped {
                    tier: Tier::Csv,
                    reason: SkipReason::Unreadable {
                        table: "profile".into(),
                        problem: "x".into(),
                    },
                },
            ],
            from_scratch: false,
        };
        assert!(broken.unexpected());

        let stale = Restored {
            skipped: vec![
                Skipped {
                    tier: Tier::Sqlite,
                    reason: SkipReason::SchemaChanged,
                },
                Skipped {
                    tier: Tier::Csv,
                    reason: SkipReason::SchemaChanged,
                },
            ],
            ..broken.clone()
        };
        assert!(!stale.unexpected());

        let none = Restored {
            snapshot: None,
            skipped: vec![Skipped {
                tier: Tier::Sqlite,
                reason: SkipReason::NoSnapshot,
            }],
            ..broken.clone()
        };
        assert!(!none.unexpected());

        let rescued = Restored {
            tier: Tier::Csv,
            ..broken
        };
        assert!(!rescued.unexpected());
    }

    /// The audit detail is JSON a person can read: the reason is tagged, not numbered.
    #[test]
    fn skip_reasons_serialize_with_a_tag() {
        let skipped = Skipped {
            tier: Tier::Sqlite,
            reason: SkipReason::TailNotCausal { events: 3 },
        };
        assert_eq!(
            serde_json::to_string(&skipped).unwrap(),
            r#"{"tier":1,"reason":"tail_not_causal","events":3}"#
        );
    }
}
