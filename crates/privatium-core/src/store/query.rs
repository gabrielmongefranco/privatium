// Project:  Privatium™  |  File: crates/privatium-core/src/store/query.rs
// Authors:  Gabriel Mongefranco (@gabrielmongefranco)
// Created:  2026-09-05  |  Modified: 2026-09-05
// Summary:  One statement on the sandboxed connection, its rows as JSON typed by the
//           schema (spec/data-api.md §1, spec/data-dictionary.md §2.1): what the data API
//           answers and what Node::query hands an embedder (spec/app-contract.md §6),
//           through one function so the two cannot type a column differently. Lived in
//           wire/data.rs until M13 gave it a second caller.

use std::time::{Duration, Instant};

use base64::Engine as _;
use rusqlite::Connection;
use rusqlite::types::ValueRef;
use serde_json::{Map, Value};

use crate::store::{Kind, Schema};

/// One result column: its name and, when it originates in a declared column, that
/// column's kind.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ColumnType {
    /// As SQLite names it — the alias, if the query gave one.
    pub name: String,
    /// The declared kind, for a column read from a table or through a view. `None` for an
    /// expression such as `count(*)`, which arrives by its storage class.
    pub kind: Option<Kind>,
    /// The declared type as written — `DECIMAL(18,2)` — for the same column; what the data
    /// API reports in `columns` (`spec/data-api.md §1`).
    pub ty: Option<String>,
}

/// Type every column of a prepared statement against the schema
/// (`spec/lua-api.md §3.2`, `spec/data-api.md §1`).
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
            let declared = match (column.table_name(), column.origin_name()) {
                (Some(table), Some(origin)) => schema
                    .table(table)
                    .and_then(|t| t.columns.iter().find(|c| c.name == origin)),
                _ => None,
            };
            ColumnType {
                name: column.name().to_owned(),
                kind: declared.map(|c| c.kind),
                ty: declared.map(|c| c.ty.clone()),
            }
        })
        .collect()
}

/// A statement's result: the typed columns and the rows, each a JSON object.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Rows {
    /// Every result column, in order.
    pub columns: Vec<ColumnType>,
    /// Every row, keyed by column name, typed by [`cell_to_json`].
    pub rows: Vec<Map<String, Value>>,
}

/// A JSON parameter as a bound value (`spec/data-api.md §1`): a string as text, an integer
/// as an integer, another number as a real, a boolean as 0/1, `null` as NULL. An object
/// or an array is not a value; the message names the index.
pub fn bind(index: usize, value: &Value) -> Result<rusqlite::types::Value, String> {
    use rusqlite::types::Value as V;
    Ok(match value {
        Value::Null => V::Null,
        Value::Bool(b) => V::Integer(i64::from(*b)),
        Value::Number(n) => match n.as_i64() {
            Some(i) => V::Integer(i),
            None => V::Real(n.as_f64().unwrap_or(f64::NAN)),
        },
        Value::String(s) => V::Text(s.clone()),
        Value::Array(_) | Value::Object(_) => {
            return Err(format!(
                "params[{index}] is not a scalar; bind strings, numbers, booleans or null"
            ));
        }
    })
}

/// Run one statement on `conn` under `deadline`, with `params` bound to its `?`
/// placeholders. A count that does not match is refused, never padded
/// (`spec/data-api.md §1`); anything SQLite refuses — a write, a `PRAGMA` on the
/// sandboxed connection (`spec/app-contract.md §7`) — is its message.
pub fn run(
    conn: &Connection,
    schema: &Schema,
    deadline: Duration,
    sql: &str,
    params: Vec<rusqlite::types::Value>,
) -> Result<Rows, String> {
    let start = Instant::now();
    conn.progress_handler(1000, Some(move || start.elapsed() > deadline))
        .map_err(|error| error.to_string())?;
    let mut statement = conn.prepare(sql).map_err(|error| error.to_string())?;
    let expected = statement.parameter_count();
    if expected != params.len() {
        return Err(format!(
            "the statement has {expected} placeholder(s) and {} parameter(s) were given; \
             the counts must match — nothing is substituted",
            params.len()
        ));
    }
    let columns = column_types(&statement, schema);
    let mut rows = statement
        .query(rusqlite::params_from_iter(params))
        .map_err(|error| error.to_string())?;
    let mut out = Vec::new();
    while let Some(row) = rows.next().map_err(|error| error.to_string())? {
        out.push(row_to_json(row, &columns));
    }
    Ok(Rows { columns, rows: out })
}

/// One row as a JSON object (`spec/data-dictionary.md §2.1`): a declared `DECIMAL` or
/// `BIGINT` is a string, a `BOOLEAN` a boolean, a `JSON` column its value, NULL is
/// `null`; a computed column arrives by storage class, so `count(*)` is a number and
/// `decimal_sum()` a string.
#[must_use]
pub fn row_to_json(row: &rusqlite::Row<'_>, columns: &[ColumnType]) -> Map<String, Value> {
    let mut object = Map::with_capacity(columns.len());
    for (index, column) in columns.iter().enumerate() {
        let raw = row.get_ref(index).unwrap_or(ValueRef::Null);
        object.insert(column.name.clone(), cell_to_json(raw, column.kind));
    }
    object
}

fn cell_to_json(raw: ValueRef<'_>, kind: Option<Kind>) -> Value {
    let text = |bytes: &[u8]| String::from_utf8_lossy(bytes).into_owned();
    match (kind, raw) {
        (_, ValueRef::Null) => Value::Null,
        (Some(Kind::Integer | Kind::Decimal { .. }), ValueRef::Integer(i)) => {
            Value::String(i.to_string())
        }
        (Some(Kind::Decimal { .. }), ValueRef::Real(r)) => Value::String(r.to_string()),
        (Some(Kind::Boolean), ValueRef::Integer(i)) => Value::Bool(i != 0),
        (Some(Kind::Boolean), ValueRef::Text(t)) => match text(t).as_str() {
            "1" | "true" => Value::Bool(true),
            "0" | "false" => Value::Bool(false),
            other => Value::String(other.to_owned()),
        },
        (Some(Kind::Json), ValueRef::Text(t)) => {
            serde_json::from_slice(t).unwrap_or_else(|_| Value::String(text(t)))
        }
        (_, ValueRef::Integer(i)) => Value::from(i),
        (_, ValueRef::Real(r)) => {
            serde_json::Number::from_f64(r).map_or(Value::Null, Value::Number)
        }
        (_, ValueRef::Text(t)) => Value::String(text(t)),
        (_, ValueRef::Blob(b)) => {
            Value::String(base64::engine::general_purpose::STANDARD.encode(b))
        }
    }
}
