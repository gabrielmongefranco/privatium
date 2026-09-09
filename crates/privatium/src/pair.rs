// Project:  Privatium™  |  File: crates/privatium/src/pair.rs
// Authors:  Gabriel Mongefranco (@gabrielmongefranco)
// Created:  2026-09-06  |  Modified: 2026-09-07
// Summary:  `privatium pair` (spec/cli.md §8): open a pairing window on the running node —
//           for devices or, with --node, for another node — print the code both ways and
//           the QR code, follow the window until it is consumed or closes; and `--join`,
//           the other machine's half, which reads the code from the terminal and asks the
//           running node to join. Both talk to the node over loopback (spec/protocol.md
//           §9.2) and open no node of their own.
//           See main README.md for full license information.

use std::io::{BufRead as _, Read as _, Write as _};
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

/// How long to wait for the node's answer: a window's is immediate; a join takes the
/// exchange with the other node, which that node bounds at thirty seconds.
const ANSWER_TIMEOUT: Duration = Duration::from_secs(10);
const JOIN_TIMEOUT: Duration = Duration::from_secs(60);

/// `pair [--open] [--timeout <seconds>] [--node]`.
pub fn pair(global: &Global, open: bool, timeout: u64, for_node: bool) -> Result<u8> {
    let node = loopback(global)?;
    let ttl = timeout.clamp(1, pair::TTL.as_secs());
    if timeout != ttl {
        eprintln!(
            "privatium pair: --timeout {timeout} is outside 1 to {} seconds; using {ttl} \
             (spec/protocol.md §7.5)",
            pair::TTL.as_secs()
        );
    }
    let body = format!("{{\"ttl\":{ttl},\"node\":{for_node}}}");
    let window = request(node, "POST", "/api/v1/pair", &body, ANSWER_TIMEOUT)?;
    let is_node_window = window["node"].as_bool().unwrap_or(false);
    if is_node_window != for_node {
        eprintln!(
            "privatium pair: a pairing window for {} is already open on this space; close it \
             from the devices page or wait for it to expire, then try again",
            if is_node_window {
                "another space"
            } else {
                "devices"
            }
        );
        return Ok(1);
    }
    let url = window["url"].as_str().unwrap_or_default().to_owned();
    print_window(&window, &url, for_node);
    if open {
        node::open_browser(&format!("http://{node}/settings/devices"));
    }

    let mut generation = window["generation"].as_u64().unwrap_or(0);
    loop {
        std::thread::sleep(POLL);
        let current = request(node, "GET", "/api/v1/pair", "", ANSWER_TIMEOUT)?;
        if current.is_null() {
            eprintln!(
                "privatium pair: the pairing window closed without {} pairing",
                if for_node { "a space" } else { "a device" }
            );
            return Ok(1);
        }
        if let Some(device) = current["consumed_by"].as_str() {
            if for_node {
                println!("privatium pair: admitted space {device} to this cluster");
            } else {
                println!("privatium pair: paired {device}");
            }
            return Ok(0);
        }
        let now = current["generation"].as_u64().unwrap_or(0);
        if now != generation {
            generation = now;
            println!("privatium pair: five attempts used up that code; the new code is:");
            print_window(&current, &url, for_node);
        }
    }
}

/// `pair --join <url>`: read the code the other machine shows from standard input and
/// ask the running node to join (`spec/cli.md §8`, `spec/protocol.md §2.3.1`). The code
/// is never taken as an argument, so it reaches no shell history and no process list.
pub fn join(global: &Global, url: &str) -> Result<u8> {
    let node = loopback(global)?;
    let url = url.trim();
    if !url.starts_with("http://") {
        bail!(
            "pair --join: the URL is the one the other space printed, starting with http:// (spec/cli.md §8)"
        );
    }
    eprintln!(
        "privatium pair: joining the space at {url}. Type the code shown there — the two \
         words, or the four emoji labels — and press Enter:"
    );
    let mut line = String::new();
    std::io::stdin()
        .lock()
        .read_line(&mut line)
        .context("reading the code from standard input")?;
    let code = line.trim();
    if code.is_empty() {
        bail!("pair --join: no code was entered");
    }
    if let Err(error) = pair::Code::parse(code) {
        bail!("pair --join: {error}");
    }
    let body = serde_json::json!({ "url": url, "code": code }).to_string();
    let joined = match request(node, "POST", "/api/v1/join", &body, JOIN_TIMEOUT) {
        Ok(joined) => joined,
        Err(error) => {
            eprintln!("privatium pair: {error:#}");
            return Ok(1);
        }
    };
    let cluster = joined["cluster_id"].as_str().unwrap_or("?");
    let peer = joined["peer"].as_str().unwrap_or("?");
    if joined["joined"].as_bool().unwrap_or(false) {
        println!(
            "privatium pair: this space joined cluster {cluster} through space {peer}{}",
            if joined["readmitted"].as_bool().unwrap_or(false) {
                " and was re-admitted with its own key"
            } else {
                ""
            }
        );
    } else {
        println!("privatium pair: admitted space {peer} to this cluster ({cluster})");
    }
    Ok(0)
}

/// The running node's loopback address, from `config.toml`; the node is not opened.
fn loopback(global: &Global) -> Result<SocketAddr> {
    let paths = node::paths(global)?;
    let config = Config::load(paths.config_file())?;
    Ok(SocketAddr::from((Ipv4Addr::LOCALHOST, config.node.port)))
}

/// The code in both renderings and the QR code with the URL beside it, to standard
/// output — the screen `spec/protocol.md §7.1` puts the code on.
fn print_window(window: &Value, url: &str, for_node: bool) {
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
    if for_node {
        println!("privatium pair: on the other space, run:");
        println!("    privatium pair --join {url}");
        println!(
            "privatium pair: or open its Space settings, choose Join a cluster, and enter {url}"
        );
    } else {
        println!("privatium pair: open {url} on the other device, or scan:");
        match qr::text(url) {
            Some(code) => print!("{code}"),
            None => println!("privatium pair: (the URL is too long for a QR code; type it)"),
        }
    }
    println!(
        "privatium pair: then {} these four emoji, in order:",
        if for_node {
            "type the labels of"
        } else {
            "tap"
        }
    );
    for glyph in &glyphs {
        println!("    {glyph}");
    }
    println!(
        "privatium pair: or type these two words: {}",
        words.join(" ")
    );
    println!(
        "privatium pair: the code is good until {} — waiting for {} (Ctrl-C stops \
         waiting; the window stays open on the node)",
        window["expires_at"].as_str().unwrap_or("it expires"),
        if for_node {
            "the other space"
        } else {
            "a device"
        }
    );
}

/// One HTTP/1.1 exchange with the running node on loopback, by hand: a connection
/// closed after the answer, the status checked, the body read as JSON. Nothing here
/// follows a redirect or leaves loopback.
fn request(
    node: SocketAddr,
    method: &str,
    path: &str,
    body: &str,
    timeout: Duration,
) -> Result<Value> {
    let mut stream = TcpStream::connect_timeout(&node, Duration::from_secs(3)).map_err(|_| {
        anyhow!(
            "no node is running on {node}: start one with `privatium` in another terminal, \
             then run `privatium pair` again (spec/cli.md §8)"
        )
    })?;
    stream.set_read_timeout(Some(timeout))?;
    let request = format!(
        "{method} {path} HTTP/1.1\r\nHost: 127.0.0.1:{}\r\nConnection: close\r\n\
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
