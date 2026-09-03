// Project:  Privatium™  |  File: crates/privatium-core/src/lua/convert.rs
// Authors:  Gabriel Mongefranco (@gabrielmongefranco)
// Created:  2026-09-03  |  Modified: 2026-09-03
// Summary:  Values crossing the Lua boundary. Lua tables to the JSON `d` of an event and
//           back (spec/data-dictionary.md §2.1), bound parameters for pv.query, and the
//           typing of a result column (spec/lua-api.md §3.2): what SQLite holds, as Lua
//           holds it — INTEGER an integer, REAL a float, TEXT a string — with two
//           conveniences for a column that originates in a declared one: BOOLEAN as a
//           boolean and JSON decoded. A DECIMAL is TEXT in the cache and stays a string.

use mlua::{Lua, Table, Value};
use rusqlite::types::ValueRef;

use crate::lua::dec::Dec;
use crate::lua::html::Html;
use crate::store::{Kind, Schema};

/// How deep a value may nest before it is refused rather than followed forever.
const MAX_DEPTH: usize = 64;

/// A JSON value as a Lua value: objects and arrays as tables, `null` as nothing.
pub fn json_to_lua(lua: &Lua, value: &serde_json::Value) -> mlua::Result<Value> {
    Ok(match value {
        serde_json::Value::Null => Value::Nil,
        serde_json::Value::Bool(b) => Value::Boolean(*b),
        serde_json::Value::Number(n) => match n.as_i64() {
            Some(i) => Value::Integer(i),
            None => Value::Number(n.as_f64().unwrap_or(f64::NAN)),
        },
        serde_json::Value::String(s) => Value::String(lua.create_string(s)?),
        serde_json::Value::Array(items) => {
            let table = lua.create_table_with_capacity(items.len(), 0)?;
            for (i, item) in items.iter().enumerate() {
                table.raw_seti(i + 1, json_to_lua(lua, item)?)?;
            }
            Value::Table(table)
        }
        serde_json::Value::Object(fields) => {
            let table = lua.create_table_with_capacity(0, fields.len())?;
            for (key, item) in fields {
                table.raw_set(key.as_str(), json_to_lua(lua, item)?)?;
            }
            Value::Table(table)
        }
    })
}

/// A Lua value as JSON. A table whose keys are exactly `1..n` is an array, an empty table
/// is an empty array, a table with string keys is an object, anything else is refused.
pub fn lua_to_json(value: &Value) -> mlua::Result<serde_json::Value> {
    to_json(value, 0)
}

/// The `d` of an event: a Lua table with string keys, as a JSON object
/// (`spec/protocol.md §4.1`: `d` is an object). `nil` values are absent keys (`§2.1`).
pub fn lua_object(table: &Table) -> mlua::Result<serde_json::Map<String, serde_json::Value>> {
    let mut object = serde_json::Map::new();
    for pair in table.pairs::<Value, Value>() {
        let (key, value) = pair?;
        let Value::String(key) = key else {
            return Err(mlua::Error::runtime(format!(
                "an event's fields are named by strings, not {}",
                key.type_name()
            )));
        };
        let key = key.to_str()?;
        object.insert(key.to_string(), to_json(&value, 1)?);
    }
    Ok(object)
}

fn to_json(value: &Value, depth: usize) -> mlua::Result<serde_json::Value> {
    if depth > MAX_DEPTH {
        return Err(mlua::Error::runtime(format!(
            "a value nests deeper than {MAX_DEPTH} levels"
        )));
    }
    Ok(match value {
        Value::Nil => serde_json::Value::Null,
        Value::Boolean(b) => serde_json::Value::Bool(*b),
        Value::Integer(i) => serde_json::Value::from(*i),
        Value::Number(n) => serde_json::Number::from_f64(*n)
            .map(serde_json::Value::Number)
            .ok_or_else(|| mlua::Error::runtime(format!("{n} is not a JSON number")))?,
        Value::String(s) => serde_json::Value::String(s.to_str()?.to_string()),
        Value::Table(table) => table_to_json(table, depth)?,
        Value::UserData(ud) if ud.is::<Dec>() => {
            serde_json::Value::String(ud.borrow::<Dec>()?.0.to_string())
        }
        Value::UserData(ud) if ud.is::<Html>() => {
            serde_json::Value::String(ud.borrow::<Html>()?.0.clone())
        }
        other => {
            return Err(mlua::Error::runtime(format!(
                "a {} cannot be written as JSON",
                other.type_name()
            )));
        }
    })
}

fn table_to_json(table: &Table, depth: usize) -> mlua::Result<serde_json::Value> {
    let mut entries: Vec<(Value, Value)> = Vec::new();
    for pair in table.pairs::<Value, Value>() {
        entries.push(pair?);
    }
    if entries.is_empty() {
        return Ok(serde_json::Value::Array(Vec::new()));
    }
    if entries
        .iter()
        .all(|(key, _)| matches!(key, Value::String(_)))
    {
        let mut object = serde_json::Map::new();
        for (key, value) in &entries {
            if let Value::String(key) = key {
                object.insert(key.to_str()?.to_string(), to_json(value, depth + 1)?);
            }
        }
        return Ok(serde_json::Value::Object(object));
    }
    if entries
        .iter()
        .all(|(key, _)| matches!(key, Value::Integer(_)))
    {
        let mut indexed: Vec<(i64, &Value)> = entries
            .iter()
            .map(|(key, value)| match key {
                Value::Integer(i) => (*i, value),
                _ => (0, value),
            })
            .collect();
        indexed.sort_by_key(|(i, _)| *i);
        let dense = indexed
            .iter()
            .enumerate()
            .all(|(position, (i, _))| i64::try_from(position + 1).ok() == Some(*i));
        if dense {
            let mut items = Vec::with_capacity(indexed.len());
            for (_, value) in indexed {
                items.push(to_json(value, depth + 1)?);
            }
            return Ok(serde_json::Value::Array(items));
        }
    }
    Err(mlua::Error::runtime(
        "a table is written as JSON only when its keys are all strings (an object) or \
         exactly 1..n (an array)",
    ))
}

/// The bound parameters of `pv.query`: a sequence of strings, integers, floats, booleans
/// and decimals. Nothing is ever interpolated (`spec/lua-api.md §3.2`).
pub fn bind_params(params: Option<Table>) -> mlua::Result<Vec<rusqlite::types::Value>> {
    let Some(params) = params else {
        return Ok(Vec::new());
    };
    let mut out = Vec::with_capacity(params.raw_len());
    for (index, value) in params.sequence_values::<Value>().enumerate() {
        out.push(match value? {
            Value::Nil => rusqlite::types::Value::Null,
            Value::Boolean(b) => rusqlite::types::Value::Integer(i64::from(b)),
            Value::Integer(i) => rusqlite::types::Value::Integer(i),
            Value::Number(n) => rusqlite::types::Value::Real(n),
            Value::String(s) => rusqlite::types::Value::Text(s.to_str()?.to_string()),
            Value::UserData(ud) if ud.is::<Dec>() => {
                rusqlite::types::Value::Text(ud.borrow::<Dec>()?.0.to_string())
            }
            other => {
                return Err(mlua::Error::runtime(format!(
                    "pv.query: parameter {} is a {}; bind strings, numbers, booleans or \
                     decimals",
                    index + 1,
                    other.type_name()
                )));
            }
        });
    }
    Ok(out)
}

/// One result column: its name and, when it originates in a declared column, that
/// column's kind.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ColumnType {
    /// As SQLite names it — the alias, if the query gave one.
    pub name: String,
    /// The declared kind, for a column read from a table or through a view. `None` for an
    /// expression such as `count(*)`, which arrives by its storage class.
    pub kind: Option<Kind>,
}

/// Type every column of a prepared statement against the schema
/// (`spec/lua-api.md §3.2`).
///
/// SQLite's declared type of a cache column is the storage type (`TEXT`, `INTEGER`); the
/// author's declaration lives only in the schema. The engine does report which table and
/// column a result column originates in, through views too, and that is what is looked up.
#[must_use]
pub fn column_types(statement: &rusqlite::Statement<'_>, schema: &Schema) -> Vec<ColumnType> {
    statement
        .columns_with_metadata()
        .into_iter()
        .map(|column| {
            let kind = match (column.table_name(), column.origin_name()) {
                (Some(table), Some(origin)) => schema
                    .table(table)
                    .and_then(|t| t.columns.iter().find(|c| c.name == origin))
                    .map(|c| c.kind),
                _ => None,
            };
            ColumnType {
                name: column.name().to_owned(),
                kind,
            }
        })
        .collect()
}

/// One row as a Lua table keyed by column name. A NULL is an absent key.
pub fn row_to_lua(
    lua: &Lua,
    row: &rusqlite::Row<'_>,
    columns: &[ColumnType],
) -> mlua::Result<Table> {
    let table = lua.create_table_with_capacity(0, columns.len())?;
    for (index, column) in columns.iter().enumerate() {
        let raw = row.get_ref(index).map_err(sql_error)?;
        let value = cell_to_lua(lua, raw, column.kind)?;
        if value != Value::Nil {
            table.raw_set(column.name.as_str(), value)?;
        }
    }
    Ok(table)
}

/// The typing rule, one cell at a time.
fn cell_to_lua(lua: &Lua, raw: ValueRef<'_>, kind: Option<Kind>) -> mlua::Result<Value> {
    Ok(match (kind, raw) {
        (_, ValueRef::Null) => Value::Nil,
        // Declared DECIMAL: a string, always — Lua has no exact decimal, and its float
        // would lose what the text keeps. Declared BIGINT falls through to the storage
        // rule below: a Lua integer is 64-bit, so nothing is lost, and the reason JSON
        // needs a string (`spec/data-dictionary.md §2.1`) does not apply here.
        (Some(Kind::Decimal { .. }), ValueRef::Integer(i)) => {
            Value::String(lua.create_string(i.to_string())?)
        }
        (Some(Kind::Decimal { .. }), ValueRef::Real(r)) => {
            Value::String(lua.create_string(r.to_string())?)
        }
        // Declared BOOLEAN: a Lua boolean.
        (Some(Kind::Boolean), ValueRef::Integer(i)) => Value::Boolean(i != 0),
        (Some(Kind::Boolean), ValueRef::Text(text)) => {
            match std::str::from_utf8(text).unwrap_or_default() {
                "1" | "true" => Value::Boolean(true),
                "0" | "false" => Value::Boolean(false),
                other => Value::String(lua.create_string(other)?),
            }
        }
        // Declared JSON: the value itself, decoded; text that is not JSON stays text.
        (Some(Kind::Json), ValueRef::Text(text)) => {
            match serde_json::from_slice::<serde_json::Value>(text) {
                Ok(parsed) => json_to_lua(lua, &parsed)?,
                Err(_) => Value::String(lua.create_string(text)?),
            }
        }
        // Everything else by storage class: an integer is a Lua integer (`count(*)`), a
        // real a Lua number, text a string.
        (_, ValueRef::Integer(i)) => Value::Integer(i),
        (_, ValueRef::Real(r)) => Value::Number(r),
        (_, ValueRef::Text(text)) => Value::String(lua.create_string(text)?),
        (_, ValueRef::Blob(bytes)) => Value::String(lua.create_string(bytes)?),
    })
}

/// An engine error as a Lua error a handler can catch.
pub fn sql_error(error: rusqlite::Error) -> mlua::Error {
    mlua::Error::runtime(format!("pv.query: {error}"))
}
