// Project:  Privatium™  |  File: crates/privatium-core/src/pair/node.rs
// Authors:  Gabriel Mongefranco (@gabrielmongefranco)
// Created:  2026-09-05  |  Modified: 2026-09-06
// Summary:  Pairing on the node (spec/app-contract.md §6, spec/protocol.md §7): opening and
//           closing the window, driving the handshake against it under the node's lock,
//           writing the device row, and every sys_audit row §7.5 requires.
//           See main README.md for full license information.

use std::net::IpAddr;
use std::time::Duration;

use super::handshake::{self, Exchange, Paired, Sealed};
use super::{PairError, Pairing, PairingSnapshot};
use crate::{Error, Node, Result, log, new_ulid, sys};

/// A wrong code, an abandoned attempt and a refused registration are `warn`: they are
/// what an owner reads `sys_audit` for. Opening, an attempt, a success and an expiry are
/// the ordinary course of pairing and are `info`.
impl Node {
    /// Open a pairing window for `ttl` (`spec/app-contract.md §6`), at most the 120
    /// seconds of `spec/protocol.md §7.5`, and hand back the code in both renderings, the
    /// URL and the expiry. While a window is already open this returns it unchanged: one
    /// code at a time (`spec/data-dictionary.md §3.3`). A `ttl` under one second is
    /// refused. Writes `pair.opened`.
    pub fn pair(&mut self, ttl: Duration) -> Result<PairingSnapshot> {
        self.pair_at(ttl, jiff::Timestamp::now())
    }

    /// [`pair`](Self::pair) with the clock supplied.
    pub fn pair_at(&mut self, ttl: Duration, now: jiff::Timestamp) -> Result<PairingSnapshot> {
        self.refresh_pairing(now)?;
        if let Some(open) = self.pairing.as_ref().filter(|p| p.is_open(now)) {
            return Ok(open.snapshot(self.listen_url()));
        }
        let window = Pairing::open(ttl, now)?;
        let detail = serde_json::to_string(&serde_json::json!({
            "ttl": ttl.min(super::TTL).as_secs(),
        }))?;
        self.audit_pair(sys::KIND_PAIR_OPENED, false, window.id(), &detail)?;
        let snapshot = window.snapshot(self.listen_url());
        self.pairing = Some(window);
        // The `pair` key of the TXT record flips the moment a window opens (`§6.1`).
        self.publish_facts()?;
        Ok(snapshot)
    }

    /// The window as it stands, expired or consumed included, until
    /// [`refresh_pairing`](Self::refresh_pairing) retires it.
    #[must_use]
    pub fn pairing(&self) -> Option<&Pairing> {
        self.pairing.as_ref()
    }

    /// Whether a device can pair at this moment — the manifest's `pair` flag
    /// (`spec/protocol.md §9.2`).
    #[must_use]
    pub fn pairing_open(&self, now: jiff::Timestamp) -> bool {
        self.pairing.as_ref().is_some_and(|p| p.is_open(now))
    }

    /// Retire a window whose TTL has passed, writing `pair.expired` once, and report
    /// what remains: the open or consumed window as `GET /api/v1/pair` answers it, or
    /// `None`.
    pub fn refresh_pairing(&mut self, now: jiff::Timestamp) -> Result<Option<PairingSnapshot>> {
        if let Some(window) = self.pairing.as_ref()
            && window.expired(now)
            && window.consumed_by().is_none()
        {
            let id = window.id().to_owned();
            let detail = serde_json::to_string(&serde_json::json!({
                "reason": "ttl",
                "attempts": window.attempts(),
                "generation": window.generation(),
            }))?;
            self.pairing = None;
            self.audit_pair(sys::KIND_PAIR_EXPIRED, false, &id, &detail)?;
            self.publish_facts()?;
        }
        Ok(self.pairing.as_ref().map(|p| p.snapshot(self.listen_url())))
    }

    /// Close the window now, whatever its state. Returns whether one was open; an open
    /// window closed by the owner writes `pair.expired` naming the owner as the reason.
    pub fn close_pairing(&mut self, now: jiff::Timestamp) -> Result<bool> {
        let Some(window) = self.pairing.take() else {
            return Ok(false);
        };
        let open = window.is_open(now);
        if open {
            let detail = serde_json::to_string(&serde_json::json!({
                "reason": "closed by owner",
                "attempts": window.attempts(),
                "generation": window.generation(),
            }))?;
            self.audit_pair(sys::KIND_PAIR_EXPIRED, false, window.id(), &detail)?;
        }
        self.publish_facts()?;
        Ok(open)
    }

    /// The node's first text frame on `/ws/pair` (`spec/protocol.md §7.4.2`).
    #[must_use]
    pub fn pairing_hello(&self, now: jiff::Timestamp) -> String {
        handshake::node_hello(&self.identity, self.pairing_open(now))
    }

    /// Accept a device's first message from `source` as an attempt against the open
    /// window and answer it (`§7.4.2`). Writes `pair.attempt` for every counted attempt,
    /// and `pair.failed` when the count exhausts the code. A refusal before the attempt
    /// is counted — closed, rate-limited — writes nothing, so a stranger's connections
    /// cannot fill a replicated table.
    pub fn pairing_begin(
        &mut self,
        source: IpAddr,
        now: jiff::Timestamp,
        text: &str,
    ) -> Result<(Exchange, String)> {
        use rand::RngExt as _;
        let secret = zeroize::Zeroizing::new(rand::rng().random::<[u8; 64]>());
        self.pairing_begin_with(source, now, text, &secret)
    }

    /// [`pairing_begin`](Self::pairing_begin) with the node's PAKE secret supplied, for
    /// the vector file; see [`Exchange::begin_with`].
    pub fn pairing_begin_with(
        &mut self,
        source: IpAddr,
        now: jiff::Timestamp,
        text: &str,
        secret: &[u8; 64],
    ) -> Result<(Exchange, String)> {
        self.refresh_pairing(now)?;
        let Some(window) = self.pairing.as_mut() else {
            return Err(PairError::Closed.into());
        };
        let before = (window.attempts(), window.generation());
        let outcome = Exchange::begin_with(&self.identity, window, source, now, text, secret);
        let after = (window.attempts(), window.generation());
        let window_id = window.id().to_owned();
        match outcome {
            Ok((exchange, reply)) => {
                let detail = serde_json::to_string(&serde_json::json!({
                    "source": source.to_string(),
                    "kind": exchange.kind(),
                    "attempt": after.0,
                }))?;
                self.audit_pair(sys::KIND_PAIR_ATTEMPT, false, exchange.device(), &detail)?;
                Ok((exchange, reply))
            }
            Err(error) => {
                if after.1 != before.1 || (after.0 != before.0 && error == PairError::Format) {
                    let detail = serde_json::to_string(&serde_json::json!({
                        "source": source.to_string(),
                        "reason": match error {
                            PairError::Exhausted => "code exhausted; a new code was issued",
                            _ => "invalid key-exchange message",
                        },
                        "new_code": after.1 != before.1,
                    }))?;
                    self.audit_pair(sys::KIND_PAIR_FAILED, true, &window_id, &detail)?;
                }
                Err(error.into())
            }
        }
    }

    /// Verify the device's `cA` at `now` and, when it verifies, produce the node's sealed
    /// message (`§7.4.2`). Every refusal is audited as `pair.failed` before it is
    /// returned: a wrong code, naming whether it was the fifth and a new code was
    /// issued; a code the window replaced since `pA`; a window that was consumed or
    /// expired since; and a confirmation that cannot be read. None of them seals
    /// anything.
    pub fn pairing_confirm(
        &mut self,
        exchange: Exchange,
        text: &str,
        now: jiff::Timestamp,
    ) -> Result<(Sealed, Vec<u8>)> {
        let device = exchange.device().to_owned();
        let source = exchange.source();
        let Some(window) = self.pairing.as_mut() else {
            self.audit_pair_failed(&device, source, "window closed before confirmation", false)?;
            return Err(PairError::Closed.into());
        };
        let before = window.generation();
        match exchange.confirm(&self.identity, window, text, now) {
            Ok(sealed) => Ok(sealed),
            Err(error) => {
                let new_code = window.generation() != before;
                let reason = match error {
                    PairError::WrongCode => "wrong code",
                    PairError::Exhausted => "code replaced before confirmation",
                    PairError::Closed => "window closed before confirmation",
                    _ => "invalid confirmation message",
                };
                self.audit_pair_failed(&device, source, reason, new_code)?;
                Err(error.into())
            }
        }
    }

    /// Write the device's `sys_device` row (`spec/data-dictionary.md §3.2`) and
    /// `pair.success` as one batch from its sealed message, mark the code consumed and
    /// close the window (`§7.4` steps 5 and 6). The window and the registry are checked
    /// before the message is opened: a window that closed since `cA` is refused, and a
    /// device key already in the registry — active or revoked — is refused, since a key
    /// is never re-registered. Every refusal is audited as `pair.failed`.
    pub fn pairing_finish(
        &mut self,
        sealed: Sealed,
        ciphertext: &[u8],
        now: jiff::Timestamp,
    ) -> Result<Paired> {
        let device = sealed.device().to_owned();
        let source = sealed.source();
        self.refresh()?;
        if !self.pairing_open(now) {
            self.audit_pair_failed(&device, source, "window closed before registration", false)?;
            return Err(PairError::Closed.into());
        }
        if self.device_known(&device)? {
            self.audit_pair_failed(&device, source, "device key already registered", false)?;
            return Err(PairError::DeviceKnown.into());
        }
        let paired = match sealed.finish(&self.identity, ciphertext) {
            Ok(paired) => paired,
            Err(error) => {
                self.audit_pair_failed(&device, source, "invalid sealed message", false)?;
                return Err(error.into());
            }
        };
        let Some(window) = self.pairing.as_mut() else {
            return Err(PairError::Closed.into());
        };
        let at = log::format_ts(now);
        let row = sys::DeviceRow {
            label: paired.label.as_deref(),
            kind: &paired.kind,
            replica: false,
            ed25519_pub: Some(&paired.ed25519_pub),
            x25519_pub: Some(&paired.x25519_pub),
            paired_at: Some(&at),
            paired_via: Some("lan"),
            last_seen_at: None,
            user_agent: paired.user_agent.as_deref(),
            revoked_at: None,
            revoked_reason: None,
        };
        let detail = serde_json::to_string(&serde_json::json!({
            "source": source.to_string(),
            "kind": paired.kind,
            "window": window.id(),
        }))?;
        // The window is consumed before the row is written: a second device whose
        // handshake is in flight finds it closed however the write below fares.
        window.consume(&device, now);
        let audit_at = log::now();
        self.sys.batch(|batch| {
            batch.put(sys::DEVICE, &device, &row)?;
            batch.put(
                sys::AUDIT,
                &new_ulid(),
                &sys::AuditRow::info(&audit_at, sys::KIND_PAIR_SUCCESS, Some(&device), &detail),
            )
        })?;
        self.refresh()?;
        self.publish_facts()?;
        Ok(paired)
    }

    /// A counted attempt whose peer went away before `cA` (`§7.5`): it fails like a wrong
    /// code and writes `pair.failed`, so five silent disconnects replace the code exactly
    /// as five wrong guesses do.
    pub fn pairing_abandon(&mut self, device: &str, source: IpAddr) -> Result<()> {
        let Some(window) = self.pairing.as_mut() else {
            return Ok(());
        };
        let new_code = window.record_failure()?;
        self.audit_pair_failed(device, source, "abandoned before confirmation", new_code)
    }

    /// The `pair.failed` row of `§7.5`: the source, why, and whether the failure replaced
    /// the code. Never the code.
    fn audit_pair_failed(
        &mut self,
        device: &str,
        source: IpAddr,
        reason: &str,
        new_code: bool,
    ) -> Result<()> {
        let detail = serde_json::to_string(&serde_json::json!({
            "source": source.to_string(),
            "reason": reason,
            "new_code": new_code,
        }))?;
        self.audit_pair(sys::KIND_PAIR_FAILED, true, device, &detail)
    }

    /// The URL a device opens to reach this node — what the pairing QR code encodes
    /// (`spec/protocol.md §7.1`), selected from the default route (spec/cli.md §2).
    #[must_use]
    pub fn listen_url(&self) -> String {
        format!(
            "http://{}",
            std::net::SocketAddr::new(crate::http::lan_address(), self.config.node.port)
        )
    }

    /// Whether `sys_device` already holds `device`, revoked or not.
    fn device_known(&self, device: &str) -> Result<bool> {
        let found: Option<String> = match self.store.conn().query_row(
            &format!("SELECT id FROM {} WHERE id = ?", sys::DEVICE),
            rusqlite::params![device],
            |row| row.get(0),
        ) {
            Ok(id) => Some(id),
            Err(rusqlite::Error::QueryReturnedNoRows) => None,
            Err(error) => return Err(Error::Store(Box::new(crate::StoreError::Sql(error)))),
        };
        Ok(found.is_some())
    }

    fn audit_pair(&mut self, kind: &str, warn: bool, subject: &str, detail: &str) -> Result<()> {
        let at = log::now();
        let row = if warn {
            sys::AuditRow::warn(&at, kind, Some(subject), detail)
        } else {
            sys::AuditRow::info(&at, kind, Some(subject), detail)
        };
        self.sys.put(sys::AUDIT, &new_ulid(), &row)?;
        Ok(())
    }
}
