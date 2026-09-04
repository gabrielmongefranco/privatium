// Project:  Privatium™  |  File: crates/privatium-core/src/wire/data.rs
// Authors:  Gabriel Mongefranco (@gabrielmongefranco)
// Created:  2026-09-03  |  Modified: 2026-09-05
// Summary:  The data API of spec/data-api.md beneath an app's mount — the one namespace the
//           framework reserves there (spec/protocol.md §9.1). Reads run on the sandboxed
//           connection off the node lock; writes go through Node::append like every other
//           writer; /api/events hands out log lines byte for byte; /api/stream is SSE
//           through the streaming Response body, a channel pumped by a task, never a
//           buffer and never the Lua host (docs/plans/phase-1.md §8, R6).

// A refusal is a `Response`, and an early return of one is how every check here reads.
// Clippy would rather the `Err` were boxed; the allocation would buy nothing on a path
// that is about to send it.
#![allow(clippy::result_large_err)]

use std::collections::{BTreeMap, HashMap};
use std::pin::Pin;
use std::sync::{Arc, Mutex, PoisonError};
use std::task::{Context, Poll};
use std::time::{Duration, Instant};

use axum::body::{Body, Bytes, to_bytes};
use axum::http::header::{ALLOW, CONTENT_LENGTH, CONTENT_TYPE, HeaderValue, RETRY_AFTER};
use axum::http::{Method, StatusCode};
use rusqlite::Connection;
use serde::Deserialize;
use serde_json::{Map, Value, json};
use tokio::sync::{broadcast, mpsc};

use crate::app::{Event, StreamEvent};
use crate::http::{self, Device, headers};
use crate::log::{Reader, batch};
use crate::store::Schema;
use crate::store::materialize::quote_ident;
use crate::store::query::{self, ColumnType};
use crate::wire::{Handler, Request, Response, node_facts};
use crate::{Error, Node};

/// The keep-alive cadence of `spec/data-api.md §3`.
pub const PING: Duration = Duration::from_secs(30);

/// The envelope fields a client MUST NOT set (`spec/data-api.md §2`, `PV304`).
const STAMPED: [&str; 5] = ["seq", "lam", "ts", "dev", "app"];

/// The fields a client supplies.
const SUPPLIED: [&str; 4] = ["op", "tbl", "id", "d"];

/// The limits of `spec/data-api.md §7`, from `sys_setting` with the table's defaults.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ApiSettings {
    /// `api.sql_rate` — ad-hoc SQL requests per second per session.
    pub sql_rate: u64,
    /// `api.max_batch` — events per append.
    pub max_batch: usize,
    /// `api.max_body` — bytes per request.
    pub max_body: usize,
    /// `api.max_rows` — the most a query returns.
    pub max_rows: usize,
    /// `api.max_streams` — SSE connections per device.
    pub max_streams: usize,
}

impl Default for ApiSettings {
    fn default() -> Self {
        Self {
            sql_rate: 20,
            max_batch: 1000,
            max_body: 4 * 1024 * 1024,
            max_rows: 10_000,
            max_streams: 8,
        }
    }
}

impl ApiSettings {
    /// Read the five keys, keeping the default for one that is unset or not a number.
    fn read(node: &Node) -> Self {
        let read = |key: &str| -> Option<u64> {
            let text = node.setting_value(key).ok().flatten()?;
            let value: Value = serde_json::from_str(&text).ok()?;
            value
                .as_u64()
                .or_else(|| value.as_str().and_then(|s| s.trim().parse().ok()))
        };
        let defaults = Self::default();
        let size = |key: &str, default: usize| {
            read(key)
                .and_then(|n| usize::try_from(n).ok())
                .unwrap_or(default)
        };
        Self {
            sql_rate: read("api.sql_rate").unwrap_or(defaults.sql_rate),
            max_batch: size("api.max_batch", defaults.max_batch),
            max_body: size("api.max_body", defaults.max_body),
            max_rows: size("api.max_rows", defaults.max_rows),
            max_streams: size("api.max_streams", defaults.max_streams),
        }
    }
}

/// What the handler keeps for the API between requests: the SQL rate buckets and the
/// open streams, both per device, and the ping cadence.
pub struct ApiState {
    sql: Mutex<HashMap<String, Bucket>>,
    streams: Arc<Mutex<HashMap<String, usize>>>,
    ping: Duration,
}

impl std::fmt::Debug for ApiState {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("ApiState")
            .field("ping", &self.ping)
            .finish()
    }
}

impl Default for ApiState {
    fn default() -> Self {
        Self {
            sql: Mutex::new(HashMap::new()),
            streams: Arc::new(Mutex::new(HashMap::new())),
            ping: PING,
        }
    }
}

impl ApiState {
    /// Change the keep-alive cadence — for a test, or an embedder on a link that drops
    /// idle connections sooner than 30 seconds.
    pub fn set_ping(&mut self, ping: Duration) {
        self.ping = ping;
    }
}

/// A token bucket: `rate` tokens, refilled at `rate` per second.
struct Bucket {
    tokens: f64,
    last: Instant,
}

/// Take one token for `key`, or say it is empty.
fn take_token(buckets: &Mutex<HashMap<String, Bucket>>, key: &str, rate: u64) -> bool {
    let rate = rate.max(1) as f64;
    let now = Instant::now();
    let mut map = buckets.lock().unwrap_or_else(PoisonError::into_inner);
    let bucket = map.entry(key.to_owned()).or_insert(Bucket {
        tokens: rate,
        last: now,
    });
    bucket.tokens =
        (bucket.tokens + now.duration_since(bucket.last).as_secs_f64() * rate).min(rate);
    bucket.last = now;
    if bucket.tokens >= 1.0 {
        bucket.tokens -= 1.0;
        true
    } else {
        false
    }
}

/// Holds one of a device's stream slots; dropping it — when the pump ends — frees it.
struct StreamSlot {
    device: String,
    streams: Arc<Mutex<HashMap<String, usize>>>,
}

impl Drop for StreamSlot {
    fn drop(&mut self) {
        let mut map = self.streams.lock().unwrap_or_else(PoisonError::into_inner);
        if let Some(count) = map.get_mut(&self.device) {
            *count = count.saturating_sub(1);
            if *count == 0 {
                map.remove(&self.device);
            }
        }
    }
}

/// The SSE body: frames read from a channel the pump task fills. One channel item is one
/// SSE message, and `Body::from_stream` hands each on as one data frame, so nothing is
/// ever held back to be sent together.
struct ChannelBody(mpsc::Receiver<Bytes>);

impl futures_core::Stream for ChannelBody {
    type Item = Result<Bytes, std::convert::Infallible>;

    fn poll_next(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Option<Self::Item>> {
        self.0.poll_recv(cx).map(|item| item.map(Ok))
    }
}

/// One SSE message.
fn frame(event: &str, data: &[u8]) -> Bytes {
    let mut out = Vec::with_capacity(data.len() + event.len() + 16);
    out.extend_from_slice(b"event: ");
    out.extend_from_slice(event.as_bytes());
    out.extend_from_slice(b"\ndata: ");
    out.extend_from_slice(data);
    out.extend_from_slice(b"\n\n");
    Bytes::from(out)
}

/// A JSON refusal: `{"error": …}` plus whatever names the offending part.
fn refuse(status: StatusCode, error: impl Into<String>) -> Response {
    headers::json(status, &json!({ "error": error.into() }))
}

fn refuse_with(
    status: StatusCode,
    mut detail: Map<String, Value>,
    error: impl Into<String>,
) -> Response {
    detail.insert("error".to_owned(), Value::String(error.into()));
    headers::json(status, &Value::Object(detail))
}

fn not_found(what: &str) -> Response {
    refuse(StatusCode::NOT_FOUND, format!("404 Not Found: {what}"))
}

fn method_not_allowed(allow: &'static str) -> Response {
    let mut response = refuse(
        StatusCode::METHOD_NOT_ALLOWED,
        format!("405 Method Not Allowed — allowed: {allow}"),
    );
    response
        .headers_mut()
        .insert(ALLOW, HeaderValue::from_static(allow));
    response
}

/// `Sec-Fetch-Site: cross-site` — a browser saying another site made this request. The
/// API is same-origin by design (`spec/data-api.md §2`); a request from anywhere else is
/// refused before anything is read, whatever its method.
fn is_cross_site(request: &Request) -> bool {
    request
        .headers()
        .get("sec-fetch-site")
        .and_then(|value| value.to_str().ok())
        .is_some_and(|site| site.eq_ignore_ascii_case("cross-site"))
}

/// A POST carries `application/json`, which a cross-origin page cannot send without a
/// preflight the node never answers (`spec/data-api.md §2`).
fn is_json(request: &Request) -> bool {
    request
        .headers()
        .get(CONTENT_TYPE)
        .and_then(|value| value.to_str().ok())
        .is_some_and(|value| {
            value
                .split(';')
                .next()
                .unwrap_or_default()
                .trim()
                .eq_ignore_ascii_case("application/json")
        })
}

fn query_of(request: &Request) -> BTreeMap<String, String> {
    request
        .uri()
        .query()
        .map(|q| http::parse_form(q.as_bytes()))
        .unwrap_or_default()
}

fn device_of(request: &Request, node_id: &str) -> String {
    request
        .extensions()
        .get::<Device>()
        .map_or_else(|| node_id.to_owned(), |device| device.0.as_str().to_owned())
}

/// Read a JSON body of at most `max` bytes. A declared length past the limit is refused
/// before a byte is read.
async fn json_body(request: Request, max: usize) -> Result<Value, Response> {
    let declared = request
        .headers()
        .get(CONTENT_LENGTH)
        .and_then(|v| v.to_str().ok())
        .and_then(|v| v.parse::<usize>().ok());
    if declared.is_some_and(|length| length > max) {
        return Err(refuse(
            StatusCode::PAYLOAD_TOO_LARGE,
            format!("413 Payload Too Large: a request body is at most {max} bytes (api.max_body)"),
        ));
    }
    let bytes = to_bytes(request.into_body(), max).await.map_err(|_| {
        refuse(
            StatusCode::PAYLOAD_TOO_LARGE,
            format!("413 Payload Too Large: a request body is at most {max} bytes (api.max_body)"),
        )
    })?;
    serde_json::from_slice::<Value>(&bytes).map_err(|error| {
        refuse(
            StatusCode::BAD_REQUEST,
            format!("400 Bad Request: the body is not JSON: {error}"),
        )
    })
}

fn is_table_name(name: &str) -> bool {
    !name.is_empty()
        && name.len() <= 64
        && name.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'_')
}

fn is_ulid(id: &str) -> bool {
    id.len() == 26 && ulid::Ulid::from_string(id).is_ok()
}

/// `?limit=` and `?offset=`: the default of 1000 rows, at most `max`, from 0.
fn paging(query: &BTreeMap<String, String>, max: usize) -> Result<(usize, usize), Response> {
    let number = |key: &str, default: usize| -> Result<usize, Response> {
        match query.get(key) {
            None => Ok(default),
            Some(text) => text.trim().parse::<usize>().map_err(|_| {
                refuse(
                    StatusCode::BAD_REQUEST,
                    format!("400 Bad Request: {key} must be a whole number, not {text:?}"),
                )
            }),
        }
    };
    let limit = number("limit", 1000)?.min(max);
    let offset = number("offset", 0)?;
    Ok((limit, offset))
}

/// The first keyword of a statement, past whitespace and comments.
fn leading_keyword(sql: &str) -> String {
    let mut rest = sql;
    loop {
        rest = rest.trim_start();
        if let Some(after) = rest.strip_prefix("--") {
            rest = after.find('\n').map_or("", |at| &after[at..]);
        } else if let Some(after) = rest.strip_prefix("/*") {
            rest = after.find("*/").map_or("", |at| &after[at + 2..]);
        } else {
            break;
        }
    }
    rest.chars()
        .take_while(|c| c.is_ascii_alphabetic())
        .collect::<String>()
        .to_ascii_uppercase()
}

/// What a read takes from under the node lock, to run without it. The statement itself
/// runs through `store::query`, which is what `Node::query` hands an embedder too, so
/// a column is typed one way for both (`spec/data-api.md §1`).
struct ReadPlan {
    conn: Connection,
    schema: Schema,
    lam: u64,
    deadline: Duration,
}

fn columns_json(columns: &[ColumnType]) -> Value {
    Value::Array(
        columns
            .iter()
            .map(|column| json!({ "name": column.name, "type": column.ty }))
            .collect(),
    )
}

/// One log line, as `/api/events`, `/api/row` and the stream's backlog need it: the
/// `§4.5` rank to order by, the fields to filter on, and the bytes to hand out untouched.
struct RawLine {
    seq: u64,
    lam: u64,
    ts: Option<String>,
    dev: String,
    put: bool,
    tbl: String,
    id: String,
    line: Vec<u8>,
}

impl RawLine {
    fn rank(&self) -> (u64, Option<&str>, &str, u64) {
        (self.lam, self.ts.as_deref(), &self.dev, self.seq)
    }
}

/// The envelope fields the API reads; everything else stays in the bytes.
#[derive(Deserialize)]
struct Head {
    seq: Option<u64>,
    lam: Option<u64>,
    ts: Option<String>,
    dev: Option<String>,
    app: Option<String>,
    op: Option<String>,
    tbl: Option<String>,
    id: Option<String>,
    /// `§4.1`'s batch marker.
    batch: Option<u64>,
}

/// Every line of `app`'s log that is an envelope, inside `§4.4`'s horizon and not part
/// of a batch that reached the disk short (`§4.1`), in `§4.5` order — `(lam, ts, dev,
/// seq)` — which is the order a client resuming from `after=` needs. Reading only;
/// nothing is re-serialized (`spec/protocol.md §4.2`).
fn read_lines(log_dir: &std::path::Path, app: &str) -> Vec<RawLine> {
    let horizon: Option<jiff::Timestamp> = crate::store::cutoff_now().parse().ok();
    let Ok(reader) = Reader::open(log_dir) else {
        return Vec::new();
    };
    let mut lines = Vec::new();
    for segment in reader.segments() {
        let Ok(segment_lines) = segment.lines() else {
            continue;
        };
        let parsed: Vec<(Head, Vec<u8>)> = segment_lines
            .filter_map(Result::ok)
            .filter_map(|line| {
                serde_json::from_slice::<Head>(line.raw())
                    .ok()
                    .map(|head| (head, line.raw().to_vec()))
            })
            .collect();
        let heads: Vec<batch::Head<'_>> = parsed
            .iter()
            .map(|(head, _)| batch::Head {
                seq: head.seq.unwrap_or(0),
                ts: head.ts.as_deref(),
                batch: head.batch,
            })
            .collect();
        let short = batch::incomplete(&heads);
        for (index, (head, raw)) in parsed.into_iter().enumerate() {
            if batch::covered(&short, index) || head.app.as_deref() != Some(app) {
                continue;
            }
            let (Some(seq), Some(lam), Some(tbl), Some(id)) =
                (head.seq, head.lam, head.tbl, head.id)
            else {
                continue;
            };
            if let (Some(ts), Some(horizon)) = (head.ts.as_deref(), horizon)
                && let Ok(at) = ts.parse::<jiff::Timestamp>()
                && at > horizon
            {
                continue;
            }
            lines.push(RawLine {
                seq,
                lam,
                ts: head.ts,
                dev: head.dev.unwrap_or_default(),
                put: head.op.as_deref() == Some("put"),
                tbl,
                id,
                line: raw,
            });
        }
    }
    lines.sort_by(|a, b| a.rank().cmp(&b.rank()));
    lines
}

/// Everything the write path takes from one client event.
struct Parsed {
    change: Event,
    /// Whether the client chose the id (a minted one is never a reuse).
    client_id: bool,
}

/// One client event as a [`Event`] (`spec/data-api.md §2`): `op`, `tbl`, `id`, `d`
/// and nothing else, the id a ULID or minted here.
fn parse_event(index: usize, value: &Value) -> Result<Parsed, Response> {
    let bad = |error: String| {
        let mut detail = Map::new();
        detail.insert("index".to_owned(), Value::from(index));
        refuse_with(
            StatusCode::BAD_REQUEST,
            detail,
            format!("400 Bad Request: events[{index}]: {error}"),
        )
    };
    let Some(object) = value.as_object() else {
        return Err(bad("not a JSON object".to_owned()));
    };
    for key in object.keys() {
        if STAMPED.contains(&key.as_str()) {
            return Err(bad(format!(
                "`{key}` is stamped by the framework and MUST NOT be set by a client (PV304)"
            )));
        }
        if !SUPPLIED.contains(&key.as_str()) {
            return Err(bad(format!(
                "`{key}` is not a field; an event is op, tbl, id and d"
            )));
        }
    }
    let put = match object.get("op").and_then(Value::as_str) {
        Some("put") => true,
        Some("del") => false,
        _ => return Err(bad("`op` must be \"put\" or \"del\"".to_owned())),
    };
    let tbl = match object.get("tbl").and_then(Value::as_str) {
        Some(tbl) if is_table_name(tbl) => tbl.to_owned(),
        _ => return Err(bad("`tbl` must be a table name".to_owned())),
    };
    let (id, client_id) = match object.get("id") {
        None | Some(Value::Null) if put => (crate::new_ulid(), false),
        None | Some(Value::Null) => return Err(bad("a del names its `id`".to_owned())),
        Some(Value::String(id)) if is_ulid(id) => (id.clone(), true),
        Some(_) => {
            return Err(bad(
                "`id` must be a ULID — 26 Crockford base32 characters; omit it and the \
                 server mints one"
                    .to_owned(),
            ));
        }
    };
    let d = match (put, object.get("d")) {
        (true, Some(Value::Object(d))) => Some(Value::Object(d.clone())),
        (true, _) => return Err(bad("a put carries `d`, a JSON object".to_owned())),
        (false, None | Some(Value::Null)) => None,
        (false, Some(_)) => return Err(bad("a del carries no `d`".to_owned())),
    };
    Ok(Parsed {
        change: Event { tbl, id, d },
        client_id,
    })
}

impl Handler {
    /// `/api/…` beneath `slug`'s mount. `rest` begins with `/api`.
    pub(super) async fn data_api(&self, slug: &str, rest: &str, request: Request) -> Response {
        if is_cross_site(&request) {
            return refuse(
                StatusCode::FORBIDDEN,
                "403 Forbidden: the data API answers its own origin only (spec/data-api.md §2)",
            );
        }
        let method = request.method().clone();
        let head = method == Method::HEAD;
        let get = method == Method::GET || head;
        let tail = rest.strip_prefix("/api").unwrap_or_default();
        let segments: Vec<String> = tail
            .split('/')
            .filter(|segment| !segment.is_empty())
            .map(|segment| http::percent_decode(segment.as_bytes()))
            .collect();
        let parts: Vec<&str> = segments.iter().map(String::as_str).collect();
        // Taken now: a `&Request` held across an await would make `handle`'s future
        // depend on the body being `Sync`, which a streaming body is not.
        let query = query_of(&request);

        let response = match parts.as_slice() {
            ["q", view] => {
                if !get {
                    return method_not_allowed("GET, HEAD");
                }
                self.api_query(slug, view, query).await
            }
            ["sql"] => {
                if method != Method::POST {
                    return method_not_allowed("POST");
                }
                self.api_sql(slug, request).await
            }
            ["row", tbl, id] => {
                if !get {
                    return method_not_allowed("GET, HEAD");
                }
                self.api_row(slug, tbl, id).await
            }
            ["events"] if get => self.api_events(slug, query).await,
            ["events"] if method == Method::POST => self.api_append(slug, request).await,
            ["events"] => return method_not_allowed("GET, HEAD, POST"),
            ["stream"] => {
                if !get {
                    return method_not_allowed("GET, HEAD");
                }
                self.api_stream(slug, &request, head)
            }
            ["schema"] => {
                if !get {
                    return method_not_allowed("GET, HEAD");
                }
                self.api_schema(slug)
            }
            ["node"] => {
                if !get {
                    return method_not_allowed("GET, HEAD");
                }
                self.api_node(slug, &request)
            }
            _ => not_found(&format!("no data API route at {rest} (spec/data-api.md)")),
        };
        if head {
            headers::strip_body(response)
        } else {
            response
        }
    }

    /// The read-only connection, the schema, the high-water mark and the deadline, taken
    /// under the lock. `lam` is read before the query runs, so the mark can only
    /// understate what the rows reflect — a resume from it repeats an event or two, which
    /// is idempotent, and never skips one.
    fn read_plan(
        &self,
        node: &Node,
        slug: &str,
    ) -> Result<(ReadPlan, crate::store::Params), Response> {
        let Some(app) = node.app(slug) else {
            return Err(not_found(&format!("no app is mounted as {slug}")));
        };
        let (conn, params) = app.store().app_conn_bound().map_err(|error| {
            refuse(
                StatusCode::INTERNAL_SERVER_ERROR,
                format!("500 Internal Server Error: could not open the app's connection: {error}"),
            )
        })?;
        Ok((
            ReadPlan {
                conn,
                schema: app.store().schema().clone(),
                lam: app.log().lam(),
                deadline: Duration::from_secs(node.config().lua.max_seconds.max(1)),
            },
            params,
        ))
    }

    /// `GET /api/q/<view>` (`spec/data-api.md §1`).
    async fn api_query(&self, slug: &str, view: &str, query: BTreeMap<String, String>) -> Response {
        let (plan, settings) = {
            let node = self.lock();
            let Some(app) = node.app(slug) else {
                return not_found(&format!("no app is mounted as {slug}"));
            };
            let Some(declared) = app
                .store()
                .schema()
                .views
                .iter()
                .find(|declared| declared.name == view)
            else {
                return not_found(&format!(
                    "{slug} declares no view named {view} in schema.sql"
                ));
            };
            for key in query.keys() {
                if key != "limit" && key != "offset" && !declared.params.contains(key) {
                    return refuse(
                        StatusCode::BAD_REQUEST,
                        format!(
                            "400 Bad Request: {view} binds no $\u{7b}{key}\u{7d}; its placeholders are {:?} \
                             (plus limit and offset)",
                            declared.params
                        ),
                    );
                }
            }
            let (plan, params) = match self.read_plan(&node, slug) {
                Ok(plan) => plan,
                Err(response) => return response,
            };
            for name in &declared.params {
                if let Some(value) = query.get(name) {
                    params.set(name, value);
                }
            }
            (plan, ApiSettings::read(&node))
        };
        let (limit, offset) = match paging(&query, settings.max_rows) {
            Ok(paging) => paging,
            Err(response) => return response,
        };
        let sql = format!("SELECT * FROM {} LIMIT ? OFFSET ?", quote_ident(view));
        let name = view.to_owned();
        let outcome = tokio::task::spawn_blocking(move || {
            let params = vec![
                rusqlite::types::Value::Integer(i64::try_from(limit).unwrap_or(i64::MAX)),
                rusqlite::types::Value::Integer(i64::try_from(offset).unwrap_or(i64::MAX)),
            ];
            let result = query::run(&plan.conn, &plan.schema, plan.deadline, &sql, params)?;
            Ok::<_, String>((result, plan.lam))
        })
        .await;
        match outcome {
            Ok(Ok((result, lam))) => headers::json(
                StatusCode::OK,
                &json!({
                    "view": name,
                    "columns": columns_json(&result.columns),
                    "rows": result.rows,
                    "lam": lam,
                }),
            ),
            Ok(Err(error)) => refuse(StatusCode::BAD_REQUEST, format!("400 Bad Request: {error}")),
            Err(join) => refuse(
                StatusCode::INTERNAL_SERVER_ERROR,
                format!("500 Internal Server Error: the query's thread panicked: {join}"),
            ),
        }
    }

    /// `POST /api/sql` (`spec/data-api.md §1`): gated on `permissions.sql`, rate limited,
    /// a single `SELECT` or `WITH … SELECT` with bound parameters only.
    async fn api_sql(&self, slug: &str, request: Request) -> Response {
        if !is_json(&request) {
            return refuse(
                StatusCode::UNSUPPORTED_MEDIA_TYPE,
                "415 Unsupported Media Type: send application/json (spec/data-api.md §2)",
            );
        }
        let (plan, settings) = {
            let node = self.lock();
            let Some(app) = node.app(slug) else {
                return not_found(&format!("no app is mounted as {slug}"));
            };
            if !app.manifest().permissions.sql {
                return refuse(
                    StatusCode::FORBIDDEN,
                    format!(
                        "403 Forbidden: {slug} has not declared permissions.sql = true in app.toml \
                         (spec/data-api.md §1)"
                    ),
                );
            }
            let settings = ApiSettings::read(&node);
            let device = device_of(&request, node.id().as_str());
            if !take_token(&self.api.sql, &device, settings.sql_rate) {
                let mut response = refuse(
                    StatusCode::TOO_MANY_REQUESTS,
                    format!(
                        "429 Too Many Requests: at most {} ad-hoc SQL requests per second \
                         (api.sql_rate)",
                        settings.sql_rate
                    ),
                );
                response
                    .headers_mut()
                    .insert(RETRY_AFTER, HeaderValue::from_static("1"));
                return response;
            }
            let (plan, _) = match self.read_plan(&node, slug) {
                Ok(plan) => plan,
                Err(response) => return response,
            };
            (plan, settings)
        };
        let body = match json_body(request, settings.max_body).await {
            Ok(body) => body,
            Err(response) => return response,
        };
        let Some(sql) = body.get("sql").and_then(Value::as_str).map(str::to_owned) else {
            return refuse(
                StatusCode::BAD_REQUEST,
                "400 Bad Request: the body is {\"sql\": \"SELECT …\", \"params\": [...]}",
            );
        };
        let mut params = Vec::new();
        match body.get("params") {
            None | Some(Value::Null) => {}
            Some(Value::Array(items)) => {
                for (index, item) in items.iter().enumerate() {
                    match query::bind(index, item) {
                        Ok(value) => params.push(value),
                        Err(error) => {
                            return refuse(
                                StatusCode::BAD_REQUEST,
                                format!("400 Bad Request: {error}"),
                            );
                        }
                    }
                }
            }
            Some(_) => {
                return refuse(
                    StatusCode::BAD_REQUEST,
                    "400 Bad Request: params must be an array",
                );
            }
        }
        let keyword = leading_keyword(&sql);
        if keyword != "SELECT" && keyword != "WITH" {
            return refuse(
                StatusCode::BAD_REQUEST,
                "400 Bad Request: the statement must be a single SELECT or WITH … SELECT \
                 (spec/data-api.md §1)",
            );
        }
        let outcome = tokio::task::spawn_blocking(move || {
            let result = query::run(&plan.conn, &plan.schema, plan.deadline, &sql, params)?;
            Ok::<_, String>((result, plan.lam))
        })
        .await;
        match outcome {
            Ok(Ok((result, lam))) => headers::json(
                StatusCode::OK,
                &json!({
                    "columns": columns_json(&result.columns),
                    "rows": result.rows,
                    "lam": lam,
                }),
            ),
            Ok(Err(error)) => refuse(StatusCode::BAD_REQUEST, format!("400 Bad Request: {error}")),
            Err(join) => refuse(
                StatusCode::INTERNAL_SERVER_ERROR,
                format!("500 Internal Server Error: the query's thread panicked: {join}"),
            ),
        }
    }

    /// `GET /api/row/<tbl>/<id>` (`spec/data-api.md §1`): the row's winning event — its
    /// log line, `d` holding the row — or 404 when there is none or it is a tombstone.
    async fn api_row(&self, slug: &str, tbl: &str, id: &str) -> Response {
        if !is_table_name(tbl) {
            return refuse(
                StatusCode::BAD_REQUEST,
                format!("400 Bad Request: {tbl:?} is not a table name"),
            );
        }
        let log_dir = {
            let node = self.lock();
            let Some(app) = node.app(slug) else {
                return not_found(&format!("no app is mounted as {slug}"));
            };
            app.log().log_dir().to_path_buf()
        };
        let (slug, tbl, id) = (slug.to_owned(), tbl.to_owned(), id.to_owned());
        let winner = tokio::task::spawn_blocking(move || {
            read_lines(&log_dir, &slug)
                .into_iter()
                .filter(|line| line.tbl == tbl && line.id == id)
                .next_back()
        })
        .await;
        match winner {
            Ok(Some(line)) if line.put => {
                headers::with_body(StatusCode::OK, headers::JSON, line.line)
            }
            Ok(_) => not_found("no such row, or its winning event is a tombstone"),
            Err(join) => refuse(
                StatusCode::INTERNAL_SERVER_ERROR,
                format!("500 Internal Server Error: {join}"),
            ),
        }
    }

    /// `GET /api/events` (`spec/data-api.md §1`): raw lines as NDJSON, byte-identical to
    /// the log, in `(lam, ts, dev)` order, filtered by `tbl`, `id` and `after`.
    async fn api_events(&self, slug: &str, query: BTreeMap<String, String>) -> Response {
        let (log_dir, settings) = {
            let node = self.lock();
            let Some(app) = node.app(slug) else {
                return not_found(&format!("no app is mounted as {slug}"));
            };
            (app.log().log_dir().to_path_buf(), ApiSettings::read(&node))
        };
        let (limit, offset) = match paging(&query, settings.max_rows) {
            Ok(paging) => paging,
            Err(response) => return response,
        };
        let after = match query.get("after") {
            None => 0,
            Some(text) => match text.trim().parse::<u64>() {
                Ok(after) => after,
                Err(_) => {
                    return refuse(
                        StatusCode::BAD_REQUEST,
                        format!("400 Bad Request: after must be a lam, not {text:?}"),
                    );
                }
            },
        };
        let tbl = query.get("tbl").cloned();
        let id = query.get("id").cloned();
        let slug = slug.to_owned();
        let body = tokio::task::spawn_blocking(move || {
            let mut out = Vec::new();
            for line in read_lines(&log_dir, &slug)
                .into_iter()
                .filter(|line| line.lam > after)
                .filter(|line| tbl.as_deref().is_none_or(|tbl| line.tbl == tbl))
                .filter(|line| id.as_deref().is_none_or(|id| line.id == id))
                .skip(offset)
                .take(limit)
            {
                out.extend_from_slice(&line.line);
                out.push(b'\n');
            }
            out
        })
        .await;
        match body {
            Ok(body) => headers::with_body(StatusCode::OK, headers::NDJSON, body),
            Err(join) => refuse(
                StatusCode::INTERNAL_SERVER_ERROR,
                format!("500 Internal Server Error: {join}"),
            ),
        }
    }

    /// `POST /api/events` (`spec/data-api.md §2`): a batch, all or nothing, through
    /// [`Node::append`] like every other writer — typed writes, `NOT NULL` and `CHECK`
    /// — after the API's own checks: the four fields and no other, a ULID for an id, and
    /// no minted id reused for a row that was deleted (`spec/protocol.md §4.6`).
    async fn api_append(&self, slug: &str, request: Request) -> Response {
        if !is_json(&request) {
            return refuse(
                StatusCode::UNSUPPORTED_MEDIA_TYPE,
                "415 Unsupported Media Type: send application/json (spec/data-api.md §2)",
            );
        }
        let (settings, device) = {
            let node = self.lock();
            if node.app(slug).is_none() {
                return not_found(&format!("no app is mounted as {slug}"));
            }
            (
                ApiSettings::read(&node),
                request
                    .extensions()
                    .get::<Device>()
                    .map(|device| device.0.as_str().to_owned()),
            )
        };
        let body = match json_body(request, settings.max_body).await {
            Ok(body) => body,
            Err(response) => return response,
        };
        let Some(events) = body.get("events").and_then(Value::as_array) else {
            return refuse(
                StatusCode::BAD_REQUEST,
                "400 Bad Request: the body is {\"events\": [{\"op\", \"tbl\", \"id\", \"d\"}, …]}",
            );
        };
        if events.len() > settings.max_batch {
            return refuse(
                StatusCode::BAD_REQUEST,
                format!(
                    "400 Bad Request: {} events in one batch; the most is {} (api.max_batch)",
                    events.len(),
                    settings.max_batch
                ),
            );
        }
        let mut parsed = Vec::with_capacity(events.len());
        for (index, event) in events.iter().enumerate() {
            match parse_event(index, event) {
                Ok(item) => parsed.push(item),
                Err(response) => return response,
            }
        }

        let appended = {
            let mut node = self.lock();
            let Some(app) = node.app(slug) else {
                return not_found(&format!("no app is mounted as {slug}"));
            };
            // `§4.6`: a minted id that belonged to a deleted row must not key another —
            // in the cache's tombstone set, or deleted earlier in this very batch.
            let mut deleted_here: Vec<&str> = Vec::new();
            for (index, item) in parsed.iter().enumerate() {
                let Event { tbl, id, d } = &item.change;
                let reuse = d.is_some()
                    && item.client_id
                    && (deleted_here.contains(&id.as_str())
                        || app.store().is_tombstoned(tbl, id).unwrap_or(false));
                if reuse {
                    let mut detail = Map::new();
                    detail.insert("index".to_owned(), Value::from(index));
                    return refuse_with(
                        StatusCode::CONFLICT,
                        detail,
                        format!(
                            "409 Conflict: events[{index}]: {tbl}/{id} was deleted, and a minted \
                             id is never reused for another row (spec/protocol.md §4.6); mint a \
                             fresh one with pv.ulid()"
                        ),
                    );
                }
                if d.is_none() {
                    deleted_here.push(id.as_str());
                }
            }
            let changes: Vec<Event> = parsed.into_iter().map(|item| item.change).collect();
            match node.append_batch(slug, changes) {
                Ok(appended) => appended,
                Err(Error::Value {
                    index,
                    column,
                    problem,
                    tbl,
                    ..
                }) => {
                    let mut detail = Map::new();
                    detail.insert("index".to_owned(), Value::from(index));
                    detail.insert("column".to_owned(), Value::String(column.clone()));
                    return refuse_with(
                        StatusCode::BAD_REQUEST,
                        detail,
                        format!("400 Bad Request: events[{index}]: {tbl}.{column}: {problem}"),
                    );
                }
                Err(Error::Constraint {
                    index,
                    problem,
                    tbl,
                    ..
                }) => {
                    let mut detail = Map::new();
                    detail.insert("index".to_owned(), Value::from(index));
                    return refuse_with(
                        StatusCode::BAD_REQUEST,
                        detail,
                        format!("400 Bad Request: events[{index}]: {tbl}: {problem}"),
                    );
                }
                Err(error) => {
                    return refuse(
                        StatusCode::INTERNAL_SERVER_ERROR,
                        format!("500 Internal Server Error: {error}"),
                    );
                }
            }
        };
        let count = appended.events.len();
        let ids: Vec<&str> = appended
            .events
            .iter()
            .map(|change| change.id.as_str())
            .collect();
        let lam = appended.lam.saturating_add(count as u64).saturating_sub(1);
        let response = headers::json(
            StatusCode::OK,
            &json!({ "appended": count, "lam": lam, "ids": ids }),
        );
        // This node's own append, so a Tier 1 app's `pv.on('append')` sees it
        // (`spec/lua-api.md §3.4`) — after the lock is released, since a handler may
        // append in turn.
        if count > 0 {
            self.fire_append(slug, appended, device).await;
        }
        response
    }

    /// `GET /api/stream` (`spec/data-api.md §3`): SSE. Subscribed and the backlog read
    /// under one hold of the lock, so an append cannot land between the two, then pumped
    /// through a channel by a task for as long as the client reads.
    fn api_stream(&self, slug: &str, request: &Request, head: bool) -> Response {
        let query = query_of(request);
        let after = match query.get("after") {
            None => None,
            Some(text) => match text.trim().parse::<u64>() {
                Ok(after) => Some(after),
                Err(_) => {
                    return refuse(
                        StatusCode::BAD_REQUEST,
                        format!("400 Bad Request: after must be a lam, not {text:?}"),
                    );
                }
            },
        };
        let node = self.lock();
        let Some(app) = node.app(slug) else {
            return not_found(&format!("no app is mounted as {slug}"));
        };
        let mut response = headers::with_body(StatusCode::OK, headers::EVENT_STREAM, Body::empty());
        if head {
            return response;
        }
        let settings = ApiSettings::read(&node);
        let device = device_of(request, node.id().as_str());
        {
            let mut streams = self
                .api
                .streams
                .lock()
                .unwrap_or_else(PoisonError::into_inner);
            let open = streams.entry(device.clone()).or_insert(0);
            if *open >= settings.max_streams {
                return refuse(
                    StatusCode::TOO_MANY_REQUESTS,
                    format!(
                        "429 Too Many Requests: {} streams are open for this device; the most is {} \
                         (api.max_streams)",
                        open, settings.max_streams
                    ),
                );
            }
            *open += 1;
        }
        let slot = StreamSlot {
            device,
            streams: Arc::clone(&self.api.streams),
        };
        let receiver = app.stream().subscribe();
        let backlog: Vec<Bytes> = match after {
            Some(after) => read_lines(app.log().log_dir(), slug)
                .into_iter()
                .filter(|line| line.lam > after)
                .map(|line| Bytes::from(line.line))
                .collect(),
            None => Vec::new(),
        };
        let resume_from = after.unwrap_or_else(|| app.log().lam());
        drop(node);

        let (tx, rx) = mpsc::channel::<Bytes>(64);
        tokio::spawn(pump(
            tx,
            receiver,
            backlog,
            Arc::clone(&self.node),
            slug.to_owned(),
            self.api.ping,
            slot,
            resume_from,
        ));
        *response.body_mut() = Body::from_stream(ChannelBody(rx));
        response
    }

    /// `GET /api/schema` (`spec/data-api.md §4`).
    fn api_schema(&self, slug: &str) -> Response {
        let node = self.lock();
        let Some(app) = node.app(slug) else {
            return not_found(&format!("no app is mounted as {slug}"));
        };
        let schema = app.store().schema();
        let tables: Vec<Value> = schema
            .tables
            .iter()
            .map(|table| {
                let mut columns =
                    vec![json!({ "name": "id", "type": "VARCHAR", "not_null": true })];
                columns.extend(table.columns.iter().map(|column| {
                    json!({ "name": column.name, "type": column.ty, "not_null": column.not_null })
                }));
                json!({ "name": table.name, "columns": columns })
            })
            .collect();
        let views: Vec<Value> = schema
            .views
            .iter()
            .map(|view| json!({ "name": view.name, "params": view.params }))
            .collect();
        headers::json(
            StatusCode::OK,
            &json!({ "tables": tables, "views": views, "schema_hash": schema.hash }),
        )
    }

    /// `GET /api/node` (`spec/data-api.md §4`): no application data. `app` is the slug
    /// this API is scoped to — what a page at a solo mount cannot read from its path,
    /// and what `pv.js` keys its outbox by (`§6`).
    fn api_node(&self, slug: &str, request: &Request) -> Response {
        let node = self.lock();
        if node.app(slug).is_none() {
            return not_found(&format!("no app is mounted as {slug}"));
        }
        let facts = node_facts(&node, slug);
        let device = device_of(request, &facts.id);
        headers::json(
            StatusCode::OK,
            &json!({
                "id": facts.id,
                "dev": device,
                "name": facts.name,
                "app": slug,
                "solo": facts.solo,
                "peers": 0,
                "restore_tier": facts.restore_tier,
            }),
        )
    }
}

/// The stream's producer: the backlog first, then whatever the app broadcasts, a ping on
/// every tick — with a stat of the app under the lock, so a log that grew or a
/// `schema.sql` that changed is noticed on an idle node and goes out as a resync — until
/// the client stops reading.
#[allow(clippy::too_many_arguments)]
async fn pump(
    tx: mpsc::Sender<Bytes>,
    mut events: broadcast::Receiver<StreamEvent>,
    backlog: Vec<Bytes>,
    node: Arc<Mutex<Node>>,
    slug: String,
    ping: Duration,
    _slot: StreamSlot,
    mut sent: u64,
) {
    for line in backlog {
        if tx.send(frame("append", &line)).await.is_err() {
            return;
        }
    }
    let mut ticker = tokio::time::interval(ping);
    ticker.tick().await;
    loop {
        tokio::select! {
            () = tx.closed() => return,
            _ = ticker.tick() => {
                let lam = {
                    let mut node = node.lock().unwrap_or_else(PoisonError::into_inner);
                    let _ = node.refresh_app(&slug);
                    node.app(&slug).map_or(0, |app| app.log().lam())
                };
                let data = json!({ "lam": lam }).to_string();
                if tx.send(frame("ping", data.as_bytes())).await.is_err() {
                    return;
                }
            }
            received = events.recv() => match received {
                Ok(StreamEvent::Append { lam, line }) => {
                    if lam <= sent {
                        continue;
                    }
                    sent = lam;
                    if tx.send(frame("append", &line)).await.is_err() {
                        return;
                    }
                }
                Ok(StreamEvent::Resync { reason, lam }) => {
                    let data = json!({ "reason": reason, "lam": lam }).to_string();
                    if tx.send(frame("resync", data.as_bytes())).await.is_err() {
                        return;
                    }
                }
                Err(broadcast::error::RecvError::Lagged(_)) => {
                    let lam = {
                        let node = node.lock().unwrap_or_else(PoisonError::into_inner);
                        node.app(&slug).map_or(0, |app| app.log().lam())
                    };
                    let data = json!({ "reason": "lagged", "lam": lam }).to_string();
                    if tx.send(frame("resync", data.as_bytes())).await.is_err() {
                        return;
                    }
                }
                Err(broadcast::error::RecvError::Closed) => return,
            }
        }
    }
}

// AGENTS.md, Style: unwrap() is permitted in tests. The crate-level deny reaches unit
// tests inside src/, so each one opts out where it is declared.
#[allow(clippy::unwrap_used, clippy::expect_used)]
#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn statements_are_recognised_past_comments() {
        assert_eq!(leading_keyword("  select 1"), "SELECT");
        assert_eq!(
            leading_keyword("-- note\n/* x */ WITH t AS (SELECT 1) SELECT * FROM t"),
            "WITH"
        );
        assert_eq!(leading_keyword("PRAGMA x"), "PRAGMA");
        assert_eq!(leading_keyword(""), "");
    }

    #[test]
    fn a_bucket_refills_at_its_rate() {
        let buckets = Mutex::new(HashMap::new());
        assert!(take_token(&buckets, "d", 2));
        assert!(take_token(&buckets, "d", 2));
        assert!(!take_token(&buckets, "d", 2));
        assert!(take_token(&buckets, "other", 2));
    }

    #[test]
    fn a_frame_is_one_sse_message() {
        assert_eq!(
            frame("append", b"{\"a\":1}"),
            Bytes::from_static(b"event: append\ndata: {\"a\":1}\n\n")
        );
    }

    #[test]
    fn events_are_parsed_strictly() {
        let ok = json!({ "op": "put", "tbl": "stroke", "d": { "x": 1 } });
        let parsed = parse_event(0, &ok).unwrap();
        assert!(!parsed.client_id);
        assert!(is_ulid(&parsed.change.id));
        for bad in [
            json!({ "op": "put", "tbl": "t", "d": {}, "seq": 1 }),
            json!({ "op": "put", "tbl": "t", "d": {}, "extra": 1 }),
            json!({ "op": "upsert", "tbl": "t", "d": {} }),
            json!({ "op": "put", "tbl": "no-dash", "d": {} }),
            json!({ "op": "put", "tbl": "t", "id": "cursor", "d": {} }),
            json!({ "op": "put", "tbl": "t" }),
            json!({ "op": "del", "tbl": "t" }),
            json!({ "op": "del", "tbl": "t", "id": "01J9YQ2W7C8XKF3M0N5RTVB6ZP", "d": {} }),
            json!([]),
        ] {
            assert!(parse_event(3, &bad).is_err(), "{bad}");
        }
    }
}
