// Project:  Privatium™  |  File: crates/privatium-core/src/wire/channel.rs
// Authors:  Gabriel Mongefranco (@gabrielmongefranco)
// Created:  2026-09-05  |  Modified: 2026-09-05
// Summary:  Encrypted WebSocket adapter over core::handle and live pairing (§7.4, §8.3).

use std::collections::{BTreeMap, HashMap, HashSet};
use std::net::SocketAddr;
use std::time::Duration;

use axum::body::Bytes;
use axum::extract::ws::{CloseFrame, Message, WebSocket};
use axum::extract::{FromRequestParts as _, WebSocketUpgrade};
use axum::http::{HeaderName, HeaderValue, Method, StatusCode, Uri};
use futures_util::StreamExt as _;
use rusqlite::OptionalExtension as _;
use serde::{Deserialize, Serialize};
use tokio::sync::mpsc;
use tokio::task::{AbortHandle, JoinSet};

use super::{ApiSettings, Body, Handler, Peer, Request, Response};
use crate::http::{auth::Session, headers};
use crate::session::handshake::{DevicePins, Handshake};
use crate::{Node, identity::NodeId};

// Bound metadata and pending work independently of the owner's body quota. A slow
// consumer must not turn an SSE stream into an unbounded plaintext queue (§8.3).
const HEAD_LIMIT: usize = 64 * 1024;
const CHUNK: usize = 64 * 1024;
const IN_FLIGHT: usize = 64;
const IDS: usize = 65_536;
const HANDSHAKE_TIMEOUT: Duration = Duration::from_secs(10);

#[cfg(test)]
#[allow(clippy::unwrap_used)]
mod tests {
    use super::*;

    fn session() -> Session {
        let key = ed25519_dalek::SigningKey::from_bytes(&[1; 32]);
        let id = NodeId::derive(&key.verifying_key());
        Session {
            device: id.clone(),
            node: id,
            x25519: "synthetic".into(),
        }
    }

    fn request(body: &'static [u8]) -> Frame {
        let mut frame = Frame::new(1, Kind::Req);
        frame.method = Some("POST".into());
        frame.path = Some("/a/hello/name".into());
        frame.headers = Some(BTreeMap::new());
        frame.payload = Bytes::from_static(body);
        frame
    }

    #[test]
    fn test_spec_8_3_metadata_and_body_limits_include_empty_and_boundary() {
        for bytes in [b"".as_slice(), &[0, 0, 0], &[255, 255, 255, 255]] {
            assert!(Frame::decode(bytes).is_err());
        }
        let mut frame = request(b"x");
        frame.id = 9_007_199_254_740_991;
        assert_eq!(
            Frame::decode(&frame.encode().unwrap()).unwrap().id,
            frame.id
        );
        frame.id += 1;
        assert!(Frame::decode(&frame.encode().unwrap()).is_err());
        let peer = Peer("192.0.2.1:4000".parse().unwrap());
        let host = HeaderValue::from_static("192.0.2.2:8420");
        assert!(
            request(b"")
                .request(peer, host.clone(), session(), 0)
                .is_ok()
        );
        assert!(
            request(b"x")
                .request(peer, host.clone(), session(), 1)
                .is_ok()
        );
        assert!(matches!(
            request(b"xx").request(peer, host, session(), 1),
            Err(ChannelError::BodyLimit)
        ));
    }

    #[test]
    fn test_spec_8_3_request_cannot_supply_transport_authority_or_inject_headers() {
        let peer = Peer("192.0.2.1:4000".parse().unwrap());
        let host = HeaderValue::from_static("192.0.2.2:8420");
        let mut frame = request(b"");
        frame.headers = Some(
            [
                ("host", "localhost"),
                ("authorization", "synthetic"),
                ("sec-fetch-site", "same-origin"),
                ("x-forwarded-for", "127.0.0.1"),
            ]
            .into_iter()
            .map(|(k, v)| (k.into(), v.into()))
            .collect(),
        );
        let req = frame.request(peer, host.clone(), session(), 0).unwrap();
        assert_eq!(req.headers()["host"], host);
        assert!(!req.headers().contains_key("authorization"));
        assert!(!req.headers().contains_key("sec-fetch-site"));
        assert!(!req.headers().contains_key("x-forwarded-for"));
        let mut frame = request(b"");
        frame
            .headers
            .as_mut()
            .unwrap()
            .insert("x-example".into(), "value\r\nHost: localhost".into());
        assert!(frame.request(peer, host.clone(), session(), 0).is_err());
        let mut frame = request(b"");
        frame.headers = None;
        assert!(frame.request(peer, host, session(), 0).is_err());
    }
}

/// Direction-specific message kinds of spec/protocol.md §8.3.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Kind {
    /// Client request head and complete bounded body.
    Req,
    /// Node response status and headers.
    Res,
    /// Node response body bytes.
    Chunk,
    /// Node response completion.
    End,
    /// Client cancellation of a response.
    Cancel,
    /// Attach to a response retained across a document transition (§8.3.1).
    Resume,
    /// Discard a response retained across a document transition (§8.3.1).
    Release,
}

/// JSON metadata plus uninterpreted payload. Application bytes are never parsed here.
#[derive(Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Frame {
    /// Client-chosen integer, exactly representable in JavaScript, unique per connection.
    pub id: u64,
    /// Frame direction and purpose.
    pub kind: Kind,
    /// Request method; absent in every other kind.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub method: Option<String>,
    /// Origin-relative request path with query; absent otherwise.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub path: Option<String>,
    /// Response status; absent otherwise.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub status: Option<u16>,
    /// Request or response headers; absent on body and control frames.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub headers: Option<BTreeMap<String, String>>,
    /// Request a full-page response handoff; only on a non-GET, non-HEAD request.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub navigation: Option<bool>,
    /// Canonical response reference on a handoff head, resume or release frame.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub handoff: Option<String>,
    /// Bytes after the metadata, bounded before allocation by the WebSocket transport.
    #[serde(skip)]
    pub payload: Bytes,
}

/// A channel refusal containing no peer input or application bytes.
#[derive(Debug, thiserror::Error)]
pub enum ChannelError {
    /// Metadata, sequencing or direction violated §8.3.
    #[error("invalid channel frame; reconnect")]
    Format,
    /// An upload attempted the reserved request chunk direction.
    #[error("streamed request chunks require Phase 3; send one bounded request")]
    Upload,
    /// The complete request body exceeded the current setting.
    #[error("request exceeds api.max_body; send a smaller request")]
    BodyLimit,
}

impl Frame {
    /// Empty frame of a chosen kind; callers supply only that kind's required fields.
    #[must_use]
    pub fn new(id: u64, kind: Kind) -> Self {
        Self {
            id,
            kind,
            method: None,
            path: None,
            status: None,
            headers: None,
            navigation: None,
            handoff: None,
            payload: Bytes::new(),
        }
    }

    /// Encode length, JSON and raw payload (§8.3). Refuses oversized metadata.
    pub fn encode(&self) -> Result<Vec<u8>, ChannelError> {
        let head = serde_json::to_vec(self).map_err(|_| ChannelError::Format)?;
        if head.len() > HEAD_LIMIT {
            return Err(ChannelError::Format);
        }
        let mut bytes = Vec::with_capacity(4 + head.len() + self.payload.len());
        bytes.extend_from_slice(&(head.len() as u32).to_be_bytes());
        bytes.extend_from_slice(&head);
        bytes.extend_from_slice(&self.payload);
        Ok(bytes)
    }

    /// Decode bounded metadata without interpreting payload bytes. Refuses malformed
    /// fields and identifiers outside JavaScript's exact integer range.
    pub fn decode(bytes: &[u8]) -> Result<Self, ChannelError> {
        let prefix: [u8; 4] = bytes
            .get(..4)
            .ok_or(ChannelError::Format)?
            .try_into()
            .map_err(|_| ChannelError::Format)?;
        let len = u32::from_be_bytes(prefix) as usize;
        if len > HEAD_LIMIT || len > bytes.len() - 4 {
            return Err(ChannelError::Format);
        }
        let mut frame: Self =
            serde_json::from_slice(&bytes[4..4 + len]).map_err(|_| ChannelError::Format)?;
        if frame.id > 9_007_199_254_740_991 {
            return Err(ChannelError::Format);
        }
        frame.payload = Bytes::copy_from_slice(&bytes[4 + len..]);
        Ok(frame)
    }

    fn request(
        self,
        peer: Peer,
        host: HeaderValue,
        session: Session,
        limit: usize,
    ) -> Result<Request, ChannelError> {
        if self.kind == Kind::Chunk {
            return Err(ChannelError::Upload);
        }
        if self.kind != Kind::Req || self.status.is_some() || self.handoff.is_some() {
            return Err(ChannelError::Format);
        }
        if self.payload.len() > limit {
            return Err(ChannelError::BodyLimit);
        }
        let method: Method = self
            .method
            .ok_or(ChannelError::Format)?
            .parse()
            .map_err(|_| ChannelError::Format)?;
        if !matches!(
            method,
            Method::GET
                | Method::HEAD
                | Method::POST
                | Method::PUT
                | Method::PATCH
                | Method::DELETE
                | Method::OPTIONS
        ) {
            return Err(ChannelError::Format);
        }
        let path = self.path.ok_or(ChannelError::Format)?;
        if self.navigation.is_some()
            && (self.navigation != Some(true) || matches!(method, Method::GET | Method::HEAD))
        {
            return Err(ChannelError::Format);
        }
        let uri: Uri = path.parse().map_err(|_| ChannelError::Format)?;
        if !path.starts_with('/')
            || path.starts_with("//")
            || path.contains(['\\', '#'])
            || uri.authority().is_some()
            || uri.scheme().is_some()
        {
            return Err(ChannelError::Format);
        }
        let mut req = Request::new(Body::from(self.payload));
        *req.method_mut() = method;
        *req.uri_mut() = uri;
        for (name, value) in self.headers.ok_or(ChannelError::Format)? {
            let name: HeaderName = name.parse().map_err(|_| ChannelError::Format)?;
            // Connection state comes only from the verified upgrade. Fetch metadata
            // describes the outer request, never one inside an authenticated channel.
            if name == "host"
                || name.as_str().starts_with("sec-")
                || matches!(
                    name.as_str(),
                    "connection"
                        | "upgrade"
                        | "transfer-encoding"
                        | "content-length"
                        | "cookie"
                        | "authorization"
                        | "proxy-authorization"
                        | "forwarded"
                        | "x-forwarded-for"
                        | "x-forwarded-host"
                        | "x-forwarded-proto"
                )
            {
                continue;
            }
            let value = HeaderValue::from_str(&value).map_err(|_| ChannelError::Format)?;
            if req.headers_mut().insert(name, value).is_some() {
                return Err(ChannelError::Format);
            }
        }
        req.headers_mut().insert("host", host);
        req.extensions_mut().insert(peer);
        req.extensions_mut().insert(session);
        Ok(req)
    }
}

pub(super) fn active(node: &Node, session: &Session) -> bool {
    pins(node, session.device.as_str())
        .is_some_and(|p| !p.revoked && p.x25519.as_deref() == Some(&session.x25519))
}

fn registered_device(node: &Node, dev: &str) -> Option<NodeId> {
    use base64::Engine as _;
    let key: String = node
        .store()
        .conn()
        .query_row(
            "SELECT ed25519_pub FROM sys_device WHERE id = ?",
            rusqlite::params![dev],
            |r| r.get(0),
        )
        .ok()?;
    let bytes: [u8; 32] = base64::engine::general_purpose::STANDARD
        .decode(key)
        .ok()?
        .try_into()
        .ok()?;
    let id = NodeId::derive(&ed25519_dalek::VerifyingKey::from_bytes(&bytes).ok()?);
    (id.as_str() == dev).then_some(id)
}

fn pins(node: &Node, dev: &str) -> Option<DevicePins> {
    registered_device(node, dev)?;
    node.store()
        .conn()
        .query_row(
            "SELECT x25519_pub, revoked_at IS NOT NULL FROM sys_device WHERE id = ?",
            rusqlite::params![dev],
            |row| {
                Ok(DevicePins {
                    x25519: row.get(0)?,
                    revoked: row.get(1)?,
                })
            },
        )
        .optional()
        .ok()
        .flatten()
}

pub(super) async fn upgrade(handler: Handler, request: Request, pairing: bool) -> Response {
    if request.method() != Method::GET {
        return headers::method_not_allowed("GET");
    }
    if request.extensions().get::<Session>().is_some() {
        return headers::text(
            StatusCode::BAD_REQUEST,
            "a channel cannot contain another WebSocket upgrade\n",
        );
    }
    let host = match request
        .headers()
        .get("host")
        .filter(|h| h.to_str().is_ok_and(super::is_authority))
    {
        Some(host) => host.clone(),
        None => {
            return headers::text(
                StatusCode::UPGRADE_REQUIRED,
                "426 Upgrade Required — open a WebSocket with a valid Host\n",
            );
        }
    };
    if let Some(origin) = request.headers().get("origin") {
        let expected = format!("http://{}", host.to_str().unwrap_or_default());
        if origin.as_bytes() != expected.as_bytes() {
            return headers::text(
                StatusCode::FORBIDDEN,
                "403 Forbidden — WebSocket origin differs from this node\n",
            );
        }
    }
    let peer = request.extensions().get::<Peer>().copied().or_else(|| {
        request
            .extensions()
            .get::<axum::extract::ConnectInfo<SocketAddr>>()
            .map(|p| Peer(p.0))
    });
    let Some(peer) = peer else {
        return headers::text(
            StatusCode::UPGRADE_REQUIRED,
            "426 Upgrade Required — a WebSocket needs a socket transport\n",
        );
    };
    let limit = if pairing {
        8192
    } else {
        ApiSettings::read(&handler.lock())
            .max_body
            .saturating_add(HEAD_LIMIT + 20)
            .max(8192)
    };
    let (mut parts, _) = request.into_parts();
    let ws = match WebSocketUpgrade::from_request_parts(&mut parts, &()).await {
        Ok(ws) => ws,
        Err(_) => {
            return headers::text(
                StatusCode::UPGRADE_REQUIRED,
                "426 Upgrade Required — open a WebSocket at this path\n",
            );
        }
    };
    ws.max_message_size(limit)
        .max_frame_size(limit)
        .on_upgrade(move |socket| async move {
            if pairing {
                serve_ws_pair(handler, peer, socket).await;
            } else {
                serve_ws(handler, peer, host, socket).await;
            }
        })
}

async fn receive(socket: &mut WebSocket) -> Result<Message, ()> {
    loop {
        match socket.recv().await {
            Some(Ok(Message::Ping(_) | Message::Pong(_))) => continue,
            Some(Ok(Message::Close(_))) | Some(Err(_)) | None => return Err(()),
            Some(Ok(message)) => return Ok(message),
        }
    }
}

async fn close(socket: &mut WebSocket, code: u16, reason: &str) {
    let end = reason
        .char_indices()
        .map(|(i, _)| i)
        .take_while(|i| *i <= 120)
        .last()
        .unwrap_or(0);
    let reason = if reason.len() > 120 {
        &reason[..end]
    } else {
        reason
    };
    let _ = tokio::time::timeout(
        Duration::from_secs(1),
        socket.send(Message::Close(Some(CloseFrame {
            code,
            reason: reason.to_owned().into(),
        }))),
    )
    .await;
}

/// Drive a node's session handshake, then dispatch concurrently through `handle`.
/// Every error closes the socket and drops all pending responses and encryption keys.
pub async fn serve_ws(handler: Handler, peer: Peer, host: HeaderValue, mut socket: WebSocket) {
    let handshake = async {
        let Message::Text(hello) = receive(&mut socket).await? else {
            return Err(());
        };
        let mut authenticated_key = None;
        let (pending, reply, node_id) = {
            let mut node = handler.lock();
            node.refresh().map_err(|_| ())?;
            let (pending, reply) = Handshake::node(
                node.identity(),
                |dev| {
                    let found = pins(&node, dev)?;
                    authenticated_key = found.x25519.clone();
                    Some(found)
                },
                &hello,
            )
            .map_err(|_| ())?;
            (pending, reply, node.id().clone())
        };
        socket
            .send(Message::Text(reply.into()))
            .await
            .map_err(|_| ())?;
        let Message::Binary(confirm) = receive(&mut socket).await? else {
            return Err(());
        };
        let crypto = pending.confirm(&confirm).map_err(|_| ())?;
        let device = registered_device(&handler.lock(), &crypto.device).ok_or(())?;
        let session = Session {
            device,
            node: node_id,
            x25519: authenticated_key.ok_or(())?,
        };
        if !active(&handler.lock(), &session) {
            return Err(());
        }
        Ok((crypto, session))
    };
    let Ok(Ok((mut crypto, session))) = tokio::time::timeout(HANDSHAKE_TIMEOUT, handshake).await
    else {
        close(&mut socket, 4403, "session refused; pair this device again").await;
        return;
    };
    let (tx, mut rx) = mpsc::channel::<Frame>(8);
    let mut jobs = JoinSet::new();
    let mut running: HashMap<u64, AbortHandle> = HashMap::new();
    let mut seen = HashSet::new();
    let outcome: Result<(), ChannelError> = async {
        loop {
            tokio::select! {
                message = receive(&mut socket) => {
                    let Message::Binary(bytes) = message.map_err(|_| ChannelError::Format)? else { return Err(ChannelError::Format); };
                    let plaintext = crypto.receive.open(&bytes).map_err(|_| ChannelError::Format)?;
                    let frame = Frame::decode(&plaintext)?;
                    if frame.kind == Kind::Cancel {
                        if !frame.payload.is_empty() || frame.method.is_some() || frame.path.is_some() || frame.status.is_some() || frame.headers.is_some() || frame.navigation.is_some() || frame.handoff.is_some() || !seen.contains(&frame.id) { return Err(ChannelError::Format); }
                        if let Some(job) = running.remove(&frame.id) { job.abort(); }
                    } else {
                        let id = frame.id;
                        if running.len() >= IN_FLIGHT || seen.len() >= IDS || !seen.insert(id) { return Err(ChannelError::Format); }
                        let h = handler.clone(); let sender = tx.clone();
                        if matches!(frame.kind, Kind::Resume | Kind::Release) {
                            if !frame.payload.is_empty() || frame.method.is_some() || frame.path.is_some() || frame.status.is_some() || frame.headers.is_some() || frame.navigation.is_some() { return Err(ChannelError::Format); }
                            let reference = frame.handoff.ok_or(ChannelError::Format)?;
                            let authorized = { let mut node = handler.lock(); node.refresh().is_ok() && active(&node, &session) };
                            if !authorized { return Err(ChannelError::Format); }
                            let response = match h.handoffs.take(&session, &reference) {
                                Some(response) if frame.kind == Kind::Resume => response,
                                Some(_) => headers::text(StatusCode::NO_CONTENT, ""),
                                None => headers::text(StatusCode::CONFLICT, "The previous response is unavailable. The operation may have completed; check before submitting again.\n"),
                            };
                            running.insert(id, jobs.spawn(async move { send_response(response, id, sender).await }));
                        } else {
                            let navigation = frame.navigation == Some(true);
                            let limit = ApiSettings::read(&handler.lock()).max_body;
                            let request = frame.request(peer, host.clone(), session.clone(), limit)?;
                            let slot = if navigation { h.handoffs.reserve(&session) } else { None };
                            running.insert(id, jobs.spawn(async move {
                                if navigation && slot.is_none() {
                                    return send_response(headers::text(StatusCode::TOO_MANY_REQUESTS, "Too many pending page responses; finish another navigation before submitting.\n"), id, sender).await;
                                }
                                respond(h, request, id, sender, slot).await
                            }));
                        }
                    }
                    if crypto.receive.is_closed() { return Ok(()); }
                }
                Some(frame) = rx.recv() => {
                    if !running.contains_key(&frame.id) { continue; }
                    if !active(&handler.lock(), &session) { return Err(ChannelError::Format); }
                    let ciphertext = crypto.send.seal(&frame.encode()?).map_err(|_| ChannelError::Format)?;
                    socket.send(Message::Binary(ciphertext.into())).await.map_err(|_| ChannelError::Format)?;
                    if frame.kind == Kind::End { running.remove(&frame.id); }
                    if crypto.send.is_closed() { return Ok(()); }
                }
                Some(result) = jobs.join_next(), if !jobs.is_empty() => {
                    match result { Ok(Ok(())) => {}, Err(e) if e.is_cancelled() => {}, _ => return Err(ChannelError::Format) }
                }
            }
        }
    }.await;
    jobs.abort_all();
    close(
        &mut socket,
        if outcome.is_ok() { 1000 } else { 4400 },
        &outcome
            .err()
            .map_or_else(|| "session ended; reconnect".to_owned(), |e| e.to_string()),
    )
    .await;
}

async fn respond(
    handler: Handler,
    request: Request,
    id: u64,
    tx: mpsc::Sender<Frame>,
    slot: Option<super::handoff::Reservation>,
) -> Result<(), ()> {
    let response = Box::pin(handler.handle(request)).await;
    if let Some(slot) = slot
        && !response.status().is_redirection()
    {
        let mut head = response_head(&response, id)?;
        head.handoff = Some(slot.publish(response)?);
        tx.send(head).await.map_err(|_| ())?;
        return tx.send(Frame::new(id, Kind::End)).await.map_err(|_| ());
    }
    send_response(response, id, tx).await
}

fn response_head(response: &Response, id: u64) -> Result<Frame, ()> {
    let mut head = Frame::new(id, Kind::Res);
    head.status = Some(response.status().as_u16());
    head.headers = Some(
        response
            .headers()
            .iter()
            .map(|(k, v)| v.to_str().map(|v| (k.to_string(), v.to_owned())))
            .collect::<Result<_, _>>()
            .map_err(|_| ())?,
    );
    Ok(head)
}

async fn send_response(response: Response, id: u64, tx: mpsc::Sender<Frame>) -> Result<(), ()> {
    tx.send(response_head(&response, id)?)
        .await
        .map_err(|_| ())?;
    let mut stream = response.into_body().into_data_stream();
    while let Some(bytes) = stream.next().await {
        let bytes = bytes.map_err(|_| ())?;
        for chunk in bytes.chunks(CHUNK) {
            let mut frame = Frame::new(id, Kind::Chunk);
            frame.payload = Bytes::copy_from_slice(chunk);
            tx.send(frame).await.map_err(|_| ())?;
        }
    }
    tx.send(Frame::new(id, Kind::End)).await.map_err(|_| ())
}

/// Drive the six pairing messages with one bounded lifetime. A counted attempt that
/// disconnects or times out is audited through the node, including task cancellation.
pub async fn serve_ws_pair(handler: Handler, peer: Peer, mut socket: WebSocket) {
    struct Attempt {
        handler: Handler,
        peer: Peer,
        device: Option<String>,
    }
    impl Drop for Attempt {
        fn drop(&mut self) {
            if let Some(device) = self.device.take()
                && self
                    .handler
                    .lock()
                    .pairing_abandon(&device, self.peer.0.ip())
                    .is_err()
            {
                eprintln!("privatium: could not record an abandoned pairing attempt");
            }
        }
    }
    let mut attempt = Attempt {
        handler: handler.clone(),
        peer,
        device: None,
    };
    let run = async {
        let now = jiff::Timestamp::now();
        let hello = handler.lock().pairing_hello(now);
        socket
            .send(Message::Text(hello.into()))
            .await
            .map_err(|_| crate::pair::PairError::Format)?;
        if !handler.lock().pairing_open(now) {
            return Err(crate::pair::PairError::Closed.into());
        }
        let Message::Text(start) = receive(&mut socket)
            .await
            .map_err(|_| crate::pair::PairError::Format)?
        else {
            return Err(crate::pair::PairError::Format.into());
        };
        let (exchange, reply) =
            handler
                .lock()
                .pairing_begin(peer.0.ip(), jiff::Timestamp::now(), &start)?;
        attempt.device = Some(exchange.device().to_owned());
        socket
            .send(Message::Text(reply.into()))
            .await
            .map_err(|_| crate::pair::PairError::Format)?;
        let Message::Text(confirm) = receive(&mut socket)
            .await
            .map_err(|_| crate::pair::PairError::Format)?
        else {
            return Err(crate::pair::PairError::Format.into());
        };
        let confirmed = handler.lock().pairing_confirm(exchange, &confirm);
        if matches!(
            &confirmed,
            Err(crate::Error::Pair(crate::pair::PairError::WrongCode))
        ) {
            attempt.device = None;
        }
        let (sealed, bytes) = confirmed?;
        socket
            .send(Message::Binary(bytes.into()))
            .await
            .map_err(|_| crate::pair::PairError::Format)?;
        let Message::Binary(bytes) = receive(&mut socket)
            .await
            .map_err(|_| crate::pair::PairError::Format)?
        else {
            return Err(crate::pair::PairError::Format.into());
        };
        let outcome = handler
            .lock()
            .pairing_finish(sealed, &bytes, jiff::Timestamp::now());
        if outcome.is_ok()
            || matches!(
                &outcome,
                Err(crate::Error::Pair(crate::pair::PairError::DeviceKnown))
            )
        {
            attempt.device = None;
        }
        outcome.map(|_| ())
    };
    let result: Result<crate::Result<()>, _> = tokio::time::timeout(crate::pair::TTL, run).await;
    let (code, reason) = match result {
        Ok(Ok(())) => (1000, "paired".to_owned()),
        Ok(Err(crate::Error::Pair(e))) => (e.close_code(), e.to_string()),
        _ => (
            4400,
            "pairing could not finish; open pairing on the node and try again".to_owned(),
        ),
    };
    close(&mut socket, code, &reason).await;
}
