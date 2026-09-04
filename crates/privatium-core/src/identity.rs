// Project:  Privatium™  |  File: crates/privatium-core/src/identity.rs
// Authors:  Gabriel Mongefranco (@gabrielmongefranco)
// Created:  2026-09-01  |  Modified: 2026-09-05
// Summary:  The node's Ed25519 keypair, the Node ID derived from it, and the CSRF key derived
//           from it (spec/protocol.md §2.1, docs/plans/phase-1.md §2.2). First run generates
//           the pair; every run after loads it, and both derivations are pure.

use std::fmt;
use std::fs;
use std::io::Write as _;
use std::path::Path;

use ed25519_dalek::{SigningKey, VerifyingKey};
use sha2::{Digest, Sha256};
use zeroize::Zeroize as _;

use crate::{Error, Result, io_at};

/// Crockford Base32, lowercase.
///
/// `i`, `l`, `o`, and `u` are absent by design: the first three are visually ambiguous
/// with `1` and `0`, and dropping `u` keeps accidental profanity out of an identifier the
/// owner is expected to read off a screen.
const CROCKFORD: &[u8; 32] = b"0123456789abcdefghjkmnpqrstvwxyz";

/// An Ed25519 private key is 32 bytes, and `identity/node.key` holds exactly those bytes
/// and nothing else — no PEM armour, no PKCS#8 wrapper.
const KEY_LEN: usize = 32;

/// `spec/protocol.md §2.1` takes the first 40 bits of the digest, which is 8 groups of 5.
const ID_CHARS: usize = 8;

/// A Node ID: the first 40 bits of `SHA-256(public_key)` rendered as 8 characters of
/// lowercase Crockford Base32 (`spec/protocol.md §2.1`).
///
/// This is also the `dev` field of every event this node writes and the filename of its
/// log files, which is why it is a type and not a `String`: `§4.1` requires `dev` to equal
/// the log filename, and the cheapest way to guarantee that is to have one value.
///
/// Other nodes' IDs are opaque (`§2.1`). Nothing here interprets one.
#[derive(Debug, Clone, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct NodeId(String);

impl NodeId {
    /// Derive the Node ID of a public key.
    ///
    /// Pure and total: the same key yields the same ID on every platform and every run,
    /// which is what `test_spec_2_1_node_id_derivation` pins against a checked-in key.
    #[must_use]
    pub fn derive(public_key: &VerifyingKey) -> Self {
        let digest = Sha256::digest(public_key.as_bytes());

        // The first 40 bits, most significant first. Five bytes fit in a u64 with room to
        // spare, so the groups can be sliced out by shifting rather than by tracking a
        // bit cursor across a byte boundary.
        let mut bits: u64 = 0;
        for byte in &digest[..5] {
            bits = (bits << 8) | u64::from(*byte);
        }

        let mut id = String::with_capacity(ID_CHARS);
        for group in (0..ID_CHARS).rev() {
            let index = ((bits >> (group * 5)) & 0b1_1111) as usize;
            id.push(char::from(CROCKFORD[index]));
        }

        Self(id)
    }

    /// The ID as it appears in `dev`, in a log filename, and in `sys_node.id`.
    #[must_use]
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl fmt::Display for NodeId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

/// This node's identity: its keypair and the ID derived from it.
///
/// Phase 1 has no cluster identity. `identity/cluster.key`, `identity/cluster.pub`, and
/// `identity/node.cert` are absent, and that is a valid state rather than a missing step
/// (`docs/plans/phase-1.md §1`). Nothing here should grow a placeholder for them.
pub struct Identity {
    signing: SigningKey,
    id: NodeId,
}

impl Identity {
    /// Load `identity/node.key`, generating a keypair on first run.
    ///
    /// `identity/node.pub` is a convenience — the public key is derivable from the private
    /// one — so a missing or deleted `node.pub` is rewritten rather than treated as an
    /// error. `node.key` is the only file whose absence means "first run".
    pub fn load_or_create(dir: &Path) -> Result<Self> {
        let key_path = dir.join("node.key");

        let signing = if key_path.exists() {
            load_signing_key(&key_path)?
        } else {
            let signing = SigningKey::generate(&mut rand::rng());
            write_private(&key_path, signing.as_bytes())?;
            signing
        };

        let public_path = dir.join("node.pub");
        if !public_path.exists() {
            let public = signing.verifying_key();
            fs::write(&public_path, public.as_bytes()).map_err(io_at(&public_path))?;
        }

        let id = NodeId::derive(&signing.verifying_key());
        Ok(Self { signing, id })
    }

    /// This node's ID.
    #[must_use]
    pub fn id(&self) -> &NodeId {
        &self.id
    }

    /// This node's public key.
    #[must_use]
    pub fn verifying_key(&self) -> VerifyingKey {
        self.signing.verifying_key()
    }

    /// The public key as base64, which is how `sys_node.pubkey` carries it
    /// (`spec/data-dictionary.md §3.1`).
    #[must_use]
    pub fn public_key_base64(&self) -> String {
        use base64::Engine as _;
        base64::engine::general_purpose::STANDARD.encode(self.verifying_key().as_bytes())
    }
}

/// A key derived from the node key for one purpose, wiped on drop.
///
/// Only ever produced by [`Identity::csrf_key`]; the bytes are readable so the HMAC in
/// `http::csrf` can be keyed with them, and nothing else has a reason to look.
pub struct DerivedKey(zeroize::Zeroizing<[u8; KEY_LEN]>);

impl DerivedKey {
    /// The key bytes.
    #[must_use]
    pub fn as_bytes(&self) -> &[u8] {
        self.0.as_ref()
    }
}

impl fmt::Debug for DerivedKey {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("DerivedKey(..)")
    }
}

/// `docs/plans/phase-1.md §2.2`: the HKDF `info` that names the CSRF purpose. A second
/// purpose gets a second string, never a reuse of this one.
const CSRF_INFO: &[u8] = b"privatium/csrf/v1";

impl Identity {
    /// The CSRF key of `docs/plans/phase-1.md §2.2`:
    /// `HKDF-SHA256(ikm = node private key, info = "privatium/csrf/v1")`, no salt.
    ///
    /// Derived, never stored: `AGENTS.md` 5 keeps secrets out of `data/`, and
    /// `spec/protocol.md §3` gives `local/` exactly one file. This is a method here so the
    /// private key itself never leaves this module — callers get a purpose-bound key and
    /// nothing they could sign with.
    #[must_use]
    pub fn csrf_key(&self) -> DerivedKey {
        let hk = hkdf::Hkdf::<Sha256>::new(None, self.signing.as_bytes());
        let mut okm = zeroize::Zeroizing::new([0u8; KEY_LEN]);
        // `expand` fails only when the output is longer than 255 hash lengths; 32 bytes is
        // not, so the empty branch is unreachable rather than an error to report.
        if hk.expand(CSRF_INFO, okm.as_mut()).is_err() {
            okm.as_mut().fill(0);
        }
        DerivedKey(okm)
    }
}

impl fmt::Debug for Identity {
    /// Hand-written so that a `{:?}` of anything holding an `Identity` cannot print key
    /// material.
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Identity")
            .field("id", &self.id)
            .finish_non_exhaustive()
    }
}

/// Read a 32-byte private key, wiping every intermediate copy.
///
/// `SigningKey` zeroizes itself on drop; these buffers are ours, so they are wiped here.
fn load_signing_key(path: &Path) -> Result<SigningKey> {
    let mut raw = fs::read(path).map_err(io_at(path))?;

    if raw.len() != KEY_LEN {
        let found = raw.len();
        raw.zeroize();
        return Err(Error::KeyLength {
            path: path.to_path_buf(),
            found,
        });
    }

    let mut bytes = [0u8; KEY_LEN];
    bytes.copy_from_slice(&raw);
    raw.zeroize();

    let signing = SigningKey::from_bytes(&bytes);
    bytes.zeroize();
    Ok(signing)
}

/// Write `identity/node.key` with mode `0600` (`spec/protocol.md §2.1`).
///
/// The mode is set in the `open` call rather than by a `set_permissions` afterwards: the
/// two-step version leaves a window in which the key exists and is world-readable.
/// `create_new` means this can never overwrite an existing key.
///
/// Windows has no mode. The file inherits the ACL of its parent, which under
/// `%LOCALAPPDATA%` is already restricted to the owning user; there is no cross-platform
/// equivalent to set, and inventing one would be worse than relying on that.
fn write_private(path: &Path, bytes: &[u8]) -> Result<()> {
    let mut options = fs::OpenOptions::new();
    options.write(true).create_new(true);

    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt as _;
        options.mode(0o600);
    }

    let mut file = options.open(path).map_err(io_at(path))?;
    file.write_all(bytes).map_err(io_at(path))?;
    file.sync_all().map_err(io_at(path))?;
    // The key's *name* has to survive a power cut too, or a first run that flushed the
    // bytes can come back without a key and mint a second identity.
    if let Some(dir) = path.parent() {
        crate::durable::sync_dir(dir).map_err(io_at(dir))?;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A typo in the alphabet would not fail any other test — the ID would simply be
    /// wrong forever — so the table itself is checked.
    #[test]
    fn crockford_alphabet_excludes_ambiguous_letters() {
        assert_eq!(CROCKFORD.len(), 32);
        for ambiguous in [b'i', b'l', b'o', b'u'] {
            assert!(!CROCKFORD.contains(&ambiguous));
        }
    }

    /// The derivation reads 40 bits most-significant-first. Pinning both ends of the
    /// alphabet at once would catch a reversed or off-by-one shift.
    #[test]
    fn five_bit_groups_are_most_significant_first() {
        let bits: u64 = 0b11111_00000_00000_00000_00000_00000_00000_00001;
        let mut id = String::new();
        for group in (0..ID_CHARS).rev() {
            let index = ((bits >> (group * 5)) & 0b1_1111) as usize;
            id.push(char::from(CROCKFORD[index]));
        }
        assert_eq!(id, "z0000001");
    }
}
