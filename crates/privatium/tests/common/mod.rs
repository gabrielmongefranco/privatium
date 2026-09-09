// Project:  Privatium™  |  File: crates/privatium/tests/common/mod.rs
// Authors:  Gabriel Mongefranco (@gabrielmongefranco)
// Created:  2026-09-07  |  Modified: 2026-09-07
// Summary:  In-process nodes behind real TCP sockets and TEST-NET peer addresses for
//           encrypted synchronization tests (spec/protocol.md §8.3 and §10). The nodes
//           run in this process because a test has to start a pass and then look inside
//           both of them, which a node running as a child binary cannot be asked to do.
//           See main README.md for full license information.

#![allow(dead_code)]

use std::collections::BTreeMap;
use std::convert::Infallible;
use std::sync::Arc;
use std::time::Duration;

use axum::ServiceExt as _;
use futures_util::{SinkExt as _, StreamExt as _};
use privatium_core::pair::{
    Code,
    handshake::{Client, ClientPaired},
};
use privatium_core::session::handshake::{ClientHandshake, NodePins};
use privatium_core::wire::channel::{Frame, Kind};
use privatium_core::{Handler, Node, Peer, Request};
use tokio::net::{TcpListener, TcpStream};
use tokio_tungstenite::{MaybeTlsStream, WebSocketStream, tungstenite::Message};

/// Slug of the mounted synthetic app.
pub const APP: &str = "sync-test";
/// One row per synthetic item.
pub const DDL: &str = "CREATE TABLE item (id TEXT PRIMARY KEY, value TEXT);";
/// Real client WebSocket transport.
pub type Socket = WebSocketStream<MaybeTlsStream<TcpStream>>;

/// Move `api.max_body` (`spec/data-dictionary.md §3.6`) on one node and rebuild `_sys`.
pub fn set_body_bound(fixture: &Fixture, bytes: u64) {
    let mut node = fixture.handler.node().lock().unwrap();
    let at = privatium_core::log::now();
    node.sys_log_mut()
        .put(
            privatium_core::sys::SETTING,
            "api.max_body",
            &serde_json::json!({ "value": bytes.to_string(), "updated_at": at }),
        )
        .unwrap();
    node.refresh().unwrap();
}

/// Isolated data root, core handler, and TCP listener; stops the listener on drop.
pub struct Fixture {
    /// Synthetic root retained across a restart.
    pub root: Arc<tempfile::TempDir>,
    /// The adapter's existing shared core handler.
    pub handler: Arc<Handler>,
    /// Ephemeral loopback port.
    pub port: u16,
    /// Listener task, owned by this fixture.
    pub task: tokio::task::JoinHandle<()>,
}

impl Drop for Fixture {
    fn drop(&mut self) {
        self.task.abort();
    }
}

impl Fixture {
    /// Create a synthetic mounted app and start its listener; panics on fixture failure.
    pub async fn new() -> Self {
        let root = Arc::new(tempfile::tempdir().unwrap());
        Self::open(root).await
    }

    /// Reopen a synthetic root and serve it; panics on storage or bind failure.
    pub async fn open(root: Arc<tempfile::TempDir>) -> Self {
        let mut node = Node::open(root.path()).unwrap();
        let folder = node.paths().apps_dir().join(APP);
        std::fs::create_dir_all(folder.join("web")).unwrap();
        std::fs::write(
            folder.join("web/index.html"),
            "<!doctype html><html lang=\"en\"><head><meta charset=\"utf-8\">\
             <title>Sync Test</title></head>\
             <body><main><h1>Sync Test</h1></main></body></html>",
        )
        .unwrap();
        std::fs::write(
            folder.join("app.toml"),
            format!(
                "[app]\nslug = \"{APP}\"\ntitle = \"Sync Test\"\n\
                 version = \"1.0.0\"\napi = 1\ntier = \"web\"\n"
            ),
        )
        .unwrap();
        std::fs::write(folder.join("schema.sql"), DDL).unwrap();
        let report = node
            .load_apps(&[privatium_core::AppRoot::local(node.paths().apps_dir())])
            .unwrap();
        assert!(report.failed.is_empty(), "{report:?}");
        let handler = Arc::new(Handler::new(node, report));
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let port = listener.local_addr().unwrap().port();
        let h = handler.clone();
        let task = tokio::spawn(async move {
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
        Self {
            root,
            handler,
            port,
            task,
        }
    }

    /// Stop this listener and release its node, retaining the synthetic root.
    pub async fn stop(mut self) -> Arc<tempfile::TempDir> {
        self.task.abort();
        let _ = (&mut self.task).await;
        self.root.clone()
    }

    /// Read this node's public identity.
    pub fn id(&self) -> String {
        self.handler.node().lock().unwrap().id().to_string()
    }
    /// The origin to dial for this fixture.
    pub fn origin(&self) -> String {
        format!("http://127.0.0.1:{}", self.port)
    }

    /// Admit this node through another fixture's real pairing socket.
    pub async fn join(&self, other: &Self) {
        self.join_at(other, other.port).await;
    }

    /// Run admission through the supplied proxy port; panics on refusal.
    pub async fn join_at(&self, other: &Self, port: u16) {
        let code = other
            .handler
            .node()
            .lock()
            .unwrap()
            .pair_node(Duration::from_secs(120))
            .unwrap()
            .words
            .join(" ");
        let handler = self.handler.clone();
        let url = format!("http://127.0.0.1:{port}");
        tokio::task::spawn_blocking(move || {
            handler
                .node()
                .lock()
                .unwrap()
                .join(&url, Code::parse(&code).unwrap())
        })
        .await
        .unwrap()
        .unwrap();
    }

    /// Run a blocking durable pass with a thirty-second test deadline.
    pub async fn pass(&self) -> privatium_core::sync::SyncReport {
        let handler = self.handler.clone();
        tokio::time::timeout(
            Duration::from_secs(30),
            tokio::task::spawn_blocking(move || handler.node().lock().unwrap().sync_now()),
        )
        .await
        .unwrap()
        .unwrap()
        .unwrap()
    }

    /// Append one synthetic item through the node API.
    pub fn append(&self, id: &str, value: &str) {
        self.handler
            .node()
            .lock()
            .unwrap()
            .append(
                APP,
                privatium_core::Event::put("item", id, serde_json::json!({"value":value})),
            )
            .unwrap();
    }

    /// Read the materialized synthetic rows in ID order.
    pub fn rows(&self) -> Vec<(String, String)> {
        let node = self.handler.node().lock().unwrap();
        node.app(APP)
            .unwrap()
            .store()
            .conn()
            .prepare("SELECT id,value FROM item ORDER BY id")
            .unwrap()
            .query_map([], |row| Ok((row.get(0)?, row.get(1)?)))
            .unwrap()
            .collect::<Result<_, _>>()
            .unwrap()
    }

    /// Read an origin's app log for exact byte comparison.
    pub fn bytes(&self, dev: &str) -> Vec<u8> {
        std::fs::read(
            self.root
                .path()
                .join("data")
                .join(APP)
                .join("log")
                .join(format!("{dev}.jsonl")),
        )
        .unwrap()
    }

    /// Open an encrypted connection using another fixture's node identity.
    pub async fn node_connection(&self, peer: &Self) -> Connection {
        let (id, secret, cluster, pins) = {
            let node = peer.handler.node().lock().unwrap();
            let target = self.handler.node().lock().unwrap();
            (
                node.id().to_string(),
                node.identity().x25519_static(),
                node.identity().cluster_public(),
                NodePins {
                    id: target.id().to_string(),
                    cluster: node.identity().cluster_public(),
                    x25519: x25519_dalek::PublicKey::from(&target.identity().x25519_static()),
                },
            )
        };
        let _ = cluster;
        let (pending, hello) = ClientHandshake::start(&id, secret, pins).unwrap();
        self.connect(pending, hello).await
    }

    /// Assert lookup refuses this unregistered or revoked identity before a hello.
    pub async fn refused(&self, id: &str) {
        use base64::{Engine as _, engine::general_purpose::STANDARD};
        let mut socket = socket(self.port, "/ws").await;
        let hello = serde_json::json!({"v":1,"dev":id,"e":STANDARD.encode([1u8;32])});
        socket
            .send(Message::Text(hello.to_string().into()))
            .await
            .unwrap();
        let Message::Close(Some(close)) = next(&mut socket).await else {
            panic!("unregistered identity was not refused");
        };
        assert_eq!(u16::from(close.code), 4403);
    }

    /// Pair a synthetic browser through the real PAKE socket.
    pub async fn pair_browser(&self) -> ClientPaired {
        let snapshot = self
            .handler
            .node()
            .lock()
            .unwrap()
            .pair(Duration::from_secs(120))
            .unwrap();
        let mut socket = socket(self.port, "/ws/pair").await;
        let hello = next(&mut socket).await.into_text().unwrap();
        let (mut client, start) = Client::start(
            &hello,
            Code::parse(&snapshot.words.join(" ")).unwrap(),
            "browser",
        )
        .unwrap();
        socket.send(Message::Text(start.into())).await.unwrap();
        let confirm = client
            .reply(&next(&mut socket).await.into_text().unwrap())
            .unwrap();
        socket.send(Message::Text(confirm.into())).await.unwrap();
        let (paired, finish) = client
            .finish(
                &next(&mut socket).await.into_data(),
                Some("Synthetic browser"),
                None,
                jiff::Timestamp::now(),
            )
            .unwrap();
        socket.send(Message::Binary(finish.into())).await.unwrap();
        assert!(matches!(next(&mut socket).await, Message::Close(_)));
        paired
    }

    /// Open a channel using the browser's cluster pin and private static.
    pub async fn browser_connection(&self, paired: &ClientPaired) -> Connection {
        let pins = {
            let target = self.handler.node().lock().unwrap();
            NodePins {
                id: target.id().to_string(),
                cluster: paired.cluster_pub,
                x25519: x25519_dalek::PublicKey::from(&target.identity().x25519_static()),
            }
        };
        let (pending, hello) = ClientHandshake::start(
            &paired.device,
            x25519_dalek::StaticSecret::from(paired.x25519.to_bytes()),
            pins,
        )
        .unwrap();
        self.connect(pending, hello).await
    }

    async fn connect(&self, pending: ClientHandshake, hello: String) -> Connection {
        let mut socket = socket(self.port, "/ws").await;
        socket.send(Message::Text(hello.into())).await.unwrap();
        let (crypto, confirm) = pending
            .finish(
                &next(&mut socket).await.into_text().unwrap(),
                jiff::Timestamp::now(),
            )
            .unwrap();
        socket.send(Message::Binary(confirm.into())).await.unwrap();
        Connection {
            socket,
            crypto,
            id: 0,
        }
    }
}

/// Open the requested real WebSocket path, or panic on transport failure.
pub async fn socket(port: u16, path: &str) -> Socket {
    tokio_tungstenite::connect_async(format!("ws://127.0.0.1:{port}{path}"))
        .await
        .unwrap()
        .0
}
/// Read one message with a ten-second deadline, or panic on disconnect.
pub async fn next(socket: &mut Socket) -> Message {
    tokio::time::timeout(Duration::from_secs(10), socket.next())
        .await
        .unwrap()
        .unwrap()
        .unwrap()
}

/// Authenticated test client using the production frame and crypto types.
pub struct Connection {
    /// Socket carrying encrypted frames.
    pub socket: Socket,
    /// Synthetic client session keys and counters.
    pub crypto: privatium_core::session::Session,
    id: u64,
}

/// Abortable TCP proxy recording bounded ciphertext for privacy assertions.
pub struct Proxy {
    /// Ephemeral loopback port.
    pub port: u16,
    /// At most two MiB of TCP bytes, never decrypted.
    pub captured: Arc<std::sync::Mutex<Vec<u8>>>,
    /// Notifies a test when bytes cross the proxy.
    pub activity: Arc<tokio::sync::Notify>,
    /// Listener task, owned by this fixture.
    pub task: tokio::task::JoinHandle<()>,
}

impl Drop for Proxy {
    fn drop(&mut self) {
        self.task.abort();
    }
}

impl Proxy {
    /// Forward a loopback port through an abortable proxy.
    pub async fn new(destination: u16) -> Self {
        use tokio::io::{AsyncReadExt as _, AsyncWriteExt as _};
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let port = listener.local_addr().unwrap().port();
        let captured = Arc::new(std::sync::Mutex::new(Vec::new()));
        let activity = Arc::new(tokio::sync::Notify::new());
        let capture = captured.clone();
        let notice = activity.clone();
        let task = tokio::spawn(async move {
            let mut jobs = tokio::task::JoinSet::new();
            loop {
                tokio::select! {
                    accepted=listener.accept()=>{
                        let (client,_)=accepted.unwrap();
                        let node=TcpStream::connect(("127.0.0.1",destination)).await.unwrap();
                        let capture=capture.clone(); let notice=notice.clone();
                        jobs.spawn(async move {
                            let (a, b) = client.into_split();
                            let (c, d) = node.into_split();
                            // Both directions are copied and kept, so a test can assert
                            // that what crossed the wire was ciphertext.
                            async fn copy(
                                mut from: tokio::net::tcp::OwnedReadHalf,
                                mut to: tokio::net::tcp::OwnedWriteHalf,
                                capture: Arc<std::sync::Mutex<Vec<u8>>>,
                                notice: Arc<tokio::sync::Notify>,
                            ) {
                                let mut buffer = [0u8; 8192];
                                while let Ok(n) = from.read(&mut buffer).await {
                                    if n == 0 {
                                        break;
                                    }
                                    {
                                        let mut bytes = capture.lock().unwrap();
                                        let room =
                                            (2 * 1024 * 1024usize).saturating_sub(bytes.len());
                                        bytes.extend_from_slice(&buffer[..n.min(room)]);
                                    }
                                    notice.notify_one();
                                    if to.write_all(&buffer[..n]).await.is_err() {
                                        break;
                                    }
                                }
                            }
                            tokio::join!(
                                copy(a, d, capture.clone(), notice.clone()),
                                copy(c, b, capture, notice)
                            );
                        });
                    }
                    _=jobs.join_next(),if !jobs.is_empty()=>{},
                }
            }
        });
        Self {
            port,
            captured,
            activity,
            task,
        }
    }
}
impl Connection {
    /// Send a bounded test request and collect its encrypted response.
    pub async fn request(
        &mut self,
        method: &str,
        path: &str,
        body: &[u8],
    ) -> (u16, BTreeMap<String, String>, Vec<u8>) {
        self.request_type(method, path, body, "application/x-ndjson")
            .await
    }

    /// Send a test request with the selected content type; panics on malformed frames.
    pub async fn request_type(
        &mut self,
        method: &str,
        path: &str,
        body: &[u8],
        content_type: &str,
    ) -> (u16, BTreeMap<String, String>, Vec<u8>) {
        self.id += 1;
        let mut frame = Frame::new(self.id, Kind::Req);
        frame.method = Some(method.into());
        frame.headers = Some(BTreeMap::new());
        frame.path = Some(path.into());
        frame.payload = body.to_vec().into();
        if method == "POST" {
            frame.headers = Some(BTreeMap::from([(
                "content-type".into(),
                content_type.into(),
            )]));
        }
        let bytes = self.crypto.send.seal(&frame.encode().unwrap()).unwrap();
        self.socket
            .send(Message::Binary(bytes.into()))
            .await
            .unwrap();
        let mut status = 0;
        let mut headers = BTreeMap::new();
        let mut body = Vec::new();
        loop {
            let message = next(&mut self.socket).await;
            assert!(
                matches!(message, Message::Binary(_)),
                "expected encrypted response, got {message:?}"
            );
            let frame =
                Frame::decode(&self.crypto.receive.open(&message.into_data()).unwrap()).unwrap();
            assert_eq!(frame.id, self.id);
            match frame.kind {
                Kind::Res => {
                    status = frame.status.unwrap();
                    headers = frame.headers.unwrap_or_default();
                }
                Kind::Chunk => body.extend_from_slice(&frame.payload),
                Kind::End => return (status, headers, body),
                _ => panic!("unexpected frame"),
            }
        }
    }
}
