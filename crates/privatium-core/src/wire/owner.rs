// Project:  Privatium™  |  File: crates/privatium-core/src/wire/owner.rs
// Authors:  Gabriel Mongefranco (@gabrielmongefranco)
// Created:  2026-09-06  |  Modified: 2026-09-07
// Summary:  What only the owner may do (spec/protocol.md §9.2, §7.1, §2.3.1): open and
//           close a pairing window — for devices or for a node — over /api/v1/pair and
//           from the devices page, join another node's cluster over /api/v1/join and from
//           the node page, name the node, label and revoke a device or a node. The owner
//           is the node's own standing — a request from this machine or an in-process
//           call — and a channel session is refused whatever its device: physical
//           presence is the authorization. Every form carries csrf(); every answer to a
//           refusal names the problem and nothing else.
//           See main README.md for full license information.

use std::collections::BTreeMap;
use std::time::Duration;

use axum::body::to_bytes;
use axum::http::header::{CONTENT_LENGTH, CONTENT_TYPE};
use axum::http::{Method, StatusCode};
use serde::Deserialize;

use super::{Handler, Request, Response, SettingsPage};
use crate::http::auth::Session;
use crate::http::shell::{self, Context, Notice, NoticeKind};
use crate::http::{self, devices, headers};
use crate::pair::join::{self, JoinClient, JoinNode, JoinOutcome};
use crate::pair::{Code, Joined};
use crate::{Error, pair};

/// What a caller with a session reads: the act needs the owner at the node.
const OWNER_ONLY: &str = "403 Forbidden — only the owner at the node can do this; open the \
                          settings on the node itself (spec/protocol.md §9.2).\n";

/// The most a `POST /api/v1/pair` or `/api/v1/join` body may carry: a short object and
/// whitespace.
const PAIR_BODY_LIMIT: usize = 1024;

/// `POST /api/v1/join`'s body (`spec/protocol.md §9.2`). Every other key is refused.
#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct JoinRequest {
    /// `http://host:port`, as the other node printed it.
    url: String,
    /// The code the other node shows: the two words, or the four glyph labels.
    code: String,
}

/// What `POST /api/v1/join` says when the body is not its shape.
const JOIN_SHAPE: &str = "400 Bad Request: the body is {\"url\": \"http://host:port\", \"code\": \
                          \"two words\"} and nothing else\n";

/// A form's body past `http::FORM_LIMIT`.
const FORM_TOO_LARGE: &str = "413 Payload Too Large: a form here carries a token and a short \
                              value and nothing else\n";

/// A form without its token.
const FORM_REFUSED: &str = "403 Forbidden: the form token is missing, stale, or for another \
                            form — reload the page and try again\n";

/// The owner's acts behind `csrf()` under `/settings`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) enum OwnerAction {
    /// `POST /settings/name`.
    NodeName,
    /// `POST /settings/devices/pair`.
    PairOpen,
    /// `POST /settings/devices/admit` — a window for a node (`spec/protocol.md §7.1`).
    AdmitNode,
    /// `POST /settings/join` — join another node's cluster (`§2.3.1`).
    Join,
    /// `POST /settings/devices/pairing/close`.
    PairClose,
    /// `POST /settings/devices/<id>/label`.
    DeviceLabel(String),
    /// `POST /settings/devices/<id>/revoke`.
    DeviceRevoke(String),
}

/// `POST /api/v1/pair`'s body (`spec/protocol.md §9.2`). Every other key is refused.
#[derive(Debug, Default, Deserialize)]
#[serde(deny_unknown_fields)]
struct PairRequest {
    /// Whether the window is for a node rather than for devices (`§7.1`).
    node: Option<bool>,
    /// Seconds; the node clamps to `spec/protocol.md §7.5`'s 120.
    ttl: Option<u64>,
}

impl Handler {
    /// Whether the request has the node's own standing: no channel session. The auth
    /// layer has already refused a non-loopback peer without one (`§8.4`).
    pub(super) fn is_owner(request: &Request) -> bool {
        request.extensions().get::<Session>().is_none()
    }

    /// `GET` and `POST /api/v1/pair` (`spec/protocol.md §9.2`): the open window as
    /// `PairingSnapshot` JSON, or `null`; a POST opens one for `ttl` seconds. A POST is
    /// read only as `application/json`, so no cross-origin page can send it without a
    /// preflight the node never answers (`spec/data-api.md §2.1`).
    pub(super) async fn pair_api(&self, request: Request) -> Response {
        if !Self::is_owner(&request) {
            return headers::text(StatusCode::FORBIDDEN, OWNER_ONLY);
        }
        let now = jiff::Timestamp::now();
        match *request.method() {
            Method::GET | Method::HEAD => {
                let head = request.method() == Method::HEAD;
                let window = {
                    let mut node = self.lock();
                    node.refresh_pairing(now)
                };
                let response = match window {
                    Ok(window) => headers::json(StatusCode::OK, &serde_json::json!(window)),
                    Err(error) => self.failure(&error),
                };
                if head {
                    headers::strip_body(response)
                } else {
                    response
                }
            }
            Method::POST => {
                let json = request
                    .headers()
                    .get(CONTENT_TYPE)
                    .and_then(|v| v.to_str().ok())
                    .is_some_and(|v| {
                        v.split(';')
                            .next()
                            .is_some_and(|t| t.trim().eq_ignore_ascii_case("application/json"))
                    });
                if !json {
                    return headers::text(
                        StatusCode::UNSUPPORTED_MEDIA_TYPE,
                        "415 Unsupported Media Type: send application/json, as {\"ttl\": 120}\n",
                    );
                }
                let declared = request
                    .headers()
                    .get(CONTENT_LENGTH)
                    .and_then(|v| v.to_str().ok())
                    .and_then(|v| v.parse::<usize>().ok());
                if declared.is_some_and(|length| length > PAIR_BODY_LIMIT) {
                    return headers::text(
                        StatusCode::PAYLOAD_TOO_LARGE,
                        "413 Payload Too Large: the body is {\"ttl\": seconds} and nothing else\n",
                    );
                }
                let Ok(body) = to_bytes(request.into_body(), PAIR_BODY_LIMIT).await else {
                    return headers::text(
                        StatusCode::PAYLOAD_TOO_LARGE,
                        "413 Payload Too Large: the body is {\"ttl\": seconds} and nothing else\n",
                    );
                };
                let asked: PairRequest = if body.iter().all(u8::is_ascii_whitespace) {
                    PairRequest::default()
                } else {
                    match serde_json::from_slice(&body) {
                        Ok(asked) => asked,
                        Err(_) => {
                            return headers::text(
                                StatusCode::BAD_REQUEST,
                                "400 Bad Request: the body is {\"ttl\": seconds} and nothing else\n",
                            );
                        }
                    }
                };
                let ttl = asked.ttl.unwrap_or(pair::TTL.as_secs());
                if !(1..=pair::TTL.as_secs()).contains(&ttl) {
                    return headers::text(
                        StatusCode::BAD_REQUEST,
                        "400 Bad Request: ttl is between 1 and 120 seconds (spec/protocol.md §7.5)\n",
                    );
                }
                let opened = {
                    let mut node = self.lock();
                    if asked.node.unwrap_or(false) {
                        node.pair_node_at(Duration::from_secs(ttl), now)
                    } else {
                        node.pair_at(Duration::from_secs(ttl), now)
                    }
                };
                match opened {
                    Ok(window) => headers::json(StatusCode::OK, &serde_json::json!(window)),
                    Err(error @ (Error::CertificateExpired | Error::NodeRevoked)) => {
                        headers::text(StatusCode::CONFLICT, format!("409 Conflict: {error}\n"))
                    }
                    Err(error) => self.failure(&error),
                }
            }
            _ => headers::method_not_allowed("GET, HEAD, POST"),
        }
    }

    /// `POST /api/v1/join` (`spec/protocol.md §9.2`, `§2.3.1`): join the cluster of the
    /// node at `url` with the code its owner read out, for the owner alone. Read only as
    /// `application/json` and bounded before it is read, as `/api/v1/pair` is; the URL
    /// and the code are checked by shape before anything is dialed, and the socket work
    /// runs with the node's lock released. Answers the outcome, or the refusal by name —
    /// never the code.
    pub(super) async fn join_api(&self, request: Request) -> Response {
        if !Self::is_owner(&request) {
            return headers::text(StatusCode::FORBIDDEN, OWNER_ONLY);
        }
        if request.method() != Method::POST {
            return headers::method_not_allowed("POST");
        }
        let json = request
            .headers()
            .get(CONTENT_TYPE)
            .and_then(|v| v.to_str().ok())
            .is_some_and(|v| {
                v.split(';')
                    .next()
                    .is_some_and(|t| t.trim().eq_ignore_ascii_case("application/json"))
            });
        if !json {
            return headers::text(
                StatusCode::UNSUPPORTED_MEDIA_TYPE,
                "415 Unsupported Media Type: send application/json, as {\"url\": \"http://host:port\", \"code\": \"two words\"}\n",
            );
        }
        let declared = request
            .headers()
            .get(CONTENT_LENGTH)
            .and_then(|v| v.to_str().ok())
            .and_then(|v| v.parse::<usize>().ok());
        if declared.is_some_and(|length| length > PAIR_BODY_LIMIT) {
            return headers::text(StatusCode::PAYLOAD_TOO_LARGE, JOIN_SHAPE);
        }
        let Ok(body) = to_bytes(request.into_body(), PAIR_BODY_LIMIT).await else {
            return headers::text(StatusCode::PAYLOAD_TOO_LARGE, JOIN_SHAPE);
        };
        let asked: JoinRequest = match serde_json::from_slice(&body) {
            Ok(asked) => asked,
            Err(_) => return headers::text(StatusCode::BAD_REQUEST, JOIN_SHAPE),
        };
        let code = match Code::parse(&asked.code) {
            Ok(code) => code,
            Err(error) => {
                return headers::text(
                    StatusCode::BAD_REQUEST,
                    format!("400 Bad Request: code: {error}\n"),
                );
            }
        };
        if let Err(error) = join::join_target(&asked.url) {
            return headers::text(
                StatusCode::BAD_REQUEST,
                format!("400 Bad Request: {error}\n"),
            );
        }
        match self.run_join(&asked.url, code).await {
            Ok(joined) => headers::json(StatusCode::OK, &serde_json::json!(joined)),
            Err(error) => headers::text(StatusCode::CONFLICT, format!("409 Conflict: {error}\n")),
        }
    }

    /// Join the cluster of the node at `url` (`§2.3.1`) with this handler's node, taking
    /// its lock for the two steps that read or write it and for nothing on the socket.
    pub async fn run_join(&self, url: &str, code: Code) -> crate::Result<Joined> {
        let target = join::join_target(url)?;
        let mut node = self.clone();
        join::join_over_socket(&target, url, code, &mut node).await
    }

    /// `GET /settings/devices/pairing`: the code page while a window is open, and a card
    /// saying pairing is closed — with the button to open it — otherwise. The owner's
    /// alone: the code is what `§7.1` puts on the screen for the person standing there.
    pub(super) fn pairing_page(&self, request: &Request) -> Response {
        if !Self::is_owner(request) {
            return headers::text(StatusCode::FORBIDDEN, OWNER_ONLY);
        }
        if request.method() != Method::GET && request.method() != Method::HEAD {
            return headers::method_not_allowed("GET, HEAD");
        }
        let now = jiff::Timestamp::now();
        let mut node = self.lock();
        if let Err(error) = node.refresh() {
            return self.failure(&error);
        }
        let window = match node.refresh_pairing(now) {
            Ok(window) => window,
            Err(error) => return self.failure(&error),
        };
        let cx = Context {
            node: &node,
            report: &self.report,
            csrf: &self.csrf,
            owner: true,
        };
        let card = match devices::pairing_card(&cx, window.as_ref(), now) {
            Ok(card) => card,
            Err(error) => return self.failure(&error),
        };
        let response =
            match shell::settings_frame(&cx, SettingsPage::Devices, "Pairing", None, &card) {
                Ok(html) => headers::html(StatusCode::OK, html),
                Err(error) => self.failure(&error),
            };
        if request.method() == Method::HEAD {
            headers::strip_body(response)
        } else {
            response
        }
    }

    /// One of the owner's form posts under `/settings`.
    pub(super) async fn owner_action(
        &self,
        action: OwnerAction,
        path: &str,
        request: Request,
    ) -> Response {
        if !Self::is_owner(&request) {
            return headers::text(StatusCode::FORBIDDEN, OWNER_ONLY);
        }
        if request.method() != Method::POST {
            return headers::method_not_allowed("POST");
        }
        let form = match self.form_with_token(path, request).await {
            Ok(form) => form,
            Err(refusal) => return refusal,
        };
        let now = jiff::Timestamp::now();
        let field = |name: &str| form.get(name).map(String::as_str).unwrap_or_default();
        let (page, outcome) = match &action {
            OwnerAction::NodeName => (
                SettingsPage::Node,
                self.lock().set_display_name(field("display_name")),
            ),
            OwnerAction::PairOpen => (
                SettingsPage::Devices,
                self.lock().pair_at(pair::TTL, now).map(|_| ()),
            ),
            OwnerAction::AdmitNode => (
                SettingsPage::Devices,
                self.lock().pair_node_at(pair::TTL, now).map(|_| ()),
            ),
            OwnerAction::Join => {
                // The socket work runs here with the lock released (§2.3.1); the code is
                // parsed by shape first so a typo never dials anything.
                let outcome = match Code::parse(field("code")) {
                    Ok(code) => self.run_join(field("url").trim(), code).await,
                    Err(error) => Err(Error::InvalidText {
                        field: "code",
                        problem: error.to_string(),
                    }),
                };
                match outcome {
                    Ok(joined) => {
                        let text = if joined.joined {
                            format!(
                                "This space joined cluster {} through space {}{}.",
                                joined.cluster_id,
                                joined.peer,
                                if joined.readmitted {
                                    " and was re-admitted with its own key"
                                } else {
                                    ""
                                }
                            )
                        } else {
                            format!(
                                "Space {} was admitted to this cluster ({}).",
                                joined.peer, joined.cluster_id
                            )
                        };
                        return self.settings_notice(
                            SettingsPage::Node,
                            StatusCode::OK,
                            NoticeKind::Ok,
                            text,
                        );
                    }
                    Err(error) => (SettingsPage::Node, Err(error)),
                }
            }
            OwnerAction::PairClose => (
                SettingsPage::Devices,
                self.lock().close_pairing(now).map(|_| ()),
            ),
            OwnerAction::DeviceLabel(id) => (
                SettingsPage::Devices,
                self.lock().label_device(id, field("label")),
            ),
            OwnerAction::DeviceRevoke(id) => {
                // A node is revoked as a node (`spec/protocol.md §2.3.4`): its device row
                // and a `sys_node_revocation` row; any other kind as a device.
                let outcome = {
                    let mut node = self.lock();
                    if node.device_is_node(id).unwrap_or(false) {
                        node.revoke_node(id, None, now)
                    } else {
                        node.revoke_device(id, None, now)
                    }
                };
                if outcome.is_ok() {
                    // The device's open channel closes at once (`spec/data-dictionary.md
                    // §3.2`, "access denied immediately"); nobody listening is fine.
                    let _ = self.revoked.send(id.clone());
                }
                (SettingsPage::Devices, outcome)
            }
        };
        match outcome {
            Ok(()) => {
                let to = match action {
                    OwnerAction::PairOpen | OwnerAction::AdmitNode => "/settings/devices/pairing",
                    _ => page.path(),
                };
                let mut response = headers::redirect(StatusCode::SEE_OTHER, to);
                if let Ok(value) = axum::http::HeaderValue::from_str(to) {
                    response.headers_mut().insert("hx-redirect", value);
                }
                response
            }
            Err(error) => {
                let status = match &error {
                    Error::InvalidText { .. } | Error::OwnDevice => StatusCode::BAD_REQUEST,
                    Error::DeviceUnknown { .. } => StatusCode::NOT_FOUND,
                    Error::Pair(_) | Error::Join { .. } => StatusCode::BAD_REQUEST,
                    Error::CertificateExpired | Error::NodeRevoked => StatusCode::CONFLICT,
                    _ => StatusCode::INTERNAL_SERVER_ERROR,
                };
                self.settings_notice(page, status, NoticeKind::Warn, error.to_string())
            }
        }
    }

    /// Read a form body of at most `http::FORM_LIMIT` bytes and verify the token
    /// `csrf()` put in it for `path`; either refusal is the response to send back.
    pub(super) async fn form_with_token(
        &self,
        path: &str,
        request: Request,
    ) -> Result<BTreeMap<String, String>, Response> {
        // A declared length past the limit is refused before the body is read at all, so a
        // client that sent `Expect: 100-continue` never sends it. A body with no declared
        // length is read up to the limit and refused there; the rest is never read.
        let declared = request
            .headers()
            .get(CONTENT_LENGTH)
            .and_then(|v| v.to_str().ok())
            .and_then(|v| v.parse::<usize>().ok());
        if declared.is_some_and(|length| length > http::FORM_LIMIT) {
            return Err(headers::text(StatusCode::PAYLOAD_TOO_LARGE, FORM_TOO_LARGE));
        }
        let body = to_bytes(request.into_body(), http::FORM_LIMIT)
            .await
            .map_err(|_| headers::text(StatusCode::PAYLOAD_TOO_LARGE, FORM_TOO_LARGE))?;
        let form = http::parse_form(&body);
        let token = form
            .get(http::csrf::FIELD)
            .map(String::as_str)
            .unwrap_or_default();
        if !self.csrf.verify(path, token) {
            return Err(headers::text(StatusCode::FORBIDDEN, FORM_REFUSED));
        }
        Ok(form)
    }

    /// A settings page with a notice at the top, under `status` — how a refused form
    /// answers so the owner reads why on the page they were on.
    pub(super) fn settings_notice(
        &self,
        page: SettingsPage,
        status: StatusCode,
        kind: NoticeKind,
        text: String,
    ) -> Response {
        let notice = Notice { kind, text };
        let mut node = self.lock();
        if let Err(error) = node.refresh() {
            return self.failure(&error);
        }
        let cx = Context {
            node: &node,
            report: &self.report,
            csrf: &self.csrf,
            owner: true,
        };
        match shell::settings(&cx, page, Some(&notice)) {
            Ok(html) => headers::html(status, html),
            Err(error) => self.failure(&error),
        }
    }
}

/// The daemon's node-side steps of a join (`spec/protocol.md §2.3.1`): each takes the
/// lock for its own duration and nothing on the socket holds it.
impl JoinNode for Handler {
    fn join_start(
        &mut self,
        url: &str,
        code: Code,
        hello: &str,
    ) -> crate::Result<(JoinClient, String)> {
        self.lock()
            .join_start_at(url, code, hello, jiff::Timestamp::now())
    }

    fn join_apply(&mut self, outcome: JoinOutcome, now: jiff::Timestamp) -> crate::Result<Joined> {
        self.lock().join_apply_at(outcome, now)
    }
}
