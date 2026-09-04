// Project:  Privatium™  |  File: crates/privatium-core/src/http/auth.rs
// Authors:  Gabriel Mongefranco (@gabrielmongefranco)
// Created:  2026-09-03  |  Modified: 2026-09-05
// Summary:  auth_layer (spec/app-contract.md §6) with its real signature — a tower::Layer —
//           and its Phase 1 body (docs/plans/phase-1.md §2.2): a loopback caller is this
//           node's own device row, anything else is 403. core::handle applies it itself, so
//           every adapter gets it; an embedder wraps their own router with it (§2.3), and
//           the peer is read from axum's ConnectInfo there, since that is what an axum
//           router has (M13).

use std::future::Future;
use std::net::{IpAddr, SocketAddr};
use std::pin::Pin;
use std::task::{Context, Poll};

use axum::extract::ConnectInfo;
use axum::http::StatusCode;
use axum::http::header::HOST;
use tower::{Layer, Service};

use crate::http::headers;
use crate::identity::NodeId;
use crate::wire::{Request, Response};

/// The remote address of the connection a request arrived on, when the transport has one.
///
/// A socket adapter inserts this as a request extension; an in-process adapter — a Tier 3
/// embedder calling `handle` directly, a native shell's custom scheme — has no peer and
/// inserts nothing, which the layer reads as "the caller is this process".
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Peer(pub SocketAddr);

/// Who the request is from, as the layer established it — inserted as a request extension
/// for everything downstream. In Phase 1 this is always this node's own `sys_device` row
/// (`docs/plans/phase-1.md §2.2`: the node is the device).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Device(pub NodeId);

/// `node.auth_layer()`.
#[derive(Debug, Clone)]
pub struct AuthLayer {
    device: NodeId,
}

impl AuthLayer {
    /// The layer for the node whose device row `device` is.
    #[must_use]
    pub fn new(device: NodeId) -> Self {
        Self { device }
    }
}

impl<S> Layer<S> for AuthLayer {
    type Service = AuthService<S>;

    fn layer(&self, inner: S) -> Self::Service {
        AuthService {
            inner,
            device: self.device.clone(),
        }
    }
}

/// The service [`AuthLayer`] wraps around whatever serves the routes.
#[derive(Debug, Clone)]
pub struct AuthService<S> {
    inner: S,
    device: NodeId,
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
        match check(&request) {
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

/// What a refused caller reads. It names the phase, not the node.
const FORBIDDEN: &str = "403 Forbidden — this build serves loopback only; LAN access arrives \
                         with pairing (spec/protocol.md §7).\n";

/// Phase 1's whole policy.
///
/// The peer is the [`Peer`] the framework's adapter inserts or, on an embedder's own
/// axum router, the `ConnectInfo<SocketAddr>` that `into_make_service_with_connect_info`
/// attaches (`spec/app-contract.md §2.3`). A request with neither came from inside the
/// process and is allowed. One with a peer is allowed only from a loopback address — and
/// only with a `Host` header naming loopback, because a browser resolving an attacker's
/// name to `127.0.0.1` (DNS rebinding) still connects from loopback, and the `Host` it
/// sends is the one thing that gives the game away. An absent `Host` is allowed: HTTP/1.0
/// clients and custom schemes send none.
fn check(request: &Request) -> Result<(), Box<Response>> {
    let extensions = request.extensions();
    let peer = extensions.get::<Peer>().map(|peer| peer.0).or_else(|| {
        extensions
            .get::<ConnectInfo<SocketAddr>>()
            .map(|info| info.0)
    });
    let Some(addr) = peer else {
        return Ok(());
    };
    if !addr.ip().is_loopback() {
        return Err(Box::new(headers::text(StatusCode::FORBIDDEN, FORBIDDEN)));
    }
    if let Some(host) = request.headers().get(HOST) {
        let host = host.to_str().unwrap_or_default();
        if !host_is_loopback(host) {
            return Err(Box::new(headers::text(StatusCode::FORBIDDEN, FORBIDDEN)));
        }
    }
    Ok(())
}

/// Whether an HTTP `Host` value names this machine: `localhost`, `*.localhost`, an IPv4
/// loopback address, or `[::1]`, each with an optional port.
#[must_use]
pub fn host_is_loopback(host: &str) -> bool {
    let name = if let Some(rest) = host.strip_prefix('[') {
        // `[::1]` or `[::1]:8420`
        let Some(end) = rest.find(']') else {
            return false;
        };
        let after = &rest[end + 1..];
        if !(after.is_empty() || after.starts_with(':')) {
            return false;
        }
        return rest[..end]
            .parse::<IpAddr>()
            .is_ok_and(|ip| ip.is_loopback());
    } else {
        host.rsplit_once(':').map_or(host, |(name, port)| {
            if port.bytes().all(|b| b.is_ascii_digit()) {
                name
            } else {
                host
            }
        })
    };
    let name = name.to_ascii_lowercase();
    if name == "localhost" || name.ends_with(".localhost") {
        return true;
    }
    name.parse::<IpAddr>().is_ok_and(|ip| ip.is_loopback())
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
