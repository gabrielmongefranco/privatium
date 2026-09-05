// Project:  Privatium™  |  File: crates/privatium-core/src/pair/mod.rs
// Authors:  Gabriel Mongefranco (@gabrielmongefranco)
// Created:  2026-09-05  |  Modified: 2026-09-05
// Summary:  The pairing window (spec/protocol.md §7.1, §7.5): one code at a time, held in
//           memory beside its PAKE secret and never written, with its TTL, its attempt
//           cap, its per-source rate limit and the refusals each produces.

use std::collections::BTreeMap;
use std::net::IpAddr;
use std::time::Duration;

use curve25519_dalek::scalar::Scalar;
use serde::Serialize;
use zeroize::Zeroizing;

pub mod code;
pub mod handshake;
mod node;
pub mod spake2;

pub use code::{Code, CodeError, GLYPHS, Glyph, WORDS};

/// `spec/protocol.md §7.5`: a code lives 120 seconds. A window may be opened for less,
/// never for more.
pub const TTL: Duration = Duration::from_secs(120);

/// `§7.5`: five attempts per code; the fifth failure destroys it and issues a new one.
pub const MAX_ATTEMPTS: u8 = 5;

/// `§7.5`: one attempt per two seconds per source.
pub const SOURCE_INTERVAL: Duration = Duration::from_secs(2);

/// Why the node refused a step of pairing. Each carries the WebSocket close code
/// `spec/protocol.md §7.4.2` assigns it, and none carries anything the peer sent.
#[derive(Debug, thiserror::Error, PartialEq, Eq)]
pub enum PairError {
    /// No window is open (`§7.1`).
    #[error("pairing is closed; open it on the node first (spec/protocol.md §7.1)")]
    Closed,
    /// This source tried less than two seconds ago (`§7.5`).
    #[error("too many pairing attempts from this address; wait two seconds and try again")]
    RateLimited,
    /// The code's five attempts are spent; a new code is on the node's screen (`§7.5`).
    #[error("that pairing code is used up; read the new code from the node and try again")]
    Exhausted,
    /// The confirmation did not verify: the other side had a different code (`§7.4.2`).
    #[error("the pairing code did not match; check the code on the node and try again")]
    WrongCode,
    /// A message was malformed, or the device ID did not derive from its key (`§7.4.2`).
    #[error("cannot pair: the other side sent an invalid message")]
    Format,
    /// A node asked to be admitted (`§2.3.1`), which this build does not do.
    #[error(
        "cannot admit a node: node admission is Phase 3 of docs/roadmap.md; spec/protocol.md §2.3.1 is its contract"
    )]
    NodeKind,
    /// The device's key is already in `sys_device`, active or revoked. A key is never
    /// re-registered: a device that lost its pairing generates a new one (`§7.6`).
    #[error(
        "this device key is already registered on the node; generate a new device key and pair again"
    )]
    DeviceKnown,
    /// `Node::pair` was asked for a window outside one second to two minutes.
    #[error("a pairing window lasts between 1 and 120 seconds (spec/protocol.md §7.5)")]
    Ttl,
}

impl PairError {
    /// The WebSocket close code for this refusal (`spec/protocol.md §7.4.2`).
    #[must_use]
    pub fn close_code(&self) -> u16 {
        match self {
            Self::Closed => 4404,
            Self::RateLimited | Self::Exhausted => 4429,
            Self::WrongCode => 4401,
            Self::Format | Self::Ttl => 4400,
            Self::NodeKind | Self::DeviceKnown => 4403,
        }
    }
}

/// The one open pairing window (`spec/data-dictionary.md §3.3`), in memory only.
///
/// The code is kept beside `w` for the length of the window because the window has to be
/// shown again on request (`spec/protocol.md §9.2`, `GET /api/v1/pair`) and because `w` is
/// a function of sixteen bits: dropping the code while keeping `w` would hide nothing.
/// Neither is ever written to a file. `Debug` is hand-written so neither can be printed.
pub struct Pairing {
    id: String,
    code: Code,
    w: Zeroizing<Scalar>,
    created_at: jiff::Timestamp,
    expires_at: jiff::Timestamp,
    attempts: u8,
    consumed_by: Option<String>,
    consumed_at: Option<jiff::Timestamp>,
    /// How many codes this window has issued after its first; the settings page and the
    /// CLI compare it to notice a new code without comparing codes.
    generation: u32,
    /// The last attempt each source began, for the two-second rule. Entries older than
    /// the interval are dropped on every call, so the map never outgrows the window.
    sources: BTreeMap<IpAddr, jiff::Timestamp>,
}

impl std::fmt::Debug for Pairing {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Pairing")
            .field("id", &self.id)
            .field("expires_at", &self.expires_at)
            .field("attempts", &self.attempts)
            .field("generation", &self.generation)
            .finish_non_exhaustive()
    }
}

/// What a caller may see of a window: both renderings of the code, the expiry, the
/// attempt count and the outcome. This is the object `POST /api/v1/pair` and `GET
/// /api/v1/pair` answer with (`spec/protocol.md §9.2`).
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct PairingSnapshot {
    /// The window's ULID.
    pub id: String,
    /// The four glyphs, in order.
    pub emoji: [&'static str; 4],
    /// Their four labels, in the same order.
    pub labels: [&'static str; 4],
    /// The two words, in order.
    pub words: [&'static str; 2],
    /// The URL a device opens to reach this node — what the QR code encodes.
    pub url: String,
    /// When the window opened (RFC 3339 UTC).
    pub created_at: String,
    /// When it closes (RFC 3339 UTC).
    pub expires_at: String,
    /// Attempts made against the current code.
    pub attempts: u8,
    /// How many times the code has been replaced after exhaustion.
    pub generation: u32,
    /// The device that paired, once one has.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub consumed_by: Option<String>,
    /// When it did.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub consumed_at: Option<String>,
}

impl Pairing {
    /// Open a window at `now` for `ttl`, which is clamped to [`TTL`]; a `ttl` under one
    /// second is [`PairError::Ttl`]. The code is fresh CSPRNG output.
    pub fn open(ttl: Duration, now: jiff::Timestamp) -> Result<Self, PairError> {
        if ttl < Duration::from_secs(1) {
            return Err(PairError::Ttl);
        }
        let ttl = ttl.min(TTL);
        let expires_at = now
            .checked_add(jiff::SignedDuration::from_secs(ttl.as_secs() as i64))
            .map_err(|_| PairError::Ttl)?;
        let (code, w) = fresh_code()?;
        Ok(Self {
            id: crate::new_ulid(),
            code,
            w,
            created_at: now,
            expires_at,
            attempts: 0,
            consumed_by: None,
            consumed_at: None,
            generation: 0,
            sources: BTreeMap::new(),
        })
    }

    /// [`Pairing::open`] with the code supplied — for a conformance test or the vector
    /// file, which need a known code. In production the code is CSPRNG output (`§7.2`).
    pub fn open_with(code: Code, ttl: Duration, now: jiff::Timestamp) -> Result<Self, PairError> {
        let mut window = Self::open(ttl, now)?;
        window.code = code;
        window.w = spake2::password(code).map_err(|_| PairError::Format)?;
        Ok(window)
    }

    /// The window's ULID — the `subject` of its audit rows.
    #[must_use]
    pub fn id(&self) -> &str {
        &self.id
    }

    /// The current code, for the surface that displays it. Never log it.
    #[must_use]
    pub fn code(&self) -> Code {
        self.code
    }

    /// When the window closes.
    #[must_use]
    pub fn expires_at(&self) -> jiff::Timestamp {
        self.expires_at
    }

    /// Whether the window has passed its expiry.
    #[must_use]
    pub fn expired(&self, now: jiff::Timestamp) -> bool {
        now >= self.expires_at
    }

    /// Whether a device can pair right now: not expired and not yet consumed. This is
    /// the manifest's `pair` flag (`spec/protocol.md §9.2`).
    #[must_use]
    pub fn is_open(&self, now: jiff::Timestamp) -> bool {
        !self.expired(now) && self.consumed_by.is_none()
    }

    /// The device that paired through this window, if one has.
    #[must_use]
    pub fn consumed_by(&self) -> Option<&str> {
        self.consumed_by.as_deref()
    }

    /// Attempts against the current code.
    #[must_use]
    pub fn attempts(&self) -> u8 {
        self.attempts
    }

    /// How many codes have been issued after the first.
    #[must_use]
    pub fn generation(&self) -> u32 {
        self.generation
    }

    /// The window as a caller may see it, with the URL the node is reached at.
    #[must_use]
    pub fn snapshot(&self, url: String) -> PairingSnapshot {
        let glyphs = self.code.glyphs();
        PairingSnapshot {
            id: self.id.clone(),
            emoji: glyphs.map(|g| g.glyph),
            labels: glyphs.map(|g| g.label),
            words: self.code.words(),
            url,
            created_at: crate::log::format_ts(self.created_at),
            expires_at: crate::log::format_ts(self.expires_at),
            attempts: self.attempts,
            generation: self.generation,
            consumed_by: self.consumed_by.clone(),
            consumed_at: self.consumed_at.map(crate::log::format_ts),
        }
    }

    /// Count an attempt from `source` and hand back `w` for it (`§7.5`). Refuses when the
    /// window is closed, when the source tried within the last two seconds, and when the
    /// current code's five attempts are spent — in which case the code is replaced first,
    /// so the refusal names a code that no longer exists.
    ///
    /// An attempt is counted here, when `pA` arrives, and not when `cA` does: the reply
    /// to `pA` already lets the other side learn whether its guess was right, so a peer
    /// that never sends `cA` would otherwise guess for free.
    pub(crate) fn begin_attempt(
        &mut self,
        source: IpAddr,
        now: jiff::Timestamp,
    ) -> Result<Zeroizing<Scalar>, PairError> {
        if !self.is_open(now) {
            return Err(PairError::Closed);
        }
        let horizon = now
            .checked_sub(jiff::SignedDuration::from_secs(
                SOURCE_INTERVAL.as_secs() as i64
            ))
            .unwrap_or(jiff::Timestamp::MIN);
        self.sources.retain(|_, at| *at > horizon);
        if self.sources.contains_key(&source) {
            return Err(PairError::RateLimited);
        }
        self.sources.insert(source, now);
        if self.attempts >= MAX_ATTEMPTS {
            self.rotate()?;
            return Err(PairError::Exhausted);
        }
        self.attempts += 1;
        Ok(self.w.clone())
    }

    /// Record that a counted attempt failed — a wrong `cA`, or a peer that went away
    /// without one. Returns whether this was the fifth failure and the code was replaced
    /// (`§7.5`).
    pub(crate) fn record_failure(&mut self) -> Result<bool, PairError> {
        if self.attempts >= MAX_ATTEMPTS {
            self.rotate()?;
            return Ok(true);
        }
        Ok(false)
    }

    /// Record the first success: the window is consumed and no further attempt is
    /// accepted (`§7.4` step 6).
    pub(crate) fn consume(&mut self, device: &str, now: jiff::Timestamp) {
        self.consumed_by = Some(device.to_owned());
        self.consumed_at = Some(now);
    }

    fn rotate(&mut self) -> Result<(), PairError> {
        let (code, w) = fresh_code()?;
        self.code = code;
        self.w = w;
        self.attempts = 0;
        self.generation = self.generation.saturating_add(1);
        Ok(())
    }
}

fn fresh_code() -> Result<(Code, Zeroizing<Scalar>), PairError> {
    // A code whose `w` reduces to zero is a one in 2²⁵² event; drawing again is the
    // whole handling it needs.
    for _ in 0..8 {
        let code = Code::random();
        if let Ok(w) = spake2::password(code) {
            return Ok((code, w));
        }
    }
    Err(PairError::Format)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn at(secs: i64) -> jiff::Timestamp {
        jiff::Timestamp::from_second(1_800_000_000 + secs).unwrap_or(jiff::Timestamp::MIN)
    }

    #[test]
    fn a_window_clamps_its_ttl_and_refuses_zero() {
        assert_eq!(
            Pairing::open(Duration::ZERO, at(0)).err(),
            Some(PairError::Ttl)
        );
        let Ok(window) = Pairing::open(Duration::from_secs(10_000), at(0)) else {
            panic!("open")
        };
        assert_eq!(window.expires_at(), at(120));
        assert!(window.is_open(at(119)));
        assert!(!window.is_open(at(120)));
    }

    #[test]
    fn debug_prints_neither_the_code_nor_w() {
        let Ok(window) = Pairing::open(TTL, at(0)) else {
            panic!("open")
        };
        let text = format!("{window:?}");
        for word in window.code().words() {
            assert!(!text.contains(word));
        }
        assert!(text.contains("Pairing"));
    }
}
