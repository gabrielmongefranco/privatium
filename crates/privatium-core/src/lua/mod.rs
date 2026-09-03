// Project:  Privatium™  |  File: crates/privatium-core/src/lua/mod.rs
// Authors:  Gabriel Mongefranco (@gabrielmongefranco)
// Created:  2026-09-03  |  Modified: 2026-09-03
// Summary:  The Lua host (spec/lua-api.md, docs/plans/phase-1.md M7): one pool of sandboxed
//           VMs per Tier 1 app, every VM loading app.lua identically so the router can hold
//           (method, pattern, index) from VM 0 (§2.4); one request holds one VM on a
//           blocking thread with a read-only connection of its own; the four limits armed
//           per run; a VM that trips one is discarded and rebuilt on the next checkout.

use std::collections::{BTreeMap, VecDeque};
use std::fmt;
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Condvar, Mutex, PoisonError};

use mlua::chunk::ChunkMode;
use mlua::{Function, Lua, Table, Value};
use rusqlite::Connection;

use crate::Node;
use crate::app::Appended;
use crate::config::LuaConfig;
use crate::store::Schema;

mod convert;
pub mod dec;
pub mod limits;
mod pv;
mod sandbox;

pub use limits::{LimitKind, Limits};

/// One registered route: `(method, pattern)` in registration order.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RouteSpec {
    /// Upper-case.
    pub method: String,
    /// As written, `/fill/:id`.
    pub pattern: String,
    segments: Vec<Segment>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
enum Segment {
    Literal(String),
    Param(String),
}

impl RouteSpec {
    /// Parse a pattern: `/` or `/seg/:param/seg`, no empty segments.
    pub fn parse(method: &str, pattern: &str) -> Result<Self, String> {
        if method.is_empty() || !method.bytes().all(|b| b.is_ascii_uppercase()) {
            return Err(format!("pv.route: {method:?} is not an HTTP method"));
        }
        let Some(rest) = pattern.strip_prefix('/') else {
            return Err(format!("route {pattern:?} must start with '/'"));
        };
        let mut segments = Vec::new();
        if !rest.is_empty() {
            for segment in rest.split('/') {
                if segment.is_empty() {
                    return Err(format!("route {pattern:?} has an empty segment"));
                }
                if let Some(name) = segment.strip_prefix(':') {
                    if name.is_empty() {
                        return Err(format!("route {pattern:?} has an unnamed parameter"));
                    }
                    segments.push(Segment::Param(name.to_owned()));
                } else {
                    segments.push(Segment::Literal(segment.to_owned()));
                }
            }
        }
        Ok(Self {
            method: method.to_owned(),
            pattern: pattern.to_owned(),
            segments,
        })
    }

    fn matches(&self, path: &[String]) -> Option<Vec<(String, String)>> {
        if path.len() != self.segments.len() {
            return None;
        }
        let mut params = Vec::new();
        for (segment, actual) in self.segments.iter().zip(path) {
            match segment {
                Segment::Literal(literal) if literal == actual => {}
                Segment::Literal(_) => return None,
                Segment::Param(name) => params.push((name.clone(), actual.clone())),
            }
        }
        Some(params)
    }
}

/// Where a request path leads within an app.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Resolved {
    /// Handler `index`, with the `:params` bound.
    Match {
        /// The route index (`§2.4`).
        index: usize,
        /// Name and decoded value, in pattern order.
        params: Vec<(String, String)>,
    },
    /// A pattern matched but not with this method.
    MethodNotAllowed {
        /// The methods that would have.
        allow: Vec<String>,
    },
    /// Nothing registered here.
    NotFound,
}

/// What `core::handle` hands a handler.
#[derive(Debug, Clone, Default)]
pub struct LuaRequest {
    /// Upper-case.
    pub method: String,
    /// The path beneath the mount, `/` for the mount point.
    pub path: String,
    /// The bound `:params`.
    pub params: Vec<(String, String)>,
    /// The query string, decoded.
    pub query: BTreeMap<String, String>,
    /// A form-encoded body, decoded.
    pub form: BTreeMap<String, String>,
    /// The body as received, bounded by `http::FORM_LIMIT` — Tier 1 does not stream.
    pub body: Vec<u8>,
    /// Lower-case names.
    pub headers: Vec<(String, String)>,
    /// The paired device's ID; this node's in Phase 1.
    pub device: String,
    /// Whether `HX-Request` was present.
    pub is_htmx: bool,
}

/// What a handler answered (`spec/lua-api.md §3.1`).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum LuaResponse {
    /// A string: `text/html`.
    Html(Vec<u8>),
    /// `pv.text`.
    Text(Vec<u8>),
    /// `pv.json`, already serialized.
    Json(String),
    /// `pv.redirect`: 303 to this location, exactly as given.
    Redirect(String),
    /// `pv.render`: the view name. Templates are M8's; until then the wire layer answers
    /// for this.
    Render(String),
    /// `nil`: 204.
    NoContent,
}

/// Why a run did not produce a response.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RunError {
    /// A limit of `§5` was exceeded. The request is a 500 and an audit row; the VM has been
    /// discarded.
    Limit {
        /// Which.
        kind: LimitKind,
        /// What the hook or the allocator said.
        detail: String,
    },
    /// The handler raised, or returned something that is not a response.
    Lua(String),
    /// The pool could not supply a VM.
    Pool(String),
}

impl fmt::Display for RunError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Limit { kind, detail } => write!(f, "{kind} limit exceeded: {detail}"),
            Self::Lua(message) | Self::Pool(message) => f.write_str(message),
        }
    }
}

/// Facts about the node a handler may ask for through `pv.node()`, taken under the node
/// lock when the request starts so the VM never needs it for them.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NodeFacts {
    /// The Node ID.
    pub id: String,
    /// `sys_node.display_name`, or the Node ID while unset (`spec/protocol.md §9.2`).
    pub name: String,
    /// `[node] mode = "solo"`.
    pub solo: bool,
    /// Which tier built this app's cache, if any has.
    pub restore_tier: Option<u8>,
}

/// The `ui.*` settings `fmt.*` reads (`spec/data-dictionary.md §3.6`).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct UiSettings {
    /// `ui.locale`, `en-US` by default.
    pub locale: String,
    /// `ui.date_format`: `iso`, `us` or `eu`.
    pub date_format: String,
}

impl Default for UiSettings {
    fn default() -> Self {
        Self {
            locale: "en-US".to_owned(),
            date_format: "iso".to_owned(),
        }
    }
}

/// Everything one run needs that the VM does not own: the connection app SQL runs on, the
/// node for appends and settings, and the facts taken at the start.
pub struct RequestCtx {
    /// `Store::app_conn()`, fresh for this request and dropped with it.
    pub conn: Connection,
    /// The node, locked briefly by `pv.append`, `pv.batch` and `pv.setting`.
    pub node: Arc<Mutex<Node>>,
    /// `pv.node()`.
    pub facts: NodeFacts,
    /// `req.device` and `pv.device()`.
    pub device: String,
    /// `fmt.*`.
    pub ui: UiSettings,
}

impl fmt::Debug for RequestCtx {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("RequestCtx")
            .field("facts", &self.facts)
            .field("device", &self.device)
            .finish_non_exhaustive()
    }
}

/// Where a VM is in its life.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Phase {
    /// `app.lua` is running: routes and `pv.on` register, nothing reads or writes.
    Loading,
    /// In the pool.
    Idle,
    /// Checked out for a request or an append dispatch.
    Request,
}

/// The per-VM state the `pv` functions reach through `Lua::app_data`.
pub(crate) struct VmData {
    pub slug: String,
    pub mount: String,
    /// `lib/`, canonical, when the folder has one.
    pub lib_dir: Option<PathBuf>,
    pub schema: Arc<Schema>,
    pub phase: Phase,
    /// Registered so far, in order.
    pub routes: Vec<RouteSpec>,
    pub ctx: Option<RequestCtx>,
    /// The events staged by the `pv.batch` in progress.
    pub batch: Option<Vec<crate::app::Change>>,
}

struct Vm {
    lua: Lua,
    limits: Arc<Limits>,
}

struct Pool {
    free: VecDeque<Vm>,
    /// VMs in existence, in the pool or checked out.
    live: usize,
}

/// One Tier 1 app's VM pool and route table.
pub struct Host {
    slug: String,
    dir: PathBuf,
    mount: String,
    config: LuaConfig,
    schema: Arc<Schema>,
    routes: Vec<RouteSpec>,
    pool: Mutex<Pool>,
    returned: Condvar,
}

impl fmt::Debug for Host {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Host")
            .field("slug", &self.slug)
            .field("routes", &self.routes.len())
            .field("pool_size", &self.config.pool_size)
            .finish_non_exhaustive()
    }
}

impl Host {
    /// Load `app.lua` into `lua.pool_size` VMs and take the route table from VM 0. Any VM
    /// whose table differs fails the load, naming the divergence (`§2.4`).
    ///
    /// `mount` is the app's mount for `url()`; an app that is loaded but not served (a
    /// solo node's other apps) gets its host-mode mount so `url()` still answers.
    pub fn build(
        slug: &str,
        dir: &Path,
        mount: Option<&str>,
        schema: Schema,
        config: &LuaConfig,
    ) -> Result<Self, String> {
        let mount = mount.map_or_else(|| format!("/a/{slug}/"), str::to_owned);
        let mut host = Self {
            slug: slug.to_owned(),
            dir: dir.to_path_buf(),
            mount,
            config: config.clone(),
            schema: Arc::new(schema),
            routes: Vec::new(),
            pool: Mutex::new(Pool {
                free: VecDeque::new(),
                live: 0,
            }),
            returned: Condvar::new(),
        };
        let size = host.size();
        let mut vms = VecDeque::with_capacity(size);
        for index in 0..size {
            let (vm, routes) = host.build_vm()?;
            if index == 0 {
                host.routes = routes;
            } else {
                same_routes(&host.routes, &routes, index)?;
            }
            vms.push_back(vm);
        }
        host.pool = Mutex::new(Pool {
            live: vms.len(),
            free: vms,
        });
        Ok(host)
    }

    /// `lua.pool_size`, at least one.
    #[must_use]
    pub fn size(&self) -> usize {
        self.config.pool_size.max(1)
    }

    /// The app.
    #[must_use]
    pub fn slug(&self) -> &str {
        &self.slug
    }

    /// The route table, in registration order — what the router holds (`§2.4`).
    #[must_use]
    pub fn routes(&self) -> &[RouteSpec] {
        &self.routes
    }

    /// Resolve a method and a path beneath the mount against the route table. `HEAD`
    /// matches a `GET` route. The first registered match wins.
    #[must_use]
    pub fn resolve(&self, method: &str, path: &str) -> Resolved {
        let segments: Vec<String> = path
            .trim_start_matches('/')
            .split('/')
            .filter(|segment| !segment.is_empty())
            .map(|segment| crate::http::percent_decode(segment.as_bytes()))
            .collect();
        let method = method.to_ascii_uppercase();
        let mut allow = Vec::new();
        for (index, route) in self.routes.iter().enumerate() {
            let Some(params) = route.matches(&segments) else {
                continue;
            };
            if route.method == method || (method == "HEAD" && route.method == "GET") {
                return Resolved::Match { index, params };
            }
            if !allow.contains(&route.method) {
                allow.push(route.method.clone());
            }
        }
        if allow.is_empty() {
            Resolved::NotFound
        } else {
            Resolved::MethodNotAllowed { allow }
        }
    }

    /// Run handler `index` for `request` on a checked-out VM. Blocking: call it from a
    /// blocking thread, never under the node lock.
    pub fn run(
        &self,
        index: usize,
        request: LuaRequest,
        ctx: RequestCtx,
    ) -> Result<LuaResponse, RunError> {
        let vm = self.checkout().map_err(RunError::Pool)?;
        let outcome = vm.run(ctx, |lua| {
            let routes: Table = lua.named_registry_value(pv::ROUTES_KEY)?;
            let handler: Function = routes.raw_get(index + 1)?;
            let req = request_table(lua, &request)?;
            let value: Value = handler.call(req)?;
            response_of(&value)
        });
        self.finish(vm, outcome)
    }

    /// Fire `pv.on('append')` for events appended outside a handler — the seed.
    pub fn fire(&self, appended: &Appended, ctx: RequestCtx) -> Result<(), RunError> {
        let vm = self.checkout().map_err(RunError::Pool)?;
        let outcome = vm.run(ctx, |lua| pv::fire_append(lua, appended));
        self.finish(vm, outcome)
    }

    /// Return the VM to the pool, or discard it if it tripped a limit.
    fn finish<T>(&self, vm: Vm, outcome: Result<T, RunError>) -> Result<T, RunError> {
        match &outcome {
            Err(RunError::Limit { .. }) => self.discard(),
            _ => self.checkin(vm),
        }
        outcome
    }

    fn build_vm(&self) -> Result<(Vm, Vec<RouteSpec>), String> {
        let lua = sandbox::new_state(&self.config).map_err(|error| error.to_string())?;
        let limits = Arc::new(Limits::new(&self.config));
        limits.arm();
        limits::install(&lua, Arc::clone(&limits)).map_err(|error| error.to_string())?;
        lua.set_app_data(VmData {
            slug: self.slug.clone(),
            mount: self.mount.clone(),
            lib_dir: fs::canonicalize(self.dir.join("lib")).ok(),
            schema: Arc::clone(&self.schema),
            phase: Phase::Loading,
            routes: Vec::new(),
            ctx: None,
            batch: None,
        });
        pv::install(&lua).map_err(|error| error.to_string())?;
        sandbox::install_globals(&lua).map_err(|error| error.to_string())?;

        let path = self.dir.join("app.lua");
        let source = fs::read(&path).map_err(|error| format!("app.lua: {error}"))?;
        let loaded = lua
            .load(source)
            .set_name("@app.lua")
            .set_mode(ChunkMode::Text)
            .exec();
        if let Some(kind) = limits.tripped() {
            return Err(format!(
                "app.lua: the {kind} limit was exceeded while loading (spec/lua-api.md §5)"
            ));
        }
        if let Err(error) = loaded {
            if limits::is_memory_error(&error) {
                return Err(
                    "app.lua: the memory limit was exceeded while loading (spec/lua-api.md §5)"
                        .to_owned(),
                );
            }
            return Err(format!("app.lua: {error}"));
        }
        let routes = {
            let mut data = lua
                .app_data_mut::<VmData>()
                .ok_or_else(|| "app.lua: the VM lost its state".to_owned())?;
            data.phase = Phase::Idle;
            data.routes.clone()
        };
        Ok((Vm { lua, limits }, routes))
    }

    /// Take a VM: the least recently used free one, or a fresh one while the pool is below
    /// size, or wait for a return.
    fn checkout(&self) -> Result<Vm, String> {
        let mut pool = self.pool.lock().unwrap_or_else(PoisonError::into_inner);
        loop {
            if let Some(vm) = pool.free.pop_front() {
                return Ok(vm);
            }
            if pool.live < self.size() {
                pool.live += 1;
                drop(pool);
                return match self.build_vm() {
                    Ok((vm, routes)) => match same_routes(&self.routes, &routes, usize::MAX) {
                        Ok(()) => Ok(vm),
                        Err(divergence) => {
                            self.discard();
                            Err(format!(
                                "the rebuilt VM registered different routes — reload the app: \
                                 {divergence}"
                            ))
                        }
                    },
                    Err(error) => {
                        self.discard();
                        Err(format!(
                            "the VM could not be rebuilt — reload the app: {error}"
                        ))
                    }
                };
            }
            pool = self
                .returned
                .wait(pool)
                .unwrap_or_else(PoisonError::into_inner);
        }
    }

    fn checkin(&self, vm: Vm) {
        let mut pool = self.pool.lock().unwrap_or_else(PoisonError::into_inner);
        pool.free.push_back(vm);
        self.returned.notify_one();
    }

    fn discard(&self) {
        let mut pool = self.pool.lock().unwrap_or_else(PoisonError::into_inner);
        pool.live = pool.live.saturating_sub(1);
        self.returned.notify_one();
    }
}

impl Vm {
    /// Arm the limits, install the context, run `body`, take the context back, and say
    /// what happened. The connection's progress handler enforces the same deadline while
    /// a statement runs in Rust, where the hook cannot fire.
    fn run<T>(
        &self,
        ctx: RequestCtx,
        body: impl FnOnce(&Lua) -> mlua::Result<T>,
    ) -> Result<T, RunError> {
        self.limits.arm();
        let progress = Arc::clone(&self.limits);
        if let Err(error) = ctx
            .conn
            .progress_handler(1000, Some(move || progress.over_time()))
        {
            return Err(RunError::Lua(format!(
                "could not install the statement deadline: {error}"
            )));
        }
        {
            let Some(mut data) = self.lua.app_data_mut::<VmData>() else {
                return Err(RunError::Lua("the VM lost its state".to_owned()));
            };
            data.ctx = Some(ctx);
            data.phase = Phase::Request;
            data.batch = None;
        }
        let result = body(&self.lua);
        // Take the context back whatever happened: the connection closes here, and a
        // handler that errored must not leave a half-built batch behind.
        let taken = self.lua.app_data_mut::<VmData>().map(|mut data| {
            data.phase = Phase::Idle;
            data.batch = None;
            data.ctx.take()
        });
        drop(taken);

        let detail = match &result {
            Ok(_) => "the handler returned after the limit tripped".to_owned(),
            Err(error) => error.to_string(),
        };
        if let Some(kind) = self.limits.tripped() {
            return Err(RunError::Limit { kind, detail });
        }
        match result {
            Ok(value) => Ok(value),
            Err(error) if limits::is_memory_error(&error) => {
                self.limits.trip(LimitKind::Memory);
                Err(RunError::Limit {
                    kind: LimitKind::Memory,
                    detail,
                })
            }
            Err(error) => Err(RunError::Lua(error.to_string())),
        }
    }
}

/// Whether two route tables agree by method, pattern and count; if not, which index
/// differs and how (`§2.4`).
fn same_routes(first: &[RouteSpec], other: &[RouteSpec], vm: usize) -> Result<(), String> {
    let name = if vm == usize::MAX {
        "a rebuilt VM".to_owned()
    } else {
        format!("VM {vm}")
    };
    if first.len() != other.len() {
        return Err(format!(
            "app.lua registers routes non-deterministically: {name} registered {} route(s) \
             where VM 0 registered {} (docs/plans/phase-1.md §2.4)",
            other.len(),
            first.len()
        ));
    }
    for (index, (a, b)) in first.iter().zip(other).enumerate() {
        if a.method != b.method || a.pattern != b.pattern {
            return Err(format!(
                "app.lua registers routes non-deterministically: at index {index} {name} \
                 registered {} {} where VM 0 registered {} {} (docs/plans/phase-1.md §2.4)",
                b.method, b.pattern, a.method, a.pattern
            ));
        }
    }
    Ok(())
}

/// The `req` table of `spec/lua-api.md §3.1`.
fn request_table(lua: &Lua, request: &LuaRequest) -> mlua::Result<Table> {
    let req = lua.create_table()?;
    req.raw_set("method", request.method.as_str())?;
    req.raw_set("path", request.path.as_str())?;
    let params = lua.create_table()?;
    for (name, value) in &request.params {
        params.raw_set(name.as_str(), value.as_str())?;
    }
    req.raw_set("params", params)?;
    let query = lua.create_table()?;
    for (name, value) in &request.query {
        query.raw_set(name.as_str(), value.as_str())?;
    }
    req.raw_set("query", query)?;
    let form = lua.create_table()?;
    for (name, value) in &request.form {
        form.raw_set(name.as_str(), value.as_str())?;
    }
    req.raw_set("form", form)?;
    req.raw_set("body", lua.create_string(&request.body)?)?;
    let headers = lua.create_table()?;
    for (name, value) in &request.headers {
        headers.raw_set(name.as_str(), value.as_str())?;
    }
    req.raw_set("headers", headers)?;
    req.raw_set("device", request.device.as_str())?;
    req.raw_set("is_htmx", request.is_htmx)?;
    Ok(req)
}

/// A handler's return value as a response (`§3.1`'s table).
fn response_of(value: &Value) -> mlua::Result<LuaResponse> {
    match value {
        Value::Nil => Ok(LuaResponse::NoContent),
        Value::String(html) => Ok(LuaResponse::Html(html.as_bytes().to_vec())),
        Value::Table(table) => {
            let kind: Option<String> = table.raw_get(pv::RESPONSE_FIELD)?;
            match kind.as_deref() {
                Some("text") => {
                    let body: mlua::LuaString = table.raw_get("body")?;
                    Ok(LuaResponse::Text(body.as_bytes().to_vec()))
                }
                Some("json") => Ok(LuaResponse::Json(table.raw_get("body")?)),
                Some("redirect") => Ok(LuaResponse::Redirect(table.raw_get("location")?)),
                Some("render") => Ok(LuaResponse::Render(table.raw_get("view")?)),
                _ => Err(mlua::Error::runtime(
                    "the handler returned a table that is not pv.render, pv.redirect, \
                     pv.json or pv.text (spec/lua-api.md §3.1)",
                )),
            }
        }
        other => Err(mlua::Error::runtime(format!(
            "the handler returned a value of type {}; return pv.render, pv.redirect, \
             pv.json, pv.text, a string or nil (spec/lua-api.md §3.1)",
            other.type_name()
        ))),
    }
}
