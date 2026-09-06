// Project:  Privatium™  |  File: crates/privatium-core/src/discover/udp.rs
// Authors:  Gabriel Mongefranco (@gabrielmongefranco)
// Created:  2026-09-06  |  Modified: 2026-09-06
// Summary:  The UDP broadcast fallback of spec/protocol.md §6.4 for networks that filter
//           multicast: a responder on port 52525 that answers `PVDISCO1` + nonce from a
//           private, link-local or loopback source with the same prefix, the nonce and
//           the TXT key set as JSON, once per source per second; and the probe a node
//           sends to find its peers.

use std::collections::{BTreeMap, HashMap};
use std::net::{IpAddr, Ipv4Addr, SocketAddr, UdpSocket};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::thread::JoinHandle;
use std::time::{Duration, Instant};

use super::{Discovered, Shared, txt};

/// The port of `§6.6`, fixed in `pv/1`.
pub const PORT: u16 = 52525;

/// The eight bytes every probe and every answer begins with (`§6.4`).
pub const MAGIC: &[u8; 8] = b"PVDISCO1";

/// A probe: the magic and a four-byte nonce, nothing else.
pub const PROBE_LEN: usize = MAGIC.len() + 4;

/// The most an answer may be: the magic, the nonce and a record under the TXT budget.
const ANSWER_MAX: usize = PROBE_LEN + txt::BUDGET + 256;

/// One answer per source per second (`§6.4`).
const PER_SOURCE: Duration = Duration::from_secs(1);

/// How long the responder's thread waits on the socket before checking for a stop.
const TICK: Duration = Duration::from_millis(500);

/// Whether a probe from `ip` may be answered (`§6.4`): RFC 1918 and RFC 4193 space,
/// link-local, or loopback. An IPv4 address carried in IPv6 is judged as IPv4.
#[must_use]
pub fn source_allowed(ip: IpAddr) -> bool {
    match ip {
        IpAddr::V4(v4) => v4.is_private() || v4.is_link_local() || v4.is_loopback(),
        IpAddr::V6(v6) => {
            if let Some(v4) = v6.to_ipv4_mapped() {
                return source_allowed(IpAddr::V4(v4));
            }
            v6.is_loopback() || v6.is_unique_local() || v6.is_unicast_link_local()
        }
    }
}

/// The per-source limit, over a clock the caller supplies.
#[derive(Debug, Default)]
pub struct RateLimit {
    last: HashMap<IpAddr, Instant>,
}

/// Entries older than this are dropped whenever the table is pruned.
const FORGET_AFTER: Duration = Duration::from_secs(60);

/// The table is pruned when it grows past this many sources.
const PRUNE_AT: usize = 1024;

impl RateLimit {
    /// Whether a probe from `ip` at `now` gets an answer, and record it if so.
    pub fn allow(&mut self, ip: IpAddr, now: Instant) -> bool {
        if self.last.len() > PRUNE_AT {
            self.last
                .retain(|_, last| now.duration_since(*last) < FORGET_AFTER);
        }
        match self.last.get(&ip) {
            Some(last) if now.duration_since(*last) < PER_SOURCE => false,
            _ => {
                self.last.insert(ip, now);
                true
            }
        }
    }
}

/// The nonce of a well-formed probe, or `None` for anything else — the wrong length, the
/// wrong magic — which is dropped without an answer.
#[must_use]
pub fn probe_nonce(datagram: &[u8]) -> Option<[u8; 4]> {
    if datagram.len() != PROBE_LEN || &datagram[..MAGIC.len()] != MAGIC {
        return None;
    }
    let mut nonce = [0u8; 4];
    nonce.copy_from_slice(&datagram[MAGIC.len()..]);
    Some(nonce)
}

/// The answer to a probe carrying `nonce`: the magic, the nonce, and the TXT record of
/// `§6.1` as a JSON object of strings.
#[must_use]
pub fn answer(facts: &super::Facts, nonce: [u8; 4], now: jiff::Timestamp) -> Vec<u8> {
    let record: serde_json::Map<String, serde_json::Value> = txt::record(facts, now)
        .into_iter()
        .map(|(k, v)| (k, serde_json::Value::String(v)))
        .collect();
    let mut out = Vec::with_capacity(ANSWER_MAX);
    out.extend_from_slice(MAGIC);
    out.extend_from_slice(&nonce);
    out.extend_from_slice(serde_json::Value::Object(record).to_string().as_bytes());
    out
}

/// Read an answer to the probe that carried `nonce`: the record's keys and values, or
/// `None` for a datagram that is not that answer.
#[must_use]
pub fn read_answer(datagram: &[u8], nonce: [u8; 4]) -> Option<BTreeMap<String, String>> {
    if datagram.len() < PROBE_LEN
        || datagram.len() > ANSWER_MAX
        || &datagram[..MAGIC.len()] != MAGIC
        || datagram[MAGIC.len()..PROBE_LEN] != nonce
    {
        return None;
    }
    let object: serde_json::Map<String, serde_json::Value> =
        serde_json::from_slice(&datagram[PROBE_LEN..]).ok()?;
    let mut record = BTreeMap::new();
    for (key, value) in object {
        record.insert(key, value.as_str()?.to_owned());
    }
    Some(record)
}

/// The responder: a socket and the thread that answers on it.
pub struct Responder {
    port: u16,
    stop: Arc<AtomicBool>,
    thread: Option<JoinHandle<()>>,
}

impl Responder {
    /// Bind `0.0.0.0:<port>` and answer probes with the facts `shared` holds at the
    /// moment each arrives. `port` is [`PORT`] for a node; `0` takes any free port.
    pub fn bind(port: u16, shared: Arc<Shared>) -> std::io::Result<Self> {
        let socket = UdpSocket::bind(SocketAddr::from((Ipv4Addr::UNSPECIFIED, port)))?;
        socket.set_read_timeout(Some(TICK))?;
        let port = socket.local_addr()?.port();
        let stop = Arc::new(AtomicBool::new(false));
        let thread = {
            let stop = Arc::clone(&stop);
            std::thread::Builder::new()
                .name("privatium-udp".to_owned())
                .spawn(move || serve(&socket, &shared, &stop))?
        };
        Ok(Self {
            port,
            stop,
            thread: Some(thread),
        })
    }

    /// The port answered on.
    #[must_use]
    pub fn port(&self) -> u16 {
        self.port
    }
}

impl Drop for Responder {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::Relaxed);
        if let Some(thread) = self.thread.take() {
            let _ = thread.join();
        }
    }
}

fn serve(socket: &UdpSocket, shared: &Shared, stop: &AtomicBool) {
    let mut limit = RateLimit::default();
    let mut buffer = [0u8; 64];
    while !stop.load(Ordering::Relaxed) {
        let (len, source) = match socket.recv_from(&mut buffer) {
            Ok(received) => received,
            Err(error)
                if matches!(
                    error.kind(),
                    std::io::ErrorKind::WouldBlock | std::io::ErrorKind::TimedOut
                ) =>
            {
                continue;
            }
            // A closed or reset socket: on Windows a datagram bounced back by a peer
            // surfaces here too, and the responder keeps answering everyone else.
            Err(_) => continue,
        };
        let Some(nonce) = probe_nonce(&buffer[..len]) else {
            continue;
        };
        if !source_allowed(source.ip()) || !limit.allow(source.ip(), Instant::now()) {
            continue;
        }
        let reply = answer(&shared.facts(), nonce, jiff::Timestamp::now());
        let _ = socket.send_to(&reply, source);
    }
}

/// Broadcast a probe and collect every answer that arrives within `timeout` (`§6.4`).
pub fn probe(timeout: Duration) -> std::io::Result<Vec<Discovered>> {
    probe_at(SocketAddr::from((Ipv4Addr::BROADCAST, PORT)), timeout)
}

/// [`probe`] aimed at one address — loopback in a test, a subnet's broadcast address on
/// a network that blocks the limited one.
pub fn probe_at(target: SocketAddr, timeout: Duration) -> std::io::Result<Vec<Discovered>> {
    use rand::RngExt as _;
    let socket = UdpSocket::bind(SocketAddr::from((Ipv4Addr::UNSPECIFIED, 0)))?;
    socket.set_broadcast(true)?;
    let nonce: [u8; 4] = rand::rng().random();
    let mut datagram = Vec::with_capacity(PROBE_LEN);
    datagram.extend_from_slice(MAGIC);
    datagram.extend_from_slice(&nonce);
    socket.send_to(&datagram, target)?;

    let deadline = Instant::now() + timeout;
    let mut found: BTreeMap<String, Discovered> = BTreeMap::new();
    let mut buffer = vec![0u8; ANSWER_MAX + 1];
    loop {
        let remaining = deadline.saturating_duration_since(Instant::now());
        if remaining.is_zero() {
            break;
        }
        socket.set_read_timeout(Some(remaining))?;
        let (len, source) = match socket.recv_from(&mut buffer) {
            Ok(received) => received,
            Err(error)
                if matches!(
                    error.kind(),
                    std::io::ErrorKind::WouldBlock | std::io::ErrorKind::TimedOut
                ) =>
            {
                break;
            }
            Err(_) => continue,
        };
        let Some(record) = read_answer(&buffer[..len], nonce) else {
            continue;
        };
        let now = jiff::Timestamp::now();
        if let Some(seen) = txt::read(&record, vec![source.ip()], target.port(), "", now) {
            found.insert(seen.id.clone(), seen);
        }
    }
    Ok(found.into_values().collect())
}
