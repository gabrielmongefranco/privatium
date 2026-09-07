// Project:  Privatium™  |  File: crates/privatium-core/src/http/auth.rs
// Authors:  Gabriel Mongefranco (@gabrielmongefranco)
// Created:  2026-09-03  |  Modified: 2026-09-07
// Summary:  auth_layer (spec/app-contract.md §6) with its real signature — a tower::Layer —
//           and the bootstrap policy of spec/protocol.md §8.4: a request from this
//           machine is this node, a channel is its paired device, and public routes
//           carry no device. core::handle applies it, so every adapter gets it; an
//           embedder wraps their own router with it (§2.3), where the peer is axum's
//           ConnectInfo and a request with no peer at all is refused — the layer fails
//           closed, never open. See main README.md for full license information.

use std::collections::BTreeSet;
use std::future::Future;
use std::net::{IpAddr, SocketAddr};
use std::pin::Pin;
use std::sync::{Mutex, PoisonError};
use std::task::{Context, Poll};
use std::time::{Duration, Instant};

use axum::extract::ConnectInfo;
use axum::http::StatusCode;
use axum::http::header::HOST;
use tower::{Layer, Service};

use crate::http::headers;
use crate::identity::NodeId;
use crate::wire::{Request, Response};

/// The remote address of the connection a request arrived on, when the transport has one.
///
/// The framework's socket adapter inserts this as a request extension. Its in-process
/// callers — a test calling `handle` directly, a native shell's custom scheme — insert
/// nothing, and the layer `Handler` applies reads that as "the caller is this process".
/// The layer `Node::auth_layer` hands an embedder refuses a request with no peer; an
/// embedder calling their own router in-process inserts this extension to say so.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Peer(pub SocketAddr);

/// Who the request is from, as the layer established it — inserted as a request extension
/// for everything downstream. A channel carries its paired device; a local owner call
/// carries this node's own `sys_device` row (spec/protocol.md §8.4).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Device(pub NodeId);

/// Standing established by a completed channel handshake. Only the channel can mint
/// it; incoming headers and a `Device` extension confer no standing (§8.3).
#[derive(Debug, Clone)]
pub struct Session {
    pub(crate) device: NodeId,
    pub(crate) node: NodeId,
    pub(crate) x25519: String,
    pub(crate) kind: SessionKind,
}

/// Capability established from the pairing registry, never supplied by a request.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SessionKind {
    /// A paired browser or native application client.
    Device,
    /// A cluster node, confined to synchronization and public discovery routes.
    Node,
}

impl Session {
    /// Whether this is an authenticated node-to-node channel.
    #[must_use]
    pub fn is_node(&self) -> bool {
        self.kind == SessionKind::Node
    }
    /// The authenticated device; no key material is exposed.
    #[must_use]
    pub fn device(&self) -> &NodeId {
        &self.device
    }
}

/// The core router alone classifies the public bootstrap set (§8.4).
#[derive(Debug, Clone, Copy)]
pub(crate) enum Public {
    Asset,
    Page,
}

/// A page request that dispatch must answer without application content.
#[derive(Debug, Clone, Copy)]
pub(crate) struct Bootstrap;

/// `node.auth_layer()`.
#[derive(Debug, Clone)]
pub struct AuthLayer {
    device: NodeId,
    /// Whether a request with neither a [`Peer`] nor a `ConnectInfo` is refused. On for
    /// the layer an embedder wraps their router with; off for the one `Handler` applies
    /// to `handle`, whose in-process callers have no peer to give.
    require_peer: bool,
}

impl AuthLayer {
    /// The layer for the node whose device row `device` is, as an embedder wraps their
    /// own router with it (`spec/app-contract.md §2.3`, `§6`). The peer is axum's
    /// `ConnectInfo<SocketAddr>` or an inserted [`Peer`]; a request with neither is
    /// refused, so a router served without `into_make_service_with_connect_info` admits
    /// nobody rather than everybody.
    #[must_use]
    pub fn new(device: NodeId) -> Self {
        Self {
            device,
            require_peer: true,
        }
    }

    /// The layer `Handler` applies to `handle`. The framework's own adapter always
    /// inserts [`Peer`], so a request with no peer came from inside the process and is
    /// this node's.
    pub(crate) fn for_adapter(device: NodeId) -> Self {
        Self {
            device,
            require_peer: false,
        }
    }
}

impl<S> Layer<S> for AuthLayer {
    type Service = AuthService<S>;

    fn layer(&self, inner: S) -> Self::Service {
        AuthService {
            inner,
            device: self.device.clone(),
            require_peer: self.require_peer,
        }
    }
}

/// The service [`AuthLayer`] wraps around whatever serves the routes.
#[derive(Debug, Clone)]
pub struct AuthService<S> {
    inner: S,
    device: NodeId,
    require_peer: bool,
}

impl<S> Service<Request> for AuthService<S>
where
    S: Service<Request, Response = Response>,
{
    type Response = Response;
    type Error = S::Error;
    type Future = AuthFuture<S::Future>;

    fn poll_ready(&mut self, cx: &mut Context<'_>) -> Poll<Result<(), Self::Error>> {
        self.inner.poll_ready(cx)
    }

    fn call(&mut self, mut request: Request) -> Self::Future {
        request.extensions_mut().remove::<Device>();
        if request.extensions().get::<Session>().is_none()
            && request
                .headers()
                .get("sec-fetch-site")
                .is_some_and(|v| v.as_bytes().eq_ignore_ascii_case(b"cross-site"))
        {
            return AuthFuture::Refused(Some(headers::text(StatusCode::FORBIDDEN, FORBIDDEN)));
        }
        if let Some(session) = request.extensions().get::<Session>() {
            if session.node != self.device {
                return AuthFuture::Refused(Some(headers::text(StatusCode::FORBIDDEN, FORBIDDEN)));
            }
            let device = session.device.clone();
            request.extensions_mut().insert(Device(device));
            return AuthFuture::Inner(Box::pin(self.inner.call(request)));
        }
        if request.extensions().get::<Public>().is_some() && !self.require_peer {
            if matches!(request.extensions().get::<Public>(), Some(Public::Page)) {
                request.extensions_mut().insert(Bootstrap);
            }
            return AuthFuture::Inner(Box::pin(self.inner.call(request)));
        }
        match check(&request, self.require_peer) {
            Err(refusal) => AuthFuture::Refused(Some(*refusal)),
            Ok(()) => {
                request.extensions_mut().insert(Device(self.device.clone()));
                AuthFuture::Inner(Box::pin(self.inner.call(request)))
            }
        }
    }
}

/// Either the refusal, ready now, or the inner service's future.
pub enum AuthFuture<F> {
    /// 403, built before the inner service was asked anything.
    Refused(Option<Response>),
    /// The request was allowed through.
    Inner(Pin<Box<F>>),
}

impl<F, E> Future for AuthFuture<F>
where
    F: Future<Output = Result<Response, E>>,
{
    type Output = Result<Response, E>;

    fn poll(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Self::Output> {
        // Both variants are `Unpin` — an `Option` and a boxed future — so the projection
        // is a plain `get_mut`.
        match self.get_mut() {
            Self::Refused(response) => Poll::Ready(Ok(response
                .take()
                .unwrap_or_else(|| headers::text(StatusCode::FORBIDDEN, FORBIDDEN)))),
            Self::Inner(future) => future.as_mut().poll(cx),
        }
    }
}

/// What a refused caller reads: how to use the authenticated channel, with no node data.
const FORBIDDEN: &str = "403 Forbidden — pair this device and use the encrypted channel; \
                         web apps use pv.js (spec/protocol.md §8.4).\n";

/// What a caller with no peer reads from an embedder's layer: which call is missing.
const NO_PEER: &str = "403 Forbidden — the request carries no peer address, so this layer \
                       cannot tell where it came from. Serve the router with \
                       into_make_service_with_connect_info::<SocketAddr>(), or insert the \
                       Peer extension for a call made in-process \
                       (spec/app-contract.md §2.3).\n";

/// The whole admission policy, in one function.
///
/// The peer is the [`Peer`] the framework's adapter inserts or, on an embedder's own
/// axum router, the `ConnectInfo<SocketAddr>` that `into_make_service_with_connect_info`
/// attaches (`spec/app-contract.md §2.3`). A request with neither is allowed only where
/// `require_peer` is off — the layer over `handle`, whose in-process callers are this
/// process — and refused everywhere else, naming the missing call. One with a peer is
/// allowed only from this machine — loopback, or one of its own interface addresses,
/// which is where a browser opened on the node's LAN address from the node's own
/// keyboard connects from — and only with a `Host` header naming this machine the same
/// way, because a browser resolving an attacker's name to `127.0.0.1` (DNS rebinding)
/// still connects from loopback, and the `Host` it sends is the one thing that gives
/// the game away. An absent `Host` is allowed: HTTP/1.0 clients and custom schemes send
/// none.
fn check(request: &Request, require_peer: bool) -> Result<(), Box<Response>> {
    let extensions = request.extensions();
    let peer = extensions.get::<Peer>().map(|peer| peer.0).or_else(|| {
        extensions
            .get::<ConnectInfo<SocketAddr>>()
            .map(|info| info.0)
    });
    let Some(addr) = peer else {
        return if require_peer {
            Err(Box::new(headers::text(StatusCode::FORBIDDEN, NO_PEER)))
        } else {
            Ok(())
        };
    };
    if !is_this_machine(addr.ip()) {
        return Err(Box::new(headers::text(StatusCode::FORBIDDEN, FORBIDDEN)));
    }
    if let Some(host) = request.headers().get(HOST) {
        let host = host.to_str().unwrap_or_default();
        if !host_names_this_machine(host) {
            return Err(Box::new(headers::text(StatusCode::FORBIDDEN, FORBIDDEN)));
        }
    }
    Ok(())
}

/// How long the interface list is trusted before it is read again: an address a cable
/// or a Wi-Fi network just gave this machine is the owner's within this long.
const INTERFACES_FOR: Duration = Duration::from_secs(5);

/// Whether `ip` is one of this machine's own addresses (`spec/protocol.md §8.4`):
/// loopback, or an address one of its interfaces holds. A connection with such a
/// source completed a handshake with this machine's own stack, so it came from here.
#[must_use]
pub fn is_this_machine(ip: IpAddr) -> bool {
    ip.is_loopback() || local_addresses().contains(&canonical(ip))
}

/// An IPv4-mapped IPv6 address compared as the IPv4 it maps, since a dual-stack
/// listener reports peers that way.
fn canonical(ip: IpAddr) -> IpAddr {
    match ip {
        IpAddr::V6(v6) => v6.to_ipv4_mapped().map_or(ip, IpAddr::V4),
        IpAddr::V4(_) => ip,
    }
}

/// This machine's interface addresses, read at most once per [`INTERFACES_FOR`]. An
/// enumeration that fails yields the empty set — loopback alone — rather than a stale
/// one, so an error never widens the owner's standing.
fn local_addresses() -> BTreeSet<IpAddr> {
    static CACHE: Mutex<Option<(Instant, BTreeSet<IpAddr>)>> = Mutex::new(None);
    let mut cache = CACHE.lock().unwrap_or_else(PoisonError::into_inner);
    if let Some((read_at, addresses)) = cache.as_ref()
        && read_at.elapsed() < INTERFACES_FOR
    {
        return addresses.clone();
    }
    let addresses: BTreeSet<IpAddr> = if_addrs::get_if_addrs()
        .map(|interfaces| {
            interfaces
                .into_iter()
                .map(|interface| canonical(interface.ip()))
                .filter(|ip| !ip.is_unspecified())
                .collect()
        })
        .unwrap_or_default();
    *cache = Some((Instant::now(), addresses.clone()));
    addresses
}

/// Whether an HTTP `Host` value names this machine (`spec/protocol.md §8.4`):
/// `localhost`, `*.localhost`, a loopback address, or one of this machine's own
/// interface addresses, each with an optional port. A name that resolves here but is
/// not one of these — an attacker's domain pointed at this machine — is refused.
#[must_use]
pub fn host_names_this_machine(host: &str) -> bool {
    host_is_loopback(host)
        || host_address(host).is_some_and(|ip| local_addresses().contains(&canonical(ip)))
}

/// Whether an HTTP `Host` value names this machine's loopback: `localhost`,
/// `*.localhost`, an IPv4 loopback address, or `[::1]`, each with an optional port.
#[must_use]
pub fn host_is_loopback(host: &str) -> bool {
    if let Some(ip) = host_address(host) {
        return ip.is_loopback();
    }
    let name = host_name(host).to_ascii_lowercase();
    name == "localhost" || name.ends_with(".localhost")
}

/// The name part of a `Host` value: the port stripped, brackets kept.
fn host_name(host: &str) -> &str {
    if host.starts_with('[') {
        return host;
    }
    host.rsplit_once(':').map_or(host, |(name, port)| {
        if port.bytes().all(|b| b.is_ascii_digit()) {
            name
        } else {
            host
        }
    })
}

/// The IP address a `Host` value names, when it names one rather than a hostname:
/// `192.0.2.5`, `192.0.2.5:8420`, `[::1]`, `[fd00::5]:8420`.
fn host_address(host: &str) -> Option<IpAddr> {
    if let Some(rest) = host.strip_prefix('[') {
        let end = rest.find(']')?;
        let after = &rest[end + 1..];
        if !(after.is_empty() || after.starts_with(':')) {
            return None;
        }
        return rest[..end].parse::<IpAddr>().ok();
    }
    host_name(host).parse::<IpAddr>().ok()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn loopback_hosts() {
        for ok in [
            "localhost",
            "localhost:8420",
            "LOCALHOST",
            "app.localhost:1",
            "127.0.0.1",
            "127.0.0.1:8420",
            "127.1.2.3:80",
            "[::1]",
            "[::1]:8420",
        ] {
            assert!(host_is_loopback(ok), "{ok}");
        }
        for bad in [
            "192.168.1.5:8420",
            "example.com",
            "evil.com:8420",
            "localhost.evil.com",
            "[2001:db8::1]:8420",
            "",
            "[::1",
            "[::1]x",
        ] {
            assert!(!host_is_loopback(bad), "{bad}");
        }
    }
}
