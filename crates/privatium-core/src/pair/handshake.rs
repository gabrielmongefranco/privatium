// Project:  Privatium™  |  File: crates/privatium-core/src/pair/handshake.rs
// Authors:  Gabriel Mongefranco (@gabrielmongefranco)
// Created:  2026-09-05  |  Modified: 2026-09-07
// Summary:  The messages of /ws/pair (spec/protocol.md §7.4.2) as data, for both roles:
//           the node's side takes the window and the identity and yields what to send, the
//           client's side takes a code and yields the same; for a node as the client, the
//           signatures, the direction rule of §2.3.1 and the admit and joined messages that
//           carry the cluster key. Neither side touches a socket, a log or a store.
//           See main README.md for full license information.

use std::net::IpAddr;

use base64::{Engine as _, engine::general_purpose::STANDARD};
use ed25519_dalek::{Signer as _, SigningKey, VerifyingKey};
use serde::{Deserialize, Serialize, de::DeserializeOwned};
use sha2::Sha256;
use x25519_dalek::{PublicKey, StaticSecret};
use zeroize::Zeroizing;

use super::code::Code;
use super::spake2::{Identities, Shared, Side, State};
use super::{PairError, Pairing};
use crate::identity::{Certificate, ClusterId, Identity, NodeId};
use crate::session::{Direction as FrameDirection, Frame};

/// The `kind` a node declares when it is the client (`spec/protocol.md §7.4.2`).
pub const KIND_NODE: &str = "node";

/// A node's own state at admission (`spec/protocol.md §2.3.1`), carried in its sealed
/// message so the two sides decide the direction by one rule.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Flags {
    /// Disposable: paired nothing and admitted nobody since it entered its cluster (`§2.3`).
    pub disposable: bool,
    /// Its own certificate has expired (`§2.3.1`).
    pub expired: bool,
}

/// Which side admits the other (`spec/protocol.md §2.3.1`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Direction {
    /// The node whose owner opened the window admits the dialer.
    NodeAdmits,
    /// The dialer admits the node whose window it answered.
    ClientAdmits,
}

/// The direction rule of `spec/protocol.md §2.3.1`, from the two sides' flags. Pure, and
/// the same function on both sides, which is what keeps them from each waiting for the
/// other's `admit`.
pub fn direction(node: Flags, client: Flags) -> Result<Direction, PairError> {
    match (node.expired, client.expired) {
        (true, true) => Err(PairError::NoAdmitter(
            "both certificates have expired; re-admit each from an established member of its cluster",
        )),
        (true, false) if client.disposable => Err(PairError::NoAdmitter(
            "an expired node is re-admitted by an established member of its cluster, not by a fresh node",
        )),
        (true, false) => Ok(Direction::ClientAdmits),
        (false, true) if node.disposable => Err(PairError::NoAdmitter(
            "an expired node is re-admitted by an established member of its cluster, not by a fresh node",
        )),
        (false, true) => Ok(Direction::NodeAdmits),
        (false, false) if client.disposable => Ok(Direction::NodeAdmits),
        (false, false) if node.disposable => Ok(Direction::ClientAdmits),
        (false, false) => Err(PairError::TwoClusters),
    }
}

/// A text frame is a few base64 keys; a sealed frame is a certificate plus a label.
const MAX_MESSAGE_BYTES: usize = 8192;

/// `sys_device.label` is owner-facing text; a browser's suggestion is trimmed to this.
const MAX_LABEL_CHARS: usize = 80;

/// `sys_device.user_agent`; longer strings are cut, never refused.
const MAX_USER_AGENT_CHARS: usize = 512;

/// The salt of the pairing frames' key schedule (`spec/protocol.md §7.4.2`).
const PAIR_SALT: &[u8] = b"pv/1 pair";

/// The node's first message: its version, ID, Ed25519 key and whether pairing is open.
#[derive(Debug, Serialize, Deserialize)]
pub struct NodeHello {
    /// Protocol major; only 1 is accepted.
    pub v: u32,
    /// Node ID.
    pub id: String,
    /// The node's Ed25519 public key, standard padded base64.
    #[serde(rename = "pub")]
    pub public: String,
    /// Whether a window is open. `false` is followed by close code 4404.
    pub open: bool,
}

/// The client's first message: its identity, its kind and `pA`.
#[derive(Debug, Serialize, Deserialize)]
pub struct ClientStart {
    /// Protocol major; only 1 is accepted.
    pub v: u32,
    /// Device ID, which must derive from `pub` (`spec/protocol.md §2.2`).
    pub dev: String,
    /// The device's Ed25519 public key, standard padded base64.
    #[serde(rename = "pub")]
    pub public: String,
    /// `browser`, `desktop`, `mobile` or `node` (`spec/data-dictionary.md §3.2`).
    pub kind: String,
    /// `pA`, base64.
    #[serde(rename = "pA")]
    pub pa: String,
}

/// The node's answer to `pA`.
#[derive(Debug, Serialize, Deserialize)]
pub struct NodeReply {
    /// `pB`, base64.
    #[serde(rename = "pB")]
    pub pb: String,
    /// `cB`, base64.
    #[serde(rename = "cB")]
    pub cb: String,
}

/// The client's confirmation.
#[derive(Debug, Serialize, Deserialize)]
pub struct ClientConfirm {
    /// `cA`, base64.
    #[serde(rename = "cA")]
    pub ca: String,
}

/// The node's sealed message: what a device pins (`spec/protocol.md §7.4` step 5). The
/// last three fields exist for a node as the client alone (`§7.4.2`) and are absent —
/// not `null` — for every other kind, so a browser's message is byte for byte what it was.
#[derive(Debug, Serialize, Deserialize)]
pub struct NodeSealed {
    /// The node's X25519 static public key, base64.
    pub x25519: String,
    /// The node's certificate, base64 (`§2.3.1`).
    pub cert: String,
    /// The cluster ID.
    pub cluster_id: String,
    /// The cluster public key, base64 — what the device pins (`§2.3.2`).
    pub cluster_pub: String,
    /// The node's Ed25519 signature over the PAKE transcript, base64 (`§7.4.2`).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub sig: Option<String>,
    /// Whether the node is disposable (`§2.3`).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub disposable: Option<bool>,
    /// Whether the node's certificate has expired (`§2.3.1`).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub expired: Option<bool>,
    /// The node's display name, for the row the client writes when it admits.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub label: Option<String>,
}

/// The client's sealed message: its X25519 key and what the devices page shows. The
/// last three fields are a node's alone, as on [`NodeSealed`].
#[derive(Debug, Serialize, Deserialize)]
pub struct ClientSealed {
    /// The device's X25519 public key, base64.
    pub x25519: String,
    /// A suggested label, e.g. the device's model name.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub label: Option<String>,
    /// The browser's user agent, for browsers.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub ua: Option<String>,
    /// The client's Ed25519 signature over the PAKE transcript, base64 (`§7.4.2`).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub sig: Option<String>,
    /// Whether the client node is disposable (`§2.3`).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub disposable: Option<bool>,
    /// Whether the client node's certificate has expired (`§2.3.1`).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub expired: Option<bool>,
}

/// The admitter's message (`spec/protocol.md §7.4.2`): the one place the cluster private
/// key crosses a network. `Debug` prints none of it.
#[derive(Serialize, Deserialize)]
pub struct Admit {
    /// The 32-byte Ed25519 seed of the cluster key, base64.
    pub cluster_key: String,
    /// The joiner's certificate, base64.
    pub cert: String,
    /// The instant the admitter records as the joiner's `paired_at`.
    pub paired_at: String,
    /// The admitter's `_sys` Lamport counter, which the joiner folds (`§4.3`).
    pub lam: u64,
}

impl std::fmt::Debug for Admit {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Admit")
            .field("paired_at", &self.paired_at)
            .field("lam", &self.lam)
            .finish_non_exhaustive()
    }
}

/// The joiner's answer (`§7.4.2`), sent once it has adopted the cluster.
#[derive(Debug, Serialize, Deserialize)]
pub struct Joined {
    /// Always `true`; a joiner that could not adopt closes instead.
    pub joined: bool,
}

/// What a joiner holds after `admit` verified (`spec/protocol.md §2.3.1`): the cluster
/// key as its seed, wiped on drop, the certificate it was issued, and the two facts it
/// writes. `Debug` prints none of the key.
pub struct ClientAdmitted {
    /// The cluster private key's 32-byte seed.
    pub cluster_key: Zeroizing<[u8; 32]>,
    /// This node's certificate, verified under that key.
    pub certificate: Certificate,
    /// The `paired_at` the admitter records.
    pub paired_at: String,
    /// The admitter's `_sys` Lamport counter.
    pub lam: u64,
}

impl std::fmt::Debug for ClientAdmitted {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("ClientAdmitted")
            .field("cluster_id", &self.certificate.cluster_id)
            .field("paired_at", &self.paired_at)
            .field("lam", &self.lam)
            .finish_non_exhaustive()
    }
}

/// The admitting side after both sealed messages: seals `admit` and opens `joined` on
/// the pairing frames, counters continuing (`§7.4.2`). Either role may hold one.
pub struct Admitter {
    send: Frame,
    receive: Frame,
}

impl Admitter {
    /// Seal `admit` for the joiner: the cluster seed, the certificate issued for it, the
    /// `paired_at` about to be recorded and this side's `_sys` counter.
    pub fn admit(
        &mut self,
        seed: &Zeroizing<[u8; 32]>,
        certificate: &Certificate,
        paired_at: &str,
        lam: u64,
    ) -> Result<Vec<u8>, PairError> {
        let message = encode(&Admit {
            cluster_key: STANDARD.encode(seed.as_ref()),
            cert: certificate.to_base64().map_err(|_| PairError::Format)?,
            paired_at: paired_at.to_owned(),
            lam,
        })?;
        self.send
            .seal(message.as_bytes())
            .map_err(|_| PairError::Format)
    }

    /// Open the joiner's `joined`. Anything else, or a message that does not open, is a
    /// refusal and nothing is written for it.
    pub fn admitted(mut self, ciphertext: &[u8]) -> Result<(), PairError> {
        if ciphertext.len() > MAX_MESSAGE_BYTES {
            return Err(PairError::Format);
        }
        let plain = self
            .receive
            .open(ciphertext)
            .map_err(|_| PairError::Format)?;
        let joined: Joined = serde_json::from_slice(&plain).map_err(|_| PairError::Format)?;
        if !joined.joined {
            return Err(PairError::Format);
        }
        Ok(())
    }
}

impl std::fmt::Debug for Admitter {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Admitter").finish_non_exhaustive()
    }
}

/// The joining side after both sealed messages: opens `admit` and seals `joined`.
pub struct Joiner {
    send: Frame,
    receive: Frame,
}

impl Joiner {
    /// Open and verify `admit` (`§7.4.2`): the certificate under the public half of the
    /// key it came with, that half against `expected_cluster` when this side was sent a
    /// cluster key to pin, and the certificate's node against this node's own ID and key.
    /// Nothing about the key is kept unless every check passes.
    pub fn verify(
        &mut self,
        ciphertext: &[u8],
        own_id: &str,
        own_pub: &VerifyingKey,
        expected_cluster: Option<&VerifyingKey>,
        now: jiff::Timestamp,
    ) -> Result<ClientAdmitted, PairError> {
        if ciphertext.len() > MAX_MESSAGE_BYTES {
            return Err(PairError::Format);
        }
        let plain = Zeroizing::new(
            self.receive
                .open(ciphertext)
                .map_err(|_| PairError::Format)?,
        );
        let admit: Admit = serde_json::from_slice(&plain).map_err(|_| PairError::Format)?;
        let seed = Zeroizing::new(decode32(&admit.cluster_key)?);
        let cluster = SigningKey::from_bytes(&seed);
        let public = cluster.verifying_key();
        if expected_cluster.is_some_and(|expected| *expected != public) {
            return Err(PairError::Admit);
        }
        let certificate = Certificate::from_base64(&admit.cert).map_err(|_| PairError::Admit)?;
        certificate
            .verify(&public, now)
            .map_err(|_| PairError::Admit)?;
        if certificate.node_id != own_id
            || certificate.node_pub != STANDARD.encode(own_pub.as_bytes())
        {
            return Err(PairError::Admit);
        }
        if crate::log::format_ts(admit.paired_at.parse().map_err(|_| PairError::Admit)?)
            != admit.paired_at
        {
            return Err(PairError::Admit);
        }
        Ok(ClientAdmitted {
            cluster_key: seed,
            certificate,
            paired_at: admit.paired_at,
            lam: admit.lam,
        })
    }

    /// Seal `joined`, once the cluster has been adopted.
    pub fn joined(mut self) -> Result<Vec<u8>, PairError> {
        let message = encode(&Joined { joined: true })?;
        self.send
            .seal(message.as_bytes())
            .map_err(|_| PairError::Format)
    }
}

impl std::fmt::Debug for Joiner {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Joiner").finish_non_exhaustive()
    }
}

/// Sign the PAKE transcript with the key the other side already holds the public half
/// of (`§7.4.2`).
fn sign_transcript(signing: &SigningKey, transcript: &[u8]) -> String {
    STANDARD.encode(signing.sign(transcript).to_bytes())
}

/// Verify the other side's signature over the transcript against the key it named
/// (`§7.4.2`): missing, malformed, by another key or over other bytes is
/// [`PairError::Signature`]. Both sides call this; it is public so a conformance test
/// can drive every case.
pub fn verify_signature(
    public_b64: &str,
    transcript: &[u8],
    sig: Option<&str>,
) -> Result<(), PairError> {
    verify_transcript(public_b64, transcript, sig)
}

fn verify_transcript(
    public_b64: &str,
    transcript: &[u8],
    sig: Option<&str>,
) -> Result<(), PairError> {
    let sig = sig.ok_or(PairError::Signature)?;
    if sig.len() != 88 {
        return Err(PairError::Signature);
    }
    let bytes: [u8; 64] = STANDARD
        .decode(sig)
        .map_err(|_| PairError::Signature)?
        .try_into()
        .map_err(|_| PairError::Signature)?;
    let public =
        VerifyingKey::from_bytes(&decode32(public_b64)?).map_err(|_| PairError::Signature)?;
    public
        .verify_strict(transcript, &ed25519_dalek::Signature::from_bytes(&bytes))
        .map_err(|_| PairError::Signature)
}

/// The node's first text frame (`§7.4.2`), for the window state given.
pub fn node_hello(identity: &Identity, open: bool) -> String {
    encode(&NodeHello {
        v: 1,
        id: identity.id().to_string(),
        public: identity.public_key_base64(),
        open,
    })
    .unwrap_or_default()
}

/// A device's request accepted as an attempt: the node's side between `pA` and `cA`.
/// Not `Clone`; wiped on drop.
pub struct Exchange {
    device: String,
    device_pub: String,
    kind: String,
    source: IpAddr,
    /// The window's code generation the attempt was counted against. A `cA` for a code
    /// the window has since replaced cannot finish (`spec/protocol.md §7.4.2`).
    generation: u32,
    shared: Shared,
}

impl Exchange {
    /// Answer a device's first message. Parses and validates it, counts the attempt on
    /// the window (`§7.5`), runs the node's side of the PAKE and returns the state plus
    /// the `{"pB","cB"}` text to send. Every refusal is a [`PairError`] with its close
    /// code and leaves nothing derived behind.
    pub fn begin(
        identity: &Identity,
        pairing: &mut Pairing,
        source: IpAddr,
        now: jiff::Timestamp,
        text: &str,
    ) -> Result<(Self, String), PairError> {
        use rand::RngExt as _;
        let secret = Zeroizing::new(rand::rng().random::<[u8; 64]>());
        Self::begin_with(identity, pairing, source, now, text, &secret)
    }

    /// [`Exchange::begin`] with the node's PAKE secret supplied — the reproducible form
    /// that `tests/fixtures/pake-vectors.json` is generated from. The secret is reduced
    /// as `w` is; in production it is fresh CSPRNG output, which `begin` draws.
    pub fn begin_with(
        identity: &Identity,
        pairing: &mut Pairing,
        source: IpAddr,
        now: jiff::Timestamp,
        text: &str,
        secret: &[u8; 64],
    ) -> Result<(Self, String), PairError> {
        let start: ClientStart = parse(text)?;
        if start.v != 1 {
            return Err(PairError::Format);
        }
        let device_key =
            VerifyingKey::from_bytes(&decode32(&start.public)?).map_err(|_| PairError::Format)?;
        if device_key.is_weak() || NodeId::derive(&device_key).as_str() != start.dev {
            return Err(PairError::Format);
        }
        if start.dev == identity.id().as_str() {
            return Err(PairError::Format);
        }
        // The window's kind is checked before the attempt is counted (§7.1): a client
        // the window was not opened for spends no guess and writes no row.
        match start.kind.as_str() {
            "browser" | "desktop" | "mobile" if pairing.is_for_node() => {
                return Err(PairError::WindowKind { node: true });
            }
            KIND_NODE if !pairing.is_for_node() => {
                return Err(PairError::WindowKind { node: false });
            }
            "browser" | "desktop" | "mobile" | KIND_NODE => {}
            _ => return Err(PairError::Format),
        }
        let pa = decode32(&start.pa)?;

        let w = pairing.begin_attempt(source, now)?;
        let generation = pairing.generation();
        let ids = Identities::new(&start.public, &identity.public_key_base64());
        let state = State::start_with(Side::B, &w, secret).map_err(|_| PairError::Format)?;
        let pb = state.message();
        let shared = match state.finish(&pa, &ids) {
            Ok(shared) => shared,
            Err(_) => {
                // A message that is no point is a spent attempt, not a free one.
                pairing.record_failure()?;
                return Err(PairError::Format);
            }
        };
        let reply = encode(&NodeReply {
            pb: STANDARD.encode(pb),
            cb: STANDARD.encode(shared.confirm_send),
        })?;
        Ok((
            Self {
                device: start.dev,
                device_pub: start.public,
                kind: start.kind,
                source,
                generation,
                shared,
            },
            reply,
        ))
    }

    /// The device ID the other side claims and has proven to hold the key of.
    #[must_use]
    pub fn device(&self) -> &str {
        &self.device
    }

    /// The kind the other side declared.
    #[must_use]
    pub fn kind(&self) -> &str {
        &self.kind
    }

    /// The address the attempt came from.
    #[must_use]
    pub fn source(&self) -> IpAddr {
        self.source
    }

    /// Whether the client is a node asking to be admitted (`§7.4.2`).
    #[must_use]
    pub fn is_node(&self) -> bool {
        self.kind == KIND_NODE
    }

    /// Verify the device's `cA` at `now`. The window is checked before the message is
    /// read: a code the window has replaced since `pA` is [`PairError::Exhausted`], and a
    /// window another device has consumed or that has expired is [`PairError::Closed`]
    /// — neither seals anything (`spec/protocol.md §7.4.2`). A confirmation that does
    /// not verify is the wrong code: the failure is recorded on the window and
    /// [`PairError::WrongCode`] returned. On success the node's sealed message is
    /// returned as the bytes to send, and the state moves on to the device's sealed
    /// message. For a node client, `flags` is this node's own state and the message
    /// carries it beside the node's signature over the transcript; a browser gets neither.
    pub fn confirm(
        self,
        identity: &Identity,
        pairing: &mut Pairing,
        text: &str,
        now: jiff::Timestamp,
        flags: Option<(Flags, Option<String>)>,
    ) -> Result<(Sealed, Vec<u8>), PairError> {
        if pairing.generation() != self.generation {
            return Err(PairError::Exhausted);
        }
        if !pairing.is_open(now) {
            return Err(PairError::Closed);
        }
        let confirm: ClientConfirm = parse(text)?;
        let ca = decode32(&confirm.ca)?;
        if !self.shared.verify(&ca) {
            pairing.record_failure()?;
            return Err(PairError::WrongCode);
        }
        let (flags, label) = match flags.filter(|_| self.kind == KIND_NODE) {
            Some((flags, label)) => (Some(flags), label),
            None => (None, None),
        };
        let (mut send, receive) = pair_frames(&self.shared.ke, Side::B);
        let message = encode(&NodeSealed {
            x25519: identity.x25519_public_base64(),
            cert: identity
                .certificate()
                .to_base64()
                .map_err(|_| PairError::Format)?,
            cluster_id: identity.cluster_id().to_string(),
            cluster_pub: STANDARD.encode(identity.cluster_public().as_bytes()),
            sig: flags.map(|_| STANDARD.encode(identity.sign(&self.shared.transcript).to_bytes())),
            disposable: flags.map(|f| f.disposable),
            expired: flags.map(|f| f.expired),
            label: flags.and(label),
        })?;
        let sealed = send
            .seal(message.as_bytes())
            .map_err(|_| PairError::Format)?;
        Ok((
            Sealed {
                device: self.device,
                device_pub: self.device_pub,
                kind: self.kind,
                source: self.source,
                send,
                receive,
                transcript: self.shared.transcript.clone(),
                flags,
            },
            sealed,
        ))
    }
}

impl std::fmt::Debug for Exchange {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Exchange")
            .field("device", &self.device)
            .field("kind", &self.kind)
            .finish_non_exhaustive()
    }
}

/// The node's side after `cA` verified: waiting for the device's sealed message.
pub struct Sealed {
    device: String,
    device_pub: String,
    kind: String,
    source: IpAddr,
    send: Frame,
    receive: Frame,
    transcript: Zeroizing<Vec<u8>>,
    /// This node's own flags, as sent — present for a node client alone.
    flags: Option<Flags>,
}

impl Sealed {
    /// The device ID.
    #[must_use]
    pub fn device(&self) -> &str {
        &self.device
    }

    /// The address the attempt came from.
    #[must_use]
    pub fn source(&self) -> IpAddr {
        self.source
    }

    /// Whether the client is a node asking to be admitted (`§7.4.2`).
    #[must_use]
    pub fn is_node(&self) -> bool {
        self.kind == KIND_NODE
    }

    /// Open the device's sealed message and produce the facts of its `sys_device` row.
    /// The X25519 key must be one this node can actually agree a key with; a label and
    /// a user agent are trimmed to their column's size and stripped of control
    /// characters, never refused. For a node client the signature over the transcript is
    /// verified first — missing or failing is [`PairError::Signature`] — and what comes
    /// back carries the peer's flags and the frames the admission continues on.
    pub fn finish(mut self, identity: &Identity, ciphertext: &[u8]) -> Result<Finished, PairError> {
        if ciphertext.len() > MAX_MESSAGE_BYTES {
            return Err(PairError::Format);
        }
        let plain = self
            .receive
            .open(ciphertext)
            .map_err(|_| PairError::Format)?;
        let sealed: ClientSealed = serde_json::from_slice(&plain).map_err(|_| PairError::Format)?;
        let node_client = self.kind == KIND_NODE;
        if node_client {
            verify_transcript(&self.device_pub, &self.transcript, sealed.sig.as_deref())?;
        }
        let x25519 = PublicKey::from(decode32(&sealed.x25519)?);
        if !identity
            .x25519_static()
            .diffie_hellman(&x25519)
            .was_contributory()
        {
            return Err(PairError::Format);
        }
        let paired = Paired {
            device: self.device,
            kind: self.kind,
            ed25519_pub: self.device_pub,
            x25519_pub: sealed.x25519,
            label: sealed.label.and_then(|l| clean(&l, MAX_LABEL_CHARS)),
            user_agent: if node_client {
                None
            } else {
                sealed.ua.and_then(|u| clean(&u, MAX_USER_AGENT_CHARS))
            },
            source: self.source,
        };
        if !node_client {
            return Ok(Finished::Device(paired));
        }
        let (Some(disposable), Some(expired), Some(own)) =
            (sealed.disposable, sealed.expired, self.flags)
        else {
            return Err(PairError::Format);
        };
        Ok(Finished::Node(NodePeer {
            paired,
            flags: Flags {
                disposable,
                expired,
            },
            own,
            send: self.send,
            receive: self.receive,
        }))
    }
}

/// What the node holds once the client's sealed message is open: a device's row facts,
/// or a node's beside the admission that follows (`§7.4.2`).
#[derive(Debug)]
pub enum Finished {
    /// A browser, desktop or mobile device; its row is written now.
    Device(Paired),
    /// A node; the direction is decided and the row waits on `joined`.
    Node(NodePeer),
}

/// The node's side of an admission after both sealed messages, on either role's frames.
pub struct NodePeer {
    /// The peer node's facts, as a device row would carry them.
    pub paired: Paired,
    /// The peer's flags, as it sent them.
    pub flags: Flags,
    /// This side's own flags, as it sent them.
    pub own: Flags,
    send: Frame,
    receive: Frame,
}

impl NodePeer {
    /// Which side admits (`§2.3.1`), from this side's flags as the node's and the peer's
    /// as the client's.
    pub fn direction(&self) -> Result<Direction, PairError> {
        direction(self.own, self.flags)
    }

    /// This side admits: the state that seals `admit` and opens `joined`.
    #[must_use]
    pub fn into_admitter(self) -> Admitter {
        Admitter {
            send: self.send,
            receive: self.receive,
        }
    }

    /// This side joins: the state that opens `admit` and seals `joined`.
    #[must_use]
    pub fn into_joiner(self) -> Joiner {
        Joiner {
            send: self.send,
            receive: self.receive,
        }
    }
}

impl std::fmt::Debug for NodePeer {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("NodePeer")
            .field("device", &self.paired.device)
            .field("flags", &self.flags)
            .field("own", &self.own)
            .finish_non_exhaustive()
    }
}

impl std::fmt::Debug for Sealed {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Sealed")
            .field("device", &self.device)
            .field("kind", &self.kind)
            .finish_non_exhaustive()
    }
}

/// A completed pairing: what `Node` writes as the device's row (`spec/data-dictionary.md
/// §3.2`) and audits as `pair.success`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Paired {
    /// The device's Node ID.
    pub device: String,
    /// `browser`, `desktop` or `mobile`.
    pub kind: String,
    /// The device's Ed25519 public key, base64.
    pub ed25519_pub: String,
    /// The device's X25519 public key, base64.
    pub x25519_pub: String,
    /// The label the device suggested, cleaned, if any.
    pub label: Option<String>,
    /// The user agent, cleaned, if any.
    pub user_agent: Option<String>,
    /// The address the pairing came from.
    pub source: IpAddr,
}

/// What a device holds after pairing (`spec/protocol.md §7.6`): its own keys, the node's
/// identity and the pinned cluster key. Wiped on drop.
pub struct ClientPaired {
    /// The device's own ID.
    pub device: String,
    /// The device's Ed25519 signing key.
    pub ed25519: ed25519_dalek::SigningKey,
    /// The device's X25519 static secret.
    pub x25519: StaticSecret,
    /// The node's ID.
    pub node_id: String,
    /// The node's Ed25519 public key.
    pub node_ed25519: VerifyingKey,
    /// The node's X25519 static public key — the static of `§8`.
    pub node_x25519: PublicKey,
    /// The node's certificate, as received.
    pub certificate: Certificate,
    /// The cluster ID.
    pub cluster_id: String,
    /// The cluster public key — what `§2.3.2` pins.
    pub cluster_pub: VerifyingKey,
    /// This side's own flags when it is a node (`§7.4.2`); `None` for a device.
    pub flags: Option<Flags>,
}

impl std::fmt::Debug for ClientPaired {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("ClientPaired")
            .field("device", &self.device)
            .field("node_id", &self.node_id)
            .field("cluster_id", &self.cluster_id)
            .finish_non_exhaustive()
    }
}

fn paired_flags(paired: &ClientPaired) -> Flags {
    paired.flags.unwrap_or(Flags {
        disposable: false,
        expired: false,
    })
}

/// A node as the client after both sealed messages (`§7.4.2`): what it pinned of the
/// node, both sides' flags, and the frames the admission continues on.
pub struct NodeClient {
    /// The node's identity, certificate and cluster as message 5 carried them.
    pub paired: ClientPaired,
    /// The node's flags, as it sent them.
    pub node_flags: Flags,
    /// The node's display name, cleaned, for the row this side writes when it admits.
    pub node_label: Option<String>,
    /// This side's own flags, as sent.
    pub own: Flags,
    send: Frame,
    receive: Frame,
}

impl NodeClient {
    /// Which side admits (`§2.3.1`).
    pub fn direction(&self) -> Result<Direction, PairError> {
        direction(self.node_flags, self.own)
    }

    /// This side joins: the state that opens `admit` and seals `joined`.
    #[must_use]
    pub fn into_joiner(self) -> Joiner {
        Joiner {
            send: self.send,
            receive: self.receive,
        }
    }

    /// This side admits the node it dialed: the state that seals `admit` and opens
    /// `joined`.
    #[must_use]
    pub fn into_admitter(self) -> Admitter {
        Admitter {
            send: self.send,
            receive: self.receive,
        }
    }
}

impl std::fmt::Debug for NodeClient {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("NodeClient")
            .field("node_id", &self.paired.node_id)
            .field("node_flags", &self.node_flags)
            .field("own", &self.own)
            .finish_non_exhaustive()
    }
}

/// The device's side of `/ws/pair`, in Rust — for the framework's own tests, for a
/// native client, and for a node joining a cluster. Not `Clone`; wiped on drop.
pub struct Client {
    device: String,
    ed25519: ed25519_dalek::SigningKey,
    x25519: StaticSecret,
    kind: String,
    node_id: String,
    node_ed25519: VerifyingKey,
    state: Option<State>,
    shared: Option<Shared>,
    /// This node's own flags, for a node as the client (`§7.4.2`).
    flags: Option<Flags>,
}

impl Client {
    /// Read the node's hello and produce the device's first message for `code`. Fresh
    /// device keys are generated here; the caller keeps the returned [`Client`] and
    /// sends the text. A hello with `open: false` is [`PairError::Closed`].
    pub fn start(hello: &str, code: Code, kind: &str) -> Result<(Self, String), PairError> {
        use rand::RngExt as _;
        let ed25519 = ed25519_dalek::SigningKey::generate(&mut rand::rng());
        let x25519 = StaticSecret::from(*Zeroizing::new(rand::rng().random::<[u8; 32]>()));
        let secret = Zeroizing::new(rand::rng().random::<[u8; 64]>());
        Self::start_with(hello, code, kind, ed25519, x25519, &secret)
    }

    /// [`Client::start`] for a node joining a cluster (`spec/protocol.md §2.3.1`): its
    /// own node key as `pub`, its derived static as `x25519`, `kind = "node"`, and its
    /// flags for the sealed message. A hello from a node this side knows to be revoked
    /// is the caller's to refuse before this is called.
    pub fn start_node(
        hello: &str,
        code: Code,
        signing: SigningKey,
        x25519: StaticSecret,
        flags: Flags,
    ) -> Result<(Self, String), PairError> {
        use rand::RngExt as _;
        let secret = Zeroizing::new(rand::rng().random::<[u8; 64]>());
        let (mut client, text) =
            Self::start_with(hello, code, KIND_NODE, signing, x25519, &secret)?;
        client.flags = Some(flags);
        Ok((client, text))
    }

    /// [`Client::start`] with every secret supplied — the reproducible form the vector
    /// file is generated from.
    pub fn start_with(
        hello: &str,
        code: Code,
        kind: &str,
        ed25519: ed25519_dalek::SigningKey,
        x25519: StaticSecret,
        secret: &[u8; 64],
    ) -> Result<(Self, String), PairError> {
        let hello: NodeHello = parse(hello)?;
        if hello.v != 1 {
            return Err(PairError::Format);
        }
        if !hello.open {
            return Err(PairError::Closed);
        }
        let node_ed25519 =
            VerifyingKey::from_bytes(&decode32(&hello.public)?).map_err(|_| PairError::Format)?;
        if node_ed25519.is_weak() || NodeId::derive(&node_ed25519).as_str() != hello.id {
            return Err(PairError::Format);
        }
        let w = super::spake2::password(code).map_err(|_| PairError::Format)?;
        let state = State::start_with(Side::A, &w, secret).map_err(|_| PairError::Format)?;
        let public = STANDARD.encode(ed25519.verifying_key().as_bytes());
        let device = NodeId::derive(&ed25519.verifying_key()).to_string();
        let text = encode(&ClientStart {
            v: 1,
            dev: device.clone(),
            public,
            kind: kind.to_owned(),
            pa: STANDARD.encode(state.message()),
        })?;
        Ok((
            Self {
                device,
                ed25519,
                x25519,
                kind: kind.to_owned(),
                node_id: hello.id,
                node_ed25519,
                state: Some(state),
                shared: None,
                flags: None,
            },
            text,
        ))
    }

    /// The node's ID, from its hello.
    #[must_use]
    pub fn node_id(&self) -> &str {
        &self.node_id
    }

    /// The device's own ID.
    #[must_use]
    pub fn device(&self) -> &str {
        &self.device
    }

    /// Read the node's `{"pB","cB"}`. A `cB` that does not verify is the wrong code, and
    /// the client says so without sending anything. Returns the `{"cA"}` text to send.
    pub fn reply(&mut self, text: &str) -> Result<String, PairError> {
        let reply: NodeReply = parse(text)?;
        let pb = decode32(&reply.pb)?;
        let cb = decode32(&reply.cb)?;
        let state = self.state.take().ok_or(PairError::Format)?;
        let ids = Identities::new(
            &STANDARD.encode(self.ed25519.verifying_key().as_bytes()),
            &STANDARD.encode(self.node_ed25519.as_bytes()),
        );
        let shared = state.finish(&pb, &ids).map_err(|_| PairError::Format)?;
        if !shared.verify(&cb) {
            return Err(PairError::WrongCode);
        }
        let text = encode(&ClientConfirm {
            ca: STANDARD.encode(shared.confirm_send),
        })?;
        self.shared = Some(shared);
        Ok(text)
    }

    /// Open the node's sealed message, verify its certificate against the cluster key it
    /// carries at `now`, and produce the device's sealed message and what it pins.
    pub fn finish(
        self,
        ciphertext: &[u8],
        label: Option<&str>,
        user_agent: Option<&str>,
        now: jiff::Timestamp,
    ) -> Result<(ClientPaired, Vec<u8>), PairError> {
        let (paired, bytes, _, _) = self.finish_with(ciphertext, label, user_agent, now)?;
        Ok((paired, bytes))
    }

    /// [`Client::finish`] for a node as the client (`§7.4.2`): the node's signature over
    /// the transcript is verified against the key its hello named, an expired certificate
    /// is accepted — the node may be the one being re-admitted — and what comes back
    /// carries both sides' flags and the frames the admission continues on.
    pub fn finish_node(
        self,
        ciphertext: &[u8],
        label: Option<&str>,
        now: jiff::Timestamp,
    ) -> Result<(NodeClient, Vec<u8>), PairError> {
        if self.flags.is_none() {
            return Err(PairError::Format);
        }
        let (paired, bytes, node_flags, frames) = self.finish_with(ciphertext, label, None, now)?;
        let (Some((node_flags, node_label)), Some((send, receive))) = (node_flags, frames) else {
            return Err(PairError::Format);
        };
        Ok((
            NodeClient {
                own: paired_flags(&paired),
                paired,
                node_flags,
                node_label,
                send,
                receive,
            },
            bytes,
        ))
    }

    #[allow(clippy::type_complexity)]
    fn finish_with(
        self,
        ciphertext: &[u8],
        label: Option<&str>,
        user_agent: Option<&str>,
        now: jiff::Timestamp,
    ) -> Result<
        (
            ClientPaired,
            Vec<u8>,
            Option<(Flags, Option<String>)>,
            Option<(Frame, Frame)>,
        ),
        PairError,
    > {
        if ciphertext.len() > MAX_MESSAGE_BYTES {
            return Err(PairError::Format);
        }
        let shared = self.shared.ok_or(PairError::Format)?;
        let (mut send, mut receive) = pair_frames(&shared.ke, Side::A);
        let plain = receive.open(ciphertext).map_err(|_| PairError::Format)?;
        let sealed: NodeSealed = serde_json::from_slice(&plain).map_err(|_| PairError::Format)?;
        let node_client = self.flags.is_some();
        if node_client {
            verify_transcript(
                &STANDARD.encode(self.node_ed25519.as_bytes()),
                &shared.transcript,
                sealed.sig.as_deref(),
            )?;
        }
        let cluster_pub = VerifyingKey::from_bytes(&decode32(&sealed.cluster_pub)?)
            .map_err(|_| PairError::Format)?;
        let certificate = Certificate::from_base64(&sealed.cert).map_err(|_| PairError::Format)?;
        match certificate.verify(&cluster_pub, now) {
            Ok(()) => {}
            // A node being re-admitted presents the certificate that expired; its key is
            // proven by `sig`, and the direction rule decides what happens next.
            Err(crate::identity::CertificateError::Expired) if node_client => {}
            Err(_) => return Err(PairError::Format),
        }
        if cluster_pub.is_weak()
            || certificate.node_id != self.node_id
            || certificate.cluster_id != sealed.cluster_id
            || ClusterId::derive(&cluster_pub).as_str() != sealed.cluster_id
            || certificate.node_pub != STANDARD.encode(self.node_ed25519.as_bytes())
        {
            return Err(PairError::Format);
        }
        let node_x25519 = PublicKey::from(decode32(&sealed.x25519)?);
        if !self.x25519.diffie_hellman(&node_x25519).was_contributory() {
            return Err(PairError::Format);
        }
        let node_flags = match (node_client, sealed.disposable, sealed.expired) {
            (false, _, _) => None,
            (true, Some(disposable), Some(expired)) => Some((
                Flags {
                    disposable,
                    expired,
                },
                sealed.label.and_then(|l| clean(&l, MAX_LABEL_CHARS)),
            )),
            (true, _, _) => return Err(PairError::Format),
        };
        let message = encode(&ClientSealed {
            x25519: STANDARD.encode(PublicKey::from(&self.x25519).as_bytes()),
            label: label.map(str::to_owned),
            ua: user_agent.map(str::to_owned),
            sig: self
                .flags
                .map(|_| sign_transcript(&self.ed25519, &shared.transcript)),
            disposable: self.flags.map(|f| f.disposable),
            expired: self.flags.map(|f| f.expired),
        })?;
        let sealed_bytes = send
            .seal(message.as_bytes())
            .map_err(|_| PairError::Format)?;
        let frames = node_client.then_some((send, receive));
        Ok((
            ClientPaired {
                device: self.device,
                ed25519: self.ed25519,
                x25519: self.x25519,
                node_id: self.node_id,
                node_ed25519: self.node_ed25519,
                node_x25519,
                certificate,
                cluster_id: sealed.cluster_id,
                cluster_pub,
                flags: self.flags,
            },
            sealed_bytes,
            node_flags,
            frames,
        ))
    }

    /// The kind this client declared.
    #[must_use]
    pub fn kind(&self) -> &str {
        &self.kind
    }
}

/// The two frames of `§7.4.2`: `HKDF-Expand(HKDF-Extract(salt = "pv/1 pair", ikm =
/// K_pair), "pv/1 c2s" | "pv/1 s2c", 32)`, counters from zero. Returns `(send, receive)`
/// for the side given.
fn pair_frames(ke: &[u8; 16], side: Side) -> (Frame, Frame) {
    let hk = hkdf::Hkdf::<Sha256>::new(Some(PAIR_SALT), ke);
    let mut c2s = Zeroizing::new([0u8; 32]);
    let mut s2c = Zeroizing::new([0u8; 32]);
    // 32 bytes is inside HKDF's output bound; the branches cannot fail.
    if hk.expand(b"pv/1 c2s", c2s.as_mut()).is_err()
        || hk.expand(b"pv/1 s2c", s2c.as_mut()).is_err()
    {
        c2s.fill(0);
        s2c.fill(0);
    }
    let c2s = Frame::new(*c2s, FrameDirection::C2s);
    let s2c = Frame::new(*s2c, FrameDirection::S2c);
    match side {
        Side::A => (c2s, s2c),
        Side::B => (s2c, c2s),
    }
}

/// Trim, drop control characters, and cut to `max` characters; `None` when nothing is
/// left.
fn clean(text: &str, max: usize) -> Option<String> {
    let cleaned: String = text.chars().filter(|c| !c.is_control()).take(max).collect();
    let trimmed = cleaned.trim();
    (!trimmed.is_empty()).then(|| trimmed.to_owned())
}

fn parse<T: DeserializeOwned>(text: &str) -> Result<T, PairError> {
    if text.len() > MAX_MESSAGE_BYTES {
        return Err(PairError::Format);
    }
    serde_json::from_str(text).map_err(|_| PairError::Format)
}

fn encode(value: &impl Serialize) -> Result<String, PairError> {
    serde_json::to_string(value).map_err(|_| PairError::Format)
}

/// 32 bytes of standard padded base64, and only that encoding of them.
fn decode32(text: &str) -> Result<[u8; 32], PairError> {
    if text.len() != 44 {
        return Err(PairError::Format);
    }
    let bytes = STANDARD.decode(text).map_err(|_| PairError::Format)?;
    if STANDARD.encode(&bytes) != text {
        return Err(PairError::Format);
    }
    bytes.try_into().map_err(|_| PairError::Format)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn flags(disposable: bool, expired: bool) -> Flags {
        Flags {
            disposable,
            expired,
        }
    }

    /// The direction rule of `spec/protocol.md §2.3.1` over every combination of the
    /// four flags: an expired side never admits and is admitted only by an established
    /// member, a disposable dialer joins the window's node, and two established nodes
    /// are refused.
    #[test]
    fn test_spec_2_3_1_direction_rule_over_every_flag_combination() {
        let fresh = flags(true, false);
        let established = flags(false, false);
        let expired = flags(false, true);
        let expired_fresh = flags(true, true);
        assert_eq!(direction(established, fresh), Ok(Direction::NodeAdmits));
        assert_eq!(direction(fresh, fresh), Ok(Direction::NodeAdmits));
        assert_eq!(direction(fresh, established), Ok(Direction::ClientAdmits));
        assert_eq!(
            direction(established, established),
            Err(PairError::TwoClusters)
        );
        assert_eq!(direction(established, expired), Ok(Direction::NodeAdmits));
        assert_eq!(direction(expired, established), Ok(Direction::ClientAdmits));
        assert!(matches!(
            direction(fresh, expired),
            Err(PairError::NoAdmitter(_))
        ));
        assert!(matches!(
            direction(expired, fresh),
            Err(PairError::NoAdmitter(_))
        ));
        assert!(matches!(
            direction(expired, expired),
            Err(PairError::NoAdmitter(_))
        ));
        assert!(matches!(
            direction(expired_fresh, established),
            Ok(Direction::ClientAdmits)
        ));
        assert!(matches!(
            direction(established, expired_fresh),
            Ok(Direction::NodeAdmits)
        ));
        assert!(matches!(
            direction(expired_fresh, expired),
            Err(PairError::NoAdmitter(_))
        ));
    }
}
