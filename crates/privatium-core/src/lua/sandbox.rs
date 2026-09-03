// Project:  Privatium™  |  File: crates/privatium-core/src/lua/sandbox.rs
// Authors:  Gabriel Mongefranco (@gabrielmongefranco)
// Created:  2026-09-03  |  Modified: 2026-09-03
// Summary:  The state an app runs in (spec/lua-api.md §5): the retained standard libraries
//           and nothing else, the closed list of removed names, `require` replaced by a
//           loader confined to the app's lib/ plus 'privatium', `print` routed to the
//           diagnostic log, the request-scoped environment that keeps one request's global
//           assignments from the next, and the sandbox globals of §4.0 — url, icon, fmt.*,
//           t — that handler code and templates (lsp) share.

use std::fs;
use std::path::PathBuf;

use mlua::chunk::ChunkMode;
use mlua::{Lua, LuaOptions, StdLib, Table, Value};

use crate::config::LuaConfig;
use crate::icons;
use crate::lua::html::Html;
use crate::lua::{Phase, UiSettings, VmData};
use crate::store::Decimal;

/// The registry key of the table `require` caches modules in — `package.loaded`.
pub(crate) const LOADED_KEY: &str = "pv.loaded";

/// The registry key of the `pv` module table.
pub(crate) const PV_KEY: &str = "pv.module";

/// The registry key of the environment app code runs in.
const ENV_KEY: &str = "pv.env";

/// The registry key of the table that holds a request's global assignments.
const SCRATCH_KEY: &str = "pv.scratch";

/// The environment every app chunk — `app.lua`, and each `lib/` module — runs in
/// (`spec/lua-api.md §5`, "global state").
///
/// A proxy that never holds a key of its own. Reads fall through to the request's scratch
/// table and then to the real globals; writes go to the real globals while `app.lua`
/// loads — that is the VM's baseline — and to the scratch table during a request, which
/// [`clear_scratch`] empties when the request ends. So a global assigned in a handler
/// lives exactly one request and is never seen by another, on this VM or any other. A
/// table the baseline holds can still be mutated in place; that persists per VM, and is
/// the footgun the linter checks.
pub(crate) fn install_env(lua: &Lua) -> mlua::Result<Table> {
    let globals = lua.globals();
    let scratch = lua.create_table()?;
    let fallback = lua.create_table()?;
    fallback.raw_set("__index", globals.clone())?;
    scratch.set_metatable(Some(fallback))?;

    let env = lua.create_table()?;
    let meta = lua.create_table()?;
    meta.raw_set("__index", scratch.clone())?;
    meta.raw_set("__newindex", lua.create_function(assign_global)?)?;
    // `setmetatable(_G, …)` and `getmetatable(_G)` see this string, not the table.
    meta.raw_set("__metatable", "the app's environment")?;
    env.set_metatable(Some(meta))?;

    globals.raw_set("_G", env.clone())?;
    lua.set_named_registry_value(ENV_KEY, env.clone())?;
    lua.set_named_registry_value(SCRATCH_KEY, scratch)?;
    Ok(env)
}

/// `__newindex` of the environment: baseline while loading, scratch during a request. A
/// template's environment (`lsp`) routes its bare assignments here too.
pub(crate) fn assign_global(
    lua: &Lua,
    (_env, key, value): (Table, Value, Value),
) -> mlua::Result<()> {
    let loading = lua
        .app_data_ref::<VmData>()
        .is_some_and(|data| data.phase == Phase::Loading);
    if loading {
        lua.globals().raw_set(key, value)
    } else {
        let scratch: Table = lua.named_registry_value(SCRATCH_KEY)?;
        scratch.raw_set(key, value)
    }
}

/// The environment app chunks run in.
pub(crate) fn env(lua: &Lua) -> mlua::Result<Table> {
    lua.named_registry_value(ENV_KEY)
}

/// Forget a request's global assignments.
pub(crate) fn clear_scratch(lua: &Lua) -> mlua::Result<()> {
    let scratch: Table = lua.named_registry_value(SCRATCH_KEY)?;
    scratch.clear()
}

/// The framework modules `require` serves. `'privatium'` and nothing else in Phase 1.
const FRAMEWORK_MODULES: [&str; 1] = ["privatium"];

/// What `os` loses. `spec/lua-api.md §5`'s six, plus `setlocale`, which is process-wide
/// state: one app calling it would change how every other app — and the node — formats a
/// number.
const OS_REMOVED: [&str; 7] = [
    "execute",
    "exit",
    "getenv",
    "remove",
    "rename",
    "tmpname",
    "setlocale",
];

/// The globals that load code from data.
const GLOBALS_REMOVED: [&str; 4] = ["load", "loadstring", "dofile", "loadfile"];

/// A fresh state with the sandbox applied and the memory limit set. No app code has run.
pub(crate) fn new_state(config: &LuaConfig) -> mlua::Result<Lua> {
    // `io`, `debug` and `package` are never loaded, so there is nothing to remove from
    // them; `require` and `package` below are the framework's own.
    let libs = StdLib::COROUTINE
        | StdLib::TABLE
        | StdLib::OS
        | StdLib::STRING
        | StdLib::UTF8
        | StdLib::MATH;
    let lua = Lua::new_with(libs, LuaOptions::default())?;

    let bytes = usize::try_from(config.max_memory_mb)
        .unwrap_or(usize::MAX)
        .saturating_mul(1024 * 1024);
    lua.set_memory_limit(bytes)?;

    let globals = lua.globals();
    let os: Table = globals.get("os")?;
    for name in OS_REMOVED {
        os.raw_set(name, Value::Nil)?;
    }
    for name in GLOBALS_REMOVED {
        globals.raw_set(name, Value::Nil)?;
    }

    let loaded = lua.create_table()?;
    lua.set_named_registry_value(LOADED_KEY, loaded.clone())?;
    let package = lua.create_table()?;
    package.raw_set("loaded", loaded)?;
    package.raw_set("path", "lib/?.lua")?;
    globals.raw_set("package", package)?;
    globals.raw_set("require", lua.create_function(require)?)?;
    globals.raw_set("print", lua.create_function(print)?)?;
    Ok(lua)
}

/// `require(name)`: `'privatium'`, or a module under the app's `lib/` named by dotted
/// identifiers. Nothing else, however the name is spelled.
fn require(lua: &Lua, name: String) -> mlua::Result<Value> {
    let loaded: Table = lua.named_registry_value(LOADED_KEY)?;
    let cached: Value = loaded.raw_get(name.as_str())?;
    if cached != Value::Nil {
        return Ok(cached);
    }
    if FRAMEWORK_MODULES.contains(&name.as_str()) {
        let module: Value = lua.named_registry_value(PV_KEY)?;
        loaded.raw_set(name.as_str(), module.clone())?;
        return Ok(module);
    }
    if !is_module_name(&name) {
        return Err(mlua::Error::runtime(format!(
            "require: {name:?} is not a module name; a module is a dotted identifier under \
             lib/ ('tree', 'shared.dates'), or 'privatium' (spec/lua-api.md §5)"
        )));
    }

    let (lib_dir, slug) = {
        let data = lua
            .app_data_ref::<VmData>()
            .ok_or_else(|| mlua::Error::runtime("require: no app is loaded in this state"))?;
        (data.lib_dir.clone(), data.slug.clone())
    };
    let Some(lib_dir) = lib_dir else {
        return Err(mlua::Error::runtime(format!(
            "require: {name:?} — app {slug} has no lib/ directory"
        )));
    };
    let relative: PathBuf = name.split('.').collect();
    let path = lib_dir.join(relative).with_extension("lua");
    // Canonical on both sides: a symlink inside lib/ pointing outside resolves to its
    // target, and the target is not under lib/.
    let canonical = fs::canonicalize(&path)
        .map_err(|_| mlua::Error::runtime(format!("require: module {name:?} not found in lib/")))?;
    if !canonical.starts_with(&lib_dir) {
        return Err(mlua::Error::runtime(format!(
            "require: module {name:?} resolves outside lib/"
        )));
    }
    let source = fs::read(&canonical)
        .map_err(|error| mlua::Error::runtime(format!("require: {name:?}: {error}")))?;
    let chunk_name = format!("@lib/{}.lua", name.replace('.', "/"));
    let value: Value = lua
        .load(source)
        .set_name(chunk_name)
        .set_mode(ChunkMode::Text)
        .set_environment(env(lua)?)
        .call(name.as_str())?;
    let value = if value == Value::Nil {
        Value::Boolean(true)
    } else {
        value
    };
    loaded.raw_set(name.as_str(), value.clone())?;
    Ok(value)
}

/// `[A-Za-z_][A-Za-z0-9_]*` segments joined by single dots.
fn is_module_name(name: &str) -> bool {
    !name.is_empty()
        && name.split('.').all(|segment| {
            let mut bytes = segment.bytes();
            bytes
                .next()
                .is_some_and(|b| b.is_ascii_alphabetic() || b == b'_')
                && bytes.all(|b| b.is_ascii_alphanumeric() || b == b'_')
        })
}

/// `print(...)` writes to the diagnostic log as `info`, never to stdout
/// (`spec/lua-api.md §3.4`).
fn print(lua: &Lua, args: mlua::Variadic<Value>) -> mlua::Result<()> {
    let tostring: mlua::Function = lua.globals().get("tostring")?;
    let mut parts = Vec::with_capacity(args.len());
    for arg in args {
        parts.push(tostring.call::<String>(arg)?);
    }
    diagnostic(lua, "info", &parts.join("\t"));
    Ok(())
}

/// The diagnostic log: the node's standard error, prefixed with the app.
pub(crate) fn diagnostic(lua: &Lua, level: &str, message: &str) {
    let slug = lua
        .app_data_ref::<VmData>()
        .map(|data| data.slug.clone())
        .unwrap_or_default();
    eprintln!("privatium: {slug}: {level}: {message}");
}

/// The sandbox globals of `spec/lua-api.md §4.0`: `url`, `icon`, `fmt.date`, `fmt.money`,
/// `fmt.rel`, `t`. Available in handler code now and shared with templates in M8.
pub(crate) fn install_globals(lua: &Lua) -> mlua::Result<()> {
    let globals = lua.globals();
    globals.raw_set("url", lua.create_function(url)?)?;
    globals.raw_set("icon", lua.create_function(icon)?)?;
    globals.raw_set("t", lua.create_function(|_, key: String| Ok(key))?)?;
    let fmt = lua.create_table()?;
    fmt.raw_set("date", lua.create_function(fmt_date)?)?;
    fmt.raw_set("money", lua.create_function(fmt_money)?)?;
    fmt.raw_set("rel", lua.create_function(fmt_rel)?)?;
    globals.raw_set("fmt", fmt)?;
    Ok(())
}

/// `url(path)` — `wire::url(mount, path)`, the only place a URL is built.
pub(crate) fn url(lua: &Lua, path: String) -> mlua::Result<String> {
    let mount = lua
        .app_data_ref::<VmData>()
        .map(|data| data.mount.clone())
        .ok_or_else(|| mlua::Error::runtime("url: no app is loaded in this state"))?;
    Ok(crate::wire::url(&mount, &path))
}

/// `icon(name[, label])` — `docs/icons.md`. A label makes it an image with a title. The
/// result is markup, so `<?= icon(...) ?>` emits it as it is (`spec/lua-api.md §4`).
fn icon(_: &Lua, (name, label): (String, Option<Value>)) -> mlua::Result<Html> {
    match label {
        None | Some(Value::Nil) => Ok(Html(icons::icon(&name))),
        Some(Value::String(label)) => Ok(Html(icons::icon_labeled(&name, &label.to_str()?))),
        Some(other) => Err(mlua::Error::runtime(format!(
            "icon(name, label): the label is a string, not a {} (docs/icons.md)",
            other.type_name()
        ))),
    }
}

fn ui(lua: &Lua) -> UiSettings {
    lua.app_data_ref::<VmData>()
        .and_then(|data| data.ctx.as_ref().map(|ctx| ctx.ui.clone()))
        .unwrap_or_default()
}

/// `fmt.date(s)` under `ui.date_format`: `iso` leaves `YYYY-MM-DD` alone, `us` is
/// `MM/DD/YYYY`, `eu` is `DD/MM/YYYY`. A timestamp is formatted by its date. Anything that
/// is not a date comes back unchanged.
fn fmt_date(lua: &Lua, value: Value) -> mlua::Result<String> {
    let text = match &value {
        Value::String(s) => s.to_str()?.to_string(),
        Value::Nil => return Ok(String::new()),
        other => return Err(bad_arg("fmt.date", other)),
    };
    let Some(date) = text
        .get(..10)
        .and_then(|d| d.parse::<jiff::civil::Date>().ok())
    else {
        return Ok(text);
    };
    Ok(match ui(lua).date_format.as_str() {
        "us" => format!("{:02}/{:02}/{:04}", date.month(), date.day(), date.year()),
        "eu" => format!("{:02}/{:02}/{:04}", date.day(), date.month(), date.year()),
        _ => date.to_string(),
    })
}

/// `fmt.money(s)`: two places, grouped, with the separators `ui.locale` implies —
/// `1,234.50` for an English locale, `1.234,50` otherwise. Not a decimal: unchanged.
fn fmt_money(lua: &Lua, value: Value) -> mlua::Result<String> {
    let decimal = match &value {
        Value::Nil => return Ok(String::new()),
        Value::UserData(ud) if ud.is::<crate::lua::dec::Dec>() => {
            ud.borrow::<crate::lua::dec::Dec>()?.0
        }
        Value::String(s) => {
            let text = s.to_str()?;
            match Decimal::parse(&text) {
                Some(decimal) => decimal,
                None => return Ok(text.to_string()),
            }
        }
        Value::Integer(i) => Decimal::from_i64(*i),
        other => return Err(bad_arg("fmt.money", other)),
    };
    let scaled = decimal.checked_with_scale(2).unwrap_or(decimal);
    let text = scaled.to_string();
    let (sign, digits) = match text.strip_prefix('-') {
        Some(rest) => ("-", rest),
        None => ("", text.as_str()),
    };
    let (whole, fraction) = digits.split_once('.').unwrap_or((digits, ""));
    let english = ui(lua).locale.to_ascii_lowercase().starts_with("en");
    let (group, point) = if english { (',', '.') } else { ('.', ',') };
    let mut grouped = String::with_capacity(whole.len() + whole.len() / 3);
    for (i, ch) in whole.chars().enumerate() {
        if i > 0 && (whole.len() - i).is_multiple_of(3) {
            grouped.push(group);
        }
        grouped.push(ch);
    }
    Ok(if fraction.is_empty() {
        format!("{sign}{grouped}")
    } else {
        format!("{sign}{grouped}{point}{fraction}")
    })
}

/// `fmt.rel(ts)`: a timestamp or date relative to now — `just now`, `5 minutes ago`,
/// `in 3 days`. Not a time: unchanged.
fn fmt_rel(_: &Lua, value: Value) -> mlua::Result<String> {
    let text = match &value {
        Value::String(s) => s.to_str()?.to_string(),
        Value::Nil => return Ok(String::new()),
        other => return Err(bad_arg("fmt.rel", other)),
    };
    let then = match text.parse::<jiff::Timestamp>() {
        Ok(ts) => ts,
        Err(_) => match text.parse::<jiff::civil::Date>() {
            Ok(date) => match date.to_zoned(jiff::tz::TimeZone::UTC) {
                Ok(zoned) => zoned.timestamp(),
                Err(_) => return Ok(text),
            },
            Err(_) => return Ok(text),
        },
    };
    let seconds = jiff::Timestamp::now().duration_since(then).as_secs();
    let (past, magnitude) = (seconds >= 0, seconds.unsigned_abs());
    let phrase = |n: u64, unit: &str| {
        let plural = if n == 1 { "" } else { "s" };
        if past {
            format!("{n} {unit}{plural} ago")
        } else {
            format!("in {n} {unit}{plural}")
        }
    };
    Ok(match magnitude {
        0..=44 => "just now".to_owned(),
        45..=3_599 => phrase(magnitude.div_ceil(60).max(1), "minute"),
        3_600..=86_399 => phrase(magnitude / 3_600, "hour"),
        86_400..=2_591_999 => phrase(magnitude / 86_400, "day"),
        2_592_000..=31_535_999 => phrase(magnitude / 2_592_000, "month"),
        _ => phrase(magnitude / 31_536_000, "year"),
    })
}

fn bad_arg(function: &str, value: &Value) -> mlua::Error {
    mlua::Error::runtime(format!(
        "{function}: expected a string, got {}",
        value.type_name()
    ))
}
