// Project:  Privatium™  |  File: crates/privatium-core/src/pair/handshake.rs
// Authors:  Gabriel Mongefranco (@gabrielmongefranco)
// Created:  2026-09-05  |  Modified: 2026-09-05
// Summary:  The six messages of /ws/pair (spec/protocol.md §7.4.2) as data, for both
//           roles: the node's side takes the window and the identity and yields what to
//           send, the client's side takes a code and yields the same; neither touches a
//           socket, a log or a store.

use std::net::IpAddr;

use base64::{Engine as _, engine::general_purpose::STANDARD};
use ed25519_dalek::VerifyingKey;
use serde::{Deserialize, Serialize, de::DeserializeOwned};
use sha2::Sha256;
use x25519_dalek::{PublicKey, StaticSecret};
use zeroize::Zeroizing;

use super::code::Code;
use super::spake2::{Identities, Shared, Side, State};
use super::{PairError, Pairing};
use crate::identity::{Certificate, ClusterId, Identity, NodeId};
use crate::session::{Direction, Frame};

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

/// The node's sealed message: what a device pins (`spec/protocol.md §7.4` step 5).
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
}

/// The client's sealed message: its X25519 key and what the devices page shows.
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
        match start.kind.as_str() {
            "browser" | "desktop" | "mobile" => {}
            "node" => return Err(PairError::NodeKind),
            _ => return Err(PairError::Format),
        }
        let pa = decode32(&start.pa)?;

        let w = pairing.begin_attempt(source, now)?;
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

    /// Verify the device's `cA`. A confirmation that does not verify is the wrong code:
    /// the failure is recorded on the window and [`PairError::WrongCode`] returned. On
    /// success the node's sealed message is returned as the bytes to send, and the
    /// state moves on to the device's sealed message.
    pub fn confirm(
        self,
        identity: &Identity,
        pairing: &mut Pairing,
        text: &str,
    ) -> Result<(Sealed, Vec<u8>), PairError> {
        let confirm: ClientConfirm = parse(text)?;
        let ca = decode32(&confirm.ca)?;
        if !self.shared.verify(&ca) {
            pairing.record_failure()?;
            return Err(PairError::WrongCode);
        }
        let (mut send, receive) = pair_frames(&self.shared.ke, Side::B);
        let message = encode(&NodeSealed {
            x25519: identity.x25519_public_base64(),
            cert: identity
                .certificate()
                .to_base64()
                .map_err(|_| PairError::Format)?,
            cluster_id: identity.cluster_id().to_string(),
            cluster_pub: STANDARD.encode(identity.cluster_public().as_bytes()),
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
                receive,
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
    receive: Frame,
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

    /// Open the device's sealed message and produce the facts of its `sys_device` row.
    /// The X25519 key must be one this node can actually agree a key with; a label and
    /// a user agent are trimmed to their column's size and stripped of control
    /// characters, never refused.
    pub fn finish(mut self, identity: &Identity, ciphertext: &[u8]) -> Result<Paired, PairError> {
        if ciphertext.len() > MAX_MESSAGE_BYTES {
            return Err(PairError::Format);
        }
        let plain = self
            .receive
            .open(ciphertext)
            .map_err(|_| PairError::Format)?;
        let sealed: ClientSealed = serde_json::from_slice(&plain).map_err(|_| PairError::Format)?;
        let x25519 = PublicKey::from(decode32(&sealed.x25519)?);
        if !identity
            .x25519_static()
            .diffie_hellman(&x25519)
            .was_contributory()
        {
            return Err(PairError::Format);
        }
        Ok(Paired {
            device: self.device,
            kind: self.kind,
            ed25519_pub: self.device_pub,
            x25519_pub: sealed.x25519,
            label: sealed.label.and_then(|l| clean(&l, MAX_LABEL_CHARS)),
            user_agent: sealed.ua.and_then(|u| clean(&u, MAX_USER_AGENT_CHARS)),
            source: self.source,
        })
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

/// The device's side of `/ws/pair`, in Rust — for the framework's own tests and for a
/// native client. Not `Clone`; wiped on drop.
pub struct Client {
    device: String,
    ed25519: ed25519_dalek::SigningKey,
    x25519: StaticSecret,
    kind: String,
    node_id: String,
    node_ed25519: VerifyingKey,
    state: Option<State>,
    shared: Option<Shared>,
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
            },
            text,
        ))
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
        if ciphertext.len() > MAX_MESSAGE_BYTES {
            return Err(PairError::Format);
        }
        let shared = self.shared.ok_or(PairError::Format)?;
        let (mut send, mut receive) = pair_frames(&shared.ke, Side::A);
        let plain = receive.open(ciphertext).map_err(|_| PairError::Format)?;
        let sealed: NodeSealed = serde_json::from_slice(&plain).map_err(|_| PairError::Format)?;
        let cluster_pub = VerifyingKey::from_bytes(&decode32(&sealed.cluster_pub)?)
            .map_err(|_| PairError::Format)?;
        let certificate = Certificate::from_base64(&sealed.cert).map_err(|_| PairError::Format)?;
        certificate
            .verify(&cluster_pub, now)
            .map_err(|_| PairError::Format)?;
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
        let message = encode(&ClientSealed {
            x25519: STANDARD.encode(PublicKey::from(&self.x25519).as_bytes()),
            label: label.map(str::to_owned),
            ua: user_agent.map(str::to_owned),
        })?;
        let sealed_bytes = send
            .seal(message.as_bytes())
            .map_err(|_| PairError::Format)?;
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
            },
            sealed_bytes,
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
    let c2s = Frame::new(*c2s, Direction::C2s);
    let s2c = Frame::new(*s2c, Direction::S2c);
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
