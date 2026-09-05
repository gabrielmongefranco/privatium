// Project:  Privatium™  |  File: crates/privatium-core/src/session/mod.rs
// Authors:  Gabriel Mongefranco (@gabrielmongefranco)
// Created:  2026-09-05  |  Modified: 2026-09-05
// Summary:  Directional session keys and authenticated frames with owned counters
//           and terminal refusals (spec/protocol.md §8, §8.3).

//! Session cryptography without transport or storage side effects.

use chacha20poly1305::{ChaCha20Poly1305, KeyInit, aead::Aead};
use sha2::{Digest, Sha256};
use x25519_dalek::{PublicKey, StaticSecret};
use zeroize::{Zeroize, Zeroizing};

pub mod handshake;

/// Protocol refusal without peer input, keys, or plaintext in its message.
#[derive(Debug, thiserror::Error, PartialEq, Eq)]
pub enum SessionError {
    /// A message, encoding, identifier, or key was invalid.
    #[error("cannot establish session: invalid message or key; reconnect")]
    Format,
    /// The peer does not speak the supported major version.
    #[error("cannot establish session: protocol version differs; use a pv/1 client")]
    Version,
    /// No active device with usable pins authorizes this handshake.
    #[error("cannot establish session: device is not authorized; pair this device again")]
    Unauthorized,
    /// A pinned identity or certificate failed verification.
    #[error(
        "cannot establish session: pinned identity verification failed; explicitly re-pair to proceed"
    )]
    PinnedKey,
    /// Authentication failed; the frame state must never be used again.
    #[error("cannot authenticate session frame; close the connection and reconnect")]
    Authentication,
    /// A previous failure or the frame budget ended this direction.
    #[error("cannot use closed session; reconnect with fresh ephemeral keys")]
    Closed,
}

impl SessionError {
    /// WebSocket close code for this refusal (`spec/protocol.md §8.3`).
    #[must_use]
    pub fn close_code(&self) -> u16 {
        match self {
            Self::Format | Self::Version => 4400,
            _ => 4403,
        }
    }
}

/// A result whose error never includes peer-controlled content.
pub type Result<T> = std::result::Result<T, SessionError>;

/// Which endpoint owns a key schedule; determines send and receive directions.
#[derive(Debug, Clone, Copy)]
pub enum Role {
    /// Paired device initiating the channel.
    Client,
    /// Node answering the channel.
    Node,
}

/// The fixed nonce prefix (`spec/protocol.md §8.3`).
#[derive(Debug, Clone, Copy)]
pub enum Direction {
    /// Client to node: big-endian 1.
    C2s = 1,
    /// Node to client: big-endian 2.
    S2c = 2,
}

/// Derived keys, wiped on drop. Not cloneable: convert once into owned frame counters.
pub struct Keys {
    role: Role,
    c2s: Zeroizing<[u8; 32]>,
    s2c: Zeroizing<[u8; 32]>,
}

impl Keys {
    /// Derive directional keys from pinned statics and fresh ephemerals, binding both
    /// eight-character IDs. Refuses malformed IDs and noncontributory X25519 keys.
    /// Caller must use new ephemeral secrets for every connection. No I/O.
    pub fn derive(
        role: Role,
        my_static: &StaticSecret,
        their_static: &PublicKey,
        my_eph: &StaticSecret,
        their_eph: &PublicKey,
        node_id: &str,
        device_id: &str,
    ) -> Result<Self> {
        validate_id(node_id)?;
        validate_id(device_id)?;
        let ss = my_static.diffie_hellman(their_static);
        let ee = my_eph.diffie_hellman(their_eph);
        if !ss.was_contributory() || !ee.was_contributory() {
            return Err(SessionError::Format);
        }
        let mut ids = [node_id, device_id];
        ids.sort_unstable();
        let mut salt = Sha256::new();
        salt.update(ids[0].as_bytes());
        salt.update(ids[1].as_bytes());
        salt.update(b"pv/1 session");
        let mut input = Zeroizing::new([0u8; 64]);
        input[..32].copy_from_slice(ss.as_bytes());
        input[32..].copy_from_slice(ee.as_bytes());
        let hk = hkdf::Hkdf::<Sha256>::new(Some(&salt.finalize()), input.as_ref());
        let mut c2s = Zeroizing::new([0u8; 32]);
        let mut s2c = Zeroizing::new([0u8; 32]);
        hk.expand(b"pv/1 c2s", c2s.as_mut())
            .map_err(|_| SessionError::Format)?;
        hk.expand(b"pv/1 s2c", s2c.as_mut())
            .map_err(|_| SessionError::Format)?;
        Ok(Self { role, c2s, s2c })
    }

    /// Borrow a directional key for protocol interoperability. Never persist or log it,
    /// or initialize multiple senders with it: that would repeat nonces. No side effects.
    #[must_use]
    pub fn key(&self, direction: Direction) -> &[u8; 32] {
        match direction {
            Direction::C2s => &self.c2s,
            Direction::S2c => &self.s2c,
        }
    }

    /// Consume keys into (send, receive) frames for this role, each starting at zero.
    /// No other frame state may be initialized with the same keys.
    #[must_use]
    pub fn into_frames(self) -> (Frame, Frame) {
        let c2s = Frame::new(*self.c2s, Direction::C2s);
        let s2c = Frame::new(*self.s2c, Direction::S2c);
        match self.role {
            Role::Client => (c2s, s2c),
            Role::Node => (s2c, c2s),
        }
    }
}

/// One direction's key and counter. Authentication failure permanently closes it.
/// The transport must close the whole connection on any error from seal or open.
pub struct Frame {
    key: Zeroizing<[u8; 32]>,
    direction: Direction,
    counter: u64,
    limit: u64,
    closed: bool,
}

impl Frame {
    /// Own a key with a fresh direction counter. Use only once per directional key;
    /// subsequent connections need fresh keys. No I/O; key is wiped on drop.
    #[must_use]
    pub fn new(mut key: [u8; 32], direction: Direction) -> Self {
        let result = Self {
            key: Zeroizing::new(key),
            direction,
            counter: 0,
            limit: 1u64 << 32,
            closed: false,
        };
        key.zeroize();
        result
    }

    fn nonce(&mut self) -> Result<[u8; 12]> {
        if self.closed || self.counter >= self.limit {
            self.close();
            return Err(SessionError::Closed);
        }
        let mut nonce = [0; 12];
        nonce[..4].copy_from_slice(&(self.direction as u32).to_be_bytes());
        nonce[4..].copy_from_slice(&self.counter.to_be_bytes());
        self.counter += 1;
        Ok(nonce)
    }

    /// Seal one plaintext frame with no associated data, advancing the nonce once.
    /// Returns ciphertext including its tag, or a terminal error; never performs I/O.
    pub fn seal(&mut self, plaintext: &[u8]) -> Result<Vec<u8>> {
        let nonce = self.nonce()?;
        let cipher = ChaCha20Poly1305::new((&*self.key).into());
        self.complete(cipher.encrypt((&nonce).into(), plaintext))
    }

    /// Authenticate one frame in order and return its plaintext. Truncation, tampering,
    /// replay, or exhaustion closes this state permanently; no plaintext escapes on error.
    pub fn open(&mut self, ciphertext: &[u8]) -> Result<Vec<u8>> {
        let nonce = self.nonce()?;
        let cipher = ChaCha20Poly1305::new((&*self.key).into());
        self.complete(cipher.decrypt((&nonce).into(), ciphertext))
    }

    fn complete(
        &mut self,
        result: std::result::Result<Vec<u8>, chacha20poly1305::Error>,
    ) -> Result<Vec<u8>> {
        match result {
            Ok(bytes) => {
                if self.counter == self.limit {
                    self.close();
                }
                Ok(bytes)
            }
            Err(_) => {
                self.close();
                Err(SessionError::Authentication)
            }
        }
    }

    /// End this direction and wipe its key. The transport must also close its peer
    /// direction and connection. Future seal/open calls refuse without processing input.
    pub fn close(&mut self) {
        self.closed = true;
        self.key.zeroize();
    }

    /// Whether a failure, explicit close, or the last allowed frame ended this direction.
    /// Transports must check after successful frames too and close at the budget boundary.
    #[must_use]
    pub fn is_closed(&self) -> bool {
        self.closed
    }
}

/// Connection frame states after key agreement. Node-side construction requires the
/// client's encrypted confirm; client-side peer proof arrives with its first valid frame.
pub struct Session {
    /// Device ID bound to this connection, not an authorization token for other calls.
    pub device: String,
    /// Outbound frame state; the client confirm already consumed c2s counter zero.
    pub send: Frame,
    /// Inbound frame state, continuing after any consumed handshake frame.
    pub receive: Frame,
}

fn validate_id(id: &str) -> Result<()> {
    if id.len() == 8
        && id
            .bytes()
            .all(|b| b"0123456789abcdefghjkmnpqrstvwxyz".contains(&b))
    {
        Ok(())
    } else {
        Err(SessionError::Format)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_spec_8_a_session_closes_at_the_counter_limit() {
        let mut send = Frame::new([7; 32], Direction::C2s);
        let mut receive = Frame::new([7; 32], Direction::C2s);
        send.limit = 2;
        receive.limit = 2;
        for _ in 0..2 {
            let Ok(sealed) = send.seal(b"synthetic") else {
                panic!("seal")
            };
            assert_eq!(
                receive.open(&sealed).ok().as_deref(),
                Some(b"synthetic".as_slice())
            );
        }
        assert_eq!(send.seal(b""), Err(SessionError::Closed));
        assert_eq!(receive.open(b""), Err(SessionError::Closed));
        assert!(send.is_closed());
        assert!(receive.is_closed());
        assert_eq!(*send.key, [0; 32]);
        assert_eq!(*receive.key, [0; 32]);
        let mut boundary = Frame::new([7; 32], Direction::C2s);
        boundary.counter = (1u64 << 32) - 1;
        assert!(boundary.seal(b"").is_ok());
        assert!(boundary.is_closed());
        assert_eq!(boundary.seal(b""), Err(SessionError::Closed));
    }
}
