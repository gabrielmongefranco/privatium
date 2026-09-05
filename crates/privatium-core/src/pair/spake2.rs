// Project:  Privatium™  |  File: crates/privatium-core/src/pair/spake2.rs
// Authors:  Gabriel Mongefranco (@gabrielmongefranco)
// Created:  2026-09-05  |  Modified: 2026-09-05
// Summary:  SPAKE2 as RFC 9382 specifies it, over edwards25519 with the RFC's M and N,
//           in the ciphersuite spec/protocol.md §7.4.1 fixes: the password scalar, the
//           two messages, the transcript, the key schedule and the confirmation MACs.

use curve25519_dalek::edwards::{CompressedEdwardsY, EdwardsPoint};
use curve25519_dalek::scalar::Scalar;
use curve25519_dalek::traits::IsIdentity as _;
use hmac::{Hmac, KeyInit as _, Mac as _};
use sha2::{Digest, Sha256};
use zeroize::{Zeroize, Zeroizing};

use super::code::Code;

/// RFC 9382 §4, the `M` point for edwards25519, as its 32-byte RFC 8032 encoding.
pub const M: [u8; 32] = [
    0xd0, 0x48, 0x03, 0x2c, 0x6e, 0xa0, 0xb6, 0xd6, 0x97, 0xdd, 0xc2, 0xe8, 0x6b, 0xda, 0x85, 0xa3,
    0x3a, 0xda, 0xc9, 0x20, 0xf1, 0xbf, 0x18, 0xe1, 0xb0, 0xc6, 0xd1, 0x66, 0xa5, 0xce, 0xcd, 0xaf,
];

/// RFC 9382 §4, the `N` point for edwards25519.
pub const N: [u8; 32] = [
    0xd3, 0xbf, 0xb5, 0x18, 0xf4, 0x4f, 0x34, 0x30, 0xf2, 0x9d, 0x0c, 0x92, 0xaf, 0x50, 0x38, 0x65,
    0xa1, 0xed, 0x32, 0x81, 0xdc, 0x69, 0xb3, 0x5d, 0xd8, 0x68, 0xba, 0x85, 0xf8, 0x86, 0xc4, 0xab,
];

/// The HKDF `info` that turns a code into `w` (`spec/protocol.md §7.4.1`).
const PASSWORD_INFO: &[u8] = b"pv/1 pake w";

/// RFC 9382 §3.3: the KDF `info` for the two confirmation keys, with no associated data.
const CONFIRMATION_INFO: &[u8] = b"ConfirmationKeys";

/// Half of a SHA-256 digest: the size of `Ke`, `Ka`, `KcA` and `KcB`.
const HALF: usize = 16;

/// What went wrong in a run of the protocol. Nothing here carries peer input.
#[derive(Debug, thiserror::Error, PartialEq, Eq)]
pub enum Spake2Error {
    /// The peer's message is not a canonical encoding of a point in the prime-order
    /// subgroup's coset, or the shared point is the identity.
    #[error("cannot pair: the other side sent an invalid key-exchange message")]
    InvalidMessage,
    /// The password scalar or a fresh secret reduced to zero — a one in 2²⁵² event that
    /// is refused rather than reasoned about.
    #[error("cannot pair: a zero scalar was drawn; try again")]
    ZeroScalar,
}

/// Which party this side is (`spec/protocol.md §7.4.1`): the client is `A`, the node `B`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Side {
    /// The device pairing: sends `pA`, verifies `cB`, then sends `cA`.
    A,
    /// The node: answers `pA` with `pB` and `cB`, then verifies `cA`.
    B,
}

/// The two identities the transcript binds (`§7.4.1`): `"pv/1 device "` and `"pv/1 node "`
/// followed by the respective Ed25519 public key in base64.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Identities {
    a: Vec<u8>,
    b: Vec<u8>,
}

impl Identities {
    /// Build the pair from the two base64 public keys as they travel on the wire.
    #[must_use]
    pub fn new(device_ed25519_base64: &str, node_ed25519_base64: &str) -> Self {
        Self {
            a: [b"pv/1 device ", device_ed25519_base64.as_bytes()].concat(),
            b: [b"pv/1 node ", node_ed25519_base64.as_bytes()].concat(),
        }
    }
}

/// The password scalar `w` (`§7.4.1`): `HKDF-SHA256(ikm = the code's two bytes, big-endian;
/// salt = empty; info = "pv/1 pake w")`, 64 bytes read as a little-endian integer and
/// reduced modulo the group order.
///
/// Sixteen bits is the whole input, so no memory-hard function is applied
/// (`spec/data-dictionary.md §3.3`); the protection is the attempt limit, not the cost of
/// a guess.
pub fn password(code: Code) -> Result<Zeroizing<Scalar>, Spake2Error> {
    let hk = hkdf::Hkdf::<Sha256>::new(None, &code.bytes());
    let mut wide = Zeroizing::new([0u8; 64]);
    // 64 bytes is far inside HKDF's 255-block limit; the error cannot occur.
    hk.expand(PASSWORD_INFO, wide.as_mut())
        .map_err(|_| Spake2Error::ZeroScalar)?;
    let w = Scalar::from_bytes_mod_order_wide(&wide);
    if w == Scalar::ZERO {
        return Err(Spake2Error::ZeroScalar);
    }
    Ok(Zeroizing::new(w))
}

/// One side's state between sending its message and finishing. Not `Clone`: a secret
/// scalar is used once. Wiped on drop.
pub struct State {
    side: Side,
    w: Scalar,
    secret: Scalar,
    message: [u8; 32],
}

impl Drop for State {
    fn drop(&mut self) {
        self.w.zeroize();
        self.secret.zeroize();
    }
}

impl State {
    /// Begin as `side` with the password scalar and a fresh CSPRNG secret.
    pub fn start(side: Side, w: &Scalar) -> Result<Self, Spake2Error> {
        use rand::RngExt as _;
        let wide = Zeroizing::new(rand::rng().random::<[u8; 64]>());
        Self::start_with(side, w, &wide)
    }

    /// Begin as `side` with a caller-supplied 64-byte secret, reduced as [`password`]
    /// reduces its input. This is what makes a run reproducible against
    /// `tests/fixtures/pake-vectors.json`; in production the secret is CSPRNG output
    /// used once, which [`State::start`] guarantees.
    pub fn start_with(side: Side, w: &Scalar, secret: &[u8; 64]) -> Result<Self, Spake2Error> {
        let secret = Scalar::from_bytes_mod_order_wide(secret);
        if secret == Scalar::ZERO || *w == Scalar::ZERO {
            return Err(Spake2Error::ZeroScalar);
        }
        let blind = match side {
            Side::A => point(&M),
            Side::B => point(&N),
        }?;
        // pA = w·M + x·G, pB = w·N + y·G (RFC 9382 §3.2).
        let message = (blind * w + EdwardsPoint::mul_base(&secret))
            .compress()
            .to_bytes();
        Ok(Self {
            side,
            w: *w,
            secret,
            message,
        })
    }

    /// The message this side sends: `pA` or `pB`, 32 bytes.
    #[must_use]
    pub fn message(&self) -> [u8; 32] {
        self.message
    }

    /// Consume the other side's message and derive the shared secret and both
    /// confirmation values. Refuses a message that is not a canonical point encoding,
    /// that has small order, or that yields the identity as `K`.
    pub fn finish(self, their_message: &[u8; 32], ids: &Identities) -> Result<Shared, Spake2Error> {
        let their = point(their_message)?;
        if their.is_small_order() {
            return Err(Spake2Error::InvalidMessage);
        }
        let unblind = match self.side {
            // A computes h·x·(pB − w·N); B computes h·y·(pA − w·M).
            Side::A => point(&N)?,
            Side::B => point(&M)?,
        };
        let k = ((their - unblind * self.w) * self.secret).mul_by_cofactor();
        if k.is_identity() {
            return Err(Spake2Error::InvalidMessage);
        }
        let (pa, pb) = match self.side {
            Side::A => (self.message, *their_message),
            Side::B => (*their_message, self.message),
        };

        // TT = len(A) ‖ A ‖ len(B) ‖ B ‖ len(pA) ‖ pA ‖ len(pB) ‖ pB ‖ len(K) ‖ K ‖ len(w) ‖ w,
        // every length eight bytes little-endian (RFC 9382 §3.3).
        let mut tt = Zeroizing::new(Vec::with_capacity(ids.a.len() + ids.b.len() + 200));
        for part in [
            ids.a.as_slice(),
            ids.b.as_slice(),
            &pa,
            &pb,
            &k.compress().to_bytes(),
            &self.w.to_bytes(),
        ] {
            tt.extend_from_slice(&(part.len() as u64).to_le_bytes());
            tt.extend_from_slice(part);
        }

        let digest = Sha256::digest(&*tt);
        let mut ke = Zeroizing::new([0u8; HALF]);
        ke.copy_from_slice(&digest[..HALF]);
        let mut keys = Zeroizing::new([0u8; 2 * HALF]);
        hkdf::Hkdf::<Sha256>::new(None, &digest[HALF..])
            .expand(CONFIRMATION_INFO, keys.as_mut())
            .map_err(|_| Spake2Error::InvalidMessage)?;
        let (kca, kcb) = keys.split_at(HALF);
        let (send_key, expect_key) = match self.side {
            Side::A => (kca, kcb),
            Side::B => (kcb, kca),
        };
        let send = mac(send_key, &tt);
        let mut expect = Zeroizing::new([0u8; HALF]);
        expect.copy_from_slice(expect_key);
        Ok(Shared {
            ke,
            confirm_send: send,
            expect_key: expect,
            transcript: tt,
        })
    }
}

/// What a finished run holds: `K_pair`, the confirmation to send, and what is needed to
/// verify the one received. Wiped on drop.
pub struct Shared {
    /// `Ke`, which `spec/protocol.md §7.4` calls `K_pair`.
    pub ke: Zeroizing<[u8; HALF]>,
    /// `cA` or `cB`, whichever this side sends.
    pub confirm_send: [u8; 32],
    expect_key: Zeroizing<[u8; HALF]>,
    /// The transcript `TT`, kept so a vector file can be checked against it.
    pub transcript: Zeroizing<Vec<u8>>,
}

impl Shared {
    /// Whether the other side's confirmation is the MAC this transcript predicts, compared
    /// in constant time. A `false` is the wrong code, and nothing derived from this run
    /// may be used.
    #[must_use]
    pub fn verify(&self, their_confirm: &[u8]) -> bool {
        let Ok(mut mac) = Hmac::<Sha256>::new_from_slice(&*self.expect_key) else {
            return false;
        };
        mac.update(&self.transcript);
        mac.verify_slice(their_confirm).is_ok()
    }
}

fn mac(key: &[u8], tt: &[u8]) -> [u8; 32] {
    let mut out = [0u8; 32];
    // HMAC accepts a key of any length; the empty branch cannot be taken for 16 bytes.
    if let Ok(mut mac) = Hmac::<Sha256>::new_from_slice(key) {
        mac.update(tt);
        out.copy_from_slice(&mac.finalize().into_bytes());
    }
    out
}

/// Decode a point, refusing a non-canonical encoding: the bytes must be exactly what the
/// decoded point re-encodes to, so that two encodings of one point cannot yield two
/// transcripts.
fn point(bytes: &[u8; 32]) -> Result<EdwardsPoint, Spake2Error> {
    let compressed = CompressedEdwardsY(*bytes);
    let point = compressed.decompress().ok_or(Spake2Error::InvalidMessage)?;
    if point.compress() != compressed {
        return Err(Spake2Error::InvalidMessage);
    }
    Ok(point)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The RFC's two constants decode, lie in the prime-order subgroup, and are distinct
    /// from each other and from the base point — a transcription error in either would
    /// fail here before it failed against a browser.
    #[test]
    fn test_spec_7_4_1_m_and_n_are_the_rfc_9382_edwards25519_points() {
        let m = point(&M).ok().filter(EdwardsPoint::is_torsion_free);
        let n = point(&N).ok().filter(EdwardsPoint::is_torsion_free);
        assert!(m.is_some() && n.is_some());
        assert_ne!(m, n);
        let g = Some(curve25519_dalek::constants::ED25519_BASEPOINT_POINT);
        assert_ne!(m, g);
        assert_ne!(n, g);
    }

    #[test]
    fn a_non_canonical_or_small_order_message_is_refused() {
        let Ok(w) = password(Code::from_u16(0x1234)) else {
            panic!("password")
        };
        let ids = Identities::new("", "");
        // The identity (small order), the same with the sign bit set (no such point), and
        // a y past the field prime (a non-canonical encoding).
        let mut identity = [0u8; 32];
        identity[0] = 1;
        let mut signed = identity;
        signed[31] |= 0x80;
        for bad in [identity, signed, [0xff; 32]] {
            let Ok(state) = State::start(Side::A, &w) else {
                panic!("start")
            };
            assert!(state.finish(&bad, &ids).is_err());
        }
    }
}
