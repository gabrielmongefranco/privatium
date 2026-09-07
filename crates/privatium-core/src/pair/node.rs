// Project:  Privatium™  |  File: crates/privatium-core/src/pair/node.rs
// Authors:  Gabriel Mongefranco (@gabrielmongefranco)
// Created:  2026-09-05  |  Modified: 2026-09-07
// Summary:  Pairing on the node (spec/app-contract.md §6, spec/protocol.md §7): opening and
//           closing the window for devices or for a node, driving the handshake against it
//           under the node's lock, writing the device row, the admission of a node in
//           either direction (§2.3.1), what a joiner adopts, the registry checks, node
//           revocation, runtime renewal, and every sys_audit row §7.5 requires.
//           See main README.md for full license information.

use std::net::IpAddr;
use std::time::Duration;

use base64::{Engine as _, engine::general_purpose::STANDARD};
use ed25519_dalek::VerifyingKey;
use serde_json::value::to_raw_value;

use super::handshake::{
    self, Admitter, ClientAdmitted, Direction, Exchange, Finished, Flags, NodePeer, Paired, Sealed,
};
use super::{Joined, PairError, Pairing, PairingSnapshot};
use crate::local::PeerHint;
use crate::store::events::{Op, read_log};
use crate::{Error, Node, Result, Standing, StoreError, log, new_ulid, store, sys};

/// What the node holds once a client's sealed message is open (`spec/protocol.md
/// §7.4.2`): a device's row is written at once; a node's admission continues in the
/// direction `§2.3.1` decided.
#[derive(Debug)]
pub enum PairOutcome {
    /// A browser, desktop or mobile device paired and its row is written.
    Device(Paired),
    /// This node admits the client: seal `admit`, then write the row on `joined`.
    Admit(Admission),
    /// The client admits this node: open its `admit`, adopt, then send `joined`.
    Join(Admission),
}

/// A node admission in progress, after both sealed messages and before `admit`.
#[derive(Debug)]
pub struct Admission {
    peer: NodePeer,
    /// The peer is a registered member whose certificate expired (`§2.3.1`,
    /// re-admission): no row is written for it, only a fresh certificate.
    readmission: bool,
    window: String,
}

/// This node's side after it sealed `admit`, waiting for the joiner's `joined`.
#[derive(Debug)]
pub struct AdmitPending {
    admitter: Admitter,
    paired: Paired,
    readmission: bool,
    paired_at: String,
    window: String,
}

/// What the registry says about a node key that asked to be admitted (`§2.3.1`).
enum Registered {
    /// Never seen: an ordinary admission.
    Unknown,
    /// A member whose certificate has expired: re-admitted with its own key.
    Readmit,
    /// Registered and not re-admissible: revoked, or a certificate still valid.
    Refused(PairError),
}

/// A wrong code, an abandoned attempt and a refused registration are `warn`: they are
/// what an owner reads `sys_audit` for. Opening, an attempt, a success and an expiry are
/// the ordinary course of pairing and are `info`; an admission is `alert`.
impl Node {
    /// Open a pairing window for devices for `ttl` (`spec/app-contract.md §6`), at most
    /// the 120 seconds of `spec/protocol.md §7.5`, and hand back the code in both
    /// renderings, the URL and the expiry. While a window is already open this returns it
    /// unchanged, whichever kind it is: one code at a time (`spec/data-dictionary.md
    /// §3.3`). A `ttl` under one second is refused, and so is a device window on a node
    /// whose certificate has expired or that is revoked (`§2.3.1`, `§2.3.4`). Writes
    /// `pair.opened`.
    pub fn pair(&mut self, ttl: Duration) -> Result<PairingSnapshot> {
        self.pair_at(ttl, jiff::Timestamp::now())
    }

    /// [`pair`](Self::pair) with the clock supplied.
    pub fn pair_at(&mut self, ttl: Duration, now: jiff::Timestamp) -> Result<PairingSnapshot> {
        self.open_window(ttl, now, false)
    }

    /// Open a pairing window **for a node** (`spec/protocol.md §7.1`, `§2.3.1`): it
    /// answers `kind = "node"` alone. Everything else is as [`pair`](Self::pair); an
    /// expired node may open one, since that is how it is re-admitted when only the
    /// cluster's node can dial.
    pub fn pair_node(&mut self, ttl: Duration) -> Result<PairingSnapshot> {
        self.pair_node_at(ttl, jiff::Timestamp::now())
    }

    /// [`pair_node`](Self::pair_node) with the clock supplied.
    pub fn pair_node_at(&mut self, ttl: Duration, now: jiff::Timestamp) -> Result<PairingSnapshot> {
        self.open_window(ttl, now, true)
    }

    fn open_window(
        &mut self,
        ttl: Duration,
        now: jiff::Timestamp,
        node: bool,
    ) -> Result<PairingSnapshot> {
        match self.standing(now)? {
            Standing::Member => {}
            Standing::Expired if node => {}
            Standing::Expired => return Err(Error::CertificateExpired),
            Standing::Revoked => return Err(Error::NodeRevoked),
        }
        self.refresh_pairing(now)?;
        if let Some(open) = self.pairing.as_ref().filter(|p| p.is_open(now)) {
            return Ok(open.snapshot(self.listen_url()));
        }
        let mut window = Pairing::open(ttl, now)?;
        if node {
            window = window.for_node();
        }
        let detail = serde_json::to_string(&serde_json::json!({
            "ttl": ttl.min(super::TTL).as_secs(),
            "node": node,
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

    /// The node's first text frame on `/ws/pair` (`spec/protocol.md §7.4.2`). A revoked
    /// node says pairing is closed whatever window it holds (`§2.3.4`).
    #[must_use]
    pub fn pairing_hello(&self, now: jiff::Timestamp) -> String {
        let open = self.pairing_open(now)
            && !self
                .node_revoked(self.identity.id().as_str())
                .unwrap_or(true);
        handshake::node_hello(&self.identity, open)
    }

    /// Accept a device's first message from `source` as an attempt against the open
    /// window and answer it (`§7.4.2`). Writes `pair.attempt` for every counted attempt,
    /// and `pair.failed` when the count exhausts the code. A refusal before the attempt
    /// is counted — closed, rate-limited, the wrong kind for the window — writes
    /// nothing, so a stranger's connections cannot fill a replicated table.
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
        if self.node_revoked(self.identity.id().as_str())? {
            return Err(PairError::Closed.into());
        }
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
    /// message (`§7.4.2`) — for a node client with this node's signature and flags. Every
    /// refusal is audited as `pair.failed` before it is returned: a wrong code, naming
    /// whether it was the fifth and a new code was issued; a code the window replaced
    /// since `pA`; a window that was consumed or expired since; and a confirmation that
    /// cannot be read. None of them seals anything.
    pub fn pairing_confirm(
        &mut self,
        exchange: Exchange,
        text: &str,
        now: jiff::Timestamp,
    ) -> Result<(Sealed, Vec<u8>)> {
        let device = exchange.device().to_owned();
        let source = exchange.source();
        let flags = if exchange.is_node() {
            let (label, _) = self.node_row_public_facts()?;
            Some((self.flags(now)?, label))
        } else {
            None
        };
        let Some(window) = self.pairing.as_mut() else {
            self.audit_pair_failed(&device, source, "window closed before confirmation", false)?;
            return Err(PairError::Closed.into());
        };
        let before = window.generation();
        match exchange.confirm(&self.identity, window, text, now, flags) {
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

    /// This node's own flags at admission (`spec/protocol.md §2.3.1`).
    pub fn flags(&self, now: jiff::Timestamp) -> Result<Flags> {
        Ok(Flags {
            disposable: self.is_disposable()?,
            expired: self.identity.is_expired(now),
        })
    }

    /// Open the client's sealed message and act on it (`§7.4` steps 5 and 6). The window
    /// is checked before the message is opened: one that closed since `cA` is refused.
    ///
    /// For a device, its key must not already be in the registry — active or revoked,
    /// a key is never re-registered — and its `sys_device` row and `pair.success` are
    /// written as one batch, the code marked consumed and the window closed.
    ///
    /// For a node (`§2.3.1`, `§7.4.2`) the signature is verified, the direction decided
    /// from both sides' flags, and — when this node admits — the registry consulted: an
    /// unknown key is admitted, a member whose certificate expired is re-admitted, and
    /// any other registered key is refused. The window is consumed here as for every
    /// kind; the row waits on `joined`. Every refusal is audited as `pair.failed`.
    pub fn pairing_finish(
        &mut self,
        sealed: Sealed,
        ciphertext: &[u8],
        now: jiff::Timestamp,
    ) -> Result<PairOutcome> {
        let device = sealed.device().to_owned();
        let source = sealed.source();
        let node_client = sealed.is_node();
        self.refresh()?;
        if !self.pairing_open(now) {
            self.audit_pair_failed(&device, source, "window closed before registration", false)?;
            return Err(PairError::Closed.into());
        }
        if !node_client && self.device_known(&device)? {
            self.audit_pair_failed(&device, source, "device key already registered", false)?;
            return Err(PairError::DeviceKnown.into());
        }
        let finished = match sealed.finish(&self.identity, ciphertext) {
            Ok(finished) => finished,
            Err(error) => {
                let reason = match error {
                    PairError::Signature => "node signature missing or invalid",
                    _ => "invalid sealed message",
                };
                self.audit_pair_failed(&device, source, reason, false)?;
                return Err(error.into());
            }
        };
        let window_id = self
            .pairing
            .as_ref()
            .map(|w| w.id().to_owned())
            .ok_or(PairError::Closed)?;
        match finished {
            Finished::Device(paired) => {
                self.write_device_row(&paired, &window_id, now)?;
                Ok(PairOutcome::Device(paired))
            }
            Finished::Node(peer) => {
                let direction = match peer.direction() {
                    Ok(direction) => direction,
                    Err(error) => {
                        let reason = match &error {
                            PairError::TwoClusters => "both nodes already belong to a cluster",
                            _ => "no side can admit the other",
                        };
                        self.audit_pair_failed(&device, source, reason, false)?;
                        return Err(error.into());
                    }
                };
                let readmission = match direction {
                    Direction::NodeAdmits => match self.registered(&device, now)? {
                        Registered::Unknown => false,
                        Registered::Readmit => true,
                        Registered::Refused(error) => {
                            let reason = match &error {
                                PairError::Revoked => "node key is revoked",
                                _ => "node key already registered",
                            };
                            self.audit_pair_failed(&device, source, reason, false)?;
                            return Err(error.into());
                        }
                    },
                    Direction::ClientAdmits => false,
                };
                let Some(window) = self.pairing.as_mut() else {
                    return Err(PairError::Closed.into());
                };
                // Consumed here, as for a device: a second client in flight finds the
                // window closed however the admission fares (§7.4.2).
                window.consume(&device, now);
                self.publish_facts()?;
                let admission = Admission {
                    peer,
                    readmission,
                    window: window_id,
                };
                Ok(match direction {
                    Direction::NodeAdmits => PairOutcome::Admit(admission),
                    Direction::ClientAdmits => PairOutcome::Join(admission),
                })
            }
        }
    }

    /// Seal `admit` for the node this one admits (`spec/protocol.md §2.3.1`, `§7.4.2`):
    /// the cluster private key, the certificate issued for the joiner's key, the
    /// `paired_at` about to be recorded and this node's `_sys` counter. The one place
    /// the cluster private key leaves a node, reached only after the joiner's signature
    /// verified and its key passed the registry check.
    pub fn pairing_admit(
        &mut self,
        admission: Admission,
        now: jiff::Timestamp,
    ) -> Result<(AdmitPending, Vec<u8>)> {
        let Admission {
            peer,
            readmission,
            window,
        } = admission;
        let paired = peer.paired.clone();
        let joiner_key = verifying_key(&paired.ed25519_pub)?;
        let certificate = self.identity.sign_certificate(&joiner_key, now)?;
        let paired_at = log::format_ts(now);
        let lam = self.sys.lam();
        let mut admitter = peer.into_admitter();
        let bytes = admitter.admit(&self.identity.cluster_seed(), &certificate, &paired_at, lam)?;
        Ok((
            AdmitPending {
                admitter,
                paired,
                readmission,
                paired_at,
                window,
            },
            bytes,
        ))
    }

    /// Open the joiner's `joined` and write what the admitter writes (`§2.3.1`,
    /// `spec/data-dictionary.md §3.2`): the joiner's `sys_device` row — `kind = 'node'`,
    /// `replica = true`, both keys, the `paired_at` that was sent, `paired_via = 'lan'`,
    /// its label, no user agent — and `node.admitted` as one batch. A re-admission writes
    /// the audit row alone. A `joined` that does not arrive or does not open writes no
    /// row and is audited as `pair.failed`.
    pub fn pairing_admitted(
        &mut self,
        pending: AdmitPending,
        ciphertext: &[u8],
        now: jiff::Timestamp,
    ) -> Result<Paired> {
        let AdmitPending {
            admitter,
            paired,
            readmission,
            paired_at,
            window,
        } = pending;
        if let Err(error) = admitter.admitted(ciphertext) {
            self.audit_pair_failed(&paired.device, paired.source, "joined not received", false)?;
            return Err(error.into());
        }
        let detail = serde_json::to_string(&serde_json::json!({
            "source": paired.source.to_string(),
            "window": window,
            "readmitted": readmission,
        }))?;
        let audit_at = log::now();
        let row = node_device_row(&paired, &paired_at);
        self.sys.batch(|batch| {
            if !readmission {
                batch.put(sys::DEVICE, &paired.device, &row)?;
            }
            batch.put(
                sys::AUDIT,
                &new_ulid(),
                &sys::AuditRow::alert(
                    &audit_at,
                    sys::KIND_NODE_ADMITTED,
                    Some(&paired.device),
                    &detail,
                ),
            )
        })?;
        let _ = now;
        self.refresh()?;
        self.publish_facts()?;
        Ok(paired)
    }

    /// The client admits this node (`§2.3.1`): open and verify its `admit`, adopt the
    /// cluster, and seal `joined`. Adoption precedes `joined`, so the admitter never
    /// writes a row for a node that did not join. A refusal is audited as `pair.failed`
    /// and nothing on this node changes.
    pub fn pairing_adopt(
        &mut self,
        admission: Admission,
        ciphertext: &[u8],
        now: jiff::Timestamp,
    ) -> Result<(Joined, Vec<u8>)> {
        let Admission { peer, .. } = admission;
        let paired = peer.paired.clone();
        let readmission = peer.own.expired;
        let mut joiner = peer.into_joiner();
        let admitted = match joiner.verify(
            ciphertext,
            self.identity.id().as_str(),
            &self.identity.verifying_key(),
            None,
            now,
        ) {
            Ok(admitted) => admitted,
            Err(error) => {
                self.audit_pair_failed(
                    &paired.device,
                    paired.source,
                    "admission not verified",
                    false,
                )?;
                return Err(error.into());
            }
        };
        let joined = self.adopt_cluster(
            admitted,
            PeerHint {
                id: paired.device.clone(),
                x25519_pub: paired.x25519_pub.clone(),
                url: None,
            },
            readmission,
            now,
        )?;
        let bytes = joiner.joined()?;
        Ok((joined, bytes))
    }

    /// Adopt the cluster an admitter handed this node (`spec/protocol.md §2.3.1`,
    /// `§2.2` of its plan): fold the admitter's counter (`§4.3`), swap the identity
    /// files (`§2.3`), tombstone the empty cluster this node founded, amend `sys_node`,
    /// re-assert this node's own `sys_device` row with the facts the admitter writes
    /// (`spec/data-dictionary.md §3.2`), remember the admitter as a peer hint (`§3.7`),
    /// and advertise the new cluster. A re-admission — this node was expired at the
    /// exchange, so the admitter wrote no row — replaces the certificate and the hint
    /// alone; `readmission` is that fact, and it is the caller's because the `admit`
    /// message does not carry it.
    pub(crate) fn adopt_cluster(
        &mut self,
        admitted: ClientAdmitted,
        hint: PeerHint,
        readmission: bool,
        now: jiff::Timestamp,
    ) -> Result<Joined> {
        let previous = self.identity.cluster_id().as_str().to_owned();
        let readmitted = readmission && admitted.certificate.cluster_id == previous;
        let ClientAdmitted {
            cluster_key,
            certificate,
            paired_at,
            lam,
        } = admitted;
        self.sys.observe_lam(lam);
        self.identity
            .adopt(&self.paths.identity_dir(), &cluster_key, certificate, now)?;
        let id = self.identity.id().as_str().to_owned();
        if !readmitted
            && let Some(row) = self.sys_row(sys::CLUSTER, &previous)?
            && row.get("created_by").is_some_and(|by| {
                serde_json::from_str::<String>(by.get()).ok().as_deref() == Some(&id)
            })
        {
            self.sys_log_mut().del(sys::CLUSTER, &previous)?;
        }
        let cert = self.identity.certificate().to_base64()?;
        let expires = self.identity.certificate().expires_at.clone();
        let cluster_id = self.identity.cluster_id().as_str().to_owned();
        self.amend_sys_row(sys::NODE, &id, |row| {
            row.insert("cluster_id".into(), to_raw_value(&cluster_id)?);
            row.insert("cert".into(), to_raw_value(&cert)?);
            row.insert("cert_expires_at".into(), to_raw_value(&expires)?);
            Ok(())
        })?;
        if !readmitted {
            let (display_name, _) = self.node_row_public_facts()?;
            let ed25519 = self.identity.public_key_base64();
            let x25519 = self.identity.x25519_public_base64();
            self.amend_sys_row(sys::DEVICE, &id, |row| {
                row.insert("kind".into(), to_raw_value("node")?);
                row.insert("replica".into(), to_raw_value(&true)?);
                row.insert("ed25519_pub".into(), to_raw_value(&ed25519)?);
                row.insert("x25519_pub".into(), to_raw_value(&x25519)?);
                row.insert("paired_at".into(), to_raw_value(&paired_at)?);
                row.insert("paired_via".into(), to_raw_value("lan")?);
                match display_name
                    .as_deref()
                    .and_then(|name| crate::registry::clean_text(name, crate::registry::LABEL_MAX))
                {
                    Some(label) => {
                        row.insert("label".into(), to_raw_value(&label)?);
                    }
                    None => {
                        row.remove("label");
                    }
                }
                row.remove("user_agent");
                Ok(())
            })?;
        }
        let peer = hint.id.clone();
        self.state.remember_peer(hint);
        self.state.flush()?;
        self.refresh()?;
        self.publish_facts()?;
        Ok(Joined {
            cluster_id,
            peer,
            joined: true,
            readmitted,
        })
    }

    /// Whether this node is disposable (`spec/protocol.md §2.3`): it has paired nothing
    /// and admitted nobody since it entered its current cluster, judged from its own
    /// `_sys` segments alone — a row another device wrote, restored or synced in, counts
    /// for nothing. The boundary is the last of this node's own `sys_node` events that
    /// set its `cluster_id` to the current cluster; a `sys_device` put for another ID
    /// after it is a pairing or an admission.
    pub fn is_disposable(&self) -> Result<bool> {
        let own = self.identity.id().as_str();
        let current = self.identity.cluster_id().as_str();
        let events = read_log(self.sys.log_dir(), sys::SLUG, &store::cutoff_now())
            .map_err(|error| Error::Store(Box::new(error)))?;
        let mut mine: Vec<_> = events.iter().filter(|event| event.dev == own).collect();
        mine.sort_by_key(|event| event.seq);
        let mut boundary = None;
        let mut previous: Option<String> = None;
        for event in &mine {
            if event.tbl != sys::NODE || event.id != own || event.op != Op::Put {
                continue;
            }
            let cluster = event
                .d
                .as_deref()
                .and_then(|d| serde_json::from_str::<serde_json::Value>(d).ok())
                .and_then(|d| d.get("cluster_id")?.as_str().map(str::to_owned));
            if cluster.as_deref() == Some(current) && previous.as_deref() != Some(current) {
                boundary = Some(event.seq);
            }
            previous = cluster;
        }
        let Some(boundary) = boundary else {
            return Ok(true);
        };
        Ok(!mine.iter().any(|event| {
            event.seq > boundary
                && event.tbl == sys::DEVICE
                && event.id != own
                && event.op == Op::Put
        }))
    }

    /// What the registry says about a node key asking to be admitted (`§2.3.1`,
    /// `§7.4.2`): unknown, re-admissible, or refused.
    fn registered(&self, device: &str, now: jiff::Timestamp) -> Result<Registered> {
        if self.node_revoked(device)? {
            return Ok(Registered::Refused(PairError::Revoked));
        }
        Ok(match self.registered_node(device, now)? {
            super::join::RegisteredNode::Unknown => Registered::Unknown,
            super::join::RegisteredNode::Readmit => Registered::Readmit,
            super::join::RegisteredNode::Refused => Registered::Refused(PairError::DeviceKnown),
        })
    }

    /// The registry's word on a node key, revocation aside: never seen, a member whose
    /// certificate expired in this cluster, or registered and not re-admissible.
    pub(crate) fn registered_node(
        &self,
        device: &str,
        now: jiff::Timestamp,
    ) -> Result<super::join::RegisteredNode> {
        use super::join::RegisteredNode;
        let sql = format!(
            "SELECT d.kind, d.revoked_at IS NOT NULL, n.cluster_id, n.cert_expires_at \
             FROM {} d LEFT JOIN {} n ON n.id = d.id WHERE d.id = ?",
            sys::DEVICE,
            sys::NODE
        );
        let found = match self
            .store
            .conn()
            .query_row(&sql, rusqlite::params![device], |row| {
                Ok((
                    row.get::<_, Option<String>>(0)?,
                    row.get::<_, bool>(1)?,
                    row.get::<_, Option<String>>(2)?,
                    row.get::<_, Option<String>>(3)?,
                ))
            }) {
            Ok(found) => found,
            Err(rusqlite::Error::QueryReturnedNoRows) => return Ok(RegisteredNode::Unknown),
            Err(error) => return Err(Error::Store(Box::new(StoreError::Sql(error)))),
        };
        let (kind, revoked, cluster, expires) = found;
        if revoked || kind.as_deref() != Some("node") {
            return Ok(RegisteredNode::Refused);
        }
        let expired = expires
            .and_then(|text| text.parse::<jiff::Timestamp>().ok())
            .is_some_and(|expires| now >= expires);
        if cluster.as_deref() == Some(self.identity.cluster_id().as_str()) && expired {
            return Ok(RegisteredNode::Readmit);
        }
        Ok(RegisteredNode::Refused)
    }

    /// Renew this node's certificate at runtime when fewer than ninety days remain
    /// (`spec/protocol.md §2.3.1`), amending `sys_node` and writing `cert.renewed`.
    /// Returns whether it did; refused at or after expiry.
    pub fn renew_certificate_if_due(&mut self, now: jiff::Timestamp) -> Result<bool> {
        if !self.identity.renew(&self.paths.identity_dir(), now)? {
            return Ok(false);
        }
        let id = self.identity.id().as_str().to_owned();
        let cert = self.identity.certificate().to_base64()?;
        let expires = self.identity.certificate().expires_at.clone();
        self.amend_sys_row(sys::NODE, &id, |row| {
            row.insert("cert".into(), to_raw_value(&cert)?);
            row.insert("cert_expires_at".into(), to_raw_value(&expires)?);
            Ok(())
        })?;
        self.audit_pair(sys::KIND_CERT_RENEWED, false, &id, "{}")?;
        self.refresh()?;
        Ok(true)
    }

    /// Revoke a node (`spec/protocol.md §2.3.4`): its `sys_device` row marked revoked as
    /// any device's is, a `sys_node_revocation` row, and `node.revoked` (alert). A row
    /// that is not a node's, this node's own, or unknown is refused; revoking twice
    /// writes nothing new.
    pub fn revoke_node(
        &mut self,
        node: &str,
        reason: Option<&str>,
        now: jiff::Timestamp,
    ) -> Result<()> {
        let kind = self.device_kind(node)?;
        if kind.as_deref() != Some("node") {
            return Err(Error::DeviceUnknown {
                device: node.to_owned(),
            });
        }
        self.revoke_device(node, reason, now)?;
        if self.node_revoked(node)? {
            return Ok(());
        }
        let at = log::format_ts(now);
        let by = self.identity.id().as_str().to_owned();
        let reason =
            reason.and_then(|r| crate::registry::clean_text(r, crate::registry::LABEL_MAX));
        let row = sys::RevocationRow {
            revoked_at: &at,
            revoked_by: &by,
            reason: reason.as_deref(),
        };
        let detail = serde_json::to_string(&serde_json::json!({ "by": "owner" }))?;
        let audit_at = log::now();
        self.sys.batch(|batch| {
            batch.put(sys::REVOCATION, node, &row)?;
            batch.put(
                sys::AUDIT,
                &new_ulid(),
                &sys::AuditRow::alert(&audit_at, sys::KIND_NODE_REVOKED, Some(node), &detail),
            )
        })?;
        self.refresh()?;
        Ok(())
    }

    /// The peers this node remembers (`spec/data-dictionary.md §3.7`): the node that
    /// admitted it, and any the owner named.
    #[must_use]
    pub fn peer_hints(&self) -> Vec<PeerHint> {
        self.state.peers().to_vec()
    }

    /// Whether the registry holds `device` as a node (`kind = 'node'`), which decides
    /// whether revoking it is a node's revocation (`spec/protocol.md §2.3.4`).
    pub fn device_is_node(&self, device: &str) -> Result<bool> {
        Ok(self.device_kind(device)?.as_deref() == Some("node"))
    }

    /// A counted attempt whose peer went away before `cA` (`§7.5`): it fails like a wrong
    /// code and writes `pair.failed`, so five silent disconnects replace the code exactly
    /// as five wrong guesses do. After the window was consumed — a node that went away
    /// between its sealed message and `joined` — the window is left as it is and the
    /// abandonment is audited alone.
    pub fn pairing_abandon(&mut self, device: &str, source: IpAddr) -> Result<()> {
        let Some(window) = self.pairing.as_mut() else {
            return Ok(());
        };
        if window.consumed_by().is_some() {
            return self.audit_pair_failed(device, source, "abandoned before admission", false);
        }
        let new_code = window.record_failure()?;
        self.audit_pair_failed(device, source, "abandoned before confirmation", new_code)
    }

    fn write_device_row(
        &mut self,
        paired: &Paired,
        window: &str,
        now: jiff::Timestamp,
    ) -> Result<()> {
        let Some(open) = self.pairing.as_mut() else {
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
            "source": paired.source.to_string(),
            "kind": paired.kind,
            "window": window,
        }))?;
        // The window is consumed before the row is written: a second device whose
        // handshake is in flight finds it closed however the write below fares.
        open.consume(&paired.device, now);
        let audit_at = log::now();
        self.sys.batch(|batch| {
            batch.put(sys::DEVICE, &paired.device, &row)?;
            batch.put(
                sys::AUDIT,
                &new_ulid(),
                &sys::AuditRow::info(
                    &audit_at,
                    sys::KIND_PAIR_SUCCESS,
                    Some(&paired.device),
                    &detail,
                ),
            )
        })?;
        self.refresh()?;
        self.publish_facts()?;
        Ok(())
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
        Ok(self.device_kind(device)?.is_some())
    }

    /// The `kind` of a registered device, or `None` when the registry has no row for it.
    fn device_kind(&self, device: &str) -> Result<Option<String>> {
        match self.store.conn().query_row(
            &format!("SELECT kind FROM {} WHERE id = ?", sys::DEVICE),
            rusqlite::params![device],
            |row| row.get::<_, Option<String>>(0),
        ) {
            Ok(kind) => Ok(kind.or(Some(String::new()))),
            Err(rusqlite::Error::QueryReturnedNoRows) => Ok(None),
            Err(error) => Err(Error::Store(Box::new(StoreError::Sql(error)))),
        }
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

/// The row the admitter writes for a node it admitted (`spec/data-dictionary.md §3.2`).
fn node_device_row<'a>(paired: &'a Paired, paired_at: &'a str) -> sys::DeviceRow<'a> {
    sys::DeviceRow {
        label: paired.label.as_deref(),
        kind: "node",
        replica: true,
        ed25519_pub: Some(&paired.ed25519_pub),
        x25519_pub: Some(&paired.x25519_pub),
        paired_at: Some(paired_at),
        paired_via: Some("lan"),
        last_seen_at: None,
        user_agent: None,
        revoked_at: None,
        revoked_reason: None,
    }
}

/// A base64 Ed25519 public key as the handshake already validated it.
pub(crate) fn verifying_key(base64: &str) -> Result<VerifyingKey> {
    let bytes: [u8; 32] = STANDARD
        .decode(base64)
        .ok()
        .and_then(|bytes| bytes.try_into().ok())
        .ok_or(PairError::Format)?;
    VerifyingKey::from_bytes(&bytes).map_err(|_| PairError::Format.into())
}
