// Project:  Privatium™  |  File: crates/privatium/src/lib.rs
// Authors:  Gabriel Mongefranco (@gabrielmongefranco)
// Created:  2026-09-03  |  Modified: 2026-09-05
// Summary:  The axum adapter (ADR 0003): a socket in, core::handle out. It holds no routing
//           table, adds no route and rewrites no path — every request goes to
//           `Handler::handle` unchanged, with the peer address attached so the core's auth
//           layer can see it. A library target beside the binary so tests can reach it.
//           See main README.md for full license information.

pub mod adapter {
    //! The daemon's transport. `bind` opens all interfaces (spec/cli.md §2),
    //! with the port from config and no bind flag; `serve` runs both families.

    use std::convert::Infallible;
    use std::net::{Ipv4Addr, Ipv6Addr, SocketAddr};
    use std::sync::Arc;

    use axum::ServiceExt as _;
    use axum::extract::ConnectInfo;
    use privatium_core::{Handler, Peer, Request, Response};
    use tokio::net::TcpListener;

    /// Every IPv4 interface. Authorization belongs to the core (spec/protocol.md §8.4).
    pub const BIND_IP: Ipv4Addr = Ipv4Addr::UNSPECIFIED;

    /// The required IPv4 listener and optional IPv6 listener, both on the same port.
    pub struct Listeners {
        ipv4: TcpListener,
        ipv6: Option<TcpListener>,
    }

    impl Listeners {
        /// The IPv4 bound address; returns a socket error if the address is unavailable.
        pub fn local_addr(&self) -> std::io::Result<SocketAddr> {
            self.ipv4.local_addr()
        }
        /// The IPv6 address, or None when this platform could not bind IPv6.
        pub fn ipv6_addr(&self) -> std::io::Result<Option<SocketAddr>> {
            self.ipv6.as_ref().map(TcpListener::local_addr).transpose()
        }
    }

    /// Bind every interface. IPv4 failure is returned; IPv6 failure is reported while
    /// IPv4 remains available. Port 0 asks the OS for a free port, which tests use.
    pub async fn bind(port: u16) -> std::io::Result<Listeners> {
        let ipv4 = TcpListener::bind(SocketAddr::from((BIND_IP, port))).await?;
        let port = ipv4.local_addr()?.port();
        let ipv6 = (|| -> std::io::Result<TcpListener> {
            let socket = socket2::Socket::new(
                socket2::Domain::IPV6,
                socket2::Type::STREAM,
                Some(socket2::Protocol::TCP),
            )?;
            // A separate IPv4 listener avoids platform-dependent dual-stack defaults.
            socket.set_only_v6(true)?;
            socket.set_nonblocking(true)?;
            socket.bind(&SocketAddr::from((Ipv6Addr::UNSPECIFIED, port)).into())?;
            socket.listen(128)?;
            TcpListener::from_std(socket.into())
        })();
        let ipv6 = match ipv6 {
            Ok(listener) => Some(listener),
            Err(_) => {
                eprintln!("privatium: IPv6 could not bind; IPv4 remains available");
                None
            }
        };
        Ok(Listeners { ipv4, ipv6 })
    }

    /// Startup's default-route URL and the local browser URL (spec/cli.md §2).
    #[must_use]
    pub fn announce(addr: SocketAddr) -> String {
        let lan = SocketAddr::new(privatium_core::http::lan_address(), addr.port());
        format!(
            "privatium: listening on http://{lan}/\n\
             privatium: local browser at http://127.0.0.1:{}/\n",
            addr.port()
        )
    }

    /// Enumerate other interface URLs for verbose startup. Refuses an enumeration
    /// error rather than presenting an incomplete list as complete (spec/cli.md §2).
    pub fn other_urls(port: u16) -> std::io::Result<Vec<String>> {
        let primary = privatium_core::http::lan_address();
        let mut urls = std::collections::BTreeSet::new();
        for interface in if_addrs::get_if_addrs()? {
            let ip = interface.ip();
            if ip == primary || ip.is_loopback() || ip.is_unspecified() {
                continue;
            }
            let address = match ip {
                std::net::IpAddr::V6(ip) if ip.is_unicast_link_local() => {
                    if let Some(index) = interface.index {
                        format!("[{ip}%25{index}]:{port}")
                    } else {
                        continue;
                    }
                }
                _ => SocketAddr::new(ip, port).to_string(),
            };
            urls.insert(format!("http://{address}/"));
        }
        Ok(urls.into_iter().collect())
    }

    /// Serve until the listener fails or the task is dropped.
    ///
    /// The whole adapter is the closure below: take the connection's remote address, hand
    /// it to the core as a [`Peer`], call `handle`. No router — `axum::serve` is given a
    /// plain `tower::service_fn` — so there is nowhere for a route or a rewrite to hide.
    /// Bodies are `axum::body::Body` on both sides, which is the core's own type, so the
    /// "body conversion" ADR 0003 allows an adapter is the identity here.
    pub async fn serve(listeners: Listeners, handler: Arc<Handler>) -> std::io::Result<()> {
        if let Some(ipv6) = listeners.ipv6 {
            tokio::select! {
                result = serve_one(listeners.ipv4, handler.clone()) => result,
                result = serve_one(ipv6, handler) => result,
            }
        } else {
            serve_one(listeners.ipv4, handler).await
        }
    }

    async fn serve_one(listener: TcpListener, handler: Arc<Handler>) -> std::io::Result<()> {
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
