// Project:  Privatium™  |  File: crates/privatium/tests/cluster.rs
// Authors:  Gabriel Mongefranco (@gabrielmongefranco)
// Created:  2026-09-07  |  Modified: 2026-09-07
// Summary:  Two nodes as two child binaries over real sockets (spec/cli.md §8,
//           spec/protocol.md §2.3.1): `pair --node` on one and `pair --join` on the other,
//           in both directions, a wrong code, and the usage error.
//           See main README.md for full license information.

#![allow(clippy::unwrap_used, clippy::expect_used)]

use std::fs;
use std::io::{BufRead as _, BufReader, Read as _, Write as _};
use std::net::TcpStream;
use std::path::Path;
use std::process::{Child, Command, Stdio};
use std::time::{Duration, Instant};

const BIN: &str = env!("CARGO_BIN_EXE_privatium");

/// A node run in the background with its output captured, killed on drop.
struct Running {
    child: Child,
    port: u16,
    dir: String,
}

impl Drop for Running {
    fn drop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}

impl Running {
    /// Start a node over `root` on a port `config.toml` names, so `privatium pair` and
    /// the node agree on it, with discovery off.
    fn start(root: &tempfile::TempDir, name: &str) -> Self {
        let port = free_port();
        let dir = root.path().join(name).to_string_lossy().into_owned();
        fs::create_dir_all(&dir).unwrap();
        fs::write(
            Path::new(&dir).join("config.toml"),
            format!("[node]\nport = {port}\n"),
        )
        .unwrap();
        let mut child = Command::new(BIN)
            .args(["--data-dir", &dir, "--no-discovery"])
            .env("PRIVATIUM_TEST_NO_CHECKOUT", "1")
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .unwrap();
        let mut reader = BufReader::new(child.stdout.take().unwrap());
        let started = Instant::now();
        loop {
            let mut line = String::new();
            let read = reader.read_line(&mut line).unwrap();
            assert_ne!(read, 0, "the node exited before announcing");
            if line.contains("privatium: listening on http://") {
                break;
            }
            assert!(
                started.elapsed() < Duration::from_secs(30),
                "no announce line"
            );
        }
        std::thread::spawn(move || {
            let mut sink = String::new();
            let _ = reader.read_to_string(&mut sink);
        });
        let stderr = child.stderr.take().unwrap();
        std::thread::spawn(move || {
            let mut sink = String::new();
            let _ = BufReader::new(stderr).read_to_string(&mut sink);
        });
        std::thread::sleep(Duration::from_millis(200));
        Self { child, port, dir }
    }

    fn id(&self) -> String {
        let (status, body) = http(self.port, "GET", "/api/v1/health", "");
        assert_eq!(status, 200, "{body}");
        let health: serde_json::Value = serde_json::from_str(&body).unwrap();
        health["id"].as_str().unwrap().to_owned()
    }

    fn cluster_pub(&self) -> Vec<u8> {
        fs::read(Path::new(&self.dir).join("identity/cluster.pub")).unwrap()
    }

    fn identity_files(&self) -> Vec<(String, Vec<u8>)> {
        let mut files: Vec<_> = fs::read_dir(Path::new(&self.dir).join("identity"))
            .unwrap()
            .map(|entry| {
                let path = entry.unwrap().path();
                (
                    path.file_name().unwrap().to_string_lossy().into_owned(),
                    fs::read(&path).unwrap(),
                )
            })
            .collect();
        files.sort();
        files
    }

    /// The URL another node dials.
    fn url(&self) -> String {
        format!("http://127.0.0.1:{}", self.port)
    }

    /// Run `pair --node` against this node in the background, and the words of the
    /// window it opened once it is open.
    fn open_node_window(&self) -> (Child, Vec<String>) {
        let pair = Command::new(BIN)
            .args(["--data-dir", &self.dir, "pair", "--node", "--timeout", "60"])
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .unwrap();
        let started = Instant::now();
        let words = loop {
            let (status, body) = http(self.port, "GET", "/api/v1/pair", "");
            assert_eq!(status, 200);
            // A window a browser consumed earlier is still reported until it is
            // retired; the one `pair --node` opened is for a node and not consumed.
            let window: serde_json::Value = serde_json::from_str(&body).unwrap();
            if window["node"] == true && window.get("consumed_by").is_none() {
                break window["words"]
                    .as_array()
                    .unwrap()
                    .iter()
                    .map(|w| w.as_str().unwrap().to_owned())
                    .collect::<Vec<_>>();
            }
            assert!(
                started.elapsed() < Duration::from_secs(20),
                "pair --node never opened a window"
            );
            std::thread::sleep(Duration::from_millis(100));
        };
        (pair, words)
    }

    /// Run `pair --join <url>` on this node with `code` on standard input; exit code,
    /// stdout, stderr.
    fn join(&self, url: &str, code: &str) -> (Option<i32>, String, String) {
        let mut join = Command::new(BIN)
            .args(["--data-dir", &self.dir, "pair", "--join", url])
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .unwrap();
        join.stdin
            .take()
            .unwrap()
            .write_all(format!("{code}\n").as_bytes())
            .unwrap();
        let output = join.wait_with_output().unwrap();
        (
            output.status.code(),
            String::from_utf8_lossy(&output.stdout).into_owned(),
            String::from_utf8_lossy(&output.stderr).into_owned(),
        )
    }
}

fn free_port() -> u16 {
    std::net::TcpListener::bind("127.0.0.1:0")
        .unwrap()
        .local_addr()
        .unwrap()
        .port()
}

/// One HTTP/1.1 exchange with a node on loopback, by hand.
fn http(port: u16, method: &str, path: &str, body: &str) -> (u16, String) {
    let mut stream = TcpStream::connect(("127.0.0.1", port)).unwrap();
    stream
        .set_read_timeout(Some(Duration::from_secs(10)))
        .unwrap();
    let request = format!(
        "{method} {path} HTTP/1.1\r\nHost: 127.0.0.1:{port}\r\nConnection: close\r\n\
         Content-Type: application/json\r\nAccept: text/html\r\nContent-Length: {}\r\n\r\n{body}",
        body.len()
    );
    stream.write_all(request.as_bytes()).unwrap();
    let mut raw = Vec::new();
    stream.read_to_end(&mut raw).unwrap();
    let split = raw.windows(4).position(|w| w == b"\r\n\r\n").unwrap();
    let head = String::from_utf8_lossy(&raw[..split]).into_owned();
    let status: u16 = head
        .lines()
        .next()
        .unwrap()
        .split(' ')
        .nth(1)
        .unwrap()
        .parse()
        .unwrap();
    (
        status,
        String::from_utf8_lossy(&raw[split + 4..]).into_owned(),
    )
}

type Ws =
    tokio_tungstenite::WebSocketStream<tokio_tungstenite::MaybeTlsStream<tokio::net::TcpStream>>;

async fn next(socket: &mut Ws) -> tokio_tungstenite::tungstenite::Message {
    use futures_util::StreamExt as _;
    tokio::time::timeout(Duration::from_secs(10), socket.next())
        .await
        .unwrap()
        .unwrap()
        .unwrap()
}

/// Pair a synthetic browser with the node on `port` over a real `/ws/pair` socket, so
/// the node has paired something and is no longer disposable.
fn pair_a_browser(port: u16) -> String {
    use futures_util::SinkExt as _;
    use privatium_core::pair::{Code, handshake::Client};
    use tokio_tungstenite::tungstenite::Message;
    let (status, body) = http(port, "POST", "/api/v1/pair", "{\"ttl\":60}");
    assert_eq!(status, 200, "{body}");
    let window: serde_json::Value = serde_json::from_str(&body).unwrap();
    let words: Vec<&str> = window["words"]
        .as_array()
        .unwrap()
        .iter()
        .map(|w| w.as_str().unwrap())
        .collect();
    let code = Code::parse(&words.join(" ")).unwrap();
    tokio::runtime::Runtime::new().unwrap().block_on(async {
        let (mut socket, _) =
            tokio_tungstenite::connect_async(format!("ws://127.0.0.1:{port}/ws/pair"))
                .await
                .unwrap();
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
                Some("Synthetic phone"),
                None,
                jiff::Timestamp::now(),
            )
            .unwrap();
        socket.send(Message::Binary(finish.into())).await.unwrap();
        let _ = next(&mut socket).await;
        paired.device
    })
}

/// `spec/cli.md §8`, `spec/protocol.md §2.3.1` — `pair --node` on the established node,
/// the code on the other's standard input: the fresh node joins, both commands exit 0
/// naming the cluster and the peer, the fresh node's `identity/` carries the cluster,
/// and the devices page on the established node lists it.
#[test]
fn test_spec_cli_8_pair_join_admits_this_node() {
    let root = tempfile::tempdir().unwrap();
    let a = Running::start(&root, "a");
    let b = Running::start(&root, "b");
    pair_a_browser(a.port);
    assert_ne!(a.cluster_pub(), b.cluster_pub());
    let (pair, words) = a.open_node_window();
    let (code, out, err) = b.join(&a.url(), &words.join(" "));
    assert_eq!(code, Some(0), "{out}\n{err}");
    assert!(
        out.contains("this space joined cluster") && out.contains(&a.id()),
        "{out}"
    );
    let output = pair.wait_with_output().unwrap();
    let pair_out = String::from_utf8_lossy(&output.stdout);
    assert_eq!(output.status.code(), Some(0), "{pair_out}");
    assert!(pair_out.contains("privatium pair --join"), "{pair_out}");
    assert!(pair_out.contains(&b.id()), "{pair_out}");
    assert_eq!(b.cluster_pub(), a.cluster_pub());
    let (status, page) = http(a.port, "GET", "/settings/devices", "");
    assert_eq!(status, 200);
    assert!(page.contains(&b.id()), "{page}");
    let (_, page) = http(b.port, "GET", "/settings", "");
    assert!(page.contains("Cluster ID"), "{page}");
}

/// `§2.3.1`, Phase 3b's direction — the fresh node opens the window and the established
/// node dials it: the dialed node joins the dialer's cluster.
#[test]
fn test_spec_cli_8_pair_join_from_the_established_node_admits_the_dialed_one() {
    let root = tempfile::tempdir().unwrap();
    let established = Running::start(&root, "home");
    let fresh = Running::start(&root, "vps");
    pair_a_browser(established.port);
    let before = established.cluster_pub();
    let (pair, words) = fresh.open_node_window();
    let (code, out, err) = established.join(&fresh.url(), &words.join(" "));
    assert_eq!(code, Some(0), "{out}\n{err}");
    assert!(
        out.contains("admitted space") && out.contains(&fresh.id()),
        "{out}"
    );
    let output = pair.wait_with_output().unwrap();
    assert_eq!(output.status.code(), Some(0));
    assert_eq!(
        established.cluster_pub(),
        before,
        "the established node kept its cluster"
    );
    assert_eq!(fresh.cluster_pub(), before, "the dialed node joined it");
    let (_, page) = http(established.port, "GET", "/settings/devices", "");
    assert!(page.contains(&fresh.id()), "{page}");
}

/// `§8` — a wrong code exits 1 by name and leaves this node's identity untouched; the
/// window on the other node stays open for another try.
#[test]
fn test_spec_cli_8_pair_join_with_a_wrong_code_exits_1_without_writing_identity() {
    let root = tempfile::tempdir().unwrap();
    let a = Running::start(&root, "a");
    let b = Running::start(&root, "b");
    let (pair, words) = a.open_node_window();
    let before = b.identity_files();
    let wrong: Vec<String> = words
        .iter()
        .map(|word| {
            let at = privatium_core::pair::WORDS
                .iter()
                .position(|w| w == word)
                .unwrap();
            privatium_core::pair::WORDS[(at + 1) % privatium_core::pair::WORDS.len()].to_owned()
        })
        .collect();
    let (code, out, err) = b.join(&a.url(), &wrong.join(" "));
    assert_eq!(code, Some(1), "{out}\n{err}");
    assert!(
        err.contains("did not match") || err.contains("refused"),
        "{err}"
    );
    assert_eq!(b.identity_files(), before);
    assert_ne!(a.cluster_pub(), b.cluster_pub());
    let (_, body) = http(a.port, "GET", "/api/v1/pair", "");
    assert_ne!(body.trim(), "null", "the window is still open");
    // Something that is not a code at all is refused before anything is dialed.
    let (code, _, err) = b.join(&a.url(), "not a code");
    assert_eq!(code, Some(1));
    assert!(!err.contains("refused"), "{err}");
    drop(pair);
}

/// `§8` — `--join` with `--node` or `--open` is a usage error, exit 2.
#[test]
fn test_spec_cli_8_pair_node_and_join_together_are_a_usage_error() {
    for args in [
        ["pair", "--node", "--join", "http://127.0.0.1:8420"],
        ["pair", "--open", "--join", "http://127.0.0.1:8420"],
    ] {
        let output = Command::new(BIN).args(args).output().unwrap();
        assert_eq!(output.status.code(), Some(2), "{args:?}");
        let err = String::from_utf8_lossy(&output.stderr);
        assert!(err.contains("--join"), "{err}");
    }
}
