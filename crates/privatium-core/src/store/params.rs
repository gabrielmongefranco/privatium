// Project:  Privatium™  |  File: crates/privatium-core/src/store/params.rs
// Authors:  Gabriel Mongefranco (@gabrielmongefranco)
// Created:  2026-09-03  |  Modified: 2026-09-03
// Summary:  `$name` placeholders in a CREATE VIEW (spec/data-api.md §1). SQLite refuses a
//           parameter inside a view — "parameters are not allowed in views" — so the
//           framework rewrites every `$name` in schema.sql to `pv_param('name')`, a scalar
//           function registered on every connection. It answers from a per-connection table
//           the data API fills before a query runs, and is NULL anywhere else.

use std::collections::BTreeMap;
use std::sync::{Arc, Mutex, PoisonError};

use rusqlite::Connection;
use rusqlite::functions::FunctionFlags;
use rusqlite::types::Value;

/// The function a `$name` becomes.
pub const FUNCTION: &str = "pv_param";

/// The values `pv_param` answers with on one connection. Cloning shares the table.
#[derive(Debug, Clone, Default)]
pub struct Params(Arc<Mutex<BTreeMap<String, String>>>);

impl Params {
    /// Bind `name` for every statement that runs from now on.
    pub fn set(&self, name: &str, value: &str) {
        self.0
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .insert(name.to_owned(), value.to_owned());
    }

    /// Unbind everything.
    pub fn clear(&self) {
        self.0
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .clear();
    }

    /// What `name` is bound to.
    #[must_use]
    pub fn get(&self, name: &str) -> Option<String> {
        self.0
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .get(name)
            .cloned()
    }
}

/// Register `pv_param` on `conn`, returning the table it reads. Not marked deterministic:
/// the same call answers differently once the table changes.
pub fn register(conn: &Connection) -> rusqlite::Result<Params> {
    let params = Params::default();
    let handle = params.clone();
    conn.create_scalar_function(FUNCTION, 1, FunctionFlags::SQLITE_UTF8, move |ctx| {
        let name: String = ctx.get(0)?;
        Ok(match handle.get(&name) {
            Some(value) => Value::Text(value),
            None => Value::Null,
        })
    })?;
    Ok(params)
}

/// `schema.sql` with every `$name` outside a string, an identifier or a comment rewritten
/// to `pv_param('name')`. Everything else is copied as it is, so the text SQLite runs is
/// the author's except for the one construct SQLite would refuse.
#[must_use]
pub fn rewrite(sql: &str) -> String {
    let bytes = sql.as_bytes();
    let mut out = String::with_capacity(sql.len() + 32);
    let mut i = 0;
    while i < bytes.len() {
        let b = bytes[i];
        // A quoted string or identifier is copied through to its closing quote; a doubled
        // quote inside is an escape and stays inside.
        if b == b'\'' || b == b'"' || b == b'`' {
            let start = i;
            i += 1;
            while i < bytes.len() {
                if bytes[i] == b {
                    if bytes.get(i + 1) == Some(&b) {
                        i += 2;
                        continue;
                    }
                    i += 1;
                    break;
                }
                i += 1;
            }
            out.push_str(&sql[start..i.min(bytes.len())]);
            continue;
        }
        if b == b'[' {
            let end = sql[i..].find(']').map_or(bytes.len(), |at| i + at + 1);
            out.push_str(&sql[i..end]);
            i = end;
            continue;
        }
        if b == b'-' && bytes.get(i + 1) == Some(&b'-') {
            let end = sql[i..].find('\n').map_or(bytes.len(), |at| i + at);
            out.push_str(&sql[i..end]);
            i = end;
            continue;
        }
        if b == b'/' && bytes.get(i + 1) == Some(&b'*') {
            let end = sql[i + 2..]
                .find("*/")
                .map_or(bytes.len(), |at| i + 2 + at + 2);
            out.push_str(&sql[i..end]);
            i = end;
            continue;
        }
        if b == b'$'
            && bytes
                .get(i + 1)
                .is_some_and(|c| c.is_ascii_alphabetic() || *c == b'_')
        {
            let start = i + 1;
            let mut end = start;
            while end < bytes.len() && (bytes[end].is_ascii_alphanumeric() || bytes[end] == b'_') {
                end += 1;
            }
            out.push_str(FUNCTION);
            out.push_str("('");
            out.push_str(&sql[start..end]);
            out.push_str("')");
            i = end;
            continue;
        }
        // Any other byte, whole UTF-8 sequence included: copy the char.
        let ch = sql[i..].chars().next().unwrap_or('\u{fffd}');
        out.push(ch);
        i += ch.len_utf8();
    }
    out
}

/// The placeholder names a rewritten statement reads — every `pv_param('name')` in it, in
/// order of first appearance.
#[must_use]
pub fn placeholders(sql: &str) -> Vec<String> {
    let mut names = Vec::new();
    let needle = format!("{FUNCTION}('");
    let lower = sql.to_ascii_lowercase();
    let mut from = 0;
    while let Some(at) = lower[from..].find(&needle) {
        let start = from + at + needle.len();
        let Some(len) = sql[start..].find('\'') else {
            break;
        };
        let name = sql[start..start + len].to_owned();
        if !name.is_empty() && !names.contains(&name) {
            names.push(name);
        }
        from = start + len;
    }
    names
}

// AGENTS.md, Style: unwrap() is permitted in tests. The crate-level deny reaches unit
// tests inside src/, so each one opts out where it is declared.
#[allow(clippy::unwrap_used, clippy::expect_used)]
#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn placeholders_are_rewritten_outside_strings_and_comments() {
        let sql = "CREATE VIEW v AS SELECT * FROM t -- $not\n\
                   WHERE due_on < date('now', '+' || $days || ' days') /* $nor */ \
                   AND note <> '$5' AND \"$col\" = $days AND x = $x_1;";
        let rewritten = rewrite(sql);
        assert_eq!(
            rewritten,
            "CREATE VIEW v AS SELECT * FROM t -- $not\n\
             WHERE due_on < date('now', '+' || pv_param('days') || ' days') /* $nor */ \
             AND note <> '$5' AND \"$col\" = pv_param('days') AND x = pv_param('x_1');"
        );
        assert_eq!(placeholders(&rewritten), vec!["days", "x_1"]);
        assert_eq!(rewrite("SELECT 1"), "SELECT 1");
        assert_eq!(rewrite("SELECT '$'"), "SELECT '$'");
        assert_eq!(rewrite("SELECT $"), "SELECT $");
        assert_eq!(rewrite("SELECT 'it''s $x'"), "SELECT 'it''s $x'");
    }

    #[test]
    fn the_function_answers_from_the_table_and_null_otherwise() {
        let conn = Connection::open_in_memory().unwrap();
        let params = register(&conn).unwrap();
        let unbound: Option<String> = conn
            .query_row("SELECT pv_param('days')", [], |row| row.get(0))
            .unwrap();
        assert_eq!(unbound, None);
        params.set("days", "30");
        let bound: String = conn
            .query_row("SELECT pv_param('days')", [], |row| row.get(0))
            .unwrap();
        assert_eq!(bound, "30");
        params.clear();
        assert_eq!(params.get("days"), None);
    }
}
