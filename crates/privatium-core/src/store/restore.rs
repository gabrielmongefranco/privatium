// Project:  Privatium™  |  File: crates/privatium-core/src/store/restore.rs
// Authors:  Gabriel Mongefranco (@gabrielmongefranco)
// Created:  2026-09-02  |  Modified: 2026-09-02
// Summary:  spec/protocol.md §5.3 — the three-tier read. Parquet plus the log tail, then
//           CSV plus schema.sql plus the tail, then the full replay; which tier succeeded,
//           and why the ones before it did not.

use std::fmt;
use std::path::PathBuf;

use serde::{Deserialize, Serialize};

use crate::store::materialize::{self, Source};
use crate::store::snapshot::{self, Manifest, SnapshotId};
use crate::store::{Store, StoreError};

/// One of `§5.3`'s three tiers.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(into = "u8", try_from = "u8")]
pub enum Tier {
    /// Parquet + log tail.
    Parquet = 1,
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
            Self::Parquet => "parquet",
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
            1 => Ok(Self::Parquet),
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
        /// What the engine said, first line.
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
    staged: bool,
}

impl Store {
    /// Rebuild every table by `spec/protocol.md §5.3`: Parquet + log tail, then CSV +
    /// `schema.sql` + log tail, then the full replay.
    ///
    /// **Unconditional** (`docs/plans/phase-1.md §2.5`): nothing in the existing cache is
    /// trusted or kept. The tiers decide where the rows come from, not whether the rebuild
    /// is partial. The tombstone set is rebuilt from the whole log on every tier, because
    /// a snapshot never carries it (`materialize::tombstone_sql`).
    ///
    /// Needs the privileged window, like [`materialize`](Self::materialize).
    pub fn restore(&mut self, cutoff: &str) -> Result<Restored, StoreError> {
        if self.is_sealed() {
            return Err(StoreError::Sealed {
                slug: self.slug().to_owned(),
            });
        }

        let attempt = self.attempt(cutoff, true)?;
        let restored = match attempt.loaded {
            Some(tier) => {
                let finished = self.finish_from_stage(cutoff);
                self.unstage()?;
                finished?;
                Restored {
                    tier,
                    snapshot: attempt.snapshot,
                    skipped: attempt.skipped,
                    from_scratch: false,
                }
            }
            None => {
                if attempt.staged {
                    self.unstage()?;
                }
                let from_scratch = self.is_fresh() && self.log_bytes()? > 0;
                self.materialize(cutoff)?;
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
        if self.is_sealed() {
            return Err(StoreError::Sealed {
                slug: self.slug().to_owned(),
            });
        }
        let attempt = self.attempt(cutoff, false)?;
        if attempt.staged {
            self.unstage()?;
        }
        Ok(Restored {
            tier: attempt.loaded.unwrap_or(Tier::Replay),
            snapshot: attempt.snapshot,
            skipped: attempt.skipped,
            from_scratch: false,
        })
    }

    /// Tiers 1 and 2. With `load`, a tier that passes its checks is loaded into the tables;
    /// without, the checks are the whole of it.
    fn attempt(&self, cutoff: &str, load: bool) -> Result<Attempt, StoreError> {
        let mut skipped = Vec::new();
        let mut skip_both = |reason: SkipReason| {
            skipped.push(Skipped {
                tier: Tier::Parquet,
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
                staged: false,
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
                    staged: false,
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
                staged: false,
            });
        }

        // One scan of the log. Everything from here reads the stage.
        let source = self.log_source()?;
        self.conn()
            .execute_batch(&materialize::stage_sql(self.slug(), &source, cutoff))
            .map_err(StoreError::Duck)?;
        let stage = Source::stage();

        // (a) and (b) of §5.3's applicability, in that order.
        let non_causal: i64 = self
            .conn()
            .query_row(
                &materialize::non_causal_sql(
                    &stage,
                    candidate.manifest.hi_lam,
                    &candidate.manifest.hi_seq,
                ),
                [],
                |row| row.get(0),
            )
            .map_err(StoreError::Duck)?;
        if non_causal > 0 {
            skip_both(SkipReason::TailNotCausal {
                events: u64::try_from(non_causal).unwrap_or_default(),
            });
            return Ok(Attempt {
                loaded: None,
                snapshot: Some(snapshot_name),
                skipped,
                staged: true,
            });
        }
        let behind = self.conn().query_row(
            &materialize::behind_sql(&stage, &candidate.manifest.hi_seq),
            [],
            |row| {
                Ok((
                    row.get::<_, String>(0)?,
                    row.get::<_, i64>(1)?,
                    row.get::<_, i64>(2)?,
                ))
            },
        );
        match behind {
            Ok((dev, have, claimed)) => {
                skip_both(SkipReason::LogBehindSnapshot {
                    dev,
                    have: u64::try_from(have).unwrap_or_default(),
                    claimed: u64::try_from(claimed).unwrap_or_default(),
                });
                return Ok(Attempt {
                    loaded: None,
                    snapshot: Some(snapshot_name),
                    skipped,
                    staged: true,
                });
            }
            Err(duckdb::Error::QueryReturnedNoRows) => {}
            Err(error) => return Err(StoreError::Duck(error)),
        }

        for tier in [Tier::Parquet, Tier::Csv] {
            match self.load_tier(&candidate, tier, cutoff, load) {
                Ok(()) => {
                    return Ok(Attempt {
                        loaded: Some(tier),
                        snapshot: Some(snapshot_name),
                        skipped,
                        staged: true,
                    });
                }
                Err(reason) => skipped.push(Skipped { tier, reason }),
            }
        }
        Ok(Attempt {
            loaded: None,
            snapshot: Some(snapshot_name),
            skipped,
            staged: true,
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

    /// One tier over every declared table: checksum first, then — if asked — create the
    /// table from the snapshot's types, load the file, and apply the tail.
    fn load_tier(
        &self,
        candidate: &Candidate,
        tier: Tier,
        cutoff: &str,
        load: bool,
    ) -> Result<(), SkipReason> {
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
                Tier::Parquet => (snapshot::parquet_file(&table.name), &entry.parquet_sha256),
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
            if !load {
                continue;
            }

            let target = self.qualified(&table.name);
            let path = snapshot::sql_path(&path);
            let load_sql = match tier {
                Tier::Parquet => materialize::load_parquet_sql(&target, table, &path),
                Tier::Csv | Tier::Replay => materialize::load_csv_sql(&target, table, &path),
            };
            let (delete, insert) = materialize::tail_sql(
                &target,
                self.slug(),
                table,
                cutoff,
                candidate.manifest.hi_lam,
            );
            let sql = format!(
                "{create}\n{load_sql}\n{delete}\n{insert}",
                create = materialize::create_table_sql(&target, table),
            );
            self.conn()
                .execute_batch(&sql)
                .map_err(|error| SkipReason::Unreadable {
                    table: table.name.clone(),
                    problem: first_line(&error.to_string()),
                })?;
        }
        Ok(())
    }

    /// After a tier loaded the tables: the tombstone set from the whole staged log, the
    /// views, and the checkpoint — the same tail `materialize` has.
    fn finish_from_stage(&self, cutoff: &str) -> Result<(), StoreError> {
        let sql = materialize::tombstone_sql(self.slug(), &Source::stage(), cutoff);
        self.conn().execute_batch(&sql).map_err(StoreError::Duck)?;
        self.create_views()?;
        self.checkpoint()
    }

    fn unstage(&self) -> Result<(), StoreError> {
        self.conn()
            .execute_batch(&materialize::unstage_sql())
            .map_err(StoreError::Duck)
    }

    /// Restore-time bookkeeping shared by the tiers: the watermark, and what happened.
    fn record_restore(&mut self, restored: &Restored) {
        self.note_rebuilt(restored.tier, restored.snapshot.clone());
        self.set_restored(restored.clone());
    }
}

/// DuckDB errors carry a stack of context; the first line is the part a person reads.
fn first_line(message: &str) -> String {
    message.lines().next().unwrap_or(message).to_owned()
}

// AGENTS.md, Style: unwrap() is permitted in tests. The crate-level deny reaches unit
// tests inside src/, so each one opts out where it is declared.
#[allow(clippy::unwrap_used, clippy::expect_used)]
#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn tiers_round_trip_through_their_number() {
        for tier in [Tier::Parquet, Tier::Csv, Tier::Replay] {
            assert_eq!(Tier::try_from(tier.as_u8()).unwrap(), tier);
            assert_eq!(
                serde_json::to_string(&tier).unwrap(),
                tier.as_u8().to_string()
            );
        }
        assert!(Tier::try_from(0).is_err());
        assert_eq!(Tier::Csv.to_string(), "tier 2 (csv)");
    }

    /// `spec/cli.md §7`: only a broken snapshot is an unexpected fall-through.
    #[test]
    fn unexpected_means_the_files_failed_not_that_the_snapshot_did_not_apply() {
        let broken = Restored {
            tier: Tier::Replay,
            snapshot: Some("2026-W35-k7m2q9xf-8830".into()),
            skipped: vec![
                Skipped {
                    tier: Tier::Parquet,
                    reason: SkipReason::ChecksumMismatch {
                        table: "profile".into(),
                        file: "profile.parquet".into(),
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
                    tier: Tier::Parquet,
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
                tier: Tier::Parquet,
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
            tier: Tier::Parquet,
            reason: SkipReason::TailNotCausal { events: 3 },
        };
        assert_eq!(
            serde_json::to_string(&skipped).unwrap(),
            r#"{"tier":1,"reason":"tail_not_causal","events":3}"#
        );
    }
}
