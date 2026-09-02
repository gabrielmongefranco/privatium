// Project:  Privatium™  |  File: crates/privatium-core/src/lib.rs
// Authors:  Gabriel Mongefranco (@gabrielmongefranco)
// Created:  2026-08-31  |  Modified: 2026-09-01
// Summary:  Crate root. The error type, the M0 linkage probe, `Node::open` — steps 1 to 4
//           of the bootstrap order in docs/plans/phase-1.md §2.6 — and the sink that turns
//           what a log scan found into sys_audit rows (spec/protocol.md §4.4).

//! Privatium core.
//!
//! The contract this crate implements is `spec/protocol.md` and `spec/app-contract.md`.
//! Neither is optional reading, and where this code and those documents disagree, they
//! are right and this is a bug.

use std::path::{Path, PathBuf};

use thiserror::Error;

pub mod config;
pub mod identity;
pub mod local;
pub mod log;
pub mod store;
pub mod sys;

pub use config::{Config, LuaConfig, Mode, NodeConfig, Paths};
pub use identity::{Identity, NodeId};
pub use log::{AppLog, Durability, Op};
pub use store::{Schema, Store, StoreError};

/// The protocol this build speaks (`spec/protocol.md §12`).
///
/// Not the `--version` string: `spec/cli.md §1` requires a build that does not satisfy
/// every item of `§13` to qualify what it prints, and Phase 1 does not. That
/// qualification belongs to the CLI (M11), not here — this constant is the wire format,
/// and the wire format really is `pv/1`.
pub const PROTOCOL: &str = "pv/1";

/// Anything that can go wrong opening or running a node.
///
/// One type rather than one per module: the binary, the shells, and eventually `uniffi`
/// all want a single thing to match on.
#[derive(Debug, Error)]
pub enum Error {
    /// The platform has no data directory and none was given.
    #[error("no platform data directory is available; pass an explicit data directory")]
    NoDataDir,

    /// A filesystem operation failed, named by the path it failed on.
    #[error("{path}: {source}")]
    Io {
        /// The file or directory involved.
        path: PathBuf,
        /// What the OS said.
        source: std::io::Error,
    },

    /// `config.toml` did not parse.
    #[error("{path}: {source}")]
    Config {
        /// The config file.
        path: PathBuf,
        /// What TOML said, including the offending key.
        ///
        /// Boxed because it is 88 bytes on its own, which made this the widest variant and
        /// put the whole enum at 128 — a cost every `Result` in the crate paid, on the
        /// success path too, for the rarest failure there is.
        source: Box<toml::de::Error>,
    },

    /// `config.toml` parsed but says something inconsistent.
    #[error("{path}: {problem}")]
    ConfigInvalid {
        /// The config file.
        path: PathBuf,
        /// What is wrong with it.
        problem: String,
    },

    /// `identity/node.key` is not an Ed25519 private key.
    #[error("{path}: expected a 32-byte Ed25519 private key, found {found} bytes")]
    KeyLength {
        /// The key file.
        path: PathBuf,
        /// How long it actually was.
        found: usize,
    },

    /// A log file that was expected to be new already exists.
    #[error("{path}: log file already exists")]
    LogExists {
        /// The log file.
        path: PathBuf,
    },

    /// A writer was aimed at a log file that is not this node's own.
    ///
    /// `AGENTS.md` 2 and `spec/protocol.md §3.1`: a device appends only to
    /// `data/<slug>/log/<its-own-node-id>.jsonl`. The one exception is `§10.2`'s sync
    /// receiver, which does not exist until Phase 3 and will not come through `log::Writer`.
    #[error("{path}: not this node's log — expected app {app} and device {dev}")]
    LogNotOurs {
        /// The file that was refused.
        path: PathBuf,
        /// The app the writer was for.
        app: String,
        /// The device the writer was for.
        dev: String,
    },

    /// A log file ends in the middle of a line.
    ///
    /// What a crash during an append leaves behind. Reported, never repaired: `§3.1` forbids
    /// truncating a log file, and the byte offset is what makes the damage inspectable
    /// instead of merely fatal.
    #[error("{path}: incomplete line at byte {offset}; the log was not modified")]
    PartialLine {
        /// The segment.
        path: PathBuf,
        /// Where the incomplete line starts.
        offset: u64,
    },

    /// An event could not be serialized.
    #[error("serializing an event: {0}")]
    Serialize(#[from] serde_json::Error),

    /// A statically linked engine failed to answer (`AGENTS.md`, Language and stack).
    #[error(transparent)]
    Engine(#[from] EngineError),

    /// Maintaining an app's `cache/<slug>.duckdb` failed (`spec/protocol.md §4.5`).
    ///
    /// Boxed for the same reason `Config` is: `duckdb::Error` is wide, and every `Result`
    /// in the crate would otherwise pay for the rarest failure there is.
    #[error(transparent)]
    Store(#[from] Box<StoreError>),
}

/// The crate's result type.
pub type Result<T> = std::result::Result<T, Error>;

/// Attach a path to an [`std::io::Error`].
///
/// `map_err(io_at(&path))` reads better than a match at every call site, and an IO error
/// without the path it happened on is close to useless in a bug report.
pub(crate) fn io_at(path: &Path) -> impl FnOnce(std::io::Error) -> Error {
    move |source| Error::Io {
        path: path.to_path_buf(),
        source,
    }
}

/// One installation of the Privatium server (`spec/protocol.md §1`).
///
/// Opening a node is the bootstrap order of `docs/plans/phase-1.md §2.6`, and the order
/// is not negotiable: the framework's own `sys_device` row has to be written through the
/// same log an app would use, before a materialized `_sys` or an app loader exists to help.
/// M2 completed steps 1 to 3 of that order — the tree and the keypair, the `_sys` log with
/// its recovered `seq` and Lamport counter, and this node's two rows in it. M3 adds step 4,
/// materializing `_sys` into the DuckDB schema `sys`. Step 5, loading `apps/`, belongs to
/// M5: it comes after this returns and is not stubbed here.
#[derive(Debug)]
pub struct Node {
    paths: Paths,
    config: Config,
    identity: Identity,
    sys: AppLog,
    store: Store,
    state: local::State,
}

impl Node {
    /// Open — or on first run, create — the node rooted at `data_dir`.
    ///
    /// This is the signature `spec/app-contract.md §2.3` gives embedded mode.
    pub fn open(data_dir: impl Into<PathBuf>) -> Result<Self> {
        Self::open_paths(Paths::rooted(data_dir))
    }

    /// Open using the two global flags of `spec/cli.md §1`, either of which may be absent.
    ///
    /// `--data-dir` defaults to the platform data directory and `--config` to
    /// `config.toml` inside it.
    pub fn open_with(data_dir: Option<&Path>, config: Option<&Path>) -> Result<Self> {
        Self::open_paths(Paths::resolve(data_dir, config)?)
    }

    fn open_paths(paths: Paths) -> Result<Self> {
        // 1. The tree, the keypair, and the Node ID derived from it.
        paths.create_tree()?;
        let identity = Identity::load_or_create(&paths.identity_dir())?;
        let config = Config::load(paths.config_file())?;

        // 2. Node-local state. A cache, and nothing below may depend on it for correctness:
        //    it makes the scan in step 3 cheap and keeps the Lamport counter monotonic if a
        //    log file is ever replaced by an older copy.
        let mut state = local::State::load(&paths.local_state())?;

        // 3. The _sys log. Opening it is what recovers `seq` and `lam` (§4.1, §4.3) and
        //    what applies §4.4 to everything already on disk.
        let (mut sys, recovered) =
            AppLog::open(&paths, sys::SLUG, identity.id(), Durability::Sync, &state)?;

        // 4. First run: this node's two rows about itself, in one batch.
        //
        //    Guarded on the log having recovered no events, not on the file existing. A
        //    crash between creating the file and writing the first line leaves a zero-byte
        //    log, and a file-existence guard would see it, conclude this was not a first
        //    run, and skip the bootstrap forever — a node with an identity and no
        //    `sys_device` row, with nothing to notice it by.
        if sys.seq() == 0 {
            bootstrap_sys(&mut sys, &identity)?;
        }

        // 5. Anything step 3 found that the owner is entitled to hear about. After the
        //    bootstrap, because an audit row cannot be written to a log that has no node
        //    behind it yet — and on a first run there is nothing to report anyway.
        audit_recovery(&mut sys, sys::SLUG, &recovered)?;

        // 6. Step 4 of §2.6: materialize `_sys`. Everything above it had to happen first —
        //    the rows this replays are the ones step 4 just wrote — and step 5, loading
        //    `apps/`, is M5's and is deliberately absent.
        //
        //    The store is left **unsealed**. `spec/app-contract.md §7`'s sandbox is
        //    instance-wide rather than per-connection (see `store::Store`), so sealing
        //    here would cost M4 the privileged window its snapshots need, and nothing
        //    serves app SQL out of `_sys` in the first place. `Store::seal` is
        //    implemented and tested; the app loader in M5 is its first caller, because M5
        //    is where an app's store first exists.
        let mut store = Store::open(&paths, sys::SLUG, store::SYS_DDL).map_err(boxed)?;
        if let Some(record) = state.get(sys::SLUG) {
            store.restore_watermark(record.materialized.clone());
        }
        store.refresh(&store::cutoff_now()).map_err(boxed)?;

        // 7. Record what we now know.
        sys.save_to(&mut state);
        store.save_to(&mut state);
        state.flush()?;

        Ok(Self {
            paths,
            config,
            identity,
            sys,
            store,
            state,
        })
    }

    /// Write `local/state.jsonl` if anything has changed since it was last written.
    ///
    /// Worth calling after a run of appends and not worth worrying about if it is missed:
    /// the file is a cache, and everything in it is recoverable by reading the logs. A node
    /// that never flushes is a node that does a little more work at its next start.
    pub fn flush(&mut self) -> Result<()> {
        self.sys.save_to(&mut self.state);
        self.store.save_to(&mut self.state);
        self.state.flush()
    }

    /// Where this node's files are.
    #[must_use]
    pub fn paths(&self) -> &Paths {
        &self.paths
    }

    /// This node's configuration, with defaults filled in.
    #[must_use]
    pub fn config(&self) -> &Config {
        &self.config
    }

    /// This node's keypair and ID.
    #[must_use]
    pub fn identity(&self) -> &Identity {
        &self.identity
    }

    /// This node's ID — the `dev` of every event it writes.
    #[must_use]
    pub fn id(&self) -> &NodeId {
        self.identity.id()
    }

    /// The framework's own log (`docs/plans/phase-1.md §2.6`).
    ///
    /// `_sys` is an app and is written through the same log an app would use. It is not
    /// discoverable, not mountable, and not lintable, which is why the app loader will skip
    /// it and why it is reachable here instead.
    #[must_use]
    pub fn sys_log(&self) -> &AppLog {
        &self.sys
    }

    /// The framework's own log, for appending.
    pub fn sys_log_mut(&mut self) -> &mut AppLog {
        &mut self.sys
    }

    /// `_sys` materialized into the DuckDB schema `sys` (`spec/data-dictionary.md §3`).
    #[must_use]
    pub fn store(&self) -> &Store {
        &self.store
    }

    /// The same, for the framework's own maintenance of it.
    pub fn store_mut(&mut self) -> &mut Store {
        &mut self.store
    }
}

/// `StoreError` is boxed inside [`Error`]; this is the conversion at the call sites.
fn boxed(source: StoreError) -> Error {
    Error::Store(Box::new(source))
}

/// Write this node's `sys_device` and `sys_node` rows, once, on first run.
///
/// One batch, not two appends. `docs/plans/phase-1.md §2.6` says both rows describe one
/// event in the world — this node coming into existence — and a batch is how that is said
/// in the log: one `ts`, contiguous `seq`, and one `write_all`, so there is no window in
/// which a reader sees a device with no node behind it.
///
/// The order within is `sys_device` then `sys_node`, which is `§2.6`'s.
fn bootstrap_sys(sys_log: &mut AppLog, identity: &Identity) -> Result<()> {
    let id = identity.id().as_str().to_owned();
    let pubkey = identity.public_key_base64();

    sys_log.batch(|batch| {
        batch.put(sys::DEVICE, &id, &sys::DeviceRow::this_node())?;
        // `created_at` is the batch's own `ts`, so the row and the envelope agree about
        // when this node came into existence.
        let created_at = batch.ts().to_owned();
        batch.put(
            sys::NODE,
            &id,
            &sys::NodeRow::this_installation(&pubkey, &created_at),
        )
    })?;

    Ok(())
}

/// Turn what a log scan found into `sys_audit` rows (`spec/protocol.md §4.4`).
///
/// This lives on [`Node`] rather than on [`AppLog`] for one reason of order: `_sys` has to
/// be open before anything can be audited, and a log that reached into another log to
/// report itself would make that dependency impossible to see. Every app's diagnostics come
/// here, which is also why the app is named in the detail rather than assumed.
///
/// Rejections are reported **once**. The scan only offers an event whose `seq` is past the
/// head recorded in `local/state.jsonl`, so a bad line that has already been audited stays
/// in the log — `§3.1` forbids removing it — without producing a fresh row on every start.
fn audit_recovery(sys_log: &mut AppLog, app: &str, recovered: &log::Recovered) -> Result<()> {
    for rejected in &recovered.rejected {
        let detail = serde_json::to_string(&serde_json::json!({
            "app": app,
            "dev": rejected.dev,
            "seq": rejected.seq,
            "ts": rejected.ts,
            "ahead_secs": rejected.ahead_secs,
            // The file name, never the full path. `sys_audit` is replicated (§3.10), and
            // this node's data root is nobody else's business.
            "segment": file_name(&rejected.segment),
            "offset": rejected.offset,
        }))?;
        let at = log::now();
        sys_log.put(
            sys::AUDIT,
            &new_ulid(),
            &sys::AuditRow::warn(
                &at,
                sys::KIND_EVENT_REJECTED,
                Some(rejected.dev.as_str()),
                &detail,
            ),
        )?;
    }

    if let Some(skew) = &recovered.skew {
        let detail = serde_json::to_string(&serde_json::json!({
            "app": app,
            "tail_ts": skew.tail_ts,
            "behind_secs": skew.behind_secs,
        }))?;
        let at = log::now();
        // No subject. §3.10's subject is a device, app, or snapshot; this is about the
        // node's own clock, and `actor: system` already says whose.
        sys_log.put(
            sys::AUDIT,
            &new_ulid(),
            &sys::AuditRow::warn(&at, sys::KIND_CLOCK_SKEW, None, &detail),
        )?;
    }

    Ok(())
}

/// A fresh ULID, Crockford Base32, 26 characters (`spec/protocol.md §4.1`).
fn new_ulid() -> String {
    ulid::Ulid::generate().to_string()
}

/// The last component of a path, for a message that must not carry a filesystem layout.
fn file_name(path: &Path) -> String {
    path.file_name()
        .map(|name| name.to_string_lossy().into_owned())
        .unwrap_or_default()
}

/// Versions of the two foreign engines statically linked into this build.
///
/// Reported by `privatium` so the number CI prints for binary size is the size of a
/// binary that genuinely contains both engines, rather than one where the linker
/// discarded them as unreferenced.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LinkedEngines {
    /// DuckDB's own `version()`, e.g. `v1.5.1`.
    pub duckdb: String,
    /// Lua's `_VERSION`, which must be `Lua 5.4` — not LuaJIT, not Luau (`AGENTS.md`).
    pub lua: String,
}

/// Failures of the linkage probe.
#[derive(Debug, Error)]
pub enum EngineError {
    /// DuckDB linked but did not answer.
    #[error("bundled DuckDB failed to answer version(): {0}")]
    DuckDb(#[from] duckdb::Error),
    /// Lua linked but did not answer.
    #[error("vendored Lua failed to answer _VERSION: {0}")]
    Lua(#[from] mlua::Error),
}

/// Open an in-memory DuckDB and a fresh Lua state, and ask each its version.
///
/// This exists to fail loudly on any platform where the bundled C++ or vendored C build is
/// broken, rather than at M3 or M7 when there is real code to blame it on.
pub fn linked_engines() -> std::result::Result<LinkedEngines, EngineError> {
    let conn = duckdb::Connection::open_in_memory()?;
    let duckdb: String = conn.query_row("SELECT version()", [], |row| row.get(0))?;

    let lua = mlua::Lua::new();
    let lua: String = lua.load("return _VERSION").eval()?;

    Ok(LinkedEngines { duckdb, lua })
}
