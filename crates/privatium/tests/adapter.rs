// Project:  Privatium™  |  File: crates/privatium/tests/adapter.rs
// Authors:  Gabriel Mongefranco (@gabrielmongefranco)
// Created:  2026-09-03  |  Modified: 2026-09-03
// Summary:  The axum adapter against ADR 0003 and docs/plans/phase-1.md §2.1: it binds
//           loopback only, it adds nothing the core does not answer, it forwards the core's
//           streamed body frames verbatim, and it never buffers a request body the core did
//           not ask for. Raw TCP on the client side, so nothing here depends on an HTTP client.

// AGENTS.md, Style: unwrap() is permitted in tests, and a test that hides a failure
// behind `?` is worse than one that panics with a line number.
#![allow(clippy::unwrap_used, clippy::expect_used)]

use std::fs;
use std::net::{Ipv4Addr, SocketAddr};
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Duration;

use privatium::adapter;
use privatium_core::{AppRoot, Body, Handler, Node, Peer};
use tokio::io::{AsyncReadExt as _, AsyncWriteExt as _};
use tokio::net::TcpStream;

/// The repository's `apps/`.
fn repo_apps_dir() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("..")
        .join("..")
        .join("apps")
}

/// A node with the reference apps and one local Tier 2 app holding a 4 MiB file.
fn handler(root: &tempfile::TempDir) -> Arc<Handler> {
    let mut node = Node::open(root.path()).unwrap();
    let apps = node.paths().apps_dir();
    let big = apps.join("big");
    fs::create_dir_all(big.join("web")).unwrap();
    fs::write(
        big.join("app.toml"),
        "[app]\nslug = \"big\"\ntitle = \"Big\"\nversion = \"1.0.0\"\napi = 1\ntier = \"web\"\n",
    )
    .unwrap();
    fs::write(big.join("web").join("index.html"), "<p>big</p>").unwrap();
    fs::write(big.join("web").join("big.bin"), vec![b'x'; 4 * 1024 * 1024]).unwrap();
    let roots = [AppRoot::local(apps), AppRoot::bundled(repo_apps_dir())];
    let report = node.load_apps(&roots).unwrap();
    Arc::new(Handler::new(node, report))
}

/// Bind port 0, serve on a task, and hand back where.
async fn serve(handler: Arc<Handler>) -> (SocketAddr, tokio::task::JoinHandle<()>) {
    let listener = adapter::bind(0).await.unwrap();
    let addr = listener.local_addr().unwrap();
    let task = tokio::spawn(async move {
        let _ = adapter::serve(listener, handler).await;
    });
    (addr, task)
}

/// One request over a fresh connection, closed by the server: status line, headers, body.
async fn fetch(
    addr: SocketAddr,
    method: &str,
    path: &str,
) -> (u16, Vec<(String, String)>, Vec<u8>) {
    let mut stream = TcpStream::connect(addr).await.unwrap();
    let request = format!(
        "{method} {path} HTTP/1.1\r\nHost: 127.0.0.1:{}\r\nConnection: close\r\n\r\n",
        addr.port()
    );
    stream.write_all(request.as_bytes()).await.unwrap();
    let mut raw = Vec::new();
    stream.read_to_end(&mut raw).await.unwrap();
    parse(&raw)
}

fn parse(raw: &[u8]) -> (u16, Vec<(String, String)>, Vec<u8>) {
    let split = raw.windows(4).position(|w| w == b"\r\n\r\n").unwrap();
    let head = std::str::from_utf8(&raw[..split]).unwrap();
    let mut lines = head.lines();
    let status: u16 = lines
        .next()
        .unwrap()
        .split(' ')
        .nth(1)
        .unwrap()
        .parse()
        .unwrap();
    let headers: Vec<(String, String)> = lines
        .filter_map(|line| line.split_once(':'))
        .map(|(k, v)| (k.trim().to_ascii_lowercase(), v.trim().to_owned()))
        .collect();
    let body = raw[split + 4..].to_vec();
    (status, headers, body)
}

fn header<'a>(headers: &'a [(String, String)], name: &str) -> Option<&'a str> {
    headers
        .iter()
        .find(|(k, _)| k == name)
        .map(|(_, v)| v.as_str())
}

/// `docs/plans/phase-1.md §2.1` — the listener is `127.0.0.1` and nothing else, with no
/// flag to say otherwise.
#[tokio::test]
async fn test_binds_loopback_only() {
    assert_eq!(adapter::BIND_IP, Ipv4Addr::LOCALHOST);
    let listener = adapter::bind(0).await.unwrap();
    let addr = listener.local_addr().unwrap();
    assert!(addr.ip().is_loopback(), "{addr}");
    assert_eq!(addr.ip(), Ipv4Addr::LOCALHOST);
    let announced = adapter::announce(addr);
    assert!(
        announced.contains(&format!("http://{addr}/")),
        "{announced}"
    );
    assert!(
        announced.contains("LAN access arrives with pairing"),
        "{announced}"
    );
    // The port is the OS's here; a real start takes it from config and never from a --bind.
    assert_ne!(addr.port(), 0);
}

/// ADR 0003 — the adapter adds no route and rewrites no path: for every path, known or
/// not, the socket answers with exactly what `handle` answers in-process.
#[tokio::test]
async fn test_adapter_registers_no_routes_of_its_own() {
    let root = tempfile::tempdir().unwrap();
    let handler = handler(&root);
    let (addr, task) = serve(Arc::clone(&handler)).await;

    for (method, path) in [
        ("GET", "/"),
        ("GET", "/settings"),
        ("GET", "/settings/apps"),
        ("GET", "/api/v1/health"),
        ("GET", "/api/v1/manifest"),
        ("GET", "/skills/privatium-overview.md"),
        ("GET", "/static/shell.css"),
        ("GET", "/a/big/"),
        ("GET", "/a/sketch/style.css"),
        ("GET", "/a/hello/"),
        ("GET", "/a/sketch"),
        ("GET", "/nope"),
        ("GET", "/api/anything"),
        ("GET", "/a/nope/"),
        ("GET", "/settings/../etc/passwd"),
        ("POST", "/"),
        ("DELETE", "/api/v1/health"),
        ("GET", "/settings/apps/hello/seed"),
    ] {
        let (status, headers, body) = fetch(addr, method, path).await;
        let mut request = axum::http::Request::builder()
            .method(method)
            .uri(path)
            .header("host", format!("127.0.0.1:{}", addr.port()))
            .body(Body::empty())
            .unwrap();
        request
            .extensions_mut()
            .insert(Peer(SocketAddr::from((Ipv4Addr::LOCALHOST, 40000))));
        let direct = handler.handle(request).await;
        assert_eq!(status, direct.status().as_u16(), "{method} {path}");
        for name in [
            "content-security-policy",
            "content-type",
            "cache-control",
            "x-content-type-options",
            "referrer-policy",
            "location",
            "allow",
        ] {
            let expected = direct
                .headers()
                .get(name)
                .map(|v| v.to_str().unwrap().to_owned());
            assert_eq!(
                header(&headers, name).map(str::to_owned),
                expected,
                "{method} {path}: {name}"
            );
        }
        let expected = axum::body::to_bytes(direct.into_body(), usize::MAX)
            .await
            .unwrap();
        assert_eq!(body, expected.to_vec(), "{method} {path}: body");
    }
    task.abort();
}

/// ADR 0003 — the core's body is a stream of frames and the adapter forwards them as they
/// come; a file larger than any one frame arrives whole and unchanged. The framing itself
/// is proved on the core (`tests/wire.rs`); this is the transport's half of it.
#[tokio::test]
async fn test_response_body_streams_without_buffering() {
    let root = tempfile::tempdir().unwrap();
    let handler = handler(&root);
    let (addr, task) = serve(handler).await;
    let (status, headers, body) = fetch(addr, "GET", "/a/big/big.bin").await;
    assert_eq!(status, 200);
    assert_eq!(header(&headers, "content-length"), Some("4194304"));
    assert_eq!(body.len(), 4 * 1024 * 1024);
    assert!(body.iter().all(|b| *b == b'x'));
    task.abort();
}

/// ADR 0003 — a request body is a stream the core reads as far as it wants, and no further.
///
/// Two shapes. With a declared `Content-Length` past the form limit and `Expect:
/// 100-continue`, the core refuses on the headers alone: no `100 Continue` is ever sent,
/// the client sends no body, and the 413 arrives with nothing buffered. Without a declared
/// length — chunked — the core reads up to the limit and stops; the client observes the
/// server stop consuming long before the body is complete. (The 413 on that path is
/// written and then the connection closes with unread bytes behind it, which on Windows
/// resets the socket and drops the answer from the client's buffer, so only the stalled
/// write is asserted there.)
#[tokio::test]
async fn test_large_request_body_never_fully_buffered() {
    let root = tempfile::tempdir().unwrap();
    let handler = handler(&root);
    let (addr, task) = serve(handler).await;
    const TOTAL: usize = 64 * 1024 * 1024;

    // Declared length: refused before a byte of the body is asked for.
    let mut stream = TcpStream::connect(addr).await.unwrap();
    let head = format!(
        "POST /settings/apps/hello/seed HTTP/1.1\r\nHost: 127.0.0.1:{}\r\n\
         Content-Type: application/x-www-form-urlencoded\r\nContent-Length: {TOTAL}\r\n\
         Expect: 100-continue\r\nConnection: close\r\n\r\n",
        addr.port()
    );
    stream.write_all(head.as_bytes()).await.unwrap();
    let mut raw = Vec::new();
    tokio::time::timeout(Duration::from_secs(10), stream.read_to_end(&mut raw))
        .await
        .unwrap()
        .unwrap();
    let text = String::from_utf8_lossy(&raw).into_owned();
    assert!(text.starts_with("HTTP/1.1 413 "), "answer: {text:?}");
    assert!(
        !text.contains("100 Continue"),
        "the body was asked for: {text:?}"
    );

    // Undeclared length: read to the limit, then not at all.
    let stream = TcpStream::connect(addr).await.unwrap();
    let (mut reader, mut writer) = stream.into_split();
    let head = format!(
        "POST /settings/apps/hello/seed HTTP/1.1\r\nHost: 127.0.0.1:{}\r\n\
         Content-Type: application/x-www-form-urlencoded\r\nTransfer-Encoding: chunked\r\n\r\n",
        addr.port()
    );
    writer.write_all(head.as_bytes()).await.unwrap();
    let response = tokio::spawn(async move {
        let mut raw = Vec::new();
        let _ = reader.read_to_end(&mut raw).await;
        raw
    });
    let payload = vec![b'a'; 64 * 1024];
    let chunk = [
        format!("{:x}\r\n", payload.len()).into_bytes(),
        payload,
        b"\r\n".to_vec(),
    ]
    .concat();
    let mut written = 0usize;
    while written < TOTAL {
        if response.is_finished() {
            break;
        }
        match tokio::time::timeout(Duration::from_millis(250), writer.write_all(&chunk)).await {
            Ok(Ok(())) => written += chunk.len(),
            // The server answered and closed; the kernel refuses the rest.
            Ok(Err(_)) => break,
            // The server stopped reading and the buffers are full — the same thing.
            Err(_) => break,
        }
    }
    drop(writer);
    let raw = tokio::time::timeout(Duration::from_secs(10), response)
        .await
        .unwrap()
        .unwrap();
    assert!(
        written < TOTAL / 4,
        "{written} of {TOTAL} bytes were accepted before the server stopped reading"
    );
    let text = String::from_utf8_lossy(&raw);
    if !text.is_empty() {
        assert!(text.starts_with("HTTP/1.1 413 "), "answer: {text:?}");
    }
    task.abort();
}
