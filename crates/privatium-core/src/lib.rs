// Project:  Privatium™  |  File: crates/privatium-core/src/lib.rs
// Authors:  Gabriel Mongefranco (@gabrielmongefranco)
// Created:  2026-08-31  |  Modified: 2026-09-01
// Summary:  Crate root. The error type, the M0 linkage probe, and `Node::open` — which
//           at M1 is the first three steps of the bootstrap order in
//           docs/plans/phase-1.md §2.6 and stops there.

//! Privatium core.
//!
//! The contract this crate implements is `spec/protocol.md` and `spec/app-contract.md`.
//! Neither is optional reading, and where this code and those documents disagree, they
//! are right and this is a bug.

use std::path::{Path, PathBuf};

use thiserror::Error;

pub mod config;
pub mod identity;
pub mod log;
pub mod sys;

pub use config::{Config, LuaConfig, Mode, NodeConfig, Paths};
pub use identity::{Identity, NodeId};

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
        source: toml::de::Error,
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

    /// An event could not be serialized.
    #[error("serializing an event: {0}")]
    Serialize(#[from] serde_json::Error),

    /// A statically linked engine failed to answer (`AGENTS.md`, Language and stack).
    #[error(transparent)]
    Engine(#[from] EngineError),
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
/// same log an app would use, before a Lamport counter, a materialized `_sys`, or an app
/// loader exists to help. M1 is steps 1 to 3. Materializing `_sys` is M3 and loading
/// `apps/` is M5; both come after this returns, and neither is stubbed here.
#[derive(Debug)]
pub struct Node {
    paths: Paths,
    config: Config,
    identity: Identity,
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

        // 2 and 3. The _sys log, and this node's two rows in it.
        bootstrap_sys(&paths, &identity)?;

        Ok(Self {
            paths,
            config,
            identity,
        })
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
}

/// Write this node's `sys_device` and `sys_node` rows, once, on first run.
///
/// Guarded on the log file rather than on the keypair. A crash between generating
/// `identity/node.key` and appending these two events would otherwise leave a node with an
/// identity, no `sys_device` row, and no way to notice — the next start would load the key,
/// conclude it was not a first run, and skip this forever.
///
/// The order within is `sys_device` then `sys_node`, which is `§2.6`'s. Both carry the
/// same `ts`, because they describe one event in the world: this node coming into
/// existence.
fn bootstrap_sys(paths: &Paths, identity: &Identity) -> Result<()> {
    let path = paths.app_log(sys::SLUG, identity.id());
    if path.exists() {
        return Ok(());
    }

    let mut writer = log::Writer::create(path, sys::SLUG, identity.id())?;
    let ts = log::now();
    let id = identity.id().as_str();

    writer.put(sys::DEVICE, id, &ts, &sys::DeviceRow::this_node())?;

    let pubkey = identity.public_key_base64();
    writer.put(
        sys::NODE,
        id,
        &ts,
        &sys::NodeRow::this_installation(&pubkey, &ts),
    )?;

    Ok(())
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
