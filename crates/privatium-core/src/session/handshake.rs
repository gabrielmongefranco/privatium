// Project:  Privatium™  |  File: crates/privatium-core/src/session/handshake.rs
// Authors:  Gabriel Mongefranco (@gabrielmongefranco)
// Created:  2026-09-05  |  Modified: 2026-09-05
// Summary:  Transport-independent hello and confirmation state machines. Pins and
//           certificate verification gate session establishment (spec/protocol.md §8.3).

//! Single-use handshake states; callers transport bytes without reserializing hellos.

use base64::{Engine as _, engine::general_purpose::STANDARD};
use ed25519_dalek::VerifyingKey;
use serde::{Deserialize, Serialize, de::DeserializeOwned};
use sha2::{Digest, Sha256};
use x25519_dalek::{PublicKey, StaticSecret};

use super::{Keys, Result, Role, Session, SessionError, validate_id};
use crate::identity::{Certificate, Identity};

// Certificates are bounded to 4096 bytes; a base64 certificate plus hello fits here.
const MAX_HELLO_BYTES: usize = 8192;

/// First text frame, sent by the device (`spec/protocol.md §8.3`).
#[derive(Debug, Serialize, Deserialize)]
pub struct ClientHello {
    /// Protocol major; only 1 is accepted.
    pub v: u32,
    /// Device ID used to look up active pairing pins.
    pub dev: String,
    /// Standard padded base64 of a fresh X25519 ephemeral public key.
    pub e: String,
}

/// Second text frame, sent by the node (`spec/protocol.md §8.3`).
#[derive(Debug, Serialize, Deserialize)]
pub struct NodeHello {
    /// Protocol major; only 1 is accepted.
    pub v: u32,
    /// Node ID, which must agree with the pinned node and certificate.
    pub id: String,
    /// Standard padded base64 of a fresh X25519 ephemeral public key.
    pub e: String,
    /// Base64 cluster-signed membership certificate.
    pub cert: String,
}

/// First encrypted c2s frame, authenticating the exact text-frame bytes.
#[derive(Debug, Serialize, Deserialize)]
pub struct Confirm {
    /// Lowercase hexadecimal SHA-256 of client hello followed by node hello.
    #[serde(rename = "confirm")]
    pub transcript: String,
}

/// Pairing registry facts supplied by the trusted caller, never by the hello.
#[derive(Debug, Clone)]
pub struct DevicePins {
    /// Stored X25519 key, standard padded base64; absence refuses access.
    pub x25519: Option<String>,
    /// True whenever the registry's revoked_at is set.
    pub revoked: bool,
}

/// Previously paired node identity. No network message may replace these pins.
pub struct NodePins {
    /// Node ID associated with the stored static key.
    pub id: String,
    /// Cluster public key pinned through pairing.
    pub cluster: VerifyingKey,
    /// Node X25519 public key exchanged through pairing.
    pub x25519: PublicKey,
}

/// Node handshake entry point. Performs no network, log, or registry writes.
pub struct Handshake;

impl Handshake {
    /// Answer exact client hello bytes using verified local identity and active registry
    /// pins from lookup. Generates a fresh ephemeral and returns a single-use pending
    /// state and node hello text. Refuses invalid/versioned input and unauthorized devices.
    /// Lookup must return None for absent or untrusted pairing records.
    pub fn node(
        identity: &Identity,
        lookup: impl FnOnce(&str) -> Option<DevicePins>,
        hello: &str,
    ) -> Result<(PendingNode, String)> {
        let client: ClientHello = parse(hello)?;
        version(client.v)?;
        validate_id(&client.dev)?;
        let pins = lookup(&client.dev)
            .filter(|p| !p.revoked)
            .ok_or(SessionError::Unauthorized)?;
        let static_key = decode_key(pins.x25519.as_deref().ok_or(SessionError::Unauthorized)?)
            .map_err(|_| SessionError::Unauthorized)?;
        let their_eph = decode_key(&client.e)?;
        let ephemeral = fresh_secret();
        let node_hello = encode(&NodeHello {
            v: 1,
            id: identity.id().to_string(),
            e: STANDARD.encode(PublicKey::from(&ephemeral).as_bytes()),
            cert: identity
                .certificate()
                .to_base64()
                .map_err(|_| SessionError::PinnedKey)?,
        })?;
        let keys = Keys::derive(
            Role::Node,
            &identity.x25519_static(),
            &static_key,
            &ephemeral,
            &their_eph,
            identity.id().as_str(),
            &client.dev,
        )
        .map_err(|_| SessionError::Unauthorized)?;
        let (send, receive) = keys.into_frames();
        Ok((
            PendingNode {
                session: Session {
                    device: client.dev,
                    send,
                    receive,
                },
                transcript: transcript(hello, &node_hello),
            },
            node_hello,
        ))
    }
}

/// Node state that cannot release its session until one valid confirmation arrives.
pub struct PendingNode {
    session: Session,
    transcript: String,
}

impl PendingNode {
    /// Consume the pending handshake and authenticate c2s frame zero and its transcript.
    /// Returns the session with the receive counter already advanced. Any failure drops
    /// both keys and requires WebSocket close 4403. No I/O or partial session escapes.
    pub fn confirm(mut self, ciphertext: &[u8]) -> Result<Session> {
        if ciphertext.len() > MAX_HELLO_BYTES {
            return Err(SessionError::Authentication);
        }
        let bytes = self.session.receive.open(ciphertext)?;
        let confirm: Confirm =
            serde_json::from_slice(&bytes).map_err(|_| SessionError::Authentication)?;
        if confirm.transcript != self.transcript {
            return Err(SessionError::Authentication);
        }
        Ok(self.session)
    }
}

/// Client state owning a fresh ephemeral and the exact hello it emitted. Not cloneable.
pub struct ClientHandshake {
    device: String,
    static_key: StaticSecret,
    ephemeral: StaticSecret,
    pins: NodePins,
    hello: String,
}

impl ClientHandshake {
    /// Start with previously paired pins and the device's static secret. Generates fresh
    /// ephemeral bytes and returns the state plus its hello text; invalid IDs refuse.
    /// No I/O; send the returned text without reserializing it.
    pub fn start(device: &str, static_key: StaticSecret, pins: NodePins) -> Result<(Self, String)> {
        validate_id(device)?;
        validate_id(&pins.id)?;
        let ephemeral = fresh_secret();
        let hello = encode(&ClientHello {
            v: 1,
            dev: device.into(),
            e: STANDARD.encode(PublicKey::from(&ephemeral).as_bytes()),
        })?;
        Ok((
            Self {
                device: device.into(),
                static_key,
                ephemeral,
                pins,
                hello: hello.clone(),
            },
            hello,
        ))
    }

    /// Consume a node hello, checking its certificate against pinned cluster identity at
    /// UTC now before sealing a confirm. Returns session and c2s frame zero; first valid
    /// inbound frame proves node possession of the pinned static key. Refuses malformed,
    /// incompatible, expired, or mismatched identity with no override and no I/O.
    pub fn finish(self, hello: &str, now: jiff::Timestamp) -> Result<(Session, Vec<u8>)> {
        let node: NodeHello = parse(hello)?;
        version(node.v)?;
        let cert = Certificate::from_base64(&node.cert).map_err(|_| SessionError::PinnedKey)?;
        cert.verify(&self.pins.cluster, now)
            .map_err(|_| SessionError::PinnedKey)?;
        if node.id != self.pins.id || cert.node_id != node.id {
            return Err(SessionError::PinnedKey);
        }
        let keys = Keys::derive(
            Role::Client,
            &self.static_key,
            &self.pins.x25519,
            &self.ephemeral,
            &decode_key(&node.e)?,
            &node.id,
            &self.device,
        )?;
        let (mut send, receive) = keys.into_frames();
        let confirm = send.seal(
            encode(&Confirm {
                transcript: transcript(&self.hello, hello),
            })?
            .as_bytes(),
        )?;
        Ok((
            Session {
                device: self.device,
                send,
                receive,
            },
            confirm,
        ))
    }
}

fn fresh_secret() -> StaticSecret {
    use rand::RngExt as _;
    let bytes = zeroize::Zeroizing::new(rand::rng().random::<[u8; 32]>());
    StaticSecret::from(*bytes)
}

fn parse<T: DeserializeOwned>(text: &str) -> Result<T> {
    if text.len() > MAX_HELLO_BYTES {
        return Err(SessionError::Format);
    }
    serde_json::from_str(text).map_err(|_| SessionError::Format)
}

fn encode(value: &impl Serialize) -> Result<String> {
    serde_json::to_string(value).map_err(|_| SessionError::Format)
}

fn version(v: u32) -> Result<()> {
    if v == 1 {
        Ok(())
    } else {
        Err(SessionError::Version)
    }
}

fn decode_key(text: &str) -> Result<PublicKey> {
    if text.len() != 44 {
        return Err(SessionError::Format);
    }
    let bytes = STANDARD.decode(text).map_err(|_| SessionError::Format)?;
    if STANDARD.encode(&bytes) != text {
        return Err(SessionError::Format);
    }
    let bytes: [u8; 32] = bytes.try_into().map_err(|_| SessionError::Format)?;
    Ok(PublicKey::from(bytes))
}

fn transcript(client: &str, node: &str) -> String {
    let mut hash = Sha256::new();
    hash.update(client.as_bytes());
    hash.update(node.as_bytes());
    hash.finalize().iter().map(|b| format!("{b:02x}")).collect()
}
