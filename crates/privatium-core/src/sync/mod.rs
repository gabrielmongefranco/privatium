// Project:  Privatium™  |  File: crates/privatium-core/src/sync/mod.rs
// Authors:  Gabriel Mongefranco (@gabrielmongefranco)
// Created:  2026-09-07  |  Modified: 2026-09-07
// Summary:  Node synchronization and durable foreign-log reception, as defined in
//           spec/protocol.md §10.2. Derived caches follow the source logs.
//           See main README.md for full license information.

/// Where a peer might be reached and in what order to try, so a dead address fails
/// over quickly instead of hanging a pass (`spec/protocol.md §10.4`).
pub mod endpoints;
mod engine;
mod receive;
mod routes;
pub(crate) mod source;

pub use receive::ReceiveReport;

use std::collections::{BTreeMap, BTreeSet};
use std::sync::Arc;
use std::thread::JoinHandle;
use std::time::Duration;

use base64::{Engine as _, engine::general_purpose::STANDARD};
use tokio::sync::{mpsc, oneshot, watch};

use crate::{Error, Node, Result, Standing, boxed, log, store, sys};

/// Outcomes for the peers considered by one explicit synchronization pass.
#[derive(Debug, Clone, Default, serde::Serialize)]
pub struct SyncReport {
    /// Per-peer results; an empty list means no active peer was known.
    pub peers: Vec<PeerReport>,
}

#[cfg(test)]
#[allow(clippy::unwrap_used)]
mod tests {
    use super::*;

    /// `spec/protocol.md §2.3.1`: a node renews its certificate by proving it is still
    /// part of a working cluster, so only a pass that finished counts. A pass that
    /// stopped part-way renews nothing, and one with time to spare renews nothing either.
    #[tokio::test(flavor = "multi_thread", worker_threads = 4)]
    async fn test_spec_2_3_1_certificate_renews_after_a_completed_pass() {
        use axum::ServiceExt as _;
        let remote_root = tempfile::tempdir().unwrap();
        let remote = Arc::new(crate::Handler::new(
            Node::open(remote_root.path()).unwrap(),
            crate::LoadReport::default(),
        ));
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let url = format!("http://{}", listener.local_addr().unwrap());
        let handler = remote.clone();
        let task = tokio::spawn(async move {
            let service = tower::service_fn(move |mut request: crate::Request| {
                let handler = handler.clone();
                async move {
                    request
                        .extensions_mut()
                        .insert(crate::Peer("192.0.2.10:4000".parse().unwrap()));
                    Ok::<_, std::convert::Infallible>(handler.handle(request).await)
                }
            });
            axum::serve(listener, service.into_make_service())
                .await
                .unwrap();
        });
        for (days_left, completes) in [(91, true), (89, true), (89, false)] {
            let root = tempfile::tempdir().unwrap();
            let mut node = Node::open(root.path()).unwrap();
            let code = remote
                .node()
                .lock()
                .unwrap()
                .pair_node(Duration::from_secs(120))
                .unwrap()
                .words
                .join(" ");
            let target = url.clone();
            node = tokio::task::spawn_blocking(move || {
                node.join(&target, crate::pair::Code::parse(&code).unwrap())
                    .unwrap();
                node
            })
            .await
            .unwrap();
            let issued =
                jiff::Timestamp::now() - jiff::SignedDuration::from_secs((180 - days_left) * 86400);
            let cert = node
                .identity
                .sign_certificate(&node.identity.verifying_key(), issued)
                .unwrap();
            std::fs::write(
                node.paths.identity_dir().join("node.cert"),
                serde_json::to_vec(&cert).unwrap(),
            )
            .unwrap();
            node.identity =
                crate::Identity::load_or_create_at(&node.paths.identity_dir(), issued).unwrap();
            if !completes {
                let mut peer = remote.node().lock().unwrap();
                peer.open_app("gap", "").unwrap();
                let path = peer.paths.app_log_dir("gap").join("aaaaaaaa.jsonl");
                std::fs::write(
                    path,
                    page("gap", 3, "item", "synthetic", serde_json::json!({})),
                )
                .unwrap();
            }
            let (mut node, report) = tokio::task::spawn_blocking(move || {
                let report = node.sync_now().unwrap();
                (node, report)
            })
            .await
            .unwrap();
            assert_eq!(
                report.peers.iter().any(|peer| peer.completed),
                completes,
                "{report:?}"
            );
            assert_eq!(
                node.identity.certificate().expires_at != cert.expires_at,
                completes && days_left < 90
            );
            node.refresh().unwrap();
            let audits: i64 = node
                .store
                .conn()
                .query_row(
                    "SELECT count(*) FROM sys_audit WHERE kind = 'cert.renewed' AND subject = ?",
                    [node.id().as_str()],
                    |row| row.get(0),
                )
                .unwrap();
            assert_eq!(audits, i64::from(completes && days_left < 90));
        }
        task.abort();
    }

    /// `spec/protocol.md §2.3.1`: a node whose certificate has expired serves its owner
    /// and opens no channel to a peer. It must refuse before spending a thread and a
    /// socket on a conversation every peer would close.
    #[test]
    fn test_spec_2_3_1_expired_membership_refuses_sync_without_starting_a_thread() {
        let root = tempfile::tempdir().unwrap();
        let mut node = Node::open(root.path()).unwrap();
        let issued = jiff::Timestamp::now() - jiff::SignedDuration::from_secs(181 * 86400);
        let cert = node
            .identity
            .sign_certificate(&node.identity.verifying_key(), issued)
            .unwrap();
        std::fs::write(
            node.paths.identity_dir().join("node.cert"),
            serde_json::to_vec(&cert).unwrap(),
        )
        .unwrap();
        node.identity =
            crate::Identity::load_or_create_at(&node.paths.identity_dir(), jiff::Timestamp::now())
                .unwrap();
        assert!(matches!(node.start_sync(), Err(Error::CertificateExpired)));
        assert!(matches!(node.sync_now(), Err(Error::CertificateExpired)));
        assert!(node.sync.is_none());
    }

    fn page(app: &str, seq: u64, table: &str, id: &str, d: serde_json::Value) -> Vec<u8> {
        let line = serde_json::json!({
            "seq": seq, "lam": seq, "ts": log::now(), "dev": "aaaaaaaa",
            "app": app, "op": "put", "tbl": table, "id": id, "d": d
        });
        format!("{line}\n").into_bytes()
    }

    /// `spec/protocol.md §10.2`: what arrives from a peer waits in a bounded queue until
    /// the one holder of the data-root lock can land it, and the system log is landed
    /// first — a device's row has to exist before the rows it wrote mean anything.
    #[test]
    fn test_sync_inbox_is_drained_by_refresh_and_by_sync_now() {
        let root = tempfile::tempdir().unwrap();
        let mut node = Node::open(root.path()).unwrap();
        node.state.remember_peer(crate::local::PeerHint {
            id: "cccccccc".into(),
            x25519_pub: STANDARD.encode([1u8; 32]),
            url: None,
        });
        let peer = node.peer_table().unwrap().peers["cccccccc"].clone();
        node.open_app(
            "hello",
            "CREATE TABLE item (id TEXT PRIMARY KEY, value TEXT);",
        )
        .unwrap();
        let (sender, receiver) = mpsc::channel(64);
        node.inbox = Some(Inbox { receiver });
        let mut answers = Vec::new();
        for (app, bytes) in [
            (
                "hello",
                page(
                    "hello",
                    1,
                    "item",
                    "one",
                    serde_json::json!({"value":"synthetic"}),
                ),
            ),
            (
                "_sys",
                page(
                    "_sys",
                    1,
                    "sys_device",
                    "bbbbbbbb",
                    serde_json::json!({"kind":"browser","replica":false}),
                ),
            ),
            (
                "hello",
                page(
                    "hello",
                    2,
                    "item",
                    "two",
                    serde_json::json!({"value":"synthetic"}),
                ),
            ),
        ] {
            let (done, answer) = oneshot::channel();
            sender
                .try_send(Item::Page {
                    peer: peer.clone(),
                    app: app.into(),
                    dev: "aaaaaaaa".into(),
                    bytes,
                    done,
                })
                .unwrap_or_else(|_| panic!("inbox full"));
            answers.push(answer);
        }
        node.refresh_app("hello").unwrap();
        for mut answer in answers {
            assert!(answer.try_recv().unwrap().is_ok());
        }
        let found: i64 = node
            .store
            .conn()
            .query_row(
                "SELECT count(*) FROM sys_device WHERE id = 'bbbbbbbb'",
                [],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(found, 1);
        let found: i64 = node
            .app("hello")
            .unwrap()
            .store()
            .conn()
            .query_row("SELECT count(*) FROM item", [], |row| row.get(0))
            .unwrap();
        assert_eq!(found, 2);
        let state = node.paths.root().join("local/state.jsonl");
        let before = std::fs::read(&state).unwrap();
        node.refresh_app("hello").unwrap();
        assert_eq!(std::fs::read(&state).unwrap(), before);
        let before = std::fs::read(node.paths.app_log_dir("hello").join("aaaaaaaa.jsonl")).unwrap();
        let (done, mut refused) = oneshot::channel();
        sender
            .try_send(Item::Page {
                peer: peer.clone(),
                app: "hello".into(),
                dev: "aaaaaaaa".into(),
                bytes: page("hello", 3, "item", "refused", serde_json::json!({})),
                done,
            })
            .unwrap_or_else(|_| panic!("inbox full"));
        let (done, mut system) = oneshot::channel();
        sender
            .try_send(Item::Page {
                peer,
                app: "_sys".into(),
                dev: "aaaaaaaa".into(),
                bytes: page(
                    "_sys",
                    2,
                    "sys_node_revocation",
                    "cccccccc",
                    serde_json::json!({"revoked_at":log::now()}),
                ),
                done,
            })
            .unwrap_or_else(|_| panic!("inbox full"));
        node.refresh_app("hello").unwrap();
        assert!(system.try_recv().unwrap().is_ok());
        assert!(refused.try_recv().unwrap().is_err());
        assert_eq!(
            std::fs::read(node.paths.app_log_dir("hello").join("aaaaaaaa.jsonl")).unwrap(),
            before
        );
        node.inbox = None;
        node.start_sync().unwrap();
        assert!(node.sync_now().unwrap().peers.is_empty());
    }

    /// `AGENTS.md` 11, `spec/protocol.md §10.2`: how far a peer has got is this node's
    /// own bookkeeping and never becomes an event. Writing it to a log would replicate
    /// one node's view of another to everyone, and every pass would grow the log.
    #[test]
    fn test_spec_10_2_sync_state_is_never_an_event() {
        let root = tempfile::tempdir().unwrap();
        let mut node = Node::open(root.path()).unwrap();
        let own = node.paths.app_log("_sys", node.id());
        let before = std::fs::read(&own).unwrap();
        node.start_sync().unwrap();
        node.sync_now().unwrap();
        assert_eq!(std::fs::read(&own).unwrap(), before);
        let unsynced: Option<i64> = node
            .store
            .conn()
            .query_row(
                "SELECT unsynced_peers FROM v_health WHERE app_id = '_sys'",
                [],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(unsynced, Some(0));
        drop(node);
        let node = Node::open(root.path()).unwrap();
        let unsynced: Option<i64> = node
            .store
            .conn()
            .query_row(
                "SELECT unsynced_peers FROM v_health WHERE app_id = '_sys'",
                [],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(unsynced, None);
    }
}

/// A pass result, with durable line counts and fixed refusal reasons.
#[derive(Debug, Clone, Default, serde::Serialize)]
pub struct PeerReport {
    /// Authenticated peer identity.
    pub id: String,
    /// Origin that answered, or the last origin attempted.
    pub url: String,
    /// Every advertised range completed and was durably received on both sides.
    pub completed: bool,
    /// Physical lines durably received locally.
    pub pulled_lines: usize,
    /// Physical lines acknowledged as durable by the peer.
    pub pushed_lines: usize,
    /// Fixed operation diagnostics, without remote text or row contents.
    pub refusals: Vec<String>,
}

/// Public pairing facts supplied to the engine; keys can change only through the node.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PeerEntry {
    /// Registry ID.
    pub id: String,
    /// Pinned X25519 public key.
    pub x25519: [u8; 32],
    /// Origin remembered during joining, if any.
    pub url: Option<String>,
}

/// Active nodes and this node's current authorization horizon.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PeerTable {
    /// Current request and log-line byte bound from the replicated API setting.
    pub max_body: usize,
    /// Active registry nodes and bootstrap hints that have no overriding registry row.
    pub peers: BTreeMap<String, PeerEntry>,
    /// Certificate expiry in UTC; absent if this node is revoked or invalid.
    pub until: Option<jiff::Timestamp>,
}

pub(crate) struct SyncFacts {
    paths: crate::Paths,
    id: String,
    cluster_id: String,
    cluster: ed25519_dalek::VerifyingKey,
    secret: x25519_dalek::StaticSecret,
    discovery: Option<Arc<crate::discover::Shared>>,
}

pub(crate) enum Item {
    Page {
        peer: PeerEntry,
        app: String,
        dev: String,
        bytes: Vec<u8>,
        done: oneshot::Sender<std::result::Result<ReceiveReport, &'static str>>,
    },
    Control {
        control: Control,
        done: oneshot::Sender<std::result::Result<(), &'static str>>,
    },
}

pub(crate) enum Control {
    PeerSeen(String),
    PeerExpired(String),
    Failover {
        peer: String,
        from: endpoints::Kind,
        to: endpoints::Kind,
    },
    Completed(String),
    Expired,
}

/// Bounded handoff from sockets to the root-lock owner. It owns no writer.
pub struct Inbox {
    receiver: mpsc::Receiver<Item>,
}

impl std::fmt::Debug for Inbox {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Inbox")
            .field("queued", &self.receiver.len())
            .finish()
    }
}

/// Running engine and its watch channels. Dropping it cancels sockets and joins its thread.
pub struct SyncHandle {
    cluster_id: String,
    thread: Option<JoinHandle<()>>,
    stop: watch::Sender<bool>,
    peers: watch::Sender<PeerTable>,
    append: watch::Sender<u64>,
    events: watch::Receiver<u64>,
    requests: mpsc::Sender<oneshot::Sender<SyncReport>>,
    automatic: bool,
}

impl std::fmt::Debug for SyncHandle {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("SyncHandle")
            .field("automatic", &self.automatic)
            .finish_non_exhaustive()
    }
}

impl Drop for SyncHandle {
    fn drop(&mut self) {
        self.stop.send_replace(true);
        if let Some(thread) = self.thread.take() {
            let _ = thread.join();
        }
    }
}

impl Node {
    /// Start automatic LAN synchronization (`spec/app-contract.md §6`). Idempotent;
    /// refuses expired or revoked membership. A dedicated runtime owns the sockets,
    /// and refresh/drain operations durably land its bounded incoming pages.
    pub fn start_sync(&mut self) -> Result<()> {
        self.require_sync_standing()?;
        if self
            .sync
            .as_ref()
            .is_some_and(|s| s.thread.as_ref().is_some_and(JoinHandle::is_finished))
        {
            self.sync.take();
            self.inbox.take();
        }
        if self.sync.is_some() {
            return Ok(());
        }
        self.launch_sync(true)
    }

    fn require_sync_standing(&self) -> Result<()> {
        match self.standing(jiff::Timestamp::now())? {
            Standing::Member => Ok(()),
            Standing::Revoked => Err(Error::NodeRevoked),
            Standing::Expired => Err(Error::CertificateExpired),
        }
    }

    fn launch_sync(&mut self, automatic: bool) -> Result<()> {
        self.require_sync_standing()?;
        let table = self.peer_table()?;
        let facts = SyncFacts {
            paths: self.paths.clone(),
            id: self.id().to_string(),
            cluster_id: self.identity.cluster_id().to_string(),
            cluster: self.identity.cluster_public(),
            secret: self.identity.x25519_static(),
            discovery: self
                .discovery
                .as_ref()
                .map(crate::discover::Discovery::shared),
        };
        let (incoming, receiver) = mpsc::channel(64);
        let events_tx = if automatic {
            self.sync_wake
                .get_or_insert_with(|| watch::channel(0u64).0)
                .clone()
        } else {
            watch::channel(0u64).0
        };
        let events = events_tx.subscribe();
        let (peers, peer_rx) = watch::channel(table);
        let (append, append_rx) = watch::channel(self.local_mark());
        let (stop, mut stop_rx) = watch::channel(false);
        let (requests, request_rx) = mpsc::channel(8);
        let runtime = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .map_err(|_| Error::Sync {
                problem: "could not build the synchronization runtime",
            })?;
        // The engine owns its sockets on a thread of its own and reaches the node only
        // through the channels above, so no lock and no writer crosses this boundary.
        let thread = std::thread::Builder::new()
            .name("privatium-sync".into())
            .spawn(move || {
                runtime.block_on(async move {
                    let engine =
                        engine::Engine::new(facts, peer_rx, append_rx, incoming, events_tx);
                    tokio::select! {
                        _ = stop_rx.changed() => {}
                        () = engine.run(request_rx, automatic) => {}
                    }
                });
            })
            .map_err(|_| Error::Sync {
                problem: "could not start the synchronization thread",
            })?;
        self.inbox = Some(Inbox { receiver });
        self.sync = Some(SyncHandle {
            cluster_id: self.identity.cluster_id().to_string(),
            thread: Some(thread),
            stop,
            peers,
            append,
            events,
            requests,
            automatic,
        });
        self.publish_peers()
    }

    /// Run one pass and return peer outcomes, draining incoming logs before reporting
    /// completion. Uses a running engine or a temporary thread/runtime when stopped.
    /// This blocking call needs no caller runtime. Refuses invalid local membership.
    pub fn sync_now(&mut self) -> Result<SyncReport> {
        self.require_sync_standing()?;
        let temporary = self.sync.is_none();
        if temporary {
            self.launch_sync(false)?;
        }
        let result = self.wait_for_pass();
        if temporary {
            self.sync.take();
            self.inbox.take();
        }
        result
    }

    fn wait_for_pass(&mut self) -> Result<SyncReport> {
        self.publish_peers()?;
        let (sender, mut answer) = oneshot::channel();
        self.sync
            .as_ref()
            .ok_or(Error::Sync {
                problem: "engine stopped",
            })?
            .requests
            .try_send(sender)
            .map_err(|_| Error::Sync {
                problem: "engine request queue is unavailable",
            })?;
        // The engine cannot finish a pass until its pages are landed, and only this
        // thread holds what landing them needs, so the wait has to keep draining rather
        // than block on the answer.
        loop {
            self.drain()?;
            match answer.try_recv() {
                Ok(report) => {
                    self.drain()?;
                    return Ok(report);
                }
                Err(oneshot::error::TryRecvError::Closed) => {
                    return Err(Error::Sync {
                        problem: "engine stopped during the pass",
                    });
                }
                Err(oneshot::error::TryRecvError::Empty) => {
                    std::thread::park_timeout(Duration::from_millis(5))
                }
            }
        }
    }

    /// Subscribe to inbox arrivals after `start_sync`. Refresh loaded apps and deliver
    /// their pending callbacks after each wake; the watch identity is stable per start.
    #[must_use]
    pub fn sync_events(&self) -> Option<watch::Receiver<u64>> {
        self.sync
            .as_ref()
            .filter(|handle| handle.automatic)
            .map(|handle| handle.events.clone())
    }

    /// A number that changes whenever this node appends anything. It counts nothing an
    /// owner would recognize; the engine only compares it with the one it last saw.
    fn local_mark(&self) -> u64 {
        self.apps.values().fold(self.sys.seq(), |mark, app| {
            mark.saturating_add(app.log.seq())
        })
    }

    pub(crate) fn notify_append(&self) {
        if let Some(sync) = &self.sync {
            let mark = self.local_mark();
            sync.append.send_if_modified(|old| {
                if *old == mark {
                    false
                } else {
                    *old = mark;
                    true
                }
            });
        }
    }

    fn peer_table(&self) -> Result<PeerTable> {
        let hints = self.peer_hints();
        let mut ids: BTreeSet<String> = hints.iter().map(|hint| hint.id.clone()).collect();
        let mut statement = self
            .store
            .conn()
            .prepare("SELECT id FROM sys_device WHERE kind = 'node'")
            .map_err(store::StoreError::Sql)
            .map_err(boxed)?;
        for id in statement
            .query_map([], |row| row.get::<_, String>(0))
            .map_err(store::StoreError::Sql)
            .map_err(boxed)?
        {
            ids.insert(id.map_err(store::StoreError::Sql).map_err(boxed)?);
        }
        let mut peers = BTreeMap::new();
        for id in ids {
            let Some((_, pins, kind)) = crate::wire::channel::peer_pins(self, &id) else {
                continue;
            };
            if pins.revoked || kind != crate::http::auth::SessionKind::Node {
                continue;
            }
            let Some(key) = pins
                .x25519
                .and_then(|key| STANDARD.decode(key).ok())
                .and_then(|bytes| <[u8; 32]>::try_from(bytes).ok())
            else {
                continue;
            };
            let url = hints
                .iter()
                .find(|hint| hint.id == id)
                .and_then(|hint| hint.url.clone());
            peers.insert(
                id.clone(),
                PeerEntry {
                    id,
                    x25519: key,
                    url,
                },
            );
        }
        let until = if self.standing(jiff::Timestamp::now())? == Standing::Member {
            self.identity.certificate().expires_at.parse().ok()
        } else {
            None
        };
        Ok(PeerTable {
            peers,
            until,
            max_body: crate::wire::ApiSettings::read(self).max_body,
        })
    }

    /// Publish current pairing and revocation facts to the engine. A revoked own node
    /// cancels its engine; public keys from discovery never become authentication pins.
    pub(crate) fn publish_peers(&mut self) -> Result<()> {
        if self.sync.is_none() {
            return Ok(());
        }
        // A running engine holds the cluster key it started with, and joining another
        // cluster replaces that key: it has to start again or it would dial the old
        // cluster's peers with a certificate they refuse (`spec/protocol.md §2.3.1`).
        if let Some(handle) = &self.sync
            && handle.cluster_id != self.identity.cluster_id().as_str()
        {
            let automatic = handle.automatic;
            self.sync.take();
            self.inbox.take();
            return self.launch_sync(automatic);
        }
        // An engine stops itself when this node's standing lapses; re-admission gives
        // it back, and only a fresh engine holds the new certificate (`§2.3.1`).
        let table = self.peer_table()?;
        if let Some(handle) = &self.sync
            && *handle.stop.borrow()
            && table.until.is_some()
        {
            let automatic = handle.automatic;
            self.sync.take();
            self.inbox.take();
            return self.launch_sync(automatic);
        }
        if let Some(sync) = &self.sync {
            if sync.automatic {
                self.store
                    .note_peers(
                        self.id().as_str(),
                        &table.peers.keys().cloned().collect::<Vec<_>>(),
                    )
                    .map_err(boxed)?;
            }
            sync.peers.send_if_modified(|old| {
                if *old == table {
                    false
                } else {
                    *old = table.clone();
                    true
                }
            });
            if table.until.is_none() {
                sync.stop.send_replace(true);
            }
        }
        self.notify_append();
        Ok(())
    }

    /// Drain one bounded inbox window, system logs first. Draining all touched apps
    /// prevents an unopened app's pages from accumulating outside the bounded inbox.
    pub(crate) fn drain(&mut self) -> Result<bool> {
        if self.draining {
            return Ok(false);
        }
        let mut items = Vec::new();
        if let Some(inbox) = &mut self.inbox {
            for _ in 0..64 {
                match inbox.receiver.try_recv() {
                    Ok(item) => items.push(item),
                    Err(_) => break,
                }
            }
        }
        if items.is_empty() {
            return Ok(false);
        }
        type Pending = (
            PeerEntry,
            Vec<u8>,
            oneshot::Sender<std::result::Result<ReceiveReport, &'static str>>,
        );
        let mut grouped: BTreeMap<String, BTreeMap<String, Vec<Pending>>> = BTreeMap::new();
        let mut controls = Vec::new();
        for item in items {
            match item {
                Item::Page {
                    peer,
                    app,
                    dev,
                    bytes,
                    done,
                } => {
                    let entry = grouped.entry(app).or_default().entry(dev).or_default();
                    entry.push((peer, bytes, done));
                }
                Item::Control { control, done } => controls.push((control, done)),
            }
        }
        let mut groups = Vec::new();
        if let Some(system) = grouped.remove(sys::SLUG) {
            groups.push((sys::SLUG.to_owned(), system));
        }
        groups.extend(grouped);
        self.draining = true;
        let result = (|| {
            for (app, group) in groups {
                // The preceding system range may revoke a sender whose app page was
                // already queued. Recheck pins here, before any destination is opened.
                let table = self.peer_table()?;
                let mut ranges = BTreeMap::new();
                let mut replies = BTreeMap::new();
                for (dev, pending) in group {
                    for (peer, bytes, done) in pending {
                        if table
                            .until
                            .is_none_or(|until| jiff::Timestamp::now() >= until)
                            || table.peers.get(&peer.id) != Some(&peer)
                        {
                            let _ = done
                                .send(Err("range sender expired, was revoked, or changed pins"));
                            continue;
                        }
                        ranges
                            .entry(dev.clone())
                            .or_insert_with(Vec::new)
                            .extend_from_slice(&bytes);
                        replies
                            .entry(dev.clone())
                            .or_insert_with(Vec::new)
                            .push(done);
                    }
                }
                let mut reports = self.receive_ranges(&app, &ranges).unwrap_or_default();
                for (dev, done) in replies {
                    let report = reports
                        .remove(&dev)
                        .and_then(Result::ok)
                        .ok_or("received range could not be durably validated and applied");
                    for done in done {
                        let _ = done.send(report);
                    }
                }
            }
            for (control, done) in controls {
                let result = self
                    .sync_control(control)
                    .map_err(|_| "synchronization control could not be recorded");
                let _ = done.send(result);
            }
            self.publish_peers()?;
            Ok(true)
        })();
        self.draining = false;
        result
    }

    fn sync_control(&mut self, control: Control) -> Result<()> {
        match control {
            Control::Completed(peer) => {
                self.require_sync_standing()?;
                if !self.peer_table()?.peers.contains_key(&peer) {
                    return Err(Error::Sync {
                        problem: "peer was revoked during the pass",
                    });
                }
                if self.sync.as_ref().is_some_and(|s| s.automatic) {
                    self.store.note_peer_synced(&peer).map_err(boxed)?;
                }
                self.renew_certificate_if_due(jiff::Timestamp::now())?;
            }
            Control::PeerSeen(peer) => {
                if self.sync_seen.contains(&peer) {
                    return Ok(());
                }
                self.sys.put(
                    sys::AUDIT,
                    &crate::new_ulid(),
                    &sys::AuditRow::info(&log::now(), sys::KIND_SYNC_PEER_SEEN, Some(&peer), "{}"),
                )?;
                self.sync_seen.insert(peer);
            }
            Control::PeerExpired(peer) => {
                if self.sync_expired.contains(&peer) {
                    return Ok(());
                }
                self.sys.put(
                    sys::AUDIT,
                    &crate::new_ulid(),
                    &sys::AuditRow::warn(&log::now(), sys::KIND_CERT_EXPIRED, Some(&peer), "{}"),
                )?;
                self.sync_expired.insert(peer);
            }
            Control::Failover { peer, from, to } => {
                let detail =
                    serde_json::json!({"from_kind":from.to_string(),"to_kind":to.to_string()})
                        .to_string();
                self.sys.put(
                    sys::AUDIT,
                    &crate::new_ulid(),
                    &sys::AuditRow::info(
                        &log::now(),
                        sys::KIND_ENDPOINT_FAILOVER,
                        Some(&peer),
                        &detail,
                    ),
                )?;
            }
            Control::Expired => {
                self.audit_standing(jiff::Timestamp::now())?;
            }
        }
        self.store.refresh(&store::cutoff_now()).map_err(boxed)?;
        self.flush()?;
        self.publish_peers()
    }
}
