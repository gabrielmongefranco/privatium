// Project:  Privatium™  |  File: crates/privatium-core/src/config.rs
// Authors:  Gabriel Mongefranco (@gabrielmongefranco)
// Created:  2026-09-01  |  Modified: 2026-09-05
// Summary:  Where the node's data lives (spec/protocol.md §3) and what config.toml may
//           say about it. Both halves are here because --data-dir picks the root and
//           --config defaults to a file inside it, so neither resolves without the other.

use std::fs;
use std::path::{Path, PathBuf};

use directories::BaseDirs;
use serde::Deserialize;

use crate::{Error, Result, io_at};

/// `spec/cli.md §2`. Ports below 1024 are never used (`AGENTS.md` 8).
pub const DEFAULT_PORT: u16 = 8420;

/// The XDG directory name, which is also the crate name, the binary name, and the mDNS
/// service type (`docs/naming.md`).
const DIR_NAME: &str = "privatium";

/// Every path under the node's data root, resolved once.
///
/// `spec/protocol.md §3` fixes this layout. It is reproduced here as accessors rather than
/// as string concatenation at call sites so that a rename is one edit, and so that no
/// caller can invent a directory the spec does not have.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Paths {
    root: PathBuf,
    config: PathBuf,
}

impl Paths {
    /// Resolve from the two CLI flags of `spec/cli.md §1`, either of which may be absent.
    ///
    /// `--data-dir` defaults to the platform data directory; `--config` defaults to
    /// `config.toml` inside whatever root was chosen.
    pub fn resolve(data_dir: Option<&Path>, config: Option<&Path>) -> Result<Self> {
        let root = match data_dir {
            Some(explicit) => explicit.to_path_buf(),
            None => default_root()?,
        };
        Ok(Self {
            config: config.map_or_else(|| root.join("config.toml"), Path::to_path_buf),
            root,
        })
    }

    /// Resolve against an explicit root, with the default config location inside it.
    #[must_use]
    pub fn rooted(root: impl Into<PathBuf>) -> Self {
        let root = root.into();
        Self {
            config: root.join("config.toml"),
            root,
        }
    }

    /// The data root. Every other path here is below it.
    #[must_use]
    pub fn root(&self) -> &Path {
        &self.root
    }

    /// `config.toml`. May be outside the root if `--config` said so.
    #[must_use]
    pub fn config_file(&self) -> &Path {
        &self.config
    }

    /// `identity/` — the keypair. Never synced, never in `data/`.
    #[must_use]
    pub fn identity_dir(&self) -> PathBuf {
        self.root.join("identity")
    }

    /// `identity/node.key`, mode `0600` (`spec/protocol.md §2.1`).
    #[must_use]
    pub fn node_key(&self) -> PathBuf {
        self.identity_dir().join("node.key")
    }

    /// `identity/node.pub`.
    #[must_use]
    pub fn node_pub(&self) -> PathBuf {
        self.identity_dir().join("node.pub")
    }

    /// `apps/` — owner-installed app folders.
    #[must_use]
    pub fn apps_dir(&self) -> PathBuf {
        self.root.join("apps")
    }

    /// `data/` — the only thing that must be backed up.
    #[must_use]
    pub fn data_dir(&self) -> PathBuf {
        self.root.join("data")
    }

    /// `data/<slug>/log/` — every log file of one app, this node's and its peers'.
    #[must_use]
    pub fn app_log_dir(&self, slug: &str) -> PathBuf {
        self.data_dir().join(slug).join("log")
    }

    /// `data/<slug>/log/<dev>.jsonl`.
    ///
    /// `§4.1` requires the `dev` field of every line to equal the filename, so this takes
    /// the same [`NodeId`](crate::NodeId) the writer stamps into the envelope.
    #[must_use]
    pub fn app_log(&self, slug: &str, dev: &crate::NodeId) -> PathBuf {
        self.app_log_dir(slug).join(format!("{dev}.jsonl"))
    }

    /// `data/<slug>/snap/`.
    #[must_use]
    pub fn app_snap_dir(&self, slug: &str) -> PathBuf {
        self.data_dir().join(slug).join("snap")
    }

    /// `local/` — node-local state. Never synced, not required for restore.
    #[must_use]
    pub fn local_dir(&self) -> PathBuf {
        self.root.join("local")
    }

    /// `local/state.jsonl`. Not created by M1, which writes no local state.
    #[must_use]
    pub fn local_state(&self) -> PathBuf {
        self.local_dir().join("state.jsonl")
    }

    /// `local/lock` — held exclusively by the process that has this root open
    /// (`spec/protocol.md §3.1`, [`DataLock`](crate::lock::DataLock)).
    #[must_use]
    pub fn local_lock(&self) -> PathBuf {
        self.local_dir().join(crate::lock::LOCK_FILE)
    }

    /// `cache/` — fully disposable. Deleting it must lose zero data.
    #[must_use]
    pub fn cache_dir(&self) -> PathBuf {
        self.root.join("cache")
    }

    /// `cache/<slug>.sqlite`.
    #[must_use]
    pub fn app_cache_db(&self, slug: &str) -> PathBuf {
        self.cache_dir().join(format!("{slug}.sqlite"))
    }

    /// Create the directory tree of `spec/protocol.md §3`, and nothing else.
    ///
    /// Idempotent, so it runs on every start rather than only on the first. Files are not
    /// created here: `config.toml` is optional and recreatable
    /// (`docs/backup-and-restore.md §1`), and `local/state.jsonl` appears when something
    /// first has local state to record. Creating either as an empty placeholder would
    /// claim a first run had produced something it had not.
    ///
    /// Per-app `data/<slug>/` directories belong to the app loader. `_sys` is the one
    /// exception, because the framework's own log must exist before any app is read
    /// (`docs/plans/phase-1.md §2.6`).
    pub fn create_tree(&self) -> Result<()> {
        create_dir(&self.root)?;
        create_private_dir(&self.identity_dir())?;
        create_dir(&self.apps_dir())?;
        create_dir(&self.data_dir())?;
        create_dir(&self.app_log_dir(crate::sys::SLUG))?;
        create_dir(&self.app_snap_dir(crate::sys::SLUG))?;
        create_dir(&self.local_dir())?;
        create_dir(&self.cache_dir())?;
        Ok(())
    }
}

/// The platform data directory, plus `privatium`.
///
/// `spec/protocol.md §3` pins Linux exactly — `$XDG_DATA_HOME/privatium`, falling back to
/// `~/.local/share/privatium` — and says "the platform equivalent elsewhere".
///
/// This is `data_local_dir`, not `data_dir`, and the difference matters on exactly one
/// platform. On Linux and macOS the two are the same path. On Windows `data_dir` is
/// `%APPDATA%`, which roams: a domain profile would copy `identity/node.key` to a second
/// machine, giving two machines one Node ID and two writers on one log file, against
/// `§3.1` and `AGENTS.md` 2. `%LOCALAPPDATA%` does not roam.
fn default_root() -> Result<PathBuf> {
    let base = BaseDirs::new().ok_or(Error::NoDataDir)?;
    Ok(base.data_local_dir().join(DIR_NAME))
}

fn create_dir(path: &Path) -> Result<()> {
    fs::create_dir_all(path).map_err(io_at(path))
}

/// `identity/` holds the node's private key, so it is `0700` where that means anything.
///
/// `spec/protocol.md §2.1` pins the mode of `node.key` and says nothing about its
/// directory; this is defence in depth around that requirement, and matches
/// `docs/backup-and-restore.md §1`, which tells owners to back `identity/` up
/// "separately and privately".
fn create_private_dir(path: &Path) -> Result<()> {
    let mut builder = fs::DirBuilder::new();
    builder.recursive(true);

    #[cfg(unix)]
    {
        use std::os::unix::fs::DirBuilderExt as _;
        builder.mode(0o700);
    }

    builder.create(path).map_err(io_at(path))
}

/// `config.toml` (`spec/app-contract.md §2`, `spec/lua-api.md §5`).
///
/// Every key has a default and the whole file is optional, so a node starts with no
/// configuration at all. Unknown keys are rejected rather than ignored: a mistyped `prot`
/// that silently leaves the node on 8420 is a worse failure than one that refuses to
/// start and names the key. The cost is that a config written by a later version will not
/// load here, which is worth revisiting once the config surface grows past these two
/// tables.
#[derive(Debug, Clone, Default, PartialEq, Eq, Deserialize)]
#[serde(deny_unknown_fields, default)]
pub struct Config {
    /// `[node]`.
    pub node: NodeConfig,
    /// `[lua]`.
    pub lua: LuaConfig,
}

impl Config {
    /// Load `config.toml`, or return the defaults if it is absent.
    pub fn load(path: &Path) -> Result<Self> {
        let text = match fs::read_to_string(path) {
            Ok(text) => text,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
                return Ok(Self::default());
            }
            Err(error) => return Err(io_at(path)(error)),
        };

        let config: Self = toml::from_str(&text).map_err(|source| Error::Config {
            path: path.to_path_buf(),
            source: Box::new(source),
        })?;
        config.validate(path)?;
        Ok(config)
    }

    /// The one way this file can be internally inconsistent.
    ///
    /// Whether the named app exists, whether its slug is well formed, and whether it is a
    /// reserved slug (`spec/protocol.md §1.1`) are all questions for the app loader, which
    /// is the only thing that can answer them.
    fn validate(&self, path: &Path) -> Result<()> {
        if self.node.mode == Mode::Solo && self.node.app.is_none() {
            return Err(Error::ConfigInvalid {
                path: path.to_path_buf(),
                problem: "[node] mode = \"solo\" requires [node] app = \"<slug>\"".into(),
            });
        }
        Ok(())
    }
}

/// `[node]`.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
#[serde(deny_unknown_fields, default)]
pub struct NodeConfig {
    /// Host or solo (`spec/app-contract.md §2`). Embedded mode is a property of the
    /// program that links this crate, not a value this file can hold.
    pub mode: Mode,
    /// The single app, in solo mode.
    pub app: Option<String>,
    /// HTTP port. `spec/cli.md §2` defaults it to 8420; `--port` overrides for one run.
    pub port: u16,
}

impl Default for NodeConfig {
    fn default() -> Self {
        Self {
            mode: Mode::Host,
            app: None,
            port: DEFAULT_PORT,
        }
    }
}

/// `[node] mode` (`spec/app-contract.md §2.1`, `§2.2`).
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Mode {
    /// Many apps at `/a/<slug>/`, with a launcher at `/`.
    #[default]
    Host,
    /// One app at `/`, no launcher and no prefix.
    Solo,
}

/// `[lua]` — the resource limits of `spec/lua-api.md §5`, all of which are REQUIRED.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
#[serde(deny_unknown_fields, default)]
pub struct LuaConfig {
    /// Instruction count per request.
    pub max_instructions: u64,
    /// Memory per VM, in megabytes.
    pub max_memory_mb: u64,
    /// Wall clock per request, in seconds.
    pub max_seconds: u64,
    /// VM pool size. Defaults to the CPU count.
    pub pool_size: usize,
}

impl Default for LuaConfig {
    fn default() -> Self {
        Self {
            max_instructions: 50_000_000,
            max_memory_mb: 64,
            max_seconds: 5,
            pool_size: std::thread::available_parallelism().map_or(1, |cpus| cpus.get()),
        }
    }
}

// AGENTS.md, Style: unwrap() is permitted in tests. The crate-level deny reaches unit
// tests inside src/, so each one opts out where it is declared.
#[allow(clippy::unwrap_used, clippy::expect_used)]
#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn defaults_match_the_spec() {
        let config = Config::default();
        assert_eq!(config.node.mode, Mode::Host);
        assert_eq!(config.node.port, 8420);
        assert_eq!(config.lua.max_instructions, 50_000_000);
        assert_eq!(config.lua.max_memory_mb, 64);
        assert_eq!(config.lua.max_seconds, 5);
        assert!(config.lua.pool_size >= 1);
    }

    #[test]
    fn a_partial_table_keeps_the_other_defaults() {
        let config: Config = toml::from_str("[node]\nport = 9000\n").unwrap();
        assert_eq!(config.node.port, 9000);
        assert_eq!(config.node.mode, Mode::Host);
        assert_eq!(config.lua.max_memory_mb, 64);
    }

    #[test]
    fn solo_mode_parses_the_app_contract_example() {
        let config: Config =
            toml::from_str("[node]\nmode = \"solo\"\napp  = \"medtracker\"\n").unwrap();
        assert_eq!(config.node.mode, Mode::Solo);
        assert_eq!(config.node.app.as_deref(), Some("medtracker"));
    }

    #[test]
    fn an_unknown_key_is_an_error_and_names_itself() {
        let error = toml::from_str::<Config>("[node]\nprot = 8421\n").unwrap_err();
        assert!(error.to_string().contains("prot"), "{error}");
    }

    #[test]
    fn an_unknown_table_is_an_error() {
        assert!(toml::from_str::<Config>("[discovery]\nmdns = true\n").is_err());
    }

    #[test]
    fn paths_follow_the_section_3_layout() {
        let paths = Paths::rooted("/tmp/root");
        let id = crate::NodeId::derive(
            &ed25519_dalek::SigningKey::from_bytes(&[7u8; 32]).verifying_key(),
        );

        assert!(paths.node_key().ends_with("identity/node.key"));
        assert!(paths.app_snap_dir("hello").ends_with("data/hello/snap"));
        assert!(paths.local_state().ends_with("local/state.jsonl"));
        assert!(paths.app_cache_db("hello").ends_with("cache/hello.sqlite"));
        assert!(
            paths
                .app_log("hello", &id)
                .ends_with(format!("data/hello/log/{id}.jsonl"))
        );
    }
}
