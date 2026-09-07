// Project:  Privatium™  |  File: crates/privatium-core/src/sync/routes.rs
// Authors:  Gabriel Mongefranco (@gabrielmongefranco)
// Created:  2026-09-07  |  Modified: 2026-09-07
// Summary:  Node-only heads, pull, and push routes through core::handle, with bounded
//           input and raw response streams (spec/protocol.md §9.2 and §10.2).
//           See main README.md for full license information.

use super::source;
use crate::http::{auth::Session, headers};
use crate::wire::ApiSettings;
use crate::{Body, Error, Handler, Request, Response};
use axum::body::{Bytes, to_bytes};
use axum::http::{Method, StatusCode};
use std::collections::BTreeMap;

fn refuse() -> Response {
    headers::text(
        StatusCode::FORBIDDEN,
        "403 Forbidden — synchronization requires an authenticated node session\n",
    )
}
fn invalid() -> Response {
    headers::text(
        StatusCode::BAD_REQUEST,
        "400 Bad Request — supply a valid app, device, and sequence\n",
    )
}

fn query(request: &Request, allowed: &[&str]) -> Option<BTreeMap<String, String>> {
    let raw = request.uri().query().unwrap_or_default();
    if raw.len() > 256 {
        return None;
    }
    let mut values = BTreeMap::new();
    if raw.is_empty() {
        return Some(values);
    }
    for pair in raw.split('&') {
        let (key, value) = pair.split_once('=')?;
        if !allowed.contains(&key) || values.insert(key.into(), value.into()).is_some() {
            return None;
        }
    }
    Some(values)
}

impl Handler {
    pub(crate) async fn sync_heads(&self, request: Request) -> Response {
        if !request
            .extensions()
            .get::<Session>()
            .is_some_and(Session::is_node)
        {
            return refuse();
        }
        if request.method() != Method::GET {
            return headers::method_not_allowed("GET");
        }
        let Some(query) = query(&request, &["app"]) else {
            return invalid();
        };
        let handler = self.clone();
        let answer = tokio::task::spawn_blocking(move || {
            let node = handler.lock();
            let paths = node.paths();
            let limit = ApiSettings::read(&node).max_body;
            if let Some(app) = query.get("app") {
                source::app_heads(paths, app, limit).map(|heads| serde_json::json!(heads))
            } else {
                source::heads(paths, limit).map(|heads| serde_json::json!(heads))
            }
        })
        .await;
        match answer {
            Ok(Ok(heads)) => headers::json(StatusCode::OK, &heads),
            Ok(Err(Error::ForeignLine { .. })) => invalid(),
            _ => server_error(),
        }
    }

    pub(crate) async fn sync_pull(&self, request: Request) -> Response {
        if !request
            .extensions()
            .get::<Session>()
            .is_some_and(Session::is_node)
        {
            return refuse();
        }
        if request.method() != Method::GET {
            return headers::method_not_allowed("GET");
        }
        let Some(query) = query(&request, &["app", "dev", "after"]) else {
            return invalid();
        };
        let (Some(app), Some(dev), Some(after)) = (
            query.get("app"),
            query.get("dev"),
            query.get("after").and_then(|s| s.parse::<u64>().ok()),
        ) else {
            return invalid();
        };
        if crate::log::foreign::validate_destination(app, dev).is_err() {
            return invalid();
        }
        let handler = self.clone();
        let (app, dev) = (app.clone(), dev.clone());
        // The page is read whole under the lock and streamed after it is released, so
        // no reader observes a line the writer has not finished (`spec/protocol.md §4.1`).
        let page = tokio::task::spawn_blocking(move || {
            let node = handler.lock();
            let limit = ApiSettings::read(&node).max_body;
            source::Source::open(node.paths(), &app, &dev, limit)?.page(after)
        })
        .await;
        match page {
            Ok(Ok(page)) => {
                let stream = futures_util::stream::iter(
                    page.lines
                        .into_iter()
                        .map(|line| Ok::<_, std::convert::Infallible>(Bytes::from(line))),
                );
                let mut response =
                    headers::with_body(StatusCode::OK, headers::NDJSON, Body::from_stream(stream));
                if let (Ok(next), Ok(head)) =
                    (page.next.to_string().parse(), page.head.to_string().parse())
                {
                    response.headers_mut().insert("pv-next", next);
                    response.headers_mut().insert("pv-head", head);
                }
                response
            }
            Ok(Err(
                Error::ForeignLine { .. }
                | Error::ForeignEnvelope { .. }
                | Error::ForeignLineTooLong { .. },
            )) => headers::json(
                StatusCode::CONFLICT,
                &serde_json::json!({"error":"origin range cannot form a bounded contiguous page"}),
            ),
            _ => server_error(),
        }
    }

    pub(crate) async fn sync_push(&self, request: Request) -> Response {
        if !request
            .extensions()
            .get::<Session>()
            .is_some_and(Session::is_node)
        {
            return refuse();
        }
        if request.method() != Method::POST {
            return headers::method_not_allowed("POST");
        }
        let Some(query) = query(&request, &["app", "dev"]) else {
            return invalid();
        };
        let (Some(app), Some(dev)) = (query.get("app"), query.get("dev")) else {
            return invalid();
        };
        if crate::log::foreign::validate_destination(app, dev).is_err() {
            return invalid();
        }
        let limit = {
            let node = self.lock();
            if dev == node.id().as_str() {
                return refuse();
            }
            ApiSettings::read(&node).max_body
        };
        if request
            .headers()
            .get("content-type")
            .and_then(|v| v.to_str().ok())
            != Some("application/x-ndjson")
        {
            return headers::text(
                StatusCode::UNSUPPORTED_MEDIA_TYPE,
                "415 Unsupported Media Type — sync push requires application/x-ndjson\n",
            );
        }
        let Some(session) = request.extensions().get::<Session>().cloned() else {
            return refuse();
        };
        // The body is read to its bound before the lock is taken: the node lock is a
        // blocking mutex and may not be held across an await.
        let body = match to_bytes(request.into_body(), limit).await {
            Ok(body) => body,
            Err(_) => {
                return headers::text(
                    StatusCode::PAYLOAD_TOO_LARGE,
                    "413 Payload Too Large — sync push exceeds api.max_body\n",
                );
            }
        };
        let (result, head) = {
            let mut node = self.lock();
            if !crate::wire::channel::active(&node, &session) {
                return refuse();
            }
            let head = match source::app_heads(node.paths(), app, limit) {
                Ok(heads) => heads.get(dev).copied().unwrap_or(0),
                Err(_) => return server_error(),
            };
            (node.receive(app, dev, &body), head)
        };
        match result {
            Ok(received) => {
                self.fire_pending(app).await;
                headers::json(StatusCode::OK, &serde_json::json!({"head":received.head}))
            }
            Err(Error::ForeignSeq { found, .. } | Error::ForeignEnvelope { seq: found, .. }) => {
                headers::json(
                    StatusCode::CONFLICT,
                    &serde_json::json!({
                        "error": "sequence does not continue the origin log",
                        "seq": found,
                        "head": head,
                    }),
                )
            }
            Err(
                Error::ForeignLine { .. }
                | Error::ForeignLineTooLong { .. }
                | Error::ForeignDiverged { .. },
            ) => headers::json(
                StatusCode::CONFLICT,
                &serde_json::json!({
                    "error": "range validation failed; pull the missing range",
                    "seq": head.saturating_add(1),
                    "head": head,
                }),
            ),
            Err(_) => server_error(),
        }
    }
}

fn server_error() -> Response {
    headers::text(
        StatusCode::INTERNAL_SERVER_ERROR,
        "500 Internal Server Error — synchronization could not read or receive the log; check node storage\n",
    )
}
