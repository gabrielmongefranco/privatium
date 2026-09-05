// Project:  Privatium™  |  File: crates/privatium/tests/channel.rs
// Authors:  Gabriel Mongefranco (@gabrielmongefranco)
// Created:  2026-09-05  |  Modified: 2026-09-05
// Summary:  Actual WebSocket pairing, authenticated routing, streaming and refusal (§8).

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
        let root = tempfile::tempdir().unwrap();
        let mut node = Node::open(root.path()).unwrap();
        let report = node
            .load_apps(&[AppRoot::bundled(
                Path::new(env!("CARGO_MANIFEST_DIR")).join("../../apps"),
            )])
            .unwrap();
        let handler = Arc::new(Handler::new(node, report));
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
async fn test_channel_refuses_a_request_chunk_naming_phase_3() {
    let f = Fixture::new().await;
    let paired = f.pair().await;
    let mut c = f.connect(&paired).await;
    c.send(Frame::new(1, Kind::Chunk)).await;
    assert!(
        matches!(next(&mut c.socket).await, Message::Close(Some(c)) if c.code == 4400.into() && c.reason.contains("Phase 3"))
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
