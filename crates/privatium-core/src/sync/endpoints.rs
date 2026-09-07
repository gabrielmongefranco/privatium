// Project:  Privatium™  |  File: crates/privatium-core/src/sync/endpoints.rs
// Authors:  Gabriel Mongefranco (@gabrielmongefranco)
// Created:  2026-09-07  |  Modified: 2026-09-07
// Summary:  Bounded endpoint candidates and preference ordering for
//           spec/protocol.md §10.4.
//           See main README.md for full license information.

use std::str::FromStr;
use std::time::{Duration, Instant};

/// Maximum time to establish one endpoint's transport.
pub const CONNECT_TIMEOUT: Duration = Duration::from_millis(2500);
/// Maximum transport search time across one peer's candidates.
pub const SEARCH_TIMEOUT: Duration = Duration::from_secs(10);
/// Maximum retained endpoints per peer; discovery cannot grow this list without bound.
pub const MAX_CANDIDATES: usize = 64;

/// Endpoint provenance, ordered by preference after the latest successful connection.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum Kind {
    /// Multicast DNS discovery on this LAN.
    LanMdns,
    /// UDP discovery on this LAN.
    LanUdp,
    /// Known address on this LAN.
    LanIp,
    /// Public-key address resolution.
    Pkarr,
    /// Domain Name System record.
    Dns,
    /// Dynamic DNS record.
    Ddns,
    /// Tunnel address.
    Tunnel,
    /// Private virtual network address.
    Vpn,
    /// Direct peer transport.
    P2p,
    /// Relayed transport.
    Relay,
    /// Origin remembered from a join.
    Static,
}

const KINDS: [(Kind, &str); 11] = [
    (Kind::LanMdns, "lan-mdns"),
    (Kind::LanUdp, "lan-udp"),
    (Kind::LanIp, "lan-ip"),
    (Kind::Pkarr, "pkarr"),
    (Kind::Dns, "dns"),
    (Kind::Ddns, "ddns"),
    (Kind::Tunnel, "tunnel"),
    (Kind::Vpn, "vpn"),
    (Kind::P2p, "p2p"),
    (Kind::Relay, "relay"),
    (Kind::Static, "static"),
];

impl FromStr for Kind {
    type Err = &'static str;
    fn from_str(text: &str) -> Result<Self, Self::Err> {
        KINDS
            .iter()
            .find(|(_, name)| *name == text)
            .map(|(kind, _)| *kind)
            .ok_or("unknown endpoint kind")
    }
}

impl std::fmt::Display for Kind {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(
            KINDS
                .iter()
                .find(|(kind, _)| kind == self)
                .map_or("", |(_, name)| name),
        )
    }
}

/// One validated endpoint and its process-local connection observations.
#[derive(Debug, Clone)]
pub struct Candidate {
    /// HTTP origin, without paths or credentials.
    pub url: String,
    /// Where the address was learned.
    pub kind: Kind,
    /// Most recent authenticated connection.
    pub last_ok: Option<Instant>,
    /// Most recent failed connection.
    pub last_fail: Option<Instant>,
    /// Connection and handshake elapsed milliseconds at the last success.
    pub rtt_ms: Option<u64>,
}

/// A bounded list; merging the same URL preserves its observations.
#[derive(Debug, Default, Clone)]
pub struct Candidates(Vec<Candidate>);

impl Candidates {
    /// Validate and merge an origin. Returns false for unsafe origins or a full list.
    pub fn merge(&mut self, url: &str, kind: Kind) -> bool {
        if target(url).is_err() {
            return false;
        }
        let url = url.trim_end_matches('/');
        if let Some(found) = self.0.iter_mut().find(|c| c.url == url) {
            found.kind = found.kind.min(kind);
            return true;
        }
        if self.0.len() == MAX_CANDIDATES {
            return false;
        }
        self.0.push(Candidate {
            url: url.into(),
            kind,
            last_ok: None,
            last_fail: None,
            rtt_ms: None,
        });
        true
    }

    /// Most recently successful first, then by kind: the endpoint that answered a
    /// minute ago is the best guess now, whatever category it belongs to
    /// (`spec/protocol.md §10.4`).
    #[must_use]
    pub fn ordered(&self) -> Vec<Candidate> {
        let mut ordered = self.0.clone();
        ordered.sort_by(|a, b| {
            b.last_ok
                .cmp(&a.last_ok)
                .then(a.kind.cmp(&b.kind))
                .then(a.url.cmp(&b.url))
        });
        ordered
    }

    /// Record a completed authentication or a failed attempt, without writing an event.
    pub fn note(&mut self, url: &str, ok: bool, elapsed: Duration) {
        if let Some(candidate) = self.0.iter_mut().find(|c| c.url == url) {
            if ok {
                candidate.last_ok = Some(Instant::now());
                candidate.rtt_ms = u64::try_from(elapsed.as_millis()).ok();
            } else {
                candidate.last_fail = Some(Instant::now());
            }
        }
    }
}

pub(crate) fn target(url: &str) -> Result<String, &'static str> {
    let target = crate::pair::join::join_target(url).map_err(|_| "invalid endpoint origin")?;
    let uri: axum::http::Uri = target.parse().map_err(|_| "invalid endpoint origin")?;
    if uri.port_u16().is_none_or(|p| p < 1024) {
        return Err("endpoint port must be at least 1024");
    }
    let origin = target
        .strip_suffix("/ws/pair")
        .ok_or("invalid endpoint origin")?;
    Ok(format!("{origin}/ws"))
}

#[cfg(test)]
#[allow(clippy::unwrap_used)]
mod tests {
    use super::*;

    /// `spec/protocol.md §10.4`: the address that answered most recently is tried first
    /// whatever kind it is, an address that is not a safe origin is never tried, and the
    /// list cannot grow without bound as discovery keeps offering more.
    #[test]
    fn test_spec_10_4_candidates_are_ordered_by_last_ok_then_kind_and_bounded() {
        let mut candidates = Candidates::default();
        assert!(candidates.merge("http://192.0.2.1:8420", Kind::Static));
        assert!(candidates.merge("http://192.0.2.2:8420", Kind::LanMdns));
        assert_eq!(candidates.ordered()[0].kind, Kind::LanMdns);
        candidates.note("http://192.0.2.1:8420", true, Duration::from_millis(12));
        assert_eq!(candidates.ordered()[0].kind, Kind::Static);
        assert_eq!(candidates.ordered()[0].rtt_ms, Some(12));
        for index in 0..100 {
            candidates.merge(&format!("http://host-{index}:8420"), Kind::Static);
        }
        assert_eq!(candidates.ordered().len(), MAX_CANDIDATES);
        for url in [
            "http://user:secret@example.test:8420",
            "http://example.test:80",
            "http://example.test:8420/path",
            "http://example.test:8420?x=1",
        ] {
            assert!(target(url).is_err());
        }
        for (kind, text) in KINDS {
            assert_eq!(text.parse::<Kind>().unwrap(), kind);
            assert_eq!(kind.to_string(), text);
        }
        assert!("future".parse::<Kind>().is_err());
        assert_eq!(CONNECT_TIMEOUT, Duration::from_millis(2500));
    }
}
