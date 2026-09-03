// Project:  Privatium™  |  File: crates/privatium-core/src/app/mod.rs
// Authors:  Gabriel Mongefranco (@gabrielmongefranco)
// Created:  2026-09-02  |  Modified: 2026-09-03
// Summary:  The app loader — step 5 of docs/plans/phase-1.md §2.6 and the lifecycle of
//           spec/app-contract.md §8 up to and including mount. Discovers app folders,
//           refuses per app and loudly (§3.1), keeps sys_app as events (§3.4), and owns
//           each app's log and store.

use std::collections::BTreeMap;
use std::fmt;
use std::fs;
use std::path::{Path, PathBuf};

use serde::Serialize;

use crate::config::Mode;
use crate::log::{self, AppLog, Durability};
use crate::store::{self, Schema, Store, StoreError};
use crate::{
    Error, Node, Result, audit_recovery, audit_restore, boxed, io_at, new_ulid, note_health, sys,
};

pub mod csp;
pub mod manifest;
pub mod seed;

pub use csp::Csp;
pub use manifest::{
    MANIFEST_FILE, MAX_ADVERTISED_SLUG, Manifest, ManifestError, Permissions, RESERVED_SLUGS,
    SUPPORTED_API, Tier, Widening,
};
pub use seed::{SEED_PATH, SeedError, SeedEvent};

/// The optional typed-table declaration (`spec/app-contract.md §4.5`).
const SCHEMA_FILE: &str = "schema.sql";

/// Where an app folder came from — `sys_app.source` (`spec/data-dictionary.md §3.4`).
///
/// `bundled` is a folder shipped with the framework: the repository's `apps/` in a
/// development checkout, the package's at install (`apps/README.md`). `local` is the
/// owner's `<data-root>/apps/`. `url:<origin>` is reserved; there is no registry in `pv/1`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum Source {
    /// `<data-root>/apps/<slug>/`, writable, survives upgrades.
    Local,
    /// Ships with the framework, read-only at runtime.
    Bundled,
}

impl Source {
    /// The `sys_app.source` value.
    #[must_use]
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Local => "local",
            Self::Bundled => "bundled",
        }
    }
}

impl fmt::Display for Source {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}

/// A directory holding app folders.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AppRoot {
    /// The directory. Absent is the same as empty.
    pub dir: PathBuf,
    /// What every folder inside it is recorded as.
    pub source: Source,
}

impl AppRoot {
    /// The owner's `<data-root>/apps/`.
    #[must_use]
    pub fn local(dir: impl Into<PathBuf>) -> Self {
        Self {
            dir: dir.into(),
            source: Source::Local,
        }
    }

    /// The framework's own `apps/`.
    #[must_use]
    pub fn bundled(dir: impl Into<PathBuf>) -> Self {
        Self {
            dir: dir.into(),
            source: Source::Bundled,
        }
    }
}

/// Where in `spec/app-contract.md §8`'s lifecycle an app was refused.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum Stage {
    /// `app.toml` could not be read or is not the manifest `§3` describes.
    Parse,
    /// `§3.1`: slug, folder, `api`, and the rest of `§3`'s constraints.
    Validate,
    /// `§8`'s tier check: `app.lua` or `web/index.html`.
    Tier,
    /// `schema.sql` does not declare tables the engine will accept.
    Schema,
    /// Another folder already owns the slug (`§3.1`).
    Collision,
    /// Opening the log or building the cache failed.
    Materialize,
}

impl Stage {
    fn as_str(self) -> &'static str {
        match self {
            Self::Parse => "parse",
            Self::Validate => "validate",
            Self::Tier => "tier",
            Self::Schema => "schema",
            Self::Collision => "collision",
            Self::Materialize => "materialize",
        }
    }
}

impl fmt::Display for Stage {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}

/// One folder that was not loaded, and why. The node started anyway (`§3.1`).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LoadFailure {
    /// The folder name — the would-be slug.
    pub folder: String,
    /// Which root it was found under.
    pub source: Source,
    /// Where it stopped.
    pub stage: Stage,
    /// The reason, as `sys_app.last_error` carries it.
    pub reason: String,
}

impl fmt::Display for LoadFailure {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            f,
            "{} ({}, {}): {}",
            self.folder, self.source, self.stage, self.reason
        )
    }
}

/// Something the owner should hear about an app that did load.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Warning {
    /// A non-default permission (`spec/app-contract.md §5.4`).
    Permission {
        /// The app.
        slug: String,
        /// What it asked for.
        widening: Widening,
    },
    /// `nav.advertise = true` on a slug longer than a DNS label allows
    /// (`spec/protocol.md §6.1`: MUST be surfaced at load). Phase 1 advertises nothing;
    /// the warning is still owed.
    SlugTooLongToAdvertise {
        /// The app.
        slug: String,
    },
    /// `[node] mode = "solo"` names an app that did not load, so nothing is mounted at `/`.
    SoloAppNotLoaded {
        /// The app.
        slug: String,
    },
    /// A route of the solo app matches a framework prefix and is shadowed by it
    /// (`spec/protocol.md §9.1`: MUST warn at load, naming the route and the prefix). For a
    /// Tier 2 app the routes are the paths under `web/`, so this is a top-level entry there;
    /// a Tier 1 app's registered routes get the same warning from the Lua host (M7).
    RouteShadowed {
        /// The app.
        slug: String,
        /// The app's route.
        route: String,
        /// The framework prefix that wins.
        prefix: &'static str,
    },
    /// `[app] icon` names an icon the vendored set lacks (`docs/icons.md`): the launcher
    /// shows `question-circle` in its place, and the app loads.
    UnknownIcon {
        /// The app.
        slug: String,
        /// The name that was asked for.
        icon: String,
    },
}

impl Warning {
    /// The app the warning is about.
    #[must_use]
    pub fn slug(&self) -> &str {
        match self {
            Self::Permission { slug, .. }
            | Self::SlugTooLongToAdvertise { slug }
            | Self::SoloAppNotLoaded { slug }
            | Self::RouteShadowed { slug, .. }
            | Self::UnknownIcon { slug, .. } => slug,
        }
    }
}

impl fmt::Display for Warning {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Permission { slug, widening } => write!(f, "{slug}: {widening}"),
            Self::SlugTooLongToAdvertise { slug } => write!(
                f,
                "{slug}: longer than {MAX_ADVERTISED_SLUG} characters, so it cannot be \
                 advertised as a DNS-SD subtype (spec/protocol.md §6.1)"
            ),
            Self::SoloAppNotLoaded { slug } => write!(
                f,
                "{slug}: named by [node] app in solo mode but not loaded; nothing is mounted at /"
            ),
            Self::RouteShadowed {
                slug,
                route,
                prefix,
            } => write!(
                f,
                "{slug}: route {route} is shadowed by the framework prefix {prefix} in solo mode \n                 (spec/protocol.md §9.1)"
            ),
            Self::UnknownIcon { slug, icon } => write!(
                f,
                "{slug}: icon {icon:?} is not in the vendored Bootstrap Icons set; question-circle \n                 is shown instead (docs/icons.md)"
            ),
        }
    }
}

/// What one [`Node::load_apps`] did.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct LoadReport {
    /// Loaded, in discovery order.
    pub loaded: Vec<String>,
    /// Valid, indexed, and `enabled = false`: data kept, nothing materialized or mounted.
    pub disabled: Vec<String>,
    /// Refused, in discovery order. Each also has an `app.load_failed` audit row, once.
    pub failed: Vec<LoadFailure>,
    /// Indexed apps whose folder was not found under any root, now marked
    /// `last_error = "folder missing"`.
    pub missing: Vec<String>,
    /// Everything surfaced to the owner.
    pub warnings: Vec<Warning>,
}

/// What [`Node::load_seed`] did.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Seeded {
    /// The app.
    pub slug: String,
    /// Events appended, as this node's, in one batch.
    pub events: usize,
}

/// A loaded app: its manifest, its folder, its log, and its cache.
#[derive(Debug)]
pub struct App {
    manifest: Manifest,
    manifest_hash: String,
    dir: PathBuf,
    source: Source,
    mount: Option<String>,
    csp: Csp,
    warnings: Vec<Warning>,
    log: AppLog,
    store: Store,
}

impl App {
    /// The slug.
    #[must_use]
    pub fn slug(&self) -> &str {
        &self.manifest.app.slug
    }

    /// `app.toml`, parsed.
    #[must_use]
    pub fn manifest(&self) -> &Manifest {
        &self.manifest
    }

    /// SHA-256 of `app.toml` — `sys_app.manifest_hash`.
    #[must_use]
    pub fn manifest_hash(&self) -> &str {
        &self.manifest_hash
    }

    /// The folder.
    #[must_use]
    pub fn dir(&self) -> &Path {
        &self.dir
    }

    /// Where the folder came from.
    #[must_use]
    pub fn source(&self) -> Source {
        self.source
    }

    /// The mount path — `/a/<slug>/` in host mode, `/` for the solo app — or `None` for
    /// an app that exists without being served: a tier 3 index entry, or any other app in
    /// solo mode. This is where `§8`'s "mount" lands until M6's router reads it.
    #[must_use]
    pub fn mount(&self) -> Option<&str> {
        self.mount.as_deref()
    }

    /// The app's Content-Security-Policy, computed at load.
    #[must_use]
    pub fn csp(&self) -> &Csp {
        &self.csp
    }

    /// What the owner was told at load.
    #[must_use]
    pub fn warnings(&self) -> &[Warning] {
        &self.warnings
    }

    /// The app's log. This node's writer, and the Lamport counter.
    #[must_use]
    pub fn log(&self) -> &AppLog {
        &self.log
    }

    /// The app's cache. [`Store::app_conn`] is the sandboxed read-only connection app SQL
    /// runs on; the framework's own is [`Store::conn`].
    #[must_use]
    pub fn store(&self) -> &Store {
        &self.store
    }

    /// The store, for the node-level maintenance.
    pub(crate) fn store_mut(&mut self) -> &mut Store {
        &mut self.store
    }

    /// `sample/seed.jsonl`, if the folder ships one (`spec/app-contract.md §9`).
    #[must_use]
    pub fn seed_path(&self) -> Option<PathBuf> {
        let path = self.dir.join(SEED_PATH);
        path.is_file().then_some(path)
    }
}

/// A folder found under a root, before anything was read from it.
struct Candidate {
    folder: String,
    dir: PathBuf,
    source: Source,
}

/// Every app folder under `root`, by name.
///
/// A folder whose name starts with `_` is not an app: `_sys` (`docs/plans/phase-1.md
/// §2.6`) and the lint corpus `_lint` alike. Files are not apps either. Everything else is,
/// and a folder with no `app.toml` is refused loudly rather than skipped, because
/// `spec/app-contract.md` defines an app as a folder containing one.
fn discover(root: &AppRoot) -> Result<Vec<Candidate>> {
    let entries = match fs::read_dir(&root.dir) {
        Ok(entries) => entries,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(Vec::new()),
        Err(error) => return Err(io_at(&root.dir)(error)),
    };
    let mut candidates = Vec::new();
    for entry in entries {
        let entry = entry.map_err(io_at(&root.dir))?;
        let dir = entry.path();
        if !dir.is_dir() {
            continue;
        }
        let folder = entry.file_name().to_string_lossy().into_owned();
        if folder.starts_with('_') {
            continue;
        }
        candidates.push(Candidate {
            folder,
            dir,
            source: root.source,
        });
    }
    candidates.sort_by(|a, b| a.folder.cmp(&b.folder));
    Ok(candidates)
}

/// Everything `§8` learns about an app before it touches the log.
struct Prepared {
    manifest: Manifest,
    manifest_hash: String,
    schema: Schema,
    mount: Option<String>,
    csp: Csp,
    warnings: Vec<Warning>,
}

/// Where `prepare` stopped, with whatever it had learned by then — the row still gets
/// written with it (`spec/data-dictionary.md §3.4`).
struct Refused {
    stage: Stage,
    reason: String,
    manifest: Option<Manifest>,
    manifest_hash: Option<String>,
}

enum Outcome {
    Loaded(Box<App>),
    Disabled,
    Refused(LoadFailure),
}

impl Node {
    /// Step 5 of `docs/plans/phase-1.md §2.6`: load every app folder under `roots`, in
    /// root order and by name within a root, through `spec/app-contract.md §8` up to and
    /// including mount.
    ///
    /// Refusal is per app and loud (`§3.1`): a folder that fails is in the report and in
    /// `sys_audit` as `app.load_failed`, its `sys_app` row carries `last_error`, and the
    /// node goes on. This never returns `Err` for anything one app did; an `Err` is the
    /// node's own trouble — a root that cannot be read, a `_sys` log that cannot be
    /// written.
    ///
    /// A slug already loaded from the same folder is reloaded in place; from a different
    /// folder it is a collision and the newcomer is refused. Indexed apps found under no
    /// root are marked `last_error = "folder missing"` (`§3.4`) — so pass every root at
    /// once rather than one call per root.
    ///
    /// Nothing here loads `sample/seed.jsonl`. That is [`load_seed`](Self::load_seed), on
    /// the owner's say-so (`§9`).
    pub fn load_apps(&mut self, roots: &[AppRoot]) -> Result<LoadReport> {
        let cutoff = store::cutoff_now();
        // `sys.sys_app` and `sys.sys_audit` have to be current: the upsert compares
        // against the row, and the audit is bounded by the last one.
        self.store.refresh(&cutoff).map_err(boxed)?;

        let mut report = LoadReport::default();
        let mut claimed: BTreeMap<String, PathBuf> = BTreeMap::new();
        for root in roots {
            for candidate in discover(root)? {
                let elsewhere = claimed.contains_key(&candidate.folder)
                    || self
                        .apps
                        .get(&candidate.folder)
                        .is_some_and(|app| app.dir != candidate.dir);
                if elsewhere {
                    let failure = LoadFailure {
                        folder: candidate.folder.clone(),
                        source: candidate.source,
                        stage: Stage::Collision,
                        reason: "collides with an app of the same slug that was installed first \
                                 (spec/app-contract.md §3.1)"
                            .to_owned(),
                    };
                    self.audit_load_failed(&failure, None)?;
                    report.failed.push(failure);
                    continue;
                }
                claimed.insert(candidate.folder.clone(), candidate.dir.clone());
                // A reload: the old store is dropped and a fresh one opened.
                self.apps.remove(&candidate.folder);

                match self.load_one(&candidate, &cutoff)? {
                    Outcome::Loaded(app) => {
                        report.warnings.extend(app.warnings.iter().cloned());
                        report.loaded.push(candidate.folder.clone());
                        self.apps.insert(candidate.folder, *app);
                    }
                    Outcome::Disabled => report.disabled.push(candidate.folder),
                    Outcome::Refused(failure) => report.failed.push(failure),
                }
            }
        }

        // `§3.4`: removing a folder MUST NOT delete the index row or the data.
        for slug in self.indexed_slugs()? {
            if claimed.contains_key(&slug) || self.apps.contains_key(&slug) {
                continue;
            }
            if self.mark_folder_missing(&slug)? {
                report.missing.push(slug);
            }
        }

        if let (Mode::Solo, Some(solo)) = (self.config.node.mode, self.config.node.app.as_deref())
            && !self.apps.contains_key(solo)
        {
            report.warnings.push(Warning::SoloAppNotLoaded {
                slug: solo.to_owned(),
            });
        }

        self.store.refresh(&cutoff).map_err(boxed)?;
        self.flush()?;
        Ok(report)
    }

    /// Every loaded app, by slug.
    pub fn apps(&self) -> impl Iterator<Item = &App> {
        self.apps.values()
    }

    /// One loaded app.
    #[must_use]
    pub fn app(&self, slug: &str) -> Option<&App> {
        self.apps.get(slug)
    }

    /// The mount table: `(path, app)` for every app that is served, which is what M6's
    /// router will be built from.
    pub fn mounts(&self) -> impl Iterator<Item = (&str, &App)> {
        self.apps
            .values()
            .filter_map(|app| app.mount.as_deref().map(|mount| (mount, app)))
    }

    /// `sample/seed.jsonl` for a loaded app, if it ships one — the offer
    /// `spec/app-contract.md §9` describes, for whichever surface makes it.
    #[must_use]
    pub fn seed_available(&self, slug: &str) -> Option<PathBuf> {
        self.apps.get(slug).and_then(App::seed_path)
    }

    /// Load `sample/seed.jsonl` into `slug`, on the owner's explicit say-so.
    ///
    /// Never called by [`load_apps`](Self::load_apps). Refused when the app's log already
    /// holds an event from any device, so a seed can only ever populate an empty app.
    /// Every line is appended through the app's own log as one batch of **this node's**
    /// events — fresh `seq`, `lam`, `ts` and `dev` — and applied to the cache
    /// incrementally; the seed's own envelope fields are discarded (`seed`).
    pub fn load_seed(&mut self, slug: &str) -> Result<Seeded> {
        let app = self.apps.get_mut(slug).ok_or_else(|| Error::AppNotLoaded {
            slug: slug.to_owned(),
        })?;
        let path = app.seed_path().ok_or_else(|| Error::NoSeed {
            slug: slug.to_owned(),
        })?;
        if app.log.seq() > 0 || !app.log.heads().is_empty() {
            return Err(Error::SeedRefused {
                slug: slug.to_owned(),
                events: app.log.heads().values().sum(),
            });
        }

        let text = fs::read_to_string(&path).map_err(io_at(&path))?;
        let events = seed::parse(&text).map_err(|error| Error::Seed {
            path: path.clone(),
            line: error.line,
            problem: error.problem,
        })?;
        if events.is_empty() {
            return Ok(Seeded {
                slug: slug.to_owned(),
                events: 0,
            });
        }

        app.log.batch(|batch| {
            for event in &events {
                match &event.d {
                    Some(d) => batch.put(&event.tbl, &event.id, d)?,
                    None => batch.del(&event.tbl, &event.id)?,
                }
            }
            Ok(())
        })?;
        for event in &events {
            app.store
                .apply(&event.tbl, &event.id, event.d.as_ref())
                .map_err(boxed)?;
        }
        app.log.save_to(&mut self.state);
        app.store.save_to(&mut self.state);
        self.state.flush()?;

        Ok(Seeded {
            slug: slug.to_owned(),
            events: events.len(),
        })
    }

    /// Rebuild a loaded app's cache if its log or `schema.sql` moved underneath it — the
    /// `echo >>` reload of `apps/hello/README.md`, per request.
    ///
    /// A stat decides; only a stale store pays for the rebuild. Returns whether it rebuilt.
    pub fn refresh_app(&mut self, slug: &str) -> Result<bool> {
        let app = self.apps.get_mut(slug).ok_or_else(|| Error::AppNotLoaded {
            slug: slug.to_owned(),
        })?;
        if !app.store.is_stale().map_err(boxed)? {
            return Ok(false);
        }
        let previous = app.store.restore_record().cloned();
        app.store.refresh(&store::cutoff_now()).map_err(boxed)?;
        if let Some(restored) = app.store.restored().cloned() {
            audit_restore(&mut self.sys, slug, &restored, previous.as_ref(), false)?;
            note_health(&self.store, &app.store, slug)?;
        }
        self.flush()?;
        Ok(true)
    }

    /// One folder through `§8`, writing its row and its audit as it goes.
    fn load_one(&mut self, candidate: &Candidate, cutoff: &str) -> Result<Outcome> {
        // The row is keyed by the folder name (`spec/app-contract.md §3.1`: the slug
        // equals the folder). A folder that could never be a slug has no row to carry
        // `last_error`; it gets the audit alone.
        let keyable =
            manifest::is_valid_slug(&candidate.folder) && !manifest::is_reserved(&candidate.folder);
        let existing = if keyable {
            self.read_app_row(&candidate.folder)?
        } else {
            None
        };
        let now = log::now();

        let prepared = match self.prepare(candidate) {
            Ok(prepared) => prepared,
            Err(refused) => {
                let failure = LoadFailure {
                    folder: candidate.folder.clone(),
                    source: candidate.source,
                    stage: refused.stage,
                    reason: refused.reason,
                };
                if keyable {
                    let row = app_row(
                        candidate,
                        refused.manifest.as_ref(),
                        None,
                        refused.manifest_hash,
                        existing.as_ref(),
                        Some(&failure.reason),
                        &now,
                    );
                    self.upsert_app_row(&candidate.folder, &row, existing.as_ref())?;
                }
                self.audit_load_failed(&failure, existing.as_ref())?;
                return Ok(Outcome::Refused(failure));
            }
        };

        // `§8`: upsert sys_app, then materialize.
        let row = app_row(
            candidate,
            Some(&prepared.manifest),
            Some(&prepared.schema.hash),
            Some(prepared.manifest_hash.clone()),
            existing.as_ref(),
            None,
            &now,
        );
        self.upsert_app_row(&candidate.folder, &row, existing.as_ref())?;
        if !row.enabled {
            return Ok(Outcome::Disabled);
        }

        match self.materialize_app(&prepared, cutoff) {
            Ok((log, store)) => Ok(Outcome::Loaded(Box::new(App {
                manifest: prepared.manifest,
                manifest_hash: prepared.manifest_hash,
                dir: candidate.dir.clone(),
                source: candidate.source,
                mount: prepared.mount,
                csp: prepared.csp,
                warnings: prepared.warnings,
                log,
                store,
            }))),
            Err(error) => {
                let failure = LoadFailure {
                    folder: candidate.folder.clone(),
                    source: candidate.source,
                    stage: Stage::Materialize,
                    reason: error.to_string(),
                };
                let failed = sys::AppRow {
                    last_error: Some(failure.reason.clone()),
                    ..row.clone()
                };
                self.upsert_app_row(&candidate.folder, &failed, Some(&row))?;
                self.audit_load_failed(&failure, existing.as_ref())?;
                Ok(Outcome::Refused(failure))
            }
        }
    }

    /// `§8` from "parse app.toml" through "compute CSP", touching no log and no cache.
    fn prepare(&self, candidate: &Candidate) -> std::result::Result<Prepared, Box<Refused>> {
        let refused = |stage: Stage, reason: String| {
            Box::new(Refused {
                stage,
                reason,
                manifest: None,
                manifest_hash: None,
            })
        };

        let path = candidate.dir.join(MANIFEST_FILE);
        let text = match fs::read_to_string(&path) {
            Ok(text) => text,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
                return Err(refused(
                    Stage::Parse,
                    format!(
                        "{MANIFEST_FILE}: not found — an app is a folder containing one \
                         (spec/app-contract.md §3)"
                    ),
                ));
            }
            Err(error) => {
                return Err(refused(Stage::Parse, format!("{MANIFEST_FILE}: {error}")));
            }
        };
        let manifest_hash = manifest::manifest_hash(&text);
        let manifest = Manifest::parse(&text).map_err(|error| {
            Box::new(Refused {
                manifest_hash: Some(manifest_hash.clone()),
                ..*refused(Stage::Parse, error.to_string())
            })
        })?;
        let refused_with = |stage: Stage, reason: String| {
            Box::new(Refused {
                stage,
                reason,
                manifest: Some(manifest.clone()),
                manifest_hash: Some(manifest_hash.clone()),
            })
        };

        manifest
            .validate(&candidate.folder, self.config.node.mode)
            .map_err(|error| refused_with(Stage::Validate, error.to_string()))?;

        // Tier check. `app.lua` is looked for and not loaded — the Lua host is M7.
        if let Some(file) = manifest.app.tier.required_file()
            && !candidate.dir.join(file).is_file()
        {
            let error = ManifestError::TierFileMissing {
                tier: manifest.app.tier,
                file,
            };
            return Err(refused_with(Stage::Tier, error.to_string()));
        }

        let schema = match fs::read_to_string(candidate.dir.join(SCHEMA_FILE)) {
            Ok(sql) if sql.trim().is_empty() => Schema::empty(),
            Ok(sql) => Schema::parse(&sql)
                .map_err(|error| refused_with(Stage::Schema, error.to_string()))?,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => Schema::empty(),
            Err(error) => {
                return Err(refused_with(
                    Stage::Schema,
                    format!("{SCHEMA_FILE}: {error}"),
                ));
            }
        };

        let slug = &manifest.app.slug;
        let mount = match (manifest.app.tier, self.config.node.mode) {
            (Tier::Rust, _) => None,
            (_, Mode::Host) => Some(format!("/a/{slug}/")),
            (_, Mode::Solo) => {
                (self.config.node.app.as_deref() == Some(slug.as_str())).then(|| "/".to_owned())
            }
        };
        let csp = Csp::for_app(mount.as_deref(), &manifest.permissions);

        let mut warnings: Vec<Warning> = manifest
            .permissions
            .widenings()
            .into_iter()
            .map(|widening| Warning::Permission {
                slug: slug.clone(),
                widening,
            })
            .collect();
        if manifest.nav.advertise && slug.len() > MAX_ADVERTISED_SLUG {
            warnings.push(Warning::SlugTooLongToAdvertise { slug: slug.clone() });
        }
        if let Some(icon) = &manifest.app.icon
            && !crate::icons::exists(icon)
        {
            warnings.push(Warning::UnknownIcon {
                slug: slug.clone(),
                icon: icon.clone(),
            });
        }
        // `§9.1`: the solo app owns `/`, so a top-level entry of its `web/` named after a
        // framework prefix is unreachable. Named here, once, rather than discovered by a 404.
        if mount.as_deref() == Some("/") && manifest.app.tier == Tier::Web {
            for route in shadowed_web_routes(&candidate.dir.join("web")) {
                let prefix = crate::wire::router::shadowing_prefix(&route).unwrap_or_default();
                warnings.push(Warning::RouteShadowed {
                    slug: slug.clone(),
                    route,
                    prefix,
                });
            }
        }

        Ok(Prepared {
            manifest,
            manifest_hash,
            schema,
            mount,
            csp,
            warnings,
        })
    }

    /// `§8`'s "materialize from data/<slug>/": the log, the three-tier restore, the
    /// health row — exactly what `Node::open` does for `_sys`, per app.
    fn materialize_app(&mut self, prepared: &Prepared, cutoff: &str) -> Result<(AppLog, Store)> {
        let slug = prepared.manifest.app.slug.as_str();
        for dir in [self.paths.app_log_dir(slug), self.paths.app_snap_dir(slug)] {
            fs::create_dir_all(&dir).map_err(io_at(&dir))?;
        }

        let (log, recovered) = AppLog::open(
            &self.paths,
            slug,
            self.identity.id(),
            Durability::Sync,
            &self.state,
        )?;
        audit_recovery(&mut self.sys, slug, &recovered)?;

        let previous = self
            .state
            .get(slug)
            .and_then(|record| record.materialized.restore.clone());
        let mut store =
            Store::open_with(&self.paths, slug, prepared.schema.clone()).map_err(boxed)?;
        if let Some(record) = self.state.get(slug) {
            store.restore_watermark(record.materialized.clone());
        }
        store.refresh(cutoff).map_err(boxed)?;
        if let Some(restored) = store.restored().cloned() {
            audit_restore(&mut self.sys, slug, &restored, previous.as_ref(), false)?;
        }
        note_health(&self.store, &store, slug)?;

        log.save_to(&mut self.state);
        store.save_to(&mut self.state);
        Ok((log, store))
    }

    /// Append the `sys_app` row if it says anything the current row does not, plus
    /// `app.installed` the first time the app loads cleanly. Returns whether it wrote.
    fn upsert_app_row(
        &mut self,
        slug: &str,
        row: &sys::AppRow,
        existing: Option<&sys::AppRow>,
    ) -> Result<bool> {
        if existing.is_some_and(|current| current.same_facts(row)) {
            return Ok(false);
        }
        let installed = row.installed_at.is_some()
            && existing.is_none_or(|current| current.installed_at.is_none());
        let at = log::now();
        let detail = serde_json::to_string(&serde_json::json!({
            "slug": slug,
            "source": row.source,
            "tier": row.tier,
            "version": row.version,
        }))?;
        self.sys.batch(|batch| {
            batch.put(sys::APP, slug, row)?;
            if installed {
                batch.put(
                    sys::AUDIT,
                    &new_ulid(),
                    &sys::AuditRow::info(&at, sys::KIND_APP_INSTALLED, Some(slug), &detail),
                )?;
            }
            Ok(())
        })?;
        Ok(true)
    }

    /// `app.load_failed`, bounded to transitions like `audit_restore`: written when the
    /// reason differs from the last one recorded — the row's `last_error` where the folder
    /// has a row, the newest audit for that subject where it cannot. A broken folder is
    /// loud once, not once per start.
    fn audit_load_failed(
        &mut self,
        failure: &LoadFailure,
        existing: Option<&sys::AppRow>,
    ) -> Result<bool> {
        let previous = match existing {
            Some(row) => row.last_error.clone(),
            None => self.last_load_failure_reason(&failure.folder)?,
        };
        if previous.as_deref() == Some(failure.reason.as_str()) {
            return Ok(false);
        }
        // The folder name and the root's kind, never a path: `sys_audit` replicates.
        let detail = serde_json::to_string(&serde_json::json!({
            "folder": failure.folder,
            "source": failure.source.as_str(),
            "stage": failure.stage.as_str(),
            "reason": failure.reason,
        }))?;
        let at = log::now();
        self.sys.put(
            sys::AUDIT,
            &new_ulid(),
            &sys::AuditRow::warn(
                &at,
                sys::KIND_APP_LOAD_FAILED,
                Some(&failure.folder),
                &detail,
            ),
        )?;
        Ok(true)
    }

    /// `§3.4`: a folder that is gone leaves its row, with `last_error = "folder missing"`.
    /// Returns whether the row changed.
    fn mark_folder_missing(&mut self, slug: &str) -> Result<bool> {
        let Some(existing) = self.read_app_row(slug)? else {
            return Ok(false);
        };
        if existing.last_error.as_deref() == Some(sys::FOLDER_MISSING) {
            return Ok(false);
        }
        let row = sys::AppRow {
            last_error: Some(sys::FOLDER_MISSING.to_owned()),
            updated_at: Some(log::now()),
            ..existing.clone()
        };
        self.upsert_app_row(slug, &row, Some(&existing))
    }

    /// The current `sys.sys_app` row, read back in the shape the writer uses so the two
    /// can be compared.
    fn read_app_row(&self, slug: &str) -> Result<Option<sys::AppRow>> {
        let sql = format!(
            "SELECT title, version, api, tier, icon, source, enabled, nav_order,
                    installed_at, updated_at,
                    schema_hash, manifest_hash, advertise, permissions, last_error
             FROM {} WHERE id = ?",
            sys::APP,
        );
        let row = self
            .store
            .conn()
            .query_row(&sql, rusqlite::params![slug], |row| {
                Ok(sys::AppRow {
                    title: row.get(0)?,
                    version: row.get(1)?,
                    api: row.get(2)?,
                    tier: row.get(3)?,
                    icon: row.get(4)?,
                    source: row.get::<_, Option<String>>(5)?.unwrap_or_default(),
                    enabled: row.get::<_, Option<bool>>(6)?.unwrap_or(true),
                    nav_order: row.get(7)?,
                    installed_at: row.get(8)?,
                    updated_at: row.get(9)?,
                    schema_hash: row.get(10)?,
                    manifest_hash: row.get(11)?,
                    advertise: row.get(12)?,
                    permissions: row.get(13)?,
                    last_error: row.get(14)?,
                })
            });
        match row {
            Ok(row) => Ok(Some(row)),
            Err(rusqlite::Error::QueryReturnedNoRows) => Ok(None),
            Err(error) => Err(boxed(StoreError::Sql(error))),
        }
    }

    /// Every slug in `sys.sys_app`.
    fn indexed_slugs(&self) -> Result<Vec<String>> {
        let sql = format!("SELECT id FROM {} ORDER BY id", sys::APP);
        let mut statement = self
            .store
            .conn()
            .prepare(&sql)
            .map_err(|error| boxed(StoreError::Sql(error)))?;
        let rows = statement
            .query_map([], |row| row.get::<_, String>(0))
            .map_err(|error| boxed(StoreError::Sql(error)))?;
        let mut slugs = Vec::new();
        for row in rows {
            slugs.push(row.map_err(|error| boxed(StoreError::Sql(error)))?);
        }
        Ok(slugs)
    }

    /// The reason in the newest `app.load_failed` audit row about `folder`, if any.
    fn last_load_failure_reason(&self, folder: &str) -> Result<Option<String>> {
        let sql = format!(
            "SELECT detail FROM {} WHERE kind = ? AND subject = ?
             ORDER BY \"at\" DESC, id DESC LIMIT 1",
            sys::AUDIT
        );
        let detail: Option<String> = match self.store.conn().query_row(
            &sql,
            rusqlite::params![sys::KIND_APP_LOAD_FAILED, folder],
            |row| row.get(0),
        ) {
            Ok(detail) => detail,
            Err(rusqlite::Error::QueryReturnedNoRows) => None,
            Err(error) => return Err(boxed(StoreError::Sql(error))),
        };
        Ok(detail
            .and_then(|text| serde_json::from_str::<serde_json::Value>(&text).ok())
            .and_then(|value| value.get("reason")?.as_str().map(str::to_owned)))
    }
}

/// The `sys_app` row for a folder, from whatever was learned about it.
///
/// `installed_at` is when the app first loaded cleanly, carried forward from the existing
/// row; a folder that has never loaded has none. `enabled` is the owner's and is carried
/// forward too (`§3.4`: uninstalling is an explicit owner action).
fn app_row(
    candidate: &Candidate,
    manifest: Option<&Manifest>,
    schema_hash: Option<&str>,
    manifest_hash: Option<String>,
    existing: Option<&sys::AppRow>,
    last_error: Option<&str>,
    now: &str,
) -> sys::AppRow {
    let installed_at = existing
        .and_then(|row| row.installed_at.clone())
        .or_else(|| last_error.is_none().then(|| now.to_owned()));
    sys::AppRow {
        title: manifest.map(|m| m.app.title.clone()),
        version: manifest.map(|m| m.app.version.clone()),
        api: manifest.and_then(|m| i32::try_from(m.app.api).ok()),
        tier: manifest.map(|m| m.app.tier.as_str().to_owned()),
        icon: manifest.and_then(|m| m.app.icon.clone()),
        source: candidate.source.as_str().to_owned(),
        enabled: existing.is_none_or(|row| row.enabled),
        nav_order: manifest.and_then(|m| m.nav.order),
        installed_at,
        updated_at: Some(now.to_owned()),
        schema_hash: schema_hash.map(str::to_owned),
        manifest_hash,
        advertise: manifest.map(|m| m.nav.advertise),
        permissions: manifest.map(|m| m.permissions.non_default_json()),
        last_error: last_error.map(str::to_owned),
    }
}

/// The top-level entries of a Tier 2 app's `web/` — files or directories — that a
/// framework prefix would shadow in solo mode, as routes (`/settings`, `/static`, …).
fn shadowed_web_routes(web: &Path) -> Vec<String> {
    let Ok(entries) = fs::read_dir(web) else {
        return Vec::new();
    };
    let mut routes: Vec<String> = entries
        .flatten()
        .map(|entry| format!("/{}", entry.file_name().to_string_lossy()))
        .filter(|route| crate::wire::router::shadowing_prefix(route).is_some())
        .collect();
    routes.sort();
    routes
}
