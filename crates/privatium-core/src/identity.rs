// Project:  Privatium™  |  File: crates/privatium-core/src/identity.rs
// Authors:  Gabriel Mongefranco (@gabrielmongefranco)
// Created:  2026-09-01  |  Modified: 2026-09-05
// Summary:  Node and cluster keys, canonical membership certificates, startup renewal,
//           and purpose-separated CSRF and X25519 derivations (spec/protocol.md §2, §8).

use std::fmt;
use std::fs;
use std::io::{Read as _, Write as _};
use std::path::Path;

use base64::{Engine as _, engine::general_purpose::STANDARD};
use ed25519_dalek::{Signature, Signer as _, SigningKey, VerifyingKey};
use serde::{Deserialize, Serialize};
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

/// A certificate has six short fields. Bound decoding before allocating from file input.
const MAX_CERT_BYTES: u64 = 4096;

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

/// This node's signing identity and cluster membership (`spec/protocol.md §2.3`).
pub struct Identity {
    signing: SigningKey,
    id: NodeId,
    cluster: SigningKey,
    cluster_id: ClusterId,
    certificate: Certificate,
    renewed: bool,
}

impl Identity {
    /// Load node and cluster identities using the current UTC clock.
    /// Requires exclusive ownership of `dir`; founds absent identities and renews valid
    /// certificates near expiry. Refuses malformed or expired material and I/O failures.
    pub fn load_or_create(dir: &Path) -> Result<Self> {
        Self::load_or_create_at(dir, jiff::Timestamp::now())
    }

    /// Load or found an identity in `dir`, evaluating certificate validity at `now`.
    /// The caller must exclusively own the directory. Writes keys and public files on
    /// first use, and renews a valid certificate with fewer than ninety days left.
    /// Refuses malformed keys, invalid or expired certificates, and filesystem failures.
    pub fn load_or_create_at(dir: &Path, now: jiff::Timestamp) -> Result<Self> {
        let key_path = dir.join("node.key");

        let signing = if key_path.try_exists().map_err(io_at(&key_path))? {
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
        let cluster_path = dir.join("cluster.key");
        let cluster = if cluster_path.try_exists().map_err(io_at(&cluster_path))? {
            load_signing_key(&cluster_path)?
        } else {
            for file in ["cluster.pub", "node.cert"] {
                let path = dir.join(file);
                if path.try_exists().map_err(io_at(&path))? {
                    return Err(CertificateError::Identity.into());
                }
            }
            let cluster = SigningKey::generate(&mut rand::rng());
            write_private(&cluster_path, cluster.as_bytes())?;
            cluster
        };
        let cluster_id = ClusterId::derive(&cluster.verifying_key());
        if cluster_id.as_str() == id.as_str() {
            return Err(CertificateError::Identity.into());
        }
        let cluster_public_path = dir.join("cluster.pub");
        let public = cluster.verifying_key();
        match read_bounded(&cluster_public_path, KEY_LEN as u64) {
            Ok(bytes) if bytes != public.as_bytes() => {
                return Err(CertificateError::Identity.into());
            }
            Ok(_) => {}
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
                crate::durable::write_synced(&cluster_public_path, public.as_bytes())
                    .map_err(io_at(&cluster_public_path))?;
            }
            Err(error) => return Err(io_at(&cluster_public_path)(error)),
        }
        let path = dir.join("node.cert");
        let (certificate, renewed, write) = match read_bounded(&path, MAX_CERT_BYTES) {
            Ok(bytes) => {
                let old = Certificate::from_json(&bytes)?;
                old.verify(&public, now)?;
                if old.node_id != id.as_str()
                    || old.node_pub != STANDARD.encode(signing.verifying_key().as_bytes())
                {
                    return Err(CertificateError::Identity.into());
                }
                if old.expires()?.duration_since(now) < jiff::SignedDuration::from_secs(90 * 86_400)
                {
                    (
                        Certificate::issue(&cluster, &signing.verifying_key(), now)?,
                        true,
                        true,
                    )
                } else {
                    (old, false, false)
                }
            }
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => (
                Certificate::issue(&cluster, &signing.verifying_key(), now)?,
                false,
                true,
            ),
            Err(error) => return Err(io_at(&path)(error)),
        };
        if write {
            // Replace only after the new bytes are durable: a crash must leave a complete
            // certificate, whose expiry still governs renewal (spec/protocol.md §2.3.1).
            let temp = dir.join("node.cert.tmp");
            crate::durable::write_synced(&temp, &serde_json::to_vec(&certificate)?)
                .map_err(io_at(&temp))?;
            fs::rename(&temp, &path).map_err(io_at(&path))?;
            crate::durable::sync_dir(dir).map_err(io_at(dir))?;
        }
        Ok(Self {
            signing,
            id,
            cluster,
            cluster_id,
            certificate,
            renewed,
        })
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

    /// The cluster's ID; no private material is exposed.
    #[must_use]
    pub fn cluster_id(&self) -> &ClusterId {
        &self.cluster_id
    }

    /// The cluster public key devices pin at pairing (`spec/protocol.md §2.3.2`).
    #[must_use]
    pub fn cluster_public(&self) -> VerifyingKey {
        self.cluster.verifying_key()
    }

    /// This node's current certificate, loaded or renewed at startup.
    #[must_use]
    pub fn certificate(&self) -> &Certificate {
        &self.certificate
    }

    /// Sign a certificate for `node_pub` at `now` without releasing the cluster key.
    /// Returns an error if the key is weak or the expiry is outside the timestamp range.
    /// This performs no admission, persistence, or network operation.
    pub fn sign_certificate(
        &self,
        node_pub: &VerifyingKey,
        now: jiff::Timestamp,
    ) -> Result<Certificate> {
        Ok(Certificate::issue(&self.cluster, node_pub, now)?)
    }

    /// Derive the X25519 static secret for sessions (`spec/protocol.md §8`).
    /// Never persisted; the returned secret and derivation buffer are wiped on drop.
    #[must_use]
    pub fn x25519_static(&self) -> x25519_dalek::StaticSecret {
        let hk = hkdf::Hkdf::<Sha256>::new(None, self.signing.as_bytes());
        let mut bytes = zeroize::Zeroizing::new([0u8; KEY_LEN]);
        // A single SHA-256 block is always within HKDF's 255-block output bound.
        assert!(hk.expand(b"privatium/x25519/v1", bytes.as_mut()).is_ok());
        x25519_dalek::StaticSecret::from(*bytes)
    }

    /// Base64 of the X25519 public key carried by `sys_device.x25519_pub`.
    #[must_use]
    pub fn x25519_public_base64(&self) -> String {
        STANDARD.encode(x25519_dalek::PublicKey::from(&self.x25519_static()).as_bytes())
    }

    /// Whether startup replaced an unexpired certificate inside its renewal window.
    pub(crate) fn renewed(&self) -> bool {
        self.renewed
    }
}

/// Cluster identity, derived like a Node ID but kept distinct in Rust (`spec/protocol.md §2.3`).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ClusterId(NodeId);

impl ClusterId {
    /// Derive the cluster ID from its Ed25519 public key, with no side effects.
    #[must_use]
    pub fn derive(public: &VerifyingKey) -> Self {
        Self(NodeId::derive(public))
    }

    /// Eight lowercase Crockford Base32 characters used in `sys_cluster.id`.
    #[must_use]
    pub fn as_str(&self) -> &str {
        self.0.as_str()
    }
}

impl fmt::Display for ClusterId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        self.0.fmt(f)
    }
}

/// Certificate refusal reasons; none includes the untrusted certificate's contents.
#[derive(Debug, thiserror::Error, PartialEq, Eq)]
pub enum CertificateError {
    /// JSON, encodings, or certificate fields do not follow the protocol.
    #[error("cannot verify node certificate: invalid format; restore a valid identity copy")]
    Format,
    /// The cluster signature does not authenticate the certificate.
    #[error("cannot verify node certificate: invalid signature; restore a valid identity copy")]
    Signature,
    /// Certificate lifetime ended; possession of the key does not permit self-renewal.
    #[error(
        "cannot use node certificate: expired; re-admit the node (node admission arrives in Phase 3)"
    )]
    Expired,
    /// The certificate is bound to a different node or cluster key.
    #[error(
        "cannot verify node certificate: identity mismatch; restore the matching identity files"
    )]
    Identity,
}

/// Cluster-signed node membership (`spec/protocol.md §2.3.1`). All fields are public.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Certificate {
    /// Node ID derived from `node_pub`.
    pub node_id: String,
    /// Standard padded base64 of the node's Ed25519 public key.
    pub node_pub: String,
    /// Cluster ID derived from the signing cluster public key.
    pub cluster_id: String,
    /// RFC 3339 UTC issuance time, with exactly three fractional digits and `Z`.
    pub issued_at: String,
    /// Issuance plus exactly 180 days, in the same format.
    pub expires_at: String,
    /// Standard padded base64 of the Ed25519 signature over the five preceding fields.
    pub sig: String,
}

impl Certificate {
    fn issue(
        cluster: &SigningKey,
        node: &VerifyingKey,
        now: jiff::Timestamp,
    ) -> std::result::Result<Self, CertificateError> {
        if node.is_weak() {
            return Err(CertificateError::Identity);
        }
        let issued_at = crate::log::format_ts(now);
        let issued = parse_instant(&issued_at)?;
        let expires = issued
            .checked_add(jiff::SignedDuration::from_secs(180 * 86_400))
            .map_err(|_| CertificateError::Format)?;
        let mut cert = Self {
            node_id: NodeId::derive(node).to_string(),
            node_pub: STANDARD.encode(node.as_bytes()),
            cluster_id: ClusterId::derive(&cluster.verifying_key()).to_string(),
            issued_at,
            expires_at: crate::log::format_ts(expires),
            sig: String::new(),
        };
        cert.sig = STANDARD.encode(cluster.sign(&cert.signed_bytes()?).to_bytes());
        Ok(cert)
    }

    /// The canonical signed JSON bytes, in protocol key order, without `sig`.
    /// Returns a serialization error as `Format`; performs no I/O.
    pub fn signed_bytes(&self) -> std::result::Result<Vec<u8>, CertificateError> {
        #[derive(Serialize)]
        struct Message<'a> {
            node_id: &'a str,
            node_pub: &'a str,
            cluster_id: &'a str,
            issued_at: &'a str,
            expires_at: &'a str,
        }
        serde_json::to_vec(&Message {
            node_id: &self.node_id,
            node_pub: &self.node_pub,
            cluster_id: &self.cluster_id,
            issued_at: &self.issued_at,
            expires_at: &self.expires_at,
        })
        .map_err(|_| CertificateError::Format)
    }

    /// Verify signature, key-derived IDs, canonical encodings, and validity at `now`.
    /// Refuses malformed, unauthentic, not-yet-valid, or expired certificates. No I/O.
    pub fn verify(
        &self,
        cluster_pub: &VerifyingKey,
        now: jiff::Timestamp,
    ) -> std::result::Result<(), CertificateError> {
        let signature = Signature::from_bytes(&decode::<64>(&self.sig)?);
        cluster_pub
            .verify_strict(&self.signed_bytes()?, &signature)
            .map_err(|_| CertificateError::Signature)?;
        let node_pub = VerifyingKey::from_bytes(&decode::<32>(&self.node_pub)?)
            .map_err(|_| CertificateError::Format)?;
        if node_pub.is_weak()
            || self.node_id != NodeId::derive(&node_pub).as_str()
            || self.cluster_id != ClusterId::derive(cluster_pub).as_str()
        {
            return Err(CertificateError::Identity);
        }
        let issued = parse_instant(&self.issued_at)?;
        let expires = self.expires()?;
        if expires.duration_since(issued) != jiff::SignedDuration::from_secs(180 * 86_400)
            || now < issued
        {
            return Err(CertificateError::Format);
        }
        if now >= expires {
            return Err(CertificateError::Expired);
        }
        Ok(())
    }

    /// Encode the complete certificate for `sys_node.cert`. No private key is included.
    pub fn to_base64(&self) -> std::result::Result<String, CertificateError> {
        Ok(STANDARD.encode(serde_json::to_vec(self).map_err(|_| CertificateError::Format)?))
    }

    /// Decode a certificate from base64, refusing malformed or oversized input.
    /// Call `verify` before trusting any field; decoding alone authenticates nothing.
    pub fn from_base64(value: &str) -> std::result::Result<Self, CertificateError> {
        if value.len() > (MAX_CERT_BYTES as usize).div_ceil(3) * 4 {
            return Err(CertificateError::Format);
        }
        let bytes = STANDARD
            .decode(value)
            .map_err(|_| CertificateError::Format)?;
        Self::from_json(&bytes)
    }

    fn from_json(bytes: &[u8]) -> std::result::Result<Self, CertificateError> {
        if bytes.len() > MAX_CERT_BYTES as usize {
            return Err(CertificateError::Format);
        }
        serde_json::from_slice(bytes).map_err(|_| CertificateError::Format)
    }

    fn expires(&self) -> std::result::Result<jiff::Timestamp, CertificateError> {
        parse_instant(&self.expires_at)
    }
}

fn parse_instant(value: &str) -> std::result::Result<jiff::Timestamp, CertificateError> {
    let at = value.parse().map_err(|_| CertificateError::Format)?;
    if crate::log::format_ts(at) != value {
        return Err(CertificateError::Format);
    }
    Ok(at)
}

fn decode<const N: usize>(value: &str) -> std::result::Result<[u8; N], CertificateError> {
    if value.len() != N.div_ceil(3) * 4 {
        return Err(CertificateError::Format);
    }
    let bytes = STANDARD
        .decode(value)
        .map_err(|_| CertificateError::Format)?;
    if STANDARD.encode(&bytes) != value {
        return Err(CertificateError::Format);
    }
    bytes.try_into().map_err(|_| CertificateError::Format)
}

fn read_bounded(path: &Path, limit: u64) -> std::io::Result<Vec<u8>> {
    let mut bytes = Vec::new();
    fs::File::open(path)?
        .take(limit + 1)
        .read_to_end(&mut bytes)?;
    Ok(bytes)
}

/// z-base32 of a public key, without padding (`spec/data-dictionary.md §3.1b`).
pub(crate) fn pkarr_name(public: &VerifyingKey) -> String {
    const ALPHABET: &[u8; 32] = b"ybndrfg8ejkmcpqxot1uwisza345h769";
    let mut result = String::with_capacity(52);
    let mut bits = 0u16;
    let mut count = 0;
    for byte in public.as_bytes() {
        bits = (bits << 8) | u16::from(*byte);
        count += 8;
        while count >= 5 {
            count -= 5;
            result.push(char::from(ALPHABET[usize::from((bits >> count) & 31)]));
        }
    }
    if count != 0 {
        result.push(char::from(
            ALPHABET[usize::from((bits << (5 - count)) & 31)],
        ));
    }
    result
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
