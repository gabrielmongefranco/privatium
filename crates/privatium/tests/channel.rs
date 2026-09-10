// This file is part of Privatium
// crates/privatium/tests/channel.rs
// Author(s): Gabriel Mongefranco
// Created: 2026-09-05
// Last Modified: 2026-09-08
// Summary: Actual WebSocket pairing, authenticated routing, streaming and refusal (§8); the owner-only
//          acts a session is refused (§9.2), the session device an app sees, and the refusal a
//          paired client makes of a re-keyed node (§8.1).
// Notes: See README file for documentation and full license information.
//
// Copyright © 2026 Gabriel Mongefranco
//
// This program is free software: you can redistribute it and/or modify
// it under the terms of the GNU General Public License as published by
// the Free Software Foundation, either version 3 of the License, or (at your option) any later version.
//
// This program is distributed in the hope that it will be useful,
// but WITHOUT ANY WARRANTY; without even the implied warranty of
// MERCHANTABILITY or FITNESS FOR A PARTICULAR PURPOSE. See the
// GNU General Public License for more details.
//
// You should have received a copy of the GNU General Public License along
// with this program. If not, see <https://www.gnu.org/licenses/>.

#![allow(clippy::unwrap_used, clippy::expect_used)]

use axum::ServiceExt as _;
use futures_util::{SinkExt as _, StreamExt as _};
use privatium_core::pair::{
    Code,
    handshake::{Client, ClientPaired},
};
use privatium_core::session::handshake::{ClientHandshake, NodePins};
use privatium_core::wire::channel::{Frame, Kind};
use privatium_core::{AppRoot, Handler, Node, Peer, Request};
use std::{
    collections::BTreeMap,
    convert::Infallible,
    path::Path,
    sync::{Arc, Mutex},
    time::Duration,
};
use tokio::{
    io::{AsyncReadExt as _, AsyncWriteExt as _},
    net::{TcpListener, TcpStream},
};
use tokio_tungstenite::{MaybeTlsStream, WebSocketStream, tungstenite::Message};

type Socket = WebSocketStream<MaybeTlsStream<TcpStream>>;

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn test_spec_8_3_browser_client_against_live_core() {
    let fixture = Fixture::new().await;
    let snapshot = fixture
        .handler
        .node()
        .lock()
        .unwrap()
        .pair(Duration::from_secs(120))
        .unwrap();
    let input = serde_json::json!({ "origin": format!("http://127.0.0.1:{}", fixture.port), "code": snapshot.words.join(" ") }).to_string();
    let result = tokio::task::spawn_blocking(move || {
        use std::io::Write as _;
        use std::process::{Command, Stdio};
        let node = std::env::var_os("PRIVATIUM_TEST_NODE").unwrap_or_else(|| "node".into());
        let script = Path::new(env!("CARGO_MANIFEST_DIR")).join("../privatium-core/tests/js/live-client.mjs");
        let mut child = Command::new(node).arg(script).stdin(Stdio::piped()).stdout(Stdio::piped()).stderr(Stdio::piped()).spawn().expect("Node.js is required for the live browser-module test; set PRIVATIUM_TEST_NODE when it is not on PATH");
        child.stdin.take().unwrap().write_all(input.as_bytes()).unwrap();
        child.wait_with_output().unwrap()
    }).await.unwrap();
    assert!(
        result.status.success(),
        "{}",
        String::from_utf8_lossy(&result.stderr)
    );
    assert_eq!(
        String::from_utf8_lossy(&result.stdout).trim(),
        "browser-live: passed"
    );
}

struct Fixture {
    _root: tempfile::TempDir,
    handler: Arc<Handler>,
    port: u16,
    capture: Arc<Mutex<Vec<u8>>>,
    tasks: Vec<tokio::task::JoinHandle<()>>,
}
impl Drop for Fixture {
    fn drop(&mut self) {
        for task in &self.tasks {
            task.abort();
        }
    }
}
impl Fixture {
    async fn new() -> Self {
        Self::with_apps(tempfile::tempdir().unwrap(), None).await
    }

    /// A node over `root` — fresh, or one whose `data/` was copied in — with the
    /// repository's apps and, when given, the owner's apps under `local`.
    async fn with_apps(root: tempfile::TempDir, local: Option<&Path>) -> Self {
        Self::build(root, local, |_| {}).await
    }

    /// [`Fixture::with_apps`] with the handler adjusted before it serves — the handshake
    /// bounds, for the tests that wait on them.
    async fn build(
        root: tempfile::TempDir,
        local: Option<&Path>,
        configure: impl FnOnce(&mut Handler),
    ) -> Self {
        let mut node = Node::open(root.path()).unwrap();
        let mut roots = vec![AppRoot::bundled(
            Path::new(env!("CARGO_MANIFEST_DIR")).join("../../apps"),
        )];
        if let Some(local) = local {
            roots.insert(0, AppRoot::local(local.to_path_buf()));
        }
        let report = node.load_apps(&roots).unwrap();
        let mut handler = Handler::new(node, report);
        configure(&mut handler);
        let handler = Arc::new(handler);
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let destination = listener.local_addr().unwrap();
        let h = handler.clone();
        let server = tokio::spawn(async move {
            let service = tower::service_fn(move |mut request: Request| {
                let h = h.clone();
                async move {
                    request
                        .extensions_mut()
                        .insert(Peer("192.0.2.10:4000".parse().unwrap()));
                    Ok::<_, Infallible>(h.handle(request).await)
                }
            });
            axum::serve(listener, service.into_make_service())
                .await
                .unwrap();
        });
        let proxy = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let port = proxy.local_addr().unwrap().port();
        let capture = Arc::new(Mutex::new(Vec::new()));
        let captured = capture.clone();
        let proxy_task = tokio::spawn(async move {
            let mut jobs = tokio::task::JoinSet::new();
            loop {
                tokio::select! {
                    accepted = proxy.accept() => {
                        let (client, _) = accepted.unwrap();
                        let node = TcpStream::connect(destination).await.unwrap();
                        let capture = captured.clone();
                        jobs.spawn(async move {
                            let (a, b) = client.into_split(); let (c, d) = node.into_split();
                            async fn copy(mut from: tokio::net::tcp::OwnedReadHalf, mut to: tokio::net::tcp::OwnedWriteHalf, capture: Arc<Mutex<Vec<u8>>>) {
                                let mut bytes = [0; 8192];
                                while let Ok(n) = from.read(&mut bytes).await {
                                    if n == 0 { break; }
                                    capture.lock().unwrap().extend_from_slice(&bytes[..n]);
                                    if to.write_all(&bytes[..n]).await.is_err() { break; }
                                }
                            }
                            tokio::join!(copy(a, d, capture.clone()), copy(c, b, capture));
                        });
                    }
                    _ = jobs.join_next(), if !jobs.is_empty() => {}
                }
            }
        });
        Self {
            _root: root,
            handler,
            port,
            capture,
            tasks: vec![server, proxy_task],
        }
    }
    async fn socket(&self, path: &str) -> Socket {
        tokio_tungstenite::connect_async(format!("ws://127.0.0.1:{}{path}", self.port))
            .await
            .unwrap()
            .0
    }
    async fn pair(&self) -> ClientPaired {
        let snapshot = self
            .handler
            .node()
            .lock()
            .unwrap()
            .pair(Duration::from_secs(120))
            .unwrap();
        let code = Code::parse(&snapshot.words.join(" ")).unwrap();
        let mut socket = self.socket("/ws/pair").await;
        let hello = next(&mut socket).await.into_text().unwrap();
        let (mut client, start) = Client::start(&hello, code, "browser").unwrap();
        socket.send(Message::Text(start.into())).await.unwrap();
        let reply = next(&mut socket).await.into_text().unwrap();
        let confirm = client.reply(&reply).unwrap();
        socket.send(Message::Text(confirm.into())).await.unwrap();
        let sealed = next(&mut socket).await.into_data();
        let (paired, finish) = client
            .finish(
                &sealed,
                Some("Synthetic browser"),
                None,
                jiff::Timestamp::now(),
            )
            .unwrap();
        socket.send(Message::Binary(finish.into())).await.unwrap();
        assert!(
            matches!(next(&mut socket).await, Message::Close(Some(c)) if c.code == 1000.into())
        );
        paired
    }
    async fn connect(&self, paired: &ClientPaired) -> Connection {
        let mut socket = self.socket("/ws").await;
        let (pending, hello) = start(paired);
        socket.send(Message::Text(hello.into())).await.unwrap();
        let reply = next(&mut socket).await.into_text().unwrap();
        let (crypto, confirm) = pending.finish(&reply, jiff::Timestamp::now()).unwrap();
        socket.send(Message::Binary(confirm.into())).await.unwrap();
        Connection {
            socket,
            crypto,
            id: 0,
        }
    }
}
fn start(paired: &ClientPaired) -> (ClientHandshake, String) {
    ClientHandshake::start(
        &paired.device,
        x25519_dalek::StaticSecret::from(paired.x25519.to_bytes()),
        NodePins {
            id: paired.node_id.clone(),
            cluster: paired.cluster_pub,
            x25519: paired.node_x25519,
        },
    )
    .unwrap()
}
async fn next(socket: &mut Socket) -> Message {
    tokio::time::timeout(Duration::from_secs(10), socket.next())
        .await
        .unwrap()
        .unwrap()
        .unwrap()
}
struct Connection {
    socket: Socket,
    crypto: privatium_core::session::Session,
    id: u64,
}
impl Connection {
    async fn navigation(&mut self, path: &str, body: &str) -> String {
        self.id += 1;
        let mut frame = Frame::new(self.id, Kind::Req);
        frame.method = Some("POST".into());
        frame.path = Some(path.into());
        frame.headers = Some(BTreeMap::from([(
            "content-type".into(),
            "application/json".into(),
        )]));
        frame.navigation = Some(true);
        frame.payload = body.to_owned().into();
        self.send(frame).await;
        let head = self.frame().await;
        assert_eq!(head.status, Some(200));
        assert_eq!(self.frame().await.kind, Kind::End);
        head.handoff.unwrap()
    }
    async fn attach(&mut self, reference: &str, kind: Kind) -> (u16, String) {
        self.id += 1;
        let mut frame = Frame::new(self.id, kind);
        frame.handoff = Some(reference.into());
        self.send(frame).await;
        self.response(self.id).await
    }
    async fn send(&mut self, frame: Frame) {
        let ciphertext = self.crypto.send.seal(&frame.encode().unwrap()).unwrap();
        self.socket
            .send(Message::Binary(ciphertext.into()))
            .await
            .unwrap();
    }
    async fn request(&mut self, method: &str, path: &str, body: &str) -> u64 {
        self.id += 1;
        let mut frame = Frame::new(self.id, Kind::Req);
        frame.method = Some(method.to_owned());
        frame.path = Some(path.to_owned());
        frame.headers = Some(BTreeMap::from([(
            "content-type".into(),
            "application/json".into(),
        )]));
        frame.payload = body.to_owned().into();
        self.send(frame).await;
        self.id
    }
    async fn frame(&mut self) -> Frame {
        let message = next(&mut self.socket).await;
        assert!(
            matches!(message, Message::Binary(_)),
            "expected encrypted message"
        );
        Frame::decode(&self.crypto.receive.open(&message.into_data()).unwrap()).unwrap()
    }
    async fn response(&mut self, id: u64) -> (u16, String) {
        let mut status = 0;
        let mut bytes = Vec::new();
        loop {
            let frame = self.frame().await;
            assert_eq!(frame.id, id);
            match frame.kind {
                Kind::Res => status = frame.status.unwrap(),
                Kind::Chunk => bytes.extend_from_slice(&frame.payload),
                Kind::End => break,
                _ => panic!("invalid response kind"),
            }
        }
        (status, String::from_utf8(bytes).unwrap())
    }
}

#[tokio::test]
async fn test_spec_8_3_1_handoff_survives_disconnect_without_repeating_a_write() {
    let f = Fixture::new().await;
    let paired = f.pair().await;
    let mut c = f.connect(&paired).await;
    let reference = c.navigation("/a/hello/api/events", r#"{"events":[{"op":"put","tbl":"notes","id":"01J00000000000000000000001","d":{"text":"Synthetic retained response"}}]}"#).await;
    c.socket.close(None).await.unwrap();
    let mut c = f.connect(&paired).await;
    let (status, body) = c.attach(&reference, Kind::Resume).await;
    assert_eq!(status, 200);
    assert!(body.contains("\"appended\":1"));
    assert_eq!(c.attach(&reference, Kind::Resume).await.0, 409);
    let id = c.request("GET", "/a/hello/api/events?tbl=notes", "").await;
    let (_, events) = c.response(id).await;
    assert_eq!(events.lines().count(), 1);
}

#[tokio::test]
async fn test_spec_8_3_1_wrong_device_cannot_consume_or_release_a_response() {
    let f = Fixture::new().await;
    let owner = f.pair().await;
    let other = f.pair().await;
    let mut a = f.connect(&owner).await;
    let reference = a
        .navigation("/a/hello/api/events", r#"{"events":[]}"#)
        .await;
    let mut b = f.connect(&other).await;
    assert_eq!(b.attach(&reference, Kind::Resume).await.0, 409);
    assert_eq!(b.attach(&reference, Kind::Release).await.0, 409);
    assert_eq!(a.attach(&reference, Kind::Resume).await.0, 200);
}

#[tokio::test]
async fn test_spec_8_3_1_capacity_refuses_before_dispatch_and_release_frees_it() {
    let f = Fixture::new().await;
    let paired = f.pair().await;
    let mut c = f.connect(&paired).await;
    let mut references = Vec::new();
    for _ in 0..4 {
        references.push(
            c.navigation("/a/hello/api/events", r#"{"events":[]}"#)
                .await,
        );
    }
    c.id += 1;
    let mut frame = Frame::new(c.id, Kind::Req);
    frame.method = Some("POST".into());
    frame.path = Some("/a/hello/api/events".into());
    frame.headers = Some(BTreeMap::from([(
        "content-type".into(),
        "application/json".into(),
    )]));
    frame.navigation = Some(true);
    frame.payload =
        r#"{"events":[{"op":"put","tbl":"notes","id":"01J00000000000000000000002","d":{}}]}"#
            .into();
    c.send(frame).await;
    assert_eq!(c.response(c.id).await.0, 429);
    let id = c.request("GET", "/a/hello/api/events?tbl=notes", "").await;
    assert!(c.response(id).await.1.trim().is_empty());
    assert_eq!(c.attach(&references[0], Kind::Release).await.0, 204);
    assert_eq!(c.attach(&references[0], Kind::Resume).await.0, 409);
    c.navigation("/a/hello/api/events", r#"{"events":[]}"#)
        .await;
}

#[tokio::test]
async fn test_spec_8_2_lan_socket_carries_no_plaintext_app_data() {
    let f = Fixture::new().await;
    let paired = f.pair().await;
    let mut c = f.connect(&paired).await;
    for path in ["/a/hello/", "/a/hello/api/schema"] {
        let id = c.request("GET", path, "").await;
        let (status, body) = c.response(id).await;
        assert_eq!(status, 200);
        let plaintext = if path.ends_with("schema") {
            body.clone()
        } else {
            body.split("<h1>")
                .nth(1)
                .unwrap()
                .split("</h1>")
                .next()
                .unwrap()
                .to_owned()
        };
        assert!(!plaintext.is_empty());
        assert!(
            !f.capture
                .lock()
                .unwrap()
                .windows(plaintext.len())
                .any(|w| w == plaintext.as_bytes())
        );
    }
}

#[tokio::test]
async fn test_channel_requests_reach_every_route_of_handle() {
    let f = Fixture::new().await;
    let paired = f.pair().await;
    let mut c = f.connect(&paired).await;
    for (path, expected) in [
        ("/", 200),
        ("/settings", 200),
        ("/settings/apps", 200),
        ("/settings/devices", 200),
        ("/api/v1/health", 200),
        ("/api/v1/manifest", 200),
        ("/skills/privatium-overview.md", 200),
        ("/static/shell.css", 200),
        ("/a/hello/", 200),
        ("/a/hello/api/events", 200),
        ("/a/hello/api/schema", 200),
        ("/a/sketch/", 200),
        ("/a/sketch", 308),
        ("/missing", 404),
    ] {
        let id = c.request("GET", path, "").await;
        assert_eq!(c.response(id).await.0, expected, "{path}");
    }
    let id = c.request("POST", "/settings/apps/hello/seed", "").await;
    assert_eq!(c.response(id).await.0, 403);
}

#[tokio::test]
async fn test_channel_requests_carry_the_session_device() {
    let f = Fixture::new().await;
    let paired = f.pair().await;
    let mut c = f.connect(&paired).await;
    let id = c.request("GET", "/a/hello/api/node", "").await;
    let (status, body) = c.response(id).await;
    assert_eq!(status, 200);
    let node: serde_json::Value = serde_json::from_str(&body).unwrap();
    assert_eq!(node["dev"], paired.device);
}

#[tokio::test]
async fn test_channel_streams_a_response_body_frame_by_frame() {
    let f = Fixture::new().await;
    let paired = f.pair().await;
    let mut c = f.connect(&paired).await;
    let stream = c.request("GET", "/a/hello/api/stream", "").await;
    assert_eq!(c.frame().await.status, Some(200));
    let write = c.request("POST", "/a/hello/api/events", r#"{"events":[{"op":"put","tbl":"notes","id":"01J00000000000000000000000","d":{"text":"Synthetic channel value"}}]}"#).await;
    let mut appended = false;
    let mut ended = false;
    while !appended || !ended {
        let frame = c.frame().await;
        if frame.id == stream {
            assert_ne!(frame.kind, Kind::End);
            appended |= String::from_utf8_lossy(&frame.payload).contains("Synthetic channel value");
        } else {
            assert_eq!(frame.id, write);
            if frame.kind == Kind::Res {
                assert_eq!(frame.status, Some(200));
            }
            ended |= frame.kind == Kind::End;
        }
    }
    c.send(Frame::new(stream, Kind::Cancel)).await;
    let id = c.request("GET", "/a/hello/api/node", "").await;
    assert_eq!(c.response(id).await.0, 200);
}

#[tokio::test]
async fn test_spec_8_1_a_revoked_device_is_refused_at_the_handshake() {
    let f = Fixture::new().await;
    let paired = f.pair().await;
    {
        let mut node = f.handler.node().lock().unwrap();
        node.sys_log_mut()
            .put(
                "sys_device",
                &paired.device,
                &serde_json::json!({"revoked_at":"2026-09-05T12:00:00.000Z"}),
            )
            .unwrap();
        node.refresh().unwrap();
    }
    let mut socket = f.socket("/ws").await;
    socket
        .send(Message::Text(start(&paired).1.into()))
        .await
        .unwrap();
    assert!(matches!(next(&mut socket).await, Message::Close(Some(c)) if c.code == 4403.into()));
}

/// `spec/protocol.md §7.4.2`, `§7.5` — a peer that opens `/ws/pair` and sends nothing is
/// closed when the handshake bound passes, with no attempt counted and no audit row
/// written, and the window is untouched for the next device; a peer that opens `/ws`
/// and sends nothing is closed the same way. The bounds are shortened for the test;
/// nothing here reads a clock.
///
/// The pairing bound is two seconds and not the handshake's fraction of one, because the
/// same bound governs the real pairing this test performs further down: a whole PAKE over
/// a live socket has to finish inside it. At three hundred milliseconds a loaded runner
/// did not make it, the server closed the socket at the bound, and `finish` read the
/// close frame as a malformed sealed message — a `Format` error a long way from its
/// cause. The handler is behind an `Arc` by the time the fixture serves, so the bound
/// cannot be relaxed part-way through; widening it is what this fixture allows.
#[tokio::test]
async fn test_spec_7_4_a_silent_pairing_peer_is_closed_without_an_attempt() {
    let f = Fixture::build(tempfile::tempdir().unwrap(), None, |handler| {
        let timeouts = handler.channel_timeouts_mut();
        timeouts.pairing = Duration::from_secs(2);
        timeouts.handshake = Duration::from_millis(300);
    })
    .await;
    let opened = f
        .handler
        .node()
        .lock()
        .unwrap()
        .pair(Duration::from_secs(120))
        .unwrap();
    let mut silent = f.socket("/ws/pair").await;
    let hello: serde_json::Value =
        serde_json::from_str(&next(&mut silent).await.into_text().unwrap()).unwrap();
    assert_eq!(hello["open"], true);
    assert!(
        matches!(next(&mut silent).await, Message::Close(Some(c)) if c.code == 4400.into() && c.reason.contains("could not finish")),
        "closed at the bound"
    );
    {
        let mut node = f.handler.node().lock().unwrap();
        node.refresh().unwrap();
        let window = node
            .refresh_pairing(jiff::Timestamp::now())
            .unwrap()
            .unwrap();
        assert_eq!(window.id, opened.id);
        assert_eq!(window.attempts, 0, "nothing was counted");
        for kind in ["pair.attempt", "pair.failed"] {
            let count: i64 = node
                .store()
                .conn()
                .query_row(
                    "SELECT count(*) FROM sys_audit WHERE kind = ?",
                    [kind],
                    |row| row.get(0),
                )
                .unwrap();
            assert_eq!(count, 0, "{kind}");
        }
    }
    // The window is still open, and the next device pairs through it.
    f.pair().await;
    // A silent peer on the channel is closed the same way, with 4403.
    let mut silent = f.socket("/ws").await;
    assert!(matches!(next(&mut silent).await, Message::Close(Some(c)) if c.code == 4403.into()));
}

/// `spec/protocol.md §8.3` — the node's own `sys_device` row, which carries keys for
/// other purposes, cannot open a channel: a hello naming this node's ID is refused with
/// 4403 before any hello is answered, exactly as an unknown device is.
#[tokio::test]
async fn test_spec_8_3_the_nodes_own_row_cannot_open_a_channel() {
    let f = Fixture::new().await;
    let paired = f.pair().await;
    let node_id = f.handler.node().lock().unwrap().id().as_str().to_owned();
    let mut socket = f.socket("/ws").await;
    let (_, hello) = ClientHandshake::start(
        &node_id,
        x25519_dalek::StaticSecret::from(paired.x25519.to_bytes()),
        NodePins {
            id: paired.node_id.clone(),
            cluster: paired.cluster_pub,
            x25519: paired.node_x25519,
        },
    )
    .unwrap();
    socket.send(Message::Text(hello.into())).await.unwrap();
    assert!(
        matches!(next(&mut socket).await, Message::Close(Some(c)) if c.code == 4403.into()),
        "refused without a node hello"
    );
    // The paired device itself is unaffected.
    let mut c = f.connect(&paired).await;
    let id = c.request("GET", "/api/v1/health", "").await;
    assert_eq!(c.response(id).await.0, 200);
}

#[tokio::test]
async fn test_spec_8_4_websocket_refuses_a_cross_origin_upgrade() {
    use tokio_tungstenite::tungstenite::client::IntoClientRequest as _;
    let f = Fixture::new().await;
    for path in ["/ws", "/ws/pair"] {
        let mut request = format!("ws://127.0.0.1:{}{path}", f.port)
            .into_client_request()
            .unwrap();
        request
            .headers_mut()
            .insert("origin", "http://example.invalid".parse().unwrap());
        assert!(
            matches!(tokio_tungstenite::connect_async(request).await, Err(tokio_tungstenite::tungstenite::Error::Http(response)) if response.status() == 403)
        );
    }
}

#[tokio::test]
async fn test_channel_refuses_a_request_chunk_naming_the_capability() {
    let f = Fixture::new().await;
    let paired = f.pair().await;
    let mut c = f.connect(&paired).await;
    c.send(Frame::new(1, Kind::Chunk)).await;
    assert!(
        matches!(next(&mut c.socket).await, Message::Close(Some(c)) if c.code == 4400.into() && c.reason.contains("streamed request bodies"))
    );
}

#[tokio::test]
async fn test_spec_8_3_reused_ids_and_invalid_paths_close_the_channel() {
    let f = Fixture::new().await;
    let paired = f.pair().await;
    for path in [
        "https://example.invalid/",
        "//example.invalid/",
        "/a/hello/#fragment",
    ] {
        let mut c = f.connect(&paired).await;
        c.request("GET", path, "").await;
        assert!(
            matches!(next(&mut c.socket).await, Message::Close(Some(c)) if c.code == 4400.into())
        );
    }
    let mut c = f.connect(&paired).await;
    let id = c.request("GET", "/api/v1/health", "").await;
    c.response(id).await;
    c.id = 0;
    c.request("GET", "/api/v1/health", "").await;
    assert!(matches!(next(&mut c.socket).await, Message::Close(Some(c)) if c.code == 4400.into()));
}

// ---------------------------------------------------------------------------------------
// The owner's standing, the session's device, and the refusal a paired client makes
// ---------------------------------------------------------------------------------------

/// A form POST through `handle` in-process — the owner's own standing (`§8.4`).
fn owner_form(f: &Fixture, path: &str, fields: &str) -> Request {
    let token = f.handler.csrf().token(path);
    let body = if fields.is_empty() {
        format!("_csrf={token}")
    } else {
        format!("{fields}&_csrf={token}")
    };
    axum::http::Request::builder()
        .method("POST")
        .uri(path)
        .header("content-type", "application/x-www-form-urlencoded")
        .body(axum::body::Body::from(body))
        .unwrap()
}

/// `spec/protocol.md §9.2` — `/api/v1/pair` and the settings acts behind it are the
/// owner's alone: a paired session is refused on every one of them, whatever its
/// device, while the devices page it may read carries none of the forms.
#[tokio::test]
async fn test_spec_9_2_pair_route_refuses_a_session() {
    let f = Fixture::new().await;
    let paired = f.pair().await;
    let mut c = f.connect(&paired).await;
    for (method, path) in [
        ("POST", "/api/v1/pair"),
        ("GET", "/api/v1/pair"),
        ("POST", "/settings/devices/pair"),
        ("GET", "/settings/devices/pairing"),
        ("POST", "/settings/devices/pairing/close"),
        ("POST", "/settings/name"),
        ("POST", "/settings/devices/b3nn8t2q/revoke"),
        ("POST", "/settings/devices/b3nn8t2q/label"),
        ("POST", "/api/v1/join"),
        ("POST", "/settings/join"),
        ("POST", "/settings/devices/admit"),
    ] {
        let id = c.request(method, path, "{\"ttl\":120}").await;
        let (status, body) = c.response(id).await;
        assert_eq!(status, 403, "{method} {path}: {body}");
        assert!(body.contains("owner"), "{body}");
    }
    assert!(
        !f.handler
            .node()
            .lock()
            .unwrap()
            .pairing_open(jiff::Timestamp::now()),
        "nothing opened"
    );
    let id = c.request("GET", "/settings/devices", "").await;
    let (status, page) = c.response(id).await;
    assert_eq!(status, 200);
    assert!(page.contains(&paired.device), "{page}");
    assert!(
        !page.contains("action=\"/settings/devices/pair\""),
        "{page}"
    );
    assert!(!page.contains("/revoke\""), "{page}");
    assert!(page.contains("only the owner"), "{page}");
}

/// `spec/lua-api.md §3.1`, `§3.4`, `spec/data-api.md §4` — through a channel, `req.device`,
/// `pv.device()` and `/api/node`'s `dev` are the session's device, not this node's; and
/// `peers` counts paired nodes alone: a paired browser is not one, an active `node` row
/// is, a revoked one is not.
#[tokio::test]
async fn test_spec_lua_3_4_device_and_peers_come_from_the_session() {
    let apps = tempfile::tempdir().unwrap();
    let dir = apps.path().join("facts");
    std::fs::create_dir_all(&dir).unwrap();
    std::fs::write(
        dir.join("app.toml"),
        "[app]\nslug = \"facts\"\ntitle = \"Facts\"\nversion = \"1.0.0\"\napi = 1\ntier = \"lua\"\n",
    )
    .unwrap();
    std::fs::write(
        dir.join("app.lua"),
        "local pv = require 'privatium'\n\
         pv.get('/', function(req)\n\
           return pv.json({ device = pv.device(), req = req.device, peers = pv.node().peers })\n\
         end)\n",
    )
    .unwrap();
    let f = Fixture::with_apps(tempfile::tempdir().unwrap(), Some(apps.path())).await;
    let paired = f.pair().await;
    let node_id = f.handler.node().lock().unwrap().id().as_str().to_owned();
    let mut c = f.connect(&paired).await;
    let facts = |body: String| -> serde_json::Value { serde_json::from_str(&body).unwrap() };
    let id = c.request("GET", "/a/facts/", "").await;
    let (status, body) = c.response(id).await;
    assert_eq!(status, 200, "{body}");
    let first = facts(body);
    assert_eq!(first["device"], paired.device);
    assert_eq!(first["req"], paired.device);
    assert_eq!(first["peers"], 0, "a paired browser is not a peer");
    let id = c.request("GET", "/a/hello/api/node", "").await;
    let node = facts(c.response(id).await.1);
    assert_eq!(node["dev"], paired.device);
    assert_eq!(node["id"], node_id);
    assert_eq!(node["peers"], 0);

    {
        let mut node = f.handler.node().lock().unwrap();
        let log = node.sys_log_mut();
        log.put(
            "sys_device",
            "n0d3aaaa",
            &serde_json::json!({"kind":"node","replica":true}),
        )
        .unwrap();
        log.put(
            "sys_device",
            "n0d3bbbb",
            &serde_json::json!({"kind":"node","replica":true,"revoked_at":"2026-09-06T00:00:00.000Z"}),
        )
        .unwrap();
        node.refresh().unwrap();
        assert_eq!(node.paired_node_count().unwrap(), 1);
    }
    let id = c.request("GET", "/a/facts/", "").await;
    assert_eq!(facts(c.response(id).await.1)["peers"], 1);
    let id = c.request("GET", "/a/hello/api/node", "").await;
    assert_eq!(facts(c.response(id).await.1)["peers"], 1);
}

/// `spec/data-dictionary.md §3.2` — revoking a device denies it at once: its open channel
/// is closed with 4403 by the revocation itself, not by its next frame, and its next
/// handshake is refused.
#[tokio::test]
async fn test_spec_3_2_revoking_a_device_closes_its_open_channel_at_once() {
    let f = Fixture::new().await;
    let paired = f.pair().await;
    let other = f.pair().await;
    let mut victim = f.connect(&paired).await;
    let mut bystander = f.connect(&other).await;
    let path = format!("/settings/devices/{}/revoke", paired.device);
    let response = f.handler.handle(owner_form(&f, &path, "")).await;
    assert_eq!(response.status(), 303);
    assert!(
        matches!(next(&mut victim.socket).await, Message::Close(Some(c)) if c.code == 4403.into()),
        "the revoked device's channel closes without a frame from it"
    );
    let id = bystander.request("GET", "/api/v1/health", "").await;
    assert_eq!(
        bystander.response(id).await.0,
        200,
        "another device is untouched"
    );
    let mut socket = f.socket("/ws").await;
    socket
        .send(Message::Text(start(&paired).1.into()))
        .await
        .unwrap();
    assert!(matches!(next(&mut socket).await, Message::Close(Some(c)) if c.code == 4403.into()));
}

/// `spec/protocol.md §8.1`, `§2.3.2` — a node whose `identity/` was replaced (a new node
/// key and a new cluster over the same `data/`) is refused by a client that paired with
/// the old one: the certificate fails the pinned cluster key before any confirm is
/// sent, with no way past it but pairing again.
#[tokio::test]
async fn test_spec_8_1_a_reinitialized_node_is_refused_by_a_paired_client() {
    let f = Fixture::new().await;
    let paired = f.pair().await;
    let old_id = f.handler.node().lock().unwrap().id().as_str().to_owned();
    let root = tempfile::tempdir().unwrap();
    copy_dir(&f._root.path().join("data"), &root.path().join("data"));
    let g = Fixture::with_apps(root, None).await;
    let new_id = g.handler.node().lock().unwrap().id().as_str().to_owned();
    assert_ne!(old_id, new_id, "a fresh identity/ is a new node");
    let mut socket = g.socket("/ws").await;
    let (pending, hello) = start(&paired);
    socket.send(Message::Text(hello.into())).await.unwrap();
    let reply = next(&mut socket).await.into_text().unwrap();
    let outcome = pending.finish(&reply, jiff::Timestamp::now());
    assert!(
        matches!(
            outcome,
            Err(privatium_core::session::SessionError::PinnedKey)
        ),
        "the certificate is not the pinned cluster's"
    );
}

fn copy_dir(from: &Path, to: &Path) {
    std::fs::create_dir_all(to).unwrap();
    for entry in std::fs::read_dir(from).unwrap() {
        let entry = entry.unwrap();
        let target = to.join(entry.file_name());
        if entry.file_type().unwrap().is_dir() {
            copy_dir(&entry.path(), &target);
        } else {
            std::fs::copy(entry.path(), target).unwrap();
        }
    }
}

/// `spec/protocol.md §2.3.1` — an expired node refuses every channel: `/ws` closes with
/// 4403 before the hello is read, `/ws/pair` answers a device with `open: false` since
/// no device window can open, and the manifest says `pair: false`; the owner's own
/// requests are answered as before.
#[tokio::test]
async fn test_spec_2_3_1_an_expired_node_refuses_every_channel_over_a_socket() {
    let root = tempfile::tempdir().unwrap();
    {
        let node = Node::open(root.path()).unwrap();
        let cert = node
            .identity()
            .sign_certificate(
                &node.identity().verifying_key(),
                jiff::Timestamp::now() - jiff::SignedDuration::from_secs(181 * 86_400),
            )
            .unwrap();
        let path = node.paths().identity_dir().join("node.cert");
        drop(node);
        std::fs::write(path, serde_json::to_vec(&cert).unwrap()).unwrap();
    }
    let f = Fixture::with_apps(root, None).await;
    assert_eq!(
        f.handler
            .node()
            .lock()
            .unwrap()
            .standing(jiff::Timestamp::now())
            .unwrap(),
        privatium_core::Standing::Expired
    );
    let mut ws = f.socket("/ws").await;
    assert!(
        matches!(next(&mut ws).await, Message::Close(Some(c)) if c.code == 4403.into() && c.reason.contains("re-admitted")),
        "refused before the hello"
    );
    assert!(
        f.handler
            .node()
            .lock()
            .unwrap()
            .pair(Duration::from_secs(120))
            .is_err(),
        "no window for devices"
    );
    let mut pair = f.socket("/ws/pair").await;
    let hello: serde_json::Value =
        serde_json::from_str(&next(&mut pair).await.into_text().unwrap()).unwrap();
    assert_eq!(hello["open"], false);
    let manifest = f
        .handler
        .handle(owner_request("GET", "/api/v1/manifest", ""))
        .await;
    let body = axum::body::to_bytes(manifest.into_body(), usize::MAX)
        .await
        .unwrap();
    let manifest: serde_json::Value = serde_json::from_slice(&body).unwrap();
    assert_eq!(manifest["pair"], false);
    let page = f
        .handler
        .handle(owner_request("GET", "/settings", ""))
        .await;
    assert_eq!(page.status(), 200);
    let body = axum::body::to_bytes(page.into_body(), usize::MAX)
        .await
        .unwrap();
    assert!(String::from_utf8_lossy(&body).contains("expired"));
}

/// An in-process request with the owner's standing: no peer, no session.
fn owner_request(method: &str, path: &str, body: &str) -> Request {
    axum::http::Request::builder()
        .method(method)
        .uri(path)
        .header("host", "127.0.0.1:8420")
        .header("accept", "text/html, application/json")
        .body(axum::body::Body::from(body.to_owned()))
        .unwrap()
}
