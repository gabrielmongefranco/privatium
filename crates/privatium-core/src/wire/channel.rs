// This file is part of Privatium
// crates/privatium-core/src/wire/channel.rs
// Author(s): Gabriel Mongefranco
// Created: 2026-09-05
// Last Modified: 2026-09-07
// Summary: Encrypted WebSocket adapter over core::handle and live pairing (§7.4, §8.3), including a
//          node's admission in either direction (§2.3.1) and the refusal an expired or revoked
//          node gives every channel.
// Notes: See README file for documentation and full license information.
//
// Copyright © 2026 Gabriel Mongefranco
//
// This program is free software: you can redistribute it and/or modify
// it under the terms of the GNU General Public License as published by
// the Free Software Foundation, either version 3 of the License, or (at your option) any later version.
//
// This program is distributed in the hope that it will be useful,
// but WITHOUT ANY WARRANTY; without even the implied warranty of
// MERCHANTABILITY or FITNESS FOR A PARTICULAR PURPOSE. See the
// GNU General Public License for more details.
//
// You should have received a copy of the GNU General Public License along
// with this program. If not, see <https://www.gnu.org/licenses/>.

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
use tokio::sync::{broadcast, mpsc};
use tokio::task::{AbortHandle, JoinSet};

use super::{ApiSettings, Body, Handler, Peer, Request, Response};
use crate::http::{auth::Session, headers};
use crate::pair::PairOutcome;
use crate::session::handshake::{DevicePins, Handshake};
use crate::{Node, identity::NodeId};

// Bound metadata and pending work independently of the owner's body quota. A slow
// consumer must not turn an SSE stream into an unbounded plaintext queue (§8.3).
const HEAD_LIMIT: usize = 64 * 1024;
const CHUNK: usize = 64 * 1024;
const IN_FLIGHT: usize = 64;
const IDS: usize = 65_536;

/// How long a peer has to complete a handshake before its socket is closed with nothing
/// counted (`spec/protocol.md §7.4.2`, `§8.3`). Both exchanges are machine-paced — the
/// person has already typed the code when `/ws/pair` opens — so a peer that is silent
/// this long is holding a task, not thinking. Shortened by a test; an embedder on a slow
/// link may lengthen them.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ChannelTimeouts {
    /// The two text frames and the confirm of `/ws`.
    pub handshake: Duration,
    /// The six messages of `/ws/pair`, from the node's hello to the device's sealed
    /// message.
    pub pairing: Duration,
}

impl Default for ChannelTimeouts {
    fn default() -> Self {
        Self {
            handshake: Duration::from_secs(10),
            pairing: Duration::from_secs(30),
        }
    }
}

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
            kind: crate::http::auth::SessionKind::Device,
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
    #[error("streamed request bodies are not accepted on this channel; send one bounded request")]
    Upload,
    /// The complete request body exceeded the current setting.
    #[error("request exceeds api.max_body; send a smaller request")]
    BodyLimit,
    /// The owner revoked this device while the channel was open
    /// (`spec/data-dictionary.md §3.2`).
    #[error("this device was revoked; pair it again")]
    Revoked,
}

impl ChannelError {
    /// The WebSocket close code: `4403` for a revoked device, as the handshake refuses
    /// one (`spec/protocol.md §8.3`), and `4400` for a frame the channel could not use.
    #[must_use]
    pub fn close_code(&self) -> u16 {
        match self {
            Self::Revoked => 4403,
            Self::Format | Self::Upload | Self::BodyLimit => 4400,
        }
    }
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

/// Recheck a channel's original pins and kind against current local membership.
pub(crate) fn active(node: &Node, session: &Session) -> bool {
    node.standing(jiff::Timestamp::now())
        .is_ok_and(|s| s == crate::Standing::Member)
        && peer_pins(node, session.device.as_str()).is_some_and(|(_, p, kind)| {
            !p.revoked && p.x25519.as_deref() == Some(&session.x25519) && kind == session.kind
        })
}

fn registered_device(node: &Node, dev: &str) -> Option<NodeId> {
    peer_pins(node, dev).map(|(id, _, _)| id)
}

/// Registry rows supersede bootstrap hints even when their keys or kind are invalid.
pub(crate) fn peer_pins(
    node: &Node,
    dev: &str,
) -> Option<(NodeId, DevicePins, crate::http::auth::SessionKind)> {
    use crate::http::auth::SessionKind;
    use base64::Engine as _;
    if dev == node.id().as_str() || !NodeId::is_valid(dev) {
        return None;
    }
    let row = node
        .store()
        .conn()
        .query_row(
            "SELECT ed25519_pub, x25519_pub, kind, revoked_at IS NOT NULL FROM sys_device WHERE id = ?",
            rusqlite::params![dev],
            |r| {
                Ok((
                    r.get::<_, Option<String>>(0)?,
                    r.get::<_, Option<String>>(1)?,
                    r.get::<_, String>(2)?,
                    r.get::<_, bool>(3)?,
                ))
            },
        )
        .optional().ok()?;
    let Some((key, x25519, kind, revoked)) = row else {
        let hint = node.peer_hints().into_iter().find(|hint| hint.id == dev)?;
        return Some((
            NodeId::from_peer(dev)?,
            DevicePins {
                x25519: Some(hint.x25519_pub),
                revoked: node.node_revoked(dev).unwrap_or(true),
            },
            SessionKind::Node,
        ));
    };
    let kind = match kind.as_str() {
        "node" => SessionKind::Node,
        "browser" | "desktop" | "mobile" => SessionKind::Device,
        _ => return None,
    };
    let bytes: [u8; 32] = base64::engine::general_purpose::STANDARD
        .decode(key?)
        .ok()?
        .try_into()
        .ok()?;
    let id = NodeId::derive(&ed25519_dalek::VerifyingKey::from_bytes(&bytes).ok()?);
    (id.as_str() == dev).then_some((
        id,
        DevicePins {
            x25519,
            revoked: revoked || node.node_revoked(dev).unwrap_or(true),
        },
        kind,
    ))
}

fn pins(node: &Node, dev: &str) -> Option<DevicePins> {
    peer_pins(node, dev).map(|(_, pins, _)| pins)
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
    // An expired or revoked node presents no certificate and answers nobody on `/ws`
    // (spec/protocol.md §2.3.1, §2.3.4): refused before the hello is read.
    let member = handler
        .lock()
        .standing(jiff::Timestamp::now())
        .is_ok_and(|standing| standing == crate::Standing::Member);
    if !member {
        close(
            &mut socket,
            4403,
            "this node serves its owner alone until it is re-admitted",
        )
        .await;
        return;
    }
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
            kind: peer_pins(&handler.lock(), &crypto.device).ok_or(())?.2,
            device,
            node: node_id,
            x25519: authenticated_key.ok_or(())?,
        };
        if !active(&handler.lock(), &session) {
            return Err(());
        }
        Ok((crypto, session))
    };
    let Ok(Ok((mut crypto, session))) =
        tokio::time::timeout(handler.timeouts.handshake, handshake).await
    else {
        close(&mut socket, 4403, "session refused; pair this device again").await;
        return;
    };
    // `last_seen_at` at the handshake, and again on a request once an hour has passed
    // (`spec/data-dictionary.md §3.2`); the node decides whether a write is due.
    device_seen(&handler, &session);
    let mut revoked = handler.revoked.subscribe();
    let (tx, mut rx) = mpsc::channel::<Frame>(8);
    let mut jobs = JoinSet::new();
    let mut running: HashMap<u64, AbortHandle> = HashMap::new();
    let mut seen = HashSet::new();
    let outcome: Result<(), ChannelError> = async {
        loop {
            tokio::select! {
                notice = revoked.recv() => {
                    // A lagged receiver missed some IDs; the registry says whether this
                    // device was among them.
                    let mine = match notice {
                        Ok(device) => device == session.device.as_str(),
                        Err(broadcast::error::RecvError::Lagged(_)) => !active(&handler.lock(), &session),
                        Err(broadcast::error::RecvError::Closed) => false,
                    };
                    if mine { return Err(ChannelError::Revoked); }
                }
                message = receive(&mut socket) => {
                    let Message::Binary(bytes) = message.map_err(|_| ChannelError::Format)? else { return Err(ChannelError::Format); };
                    let plaintext = crypto.receive.open(&bytes).map_err(|_| ChannelError::Format)?;
                    let frame = Frame::decode(&plaintext)?;
                    if frame.kind == Kind::Req { device_seen(&handler, &session); }
                    if frame.kind == Kind::Cancel {
                        if !frame.payload.is_empty() || frame.method.is_some() || frame.path.is_some() || frame.status.is_some() || frame.headers.is_some() || frame.navigation.is_some() || frame.handoff.is_some() || !seen.contains(&frame.id) { return Err(ChannelError::Format); }
                        if let Some(job) = running.remove(&frame.id) { job.abort(); }
                    } else {
                        let id = frame.id;
                        if running.len() >= IN_FLIGHT || seen.len() >= IDS || !seen.insert(id) { return Err(ChannelError::Format); }
                        let h = handler.clone(); let sender = tx.clone();
                        if matches!(frame.kind, Kind::Resume | Kind::Release) {
                            if session.is_node() { return Err(ChannelError::Format); }
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
                            // A request too large is the client's mistake, not a broken
                            // channel: it is answered so the page can say so, where a
                            // malformed frame closes the connection (§8.3).
                            let request = match frame.request(peer, host.clone(), session.clone(), limit) {
                                Ok(request) => request,
                                Err(ChannelError::BodyLimit) => {
                                    running.insert(id, jobs.spawn(async move {
                                        send_response(headers::text(StatusCode::PAYLOAD_TOO_LARGE,
                                            "413 Payload Too Large — request exceeds api.max_body; send a smaller request\n"), id, sender).await
                                    }));
                                    continue;
                                }
                                Err(error) => return Err(error),
                            };
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
    let (code, reason) = match &outcome {
        Ok(()) => (1000, "session ended; reconnect".to_owned()),
        Err(error) => (error.close_code(), error.to_string()),
    };
    close(&mut socket, code, &reason).await;
}

/// Mark the session's device seen now; the node writes `last_seen_at` only when an hour
/// has passed (`spec/data-dictionary.md §3.2`). A failed write is the node's own
/// trouble, reported and never a reason to drop the channel.
fn device_seen(handler: &Handler, session: &Session) {
    if let Err(error) = handler
        .lock()
        .note_device_seen(session.device.as_str(), jiff::Timestamp::now())
    {
        eprintln!("privatium: could not record last_seen_at: {error}");
    }
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

/// Drive the pairing messages with one bounded lifetime — six for a device, eight for a
/// node, in whichever direction §2.3.1 decided. A counted attempt whose peer disconnects
/// or falls silent is audited through the node as abandoned, including on task
/// cancellation; an outcome the node itself audited — a wrong code, a replaced code, a
/// closed window, a refused registration, a success — is not audited twice.
pub async fn serve_ws_pair(handler: Handler, peer: Peer, mut socket: WebSocket) {
    struct Attempt {
        handler: Handler,
        peer: Peer,
        /// The device of a counted attempt the node has not yet audited an outcome for.
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
        let confirmed = handler
            .lock()
            .pairing_confirm(exchange, &confirm, jiff::Timestamp::now());
        if confirmed.is_err() {
            // The node audited the refusal, whichever it was.
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
        // Every outcome of a registration is audited by the node itself. A node's
        // admission keeps the guard until its row is written or its adoption done, so
        // a peer that leaves between its sealed message and `joined` is audited once.
        let outcome = match outcome {
            Ok(outcome) => outcome,
            Err(error) => {
                attempt.device = None;
                return Err(error);
            }
        };
        match outcome {
            PairOutcome::Device(_) => {
                attempt.device = None;
                Ok("paired")
            }
            PairOutcome::Admit(admission) => {
                let (pending, bytes) = handler
                    .lock()
                    .pairing_admit(admission, jiff::Timestamp::now())?;
                socket
                    .send(Message::Binary(bytes.into()))
                    .await
                    .map_err(|_| crate::pair::PairError::Format)?;
                let Message::Binary(joined) = receive(&mut socket)
                    .await
                    .map_err(|_| crate::pair::PairError::Format)?
                else {
                    return Err(crate::pair::PairError::Format.into());
                };
                let written =
                    handler
                        .lock()
                        .pairing_admitted(pending, &joined, jiff::Timestamp::now());
                attempt.device = None;
                written.map(|_| "admitted")
            }
            PairOutcome::Join(admission) => {
                let Message::Binary(admit) = receive(&mut socket)
                    .await
                    .map_err(|_| crate::pair::PairError::Format)?
                else {
                    return Err(crate::pair::PairError::Format.into());
                };
                let adopted =
                    handler
                        .lock()
                        .pairing_adopt(admission, &admit, jiff::Timestamp::now());
                attempt.device = None;
                let (_, bytes) = adopted?;
                socket
                    .send(Message::Binary(bytes.into()))
                    .await
                    .map_err(|_| crate::pair::PairError::Format)?;
                Ok("joined")
            }
        }
    };
    let result: Result<crate::Result<&str>, _> =
        tokio::time::timeout(handler.timeouts.pairing, run).await;
    let (code, reason) = match result {
        Ok(Ok(outcome)) => (1000, outcome.to_owned()),
        Ok(Err(crate::Error::Pair(e))) => (e.close_code(), e.to_string()),
        Ok(Err(error @ (crate::Error::CertificateExpired | crate::Error::NodeRevoked))) => {
            (4403, error.to_string())
        }
        _ => (
            4400,
            "pairing could not finish; open pairing on the node and try again".to_owned(),
        ),
    };
    close(&mut socket, code, &reason).await;
}
