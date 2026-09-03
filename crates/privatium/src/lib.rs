// Project:  Privatium™  |  File: crates/privatium/src/lib.rs
// Authors:  Gabriel Mongefranco (@gabrielmongefranco)
// Created:  2026-09-03  |  Modified: 2026-09-03
// Summary:  The axum adapter (ADR 0003): a loopback socket in, core::handle out. It holds no
//           routing table, adds no route and rewrites no path — every request goes to
//           `Handler::handle` unchanged, with the peer address attached so the core's auth
//           layer can see it. A library target beside the binary so tests can reach it.

pub mod adapter {
    //! The daemon's transport. `bind` opens the listener of `docs/plans/phase-1.md §2.1` —
    //! `127.0.0.1`, port from config, no `--bind` — and `serve` runs it.

    use std::convert::Infallible;
    use std::net::{Ipv4Addr, SocketAddr};
    use std::sync::Arc;

    use axum::ServiceExt as _;
    use axum::extract::ConnectInfo;
    use privatium_core::{Handler, Peer, Request, Response};
    use tokio::net::TcpListener;

    /// The one address Phase 1 listens on. Not configurable, by decision
    /// (`docs/plans/phase-1.md §2.1`): binding the LAN is what pairing makes safe, and
    /// pairing is Phase 2.
    pub const BIND_IP: Ipv4Addr = Ipv4Addr::LOCALHOST;

    /// Bind `127.0.0.1:<port>`. Port 0 asks the OS for a free one, which tests use.
    pub async fn bind(port: u16) -> std::io::Result<TcpListener> {
        TcpListener::bind(SocketAddr::from((BIND_IP, port))).await
    }

    /// What startup prints: the loopback URL, and the one line `§2.1` asks for so a reader
    /// of `spec/cli.md §2` is not left wondering where the LAN URL went.
    #[must_use]
    pub fn announce(addr: SocketAddr) -> String {
        format!(
            "privatium: listening on http://{addr}/\n\
             privatium: loopback only — LAN access arrives with pairing (spec/protocol.md §7)\n"
        )
    }

    /// Serve until the listener fails or the task is dropped.
    ///
    /// The whole adapter is the closure below: take the connection's remote address, hand
    /// it to the core as a [`Peer`], call `handle`. No router — `axum::serve` is given a
    /// plain `tower::service_fn` — so there is nowhere for a route or a rewrite to hide.
    /// Bodies are `axum::body::Body` on both sides, which is the core's own type, so the
    /// "body conversion" ADR 0003 allows an adapter is the identity here.
    pub async fn serve(listener: TcpListener, handler: Arc<Handler>) -> std::io::Result<()> {
        let service = tower::service_fn(move |mut request: Request| {
            let handler = Arc::clone(&handler);
            async move {
                if let Some(ConnectInfo(addr)) = request
                    .extensions()
                    .get::<ConnectInfo<SocketAddr>>()
                    .copied()
                {
                    request.extensions_mut().insert(Peer(addr));
                }
                Ok::<Response, Infallible>(handler.handle(request).await)
            }
        });
        axum::serve(
            listener,
            service.into_make_service_with_connect_info::<SocketAddr>(),
        )
        .await
    }
}
