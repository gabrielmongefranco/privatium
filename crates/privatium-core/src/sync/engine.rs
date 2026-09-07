// Project:  Privatium™  |  File: crates/privatium-core/src/sync/engine.rs
// Authors:  Gabriel Mongefranco (@gabrielmongefranco)
// Created:  2026-09-07  |  Modified: 2026-09-07
// Summary:  Encrypted peer synchronization over the existing application channel,
//           with durable inbox acknowledgements and bounded failover (spec/protocol.md §10).
//           See main README.md for full license information.

use std::collections::{BTreeMap, BTreeSet};
use std::time::{Duration, Instant};

use axum::body::Bytes;
use futures_util::{SinkExt, StreamExt};
use tokio::sync::{mpsc, oneshot, watch};
use tokio_tungstenite::tungstenite::{Message, protocol::WebSocketConfig};

use super::{
    Control, Item, PeerEntry, PeerReport, PeerTable, SyncFacts, SyncReport, endpoints, source,
};
use crate::session::handshake::{ClientHandshake, NodePins};
use crate::wire::channel::{Frame, Kind};

const EXCHANGE_TIMEOUT: Duration = Duration::from_secs(10);
const HEAD_LIMIT: usize = 1024 * 1024;
const FRAME_ALLOWANCE: usize = 64 * 1024 + 20;
const PAGE_LIMIT: usize = 1024;
type Outcome<T> = std::result::Result<T, &'static str>;

/// Why one log's ranges stopped. A refusal concerns that log alone and leaves the
/// channel usable, so the rest of the pass still runs; a fault ends the pass. Either
/// way the pass is not completed, and nothing renews on it (`spec/protocol.md §2.3.1`).
enum Refused {
    /// This log alone cannot finish. The next log is still worth trying.
    Log(&'static str),
    /// The channel, the peer's answers or this node's standing cannot be relied on.
    Fault(&'static str),
}

impl From<&'static str> for Refused {
    fn from(reason: &'static str) -> Self {
        Self::Fault(reason)
    }
}

/// One origin's log and how far each side has it.
struct LogRange<'a> {
    app: &'a str,
    dev: &'a str,
    ours: u64,
    theirs: u64,
}
type Socket =
    tokio_tungstenite::WebSocketStream<tokio_tungstenite::MaybeTlsStream<tokio::net::TcpStream>>;

pub(super) struct Engine {
    facts: SyncFacts,
    peers: watch::Receiver<PeerTable>,
    append: watch::Receiver<u64>,
    inbox: mpsc::Sender<Item>,
    events: watch::Sender<u64>,
    candidates: BTreeMap<String, endpoints::Candidates>,
    seen: BTreeSet<String>,
    answered: bool,
}

impl Engine {
    pub(super) fn new(
        facts: SyncFacts,
        peers: watch::Receiver<PeerTable>,
        append: watch::Receiver<u64>,
        inbox: mpsc::Sender<Item>,
        events: watch::Sender<u64>,
    ) -> Self {
        Self {
            facts,
            peers,
            append,
            inbox,
            events,
            candidates: BTreeMap::new(),
            seen: BTreeSet::new(),
            answered: false,
        }
    }

    /// Wait for a reason to sync and then sync: a peer appearing on the network, a
    /// local write settling, the peer table changing, an explicit request, or the timer
    /// that catches everything the others miss. A node that waited on the timer alone
    /// would leave a write sitting for a minute (`spec/protocol.md §10.4`).
    pub(super) async fn run(
        mut self,
        mut requests: mpsc::Receiver<oneshot::Sender<SyncReport>>,
        automatic: bool,
    ) {
        let mut poll = tokio::time::interval(Duration::from_secs(5));
        let mut periodic = tokio::time::interval(Duration::from_secs(60));
        poll.tick().await;
        periodic.tick().await;
        let mut discovery = self.discovered();
        if automatic {
            self.pass().await;
        }
        loop {
            // Each arm answers with the caller waiting on this pass, if any. A dropped
            // channel or a lapsed standing ends the engine rather than spinning.
            let request = tokio::select! {
                request = requests.recv() => match request {
                    Some(request) => Some(request),
                    None => return,
                },

                // A local write. Waiting out a second of quiet keeps a burst of appends
                // to one pass instead of one pass each.
                changed = self.append.changed(), if automatic => {
                    if changed.is_err() {
                        return;
                    }
                    while let Ok(changed) =
                        tokio::time::timeout(Duration::from_secs(1), self.append.changed()).await
                    {
                        if changed.is_err() {
                            return;
                        }
                    }
                    None
                },

                // A peer was admitted, revoked or re-keyed: what to dial has changed.
                changed = self.peers.changed(), if automatic => {
                    if changed.is_err() {
                        return;
                    }
                    None
                },

                // The fallback, once a peer has ever answered: it catches whatever the
                // other arms missed, such as a write on a node that cannot reach us.
                _ = periodic.tick(), if automatic && self.answered => None,

                // Discovery is a table another thread fills in, so it is read on a timer
                // rather than plumbed through as a callback. A pass follows only a
                // change; an unchanged table is not a reason to dial anyone.
                _ = poll.tick(), if automatic => {
                    let lapsed = self
                        .peers
                        .borrow()
                        .until
                        .is_none_or(|until| jiff::Timestamp::now() >= until);
                    if lapsed {
                        let _ = self.control(Control::Expired).await;
                        return;
                    }
                    let found = self.discovered();
                    if found == discovery {
                        continue;
                    }
                    discovery = found;
                    None
                },
            };
            let report = self.pass().await;
            if let Some(request) = request {
                let _ = request.send(report);
            }
        }
    }

    fn discovered(&self) -> Vec<(String, Vec<std::net::IpAddr>, u16)> {
        self.facts
            .discovery
            .as_ref()
            .map(|d| d.discovered())
            .unwrap_or_default()
            .into_iter()
            .filter(|d| d.cluster == self.facts.cluster_id && d.id != self.facts.id)
            .map(|d| (d.id, d.addrs, d.port))
            .collect()
    }

    fn authorized(&self, peer: &PeerEntry) -> Outcome<()> {
        let table = self.peers.borrow();
        if table
            .until
            .is_none_or(|until| jiff::Timestamp::now() >= until)
        {
            return Err("local membership expired or was revoked");
        }
        if table.peers.get(&peer.id) != Some(peer) {
            return Err("peer pins changed or peer was revoked");
        }
        Ok(())
    }

    fn limit(&self) -> usize {
        self.peers.borrow().max_body
    }

    async fn enqueue(&self, item: Item) -> Outcome<()> {
        self.inbox
            .send(item)
            .await
            .map_err(|_| "node stopped receiving synchronization pages")?;
        self.events.send_modify(|n| *n = n.wrapping_add(1));
        Ok(())
    }

    async fn control(&self, control: Control) -> Outcome<()> {
        let (done, answer) = oneshot::channel();
        self.enqueue(Item::Control { control, done }).await?;
        answer
            .await
            .map_err(|_| "node stopped before recording synchronization control")?
    }

    async fn receive(
        &self,
        peer: &PeerEntry,
        app: &str,
        dev: &str,
        bytes: Vec<u8>,
    ) -> Outcome<super::ReceiveReport> {
        self.authorized(peer)?;
        let (done, answer) = oneshot::channel();
        self.enqueue(Item::Page {
            peer: peer.clone(),
            app: app.into(),
            dev: dev.into(),
            bytes,
            done,
        })
        .await?;
        // This is a local durability handoff, not an acknowledgement protocol in logs.
        // Completion cannot precede the receiver's fsync and cache rebuild (§10.2).
        answer
            .await
            .map_err(|_| "node stopped before durably receiving the range")?
    }

    async fn pass(&mut self) -> SyncReport {
        let peers: Vec<_> = self.peers.borrow().peers.values().cloned().collect();
        let mut report = SyncReport::default();
        self.candidates
            .retain(|id, _| peers.iter().any(|peer| &peer.id == id));
        for peer in peers {
            let mut outcome = PeerReport {
                id: peer.id.clone(),
                ..PeerReport::default()
            };
            let deadline = Instant::now() + endpoints::SEARCH_TIMEOUT;
            let first = self.peer_pass(&peer, &mut outcome, deadline).await;
            let result = if first.as_ref().is_err_and(|reason| {
                matches!(
                    *reason,
                    "peer connection closed"
                        | "peer connection was lost"
                        | "peer response timed out"
                )
            }) && Instant::now() < deadline
            {
                self.peer_pass(&peer, &mut outcome, deadline).await
            } else {
                first
            };
            if let Err(reason) = result {
                outcome.refusals.push(reason.into());
            }
            report.peers.push(outcome);
        }
        report
    }

    async fn connect(
        &mut self,
        peer: &PeerEntry,
        report: &mut PeerReport,
        deadline: Instant,
    ) -> Outcome<Client> {
        self.authorized(peer)?;
        let candidates = self.candidates.entry(peer.id.clone()).or_default();
        if let Some(url) = &peer.url {
            candidates.merge(url, endpoints::Kind::Static);
        }
        if let Some(discovery) = &self.facts.discovery {
            for found in discovery
                .discovered()
                .into_iter()
                .filter(|found| found.id == peer.id && found.cluster == self.facts.cluster_id)
            {
                for addr in found.addrs {
                    let url = format!("http://{}", std::net::SocketAddr::new(addr, found.port));
                    candidates.merge(
                        &url,
                        if found.instance.is_empty() {
                            endpoints::Kind::LanUdp
                        } else {
                            endpoints::Kind::LanMdns
                        },
                    );
                }
            }
        }
        let ordered = candidates.ordered();
        // The endpoint that last worked, remembered before anything is tried: answering
        // on a different one is what failover means, and the owner is told once it
        // happens (`spec/protocol.md §10.4`).
        let previous = ordered
            .iter()
            .find(|candidate| candidate.last_ok.is_some())
            .cloned();
        let mut reason = "no usable endpoint is known";
        for candidate in ordered {
            if Instant::now() >= deadline {
                break;
            }
            report.url = candidate.url.clone();
            let started = Instant::now();
            let target = endpoints::target(&candidate.url)?;
            let remaining = deadline
                .saturating_duration_since(Instant::now())
                .min(endpoints::CONNECT_TIMEOUT);
            // One envelope plus its bounded preceding filler may exceed one request
            // body. The page cap is twice that body; frame metadata has its own bound.
            let maximum = self
                .limit()
                .saturating_mul(2)
                .max(HEAD_LIMIT)
                .saturating_add(FRAME_ALLOWANCE);
            let config = WebSocketConfig::default()
                .max_message_size(Some(maximum))
                .max_frame_size(Some(maximum));
            let connected = tokio::time::timeout(
                remaining,
                tokio_tungstenite::connect_async_with_config(&target, Some(config), false),
            )
            .await;
            let result = match connected {
                Ok(Ok((socket, _))) => tokio::time::timeout(
                    EXCHANGE_TIMEOUT,
                    Client::handshake(socket, &self.facts, peer),
                )
                .await
                .unwrap_or(Err("peer handshake timed out")),
                _ => Err("endpoint connection failed or timed out"),
            };
            if let Some(candidates) = self.candidates.get_mut(&peer.id) {
                candidates.note(&candidate.url, result.is_ok(), started.elapsed());
            }
            match result {
                Ok(client) => {
                    self.answered = true;
                    if let Some(previous) = &previous
                        && previous.url != candidate.url
                    {
                        self.control(Control::Failover {
                            peer: peer.id.clone(),
                            from: previous.kind,
                            to: candidate.kind,
                        })
                        .await?;
                    }
                    if self.seen.insert(peer.id.clone()) {
                        self.control(Control::PeerSeen(peer.id.clone())).await?;
                    }
                    return Ok(client);
                }
                Err(why) => {
                    if why == "peer certificate expired" {
                        self.control(Control::PeerExpired(peer.id.clone())).await?;
                    }
                    reason = why;
                }
            }
        }
        Err(reason)
    }

    async fn peer_pass(
        &mut self,
        peer: &PeerEntry,
        report: &mut PeerReport,
        deadline: Instant,
    ) -> Outcome<()> {
        let mut client = self.connect(peer, report, deadline).await?;
        let remote = client.heads(None).await?;
        let local = source::heads(&self.facts.paths, self.limit())
            .map_err(|_| "local log heads could not be read")?;
        let mut apps: BTreeSet<String> = local.keys().chain(remote.keys()).cloned().collect();
        apps.remove("_sys");
        let apps = std::iter::once("_sys".to_owned()).chain(apps);
        let mut whole = true;
        for app in apps {
            self.authorized(peer)?;
            // Ask again after _sys is durable; it may have taught either node new peers.
            let remote = client
                .heads(Some(&app))
                .await?
                .remove(&app)
                .unwrap_or_default();
            let local = source::app_heads(&self.facts.paths, &app, self.limit())
                .map_err(|_| "local app heads could not be read")?;
            let devices: BTreeSet<_> = local.keys().chain(remote.keys()).cloned().collect();
            for dev in devices {
                let ours = local.get(&dev).copied().unwrap_or(0);
                let theirs = remote.get(&dev).copied().unwrap_or(0);
                let range = LogRange {
                    app: &app,
                    dev: &dev,
                    ours,
                    theirs,
                };
                match self.sync_log(&mut client, peer, range, report).await {
                    Ok(()) => {}
                    // One log a peer will not take, or will not give, must not stop the
                    // apps after it: they would never sync while it stood.
                    Err(Refused::Log(reason)) => {
                        whole = false;
                        if !report.refusals.iter().any(|seen| seen == reason) {
                            report.refusals.push(reason.into());
                        }
                    }
                    Err(Refused::Fault(reason)) => return Err(reason),
                }
            }
        }
        if whole {
            self.authorized(peer)?;
            self.control(Control::Completed(peer.id.clone())).await?;
            report.completed = true;
        }
        let _ = tokio::time::timeout(Duration::from_secs(1), client.socket.close(None)).await;
        Ok(())
    }

    /// Bring one origin's log level with a peer's, in whichever direction is behind.
    /// A problem with this one log is a [`Refused::Log`], so the pass goes on to the
    /// next; only a channel or standing problem is a [`Refused::Fault`].
    async fn sync_log(
        &self,
        client: &mut Client,
        peer: &PeerEntry,
        range: LogRange<'_>,
        report: &mut PeerReport,
    ) -> std::result::Result<(), Refused> {
        let LogRange {
            app,
            dev,
            ours,
            theirs,
        } = range;
        if theirs > ours {
            if dev == self.facts.id {
                return Err(Refused::Fault(
                    "peer advertises missing lines for this node's own log",
                ));
            }
            let mut after = ours;
            for page in 0..PAGE_LIMIT {
                self.authorized(peer)?;
                let response = client
                    .request(
                        "GET",
                        &format!("/api/v1/sync/pull?app={app}&dev={dev}&after={after}"),
                        Vec::new(),
                        self.limit().saturating_mul(2),
                    )
                    .await?;
                if response.status != 200 {
                    return Err(Refused::Log("peer refused a pull range"));
                }
                let next = response.number("pv-next")?;
                let head = response.number("pv-head")?;
                if next <= after || next > head || head < theirs || response.body.is_empty() {
                    return Err(Refused::Log("pull page made no valid sequence progress"));
                }
                let received =
                    self.receive(peer, app, dev, response.body)
                        .await
                        .map_err(|reason| {
                            // A closed inbox is the node going away, not this log's problem.
                            if self.inbox.is_closed() {
                                Refused::Fault(reason)
                            } else {
                                Refused::Log(reason)
                            }
                        })?;
                if received.head != next {
                    return Err(Refused::Log(
                        "pull cursor differs from the durable received head",
                    ));
                }
                report.pulled_lines += received.lines;
                after = next;
                if after == head {
                    break;
                }
                if page + 1 == PAGE_LIMIT {
                    return Err(Refused::Log("pull range exceeded the page limit"));
                }
            }
        } else if ours > theirs {
            let source = source::Source::open(&self.facts.paths, app, dev, self.limit())
                .map_err(|_| Refused::Log("local log source could not be opened"))?;
            let mut after = theirs;
            for index in 0..PAGE_LIMIT {
                self.authorized(peer)?;
                let page = source
                    .page(after)
                    .map_err(|_| Refused::Log("local log cannot form a bounded page"))?;
                if page.next <= after {
                    return Err(Refused::Log("local push range made no sequence progress"));
                }
                let body: Vec<u8> = page.lines.into_iter().flatten().collect();
                if body.len() > self.limit() {
                    return Err(Refused::Log(
                        "push range exceeds the peer request body bound",
                    ));
                }
                let count = body.iter().filter(|b| **b == b'\n').count();
                let response = client
                    .request(
                        "POST",
                        &format!("/api/v1/sync/push?app={app}&dev={dev}"),
                        body,
                        HEAD_LIMIT,
                    )
                    .await?;
                if response.status != 200 {
                    return Err(Refused::Log(
                        "peer refused a push range; ask its heads on the next pass",
                    ));
                }
                let value: serde_json::Value = serde_json::from_slice(&response.body)
                    .map_err(|_| Refused::Log("invalid push response"))?;
                if value.get("head").and_then(serde_json::Value::as_u64) != Some(page.next) {
                    return Err(Refused::Log(
                        "push acknowledgement differs from the sent range",
                    ));
                }
                report.pushed_lines += count;
                after = page.next;
                if after == page.head {
                    break;
                }
                if index + 1 == PAGE_LIMIT {
                    return Err(Refused::Log("push range exceeded the page limit"));
                }
            }
        }
        Ok(())
    }
}

struct Client {
    socket: Socket,
    crypto: crate::session::Session,
    id: u64,
}
struct Response {
    status: u16,
    headers: BTreeMap<String, String>,
    body: Vec<u8>,
}

impl Response {
    fn number(&self, name: &str) -> Outcome<u64> {
        self.headers
            .get(name)
            .and_then(|n| n.parse().ok())
            .ok_or("missing or invalid sequence header")
    }
}

impl Client {
    async fn handshake(mut socket: Socket, facts: &SyncFacts, peer: &PeerEntry) -> Outcome<Self> {
        let pins = NodePins {
            id: peer.id.clone(),
            cluster: facts.cluster,
            x25519: x25519_dalek::PublicKey::from(peer.x25519),
        };
        let (handshake, hello) = ClientHandshake::start(&facts.id, facts.secret.clone(), pins)
            .map_err(|_| "could not start the pinned handshake")?;
        socket
            .send(Message::Text(hello.into()))
            .await
            .map_err(|_| "peer disconnected during handshake")?;
        let Message::Text(hello) = next(&mut socket).await? else {
            return Err("peer sent an invalid handshake message");
        };
        if hello.len() > 8192 {
            return Err("peer handshake exceeds the message bound");
        }
        if let Ok(value) = serde_json::from_str::<serde_json::Value>(&hello)
            && let Some(cert) = value.get("cert").and_then(serde_json::Value::as_str)
            && let Ok(cert) = crate::identity::Certificate::from_base64(cert)
            && cert.node_id == peer.id
            && matches!(
                cert.verify(&facts.cluster, jiff::Timestamp::now()),
                Err(crate::identity::CertificateError::Expired)
            )
        {
            return Err("peer certificate expired");
        }
        let (crypto, confirm) = handshake
            .finish(&hello, jiff::Timestamp::now())
            .map_err(|_| "peer certificate or pinned identity was refused")?;
        socket
            .send(Message::Binary(confirm.into()))
            .await
            .map_err(|_| "peer disconnected during handshake")?;
        let mut client = Self {
            socket,
            crypto,
            id: 0,
        };
        let response = client
            .request("GET", "/api/v1/health", Vec::new(), HEAD_LIMIT)
            .await?;
        if response.status != 200 {
            return Err("peer refused the authenticated health request");
        }
        Ok(client)
    }

    async fn heads(&mut self, app: Option<&str>) -> Outcome<source::Heads> {
        let path = app.map_or_else(
            || "/api/v1/sync/heads".into(),
            |app| format!("/api/v1/sync/heads?app={app}"),
        );
        let response = self.request("GET", &path, Vec::new(), HEAD_LIMIT).await?;
        if response.status != 200 {
            return Err("peer refused its sequence heads");
        }
        let heads: source::Heads = if let Some(app) = app {
            let heads = serde_json::from_slice::<BTreeMap<String, u64>>(&response.body)
                .map_err(|_| "invalid peer sequence heads")?;
            BTreeMap::from([(app.into(), heads)])
        } else {
            serde_json::from_slice(&response.body).map_err(|_| "invalid peer sequence heads")?
        };
        for (app, devices) in &heads {
            crate::log::foreign::validate_destination(app, "00000000")
                .map_err(|_| "invalid app in peer heads")?;
            if devices.keys().any(|dev| !crate::NodeId::is_valid(dev)) {
                return Err("invalid device in peer heads");
            }
        }
        Ok(heads)
    }

    async fn request(
        &mut self,
        method: &str,
        path: &str,
        body: Vec<u8>,
        limit: usize,
    ) -> Outcome<Response> {
        tokio::time::timeout(EXCHANGE_TIMEOUT, self.exchange(method, path, body, limit))
            .await
            .unwrap_or(Err("peer response timed out"))
    }

    async fn exchange(
        &mut self,
        method: &str,
        path: &str,
        body: Vec<u8>,
        limit: usize,
    ) -> Outcome<Response> {
        self.id = self
            .id
            .checked_add(1)
            .ok_or("channel request counter exhausted")?;
        let mut frame = Frame::new(self.id, Kind::Req);
        frame.method = Some(method.into());
        frame.headers = Some(BTreeMap::new());
        frame.path = Some(path.into());
        frame.payload = Bytes::from(body);
        if method == "POST" {
            frame.headers = Some(BTreeMap::from([(
                "content-type".into(),
                "application/x-ndjson".into(),
            )]));
        }
        let encoded = frame
            .encode()
            .map_err(|_| "could not encode sync request")?;
        let sealed = self
            .crypto
            .send
            .seal(&encoded)
            .map_err(|_| "channel encryption failed")?;
        self.socket
            .send(Message::Binary(sealed.into()))
            .await
            .map_err(|_| "peer connection was lost")?;
        let mut response = None;
        loop {
            let Message::Binary(bytes) = next(&mut self.socket).await? else {
                return Err("peer sent a nonbinary response");
            };
            let plain = self
                .crypto
                .receive
                .open(&bytes)
                .map_err(|_| "peer response authentication failed")?;
            let frame = Frame::decode(&plain).map_err(|_| "invalid encrypted response frame")?;
            if frame.id != self.id
                || frame.method.is_some()
                || frame.path.is_some()
                || frame.navigation.is_some()
                || frame.handoff.is_some()
            {
                return Err("peer response has invalid request metadata");
            }
            match frame.kind {
                Kind::Res if response.is_none() && frame.payload.is_empty() => {
                    response = Some(Response {
                        status: frame.status.ok_or("peer omitted response status")?,
                        headers: frame.headers.unwrap_or_default(),
                        body: Vec::new(),
                    });
                }
                Kind::Chunk if frame.status.is_none() && frame.headers.is_none() => {
                    let response = response
                        .as_mut()
                        .ok_or("peer sent a body before its response head")?;
                    if frame.payload.len() > limit.saturating_sub(response.body.len()) {
                        return Err("peer response exceeds its byte bound");
                    }
                    response.body.extend_from_slice(&frame.payload);
                }
                Kind::End
                    if frame.status.is_none()
                        && frame.headers.is_none()
                        && frame.payload.is_empty() =>
                {
                    return response.ok_or("peer ended a response without its head");
                }
                _ => return Err("peer sent an unexpected response frame"),
            }
        }
    }
}

async fn next(socket: &mut Socket) -> Outcome<Message> {
    loop {
        match socket.next().await {
            Some(Ok(Message::Ping(_) | Message::Pong(_))) => continue,
            Some(Ok(Message::Close(_))) | Some(Err(_)) | None => {
                return Err("peer connection closed");
            }
            Some(Ok(message)) => return Ok(message),
        }
    }
}
