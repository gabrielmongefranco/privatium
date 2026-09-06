// Project:  Privatium™  |  File: crates/privatium/src/pair.rs
// Authors:  Gabriel Mongefranco (@gabrielmongefranco)
// Created:  2026-09-06  |  Modified: 2026-09-06
// Summary:  `privatium pair` (spec/cli.md §8): ask the running node over loopback to open
//           a pairing window (POST /api/v1/pair), print the code as four emoji with their
//           labels and as two words, the QR code of the node's URL with the URL in text
//           beside it, then follow the window (GET /api/v1/pair) every two seconds until a
//           device pairs (exit 0, naming it) or the window expires (exit 1). A data root is
//           one process's (§1), so this command opens no node of its own; with none
//           running it is a runtime error saying to start one.

use std::io::{Read as _, Write as _};
use std::net::{Ipv4Addr, SocketAddr, TcpStream};
use std::time::Duration;

use anyhow::{Context as _, Result, anyhow, bail};
use privatium_core::pair::qr;
use privatium_core::{Config, pair};
use serde_json::Value;

use crate::cli::Global;
use crate::node;

/// How often the window is read while it is open (`spec/cli.md §8`).
const POLL: Duration = Duration::from_secs(2);

/// The most bytes an answer may carry; a window's JSON is a few hundred.
const ANSWER_LIMIT: usize = 64 * 1024;

/// `pair [--open] [--timeout <seconds>]`.
pub fn pair(global: &Global, open: bool, timeout: u64) -> Result<u8> {
    let paths = node::paths(global)?;
    let config = Config::load(paths.config_file())?;
    let node = SocketAddr::from((Ipv4Addr::LOCALHOST, config.node.port));
    let ttl = timeout.clamp(1, pair::TTL.as_secs());
    if timeout != ttl {
        eprintln!(
            "privatium pair: --timeout {timeout} is outside 1 to {} seconds; using {ttl} \
             (spec/protocol.md §7.5)",
            pair::TTL.as_secs()
        );
    }
    let window = request(node, "POST", &format!("{{\"ttl\":{ttl}}}"))?;
    let url = window["url"].as_str().unwrap_or_default().to_owned();
    print_window(&window, &url);
    if open {
        node::open_browser(&format!("http://{node}/settings/devices"));
    }

    let mut generation = window["generation"].as_u64().unwrap_or(0);
    loop {
        std::thread::sleep(POLL);
        let current = request(node, "GET", "")?;
        if current.is_null() {
            eprintln!("privatium pair: the pairing window closed without a device pairing");
            return Ok(1);
        }
        if let Some(device) = current["consumed_by"].as_str() {
            println!("privatium pair: paired {device}");
            return Ok(0);
        }
        let now = current["generation"].as_u64().unwrap_or(0);
        if now != generation {
            generation = now;
            println!("privatium pair: five attempts used up that code; the new code is:");
            print_window(&current, &url);
        }
    }
}

/// The code in both renderings and the QR code with the URL beside it, to standard
/// output — the screen `spec/protocol.md §7.1` puts the code on.
fn print_window(window: &Value, url: &str) {
    let strings = |key: &str| -> Vec<String> {
        window[key]
            .as_array()
            .map(|items| {
                items
                    .iter()
                    .filter_map(Value::as_str)
                    .map(str::to_owned)
                    .collect()
            })
            .unwrap_or_default()
    };
    let (emoji, labels, words) = (strings("emoji"), strings("labels"), strings("words"));
    let glyphs: Vec<String> = emoji
        .iter()
        .zip(labels.iter())
        .map(|(glyph, label)| format!("{glyph}  {label}"))
        .collect();
    println!("privatium pair: open {url} on the other device, or scan:");
    match qr::text(url) {
        Some(code) => print!("{code}"),
        None => println!("privatium pair: (the URL is too long for a QR code; type it)"),
    }
    println!("privatium pair: then tap these four emoji, in order:");
    for glyph in &glyphs {
        println!("    {glyph}");
    }
    println!(
        "privatium pair: or type these two words: {}",
        words.join(" ")
    );
    println!(
        "privatium pair: the code is good until {} — waiting for a device (Ctrl-C stops \
         waiting; the window stays open on the node)",
        window["expires_at"].as_str().unwrap_or("it expires")
    );
}

/// One HTTP/1.1 exchange with the running node on loopback, by hand: a connection
/// closed after the answer, the status checked, the body read as JSON. Nothing here
/// follows a redirect or leaves loopback.
fn request(node: SocketAddr, method: &str, body: &str) -> Result<Value> {
    let mut stream = TcpStream::connect_timeout(&node, Duration::from_secs(3)).map_err(|_| {
        anyhow!(
            "no node is running on {node}: start one with `privatium` in another terminal, \
             then run `privatium pair` again (spec/cli.md §8)"
        )
    })?;
    stream.set_read_timeout(Some(Duration::from_secs(10)))?;
    let request = format!(
        "{method} /api/v1/pair HTTP/1.1\r\nHost: 127.0.0.1:{}\r\nConnection: close\r\n\
         Accept: application/json\r\nContent-Type: application/json\r\nContent-Length: {}\r\n\r\n{body}",
        node.port(),
        body.len()
    );
    stream
        .write_all(request.as_bytes())
        .context("sending the request to the node")?;
    let mut raw = Vec::new();
    stream
        .take(ANSWER_LIMIT as u64)
        .read_to_end(&mut raw)
        .context("reading the node's answer")?;
    let split = raw
        .windows(4)
        .position(|w| w == b"\r\n\r\n")
        .ok_or_else(|| anyhow!("the node's answer was not HTTP"))?;
    let head = String::from_utf8_lossy(&raw[..split]).into_owned();
    let status: u16 = head
        .lines()
        .next()
        .and_then(|line| line.split(' ').nth(1))
        .and_then(|code| code.parse().ok())
        .ok_or_else(|| anyhow!("the node's answer had no status line"))?;
    let body = &raw[split + 4..];
    if status != 200 {
        bail!(
            "the node refused ({status}): {}",
            String::from_utf8_lossy(body).trim()
        );
    }
    serde_json::from_slice(body).context("reading the node's answer as JSON")
}
