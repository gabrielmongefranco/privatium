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

/// One statement of a `schema.sql`, for the linter's `PV107` (`spec/cli.md §5.1`).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Statement {
    /// 1-based line the statement's first token is on.
    pub line: u32,
    /// The statement's text, the terminating `;` excluded, comments inside kept.
    pub sql: String,
}

/// Split `sql` at every `;` outside a string, a quoted identifier or a comment. A piece
/// that is only whitespace and comments is not a statement — that is what "and comments"
/// means in `PV107` — so the file's leading commentary yields nothing.
#[must_use]
pub fn split_statements(sql: &str) -> Vec<Statement> {
    let bytes = sql.as_bytes();
    let mut statements = Vec::new();
    let mut i = 0;
    let mut line = 1u32;
    // The current statement: where its first non-trivia byte is, and the line it is on.
    let mut start: Option<(usize, u32)> = None;
    // A trigger body runs from BEGIN to its END and holds `;` of its own; CASE … END
    // inside it nests.
    let mut trigger_pending = false;
    let mut trigger_depth = 0u32;
    let finish = |statements: &mut Vec<Statement>, start: Option<(usize, u32)>, end: usize| {
        if let Some((from, at)) = start {
            let text = sql[from..end].trim_end();
            if !text.is_empty() {
                statements.push(Statement {
                    line: at,
                    sql: text.to_owned(),
                });
            }
        }
    };
    while i < bytes.len() {
        let b = bytes[i];
        let skip = |from: usize, to: usize| -> u32 {
            sql[from..to].bytes().filter(|c| *c == b'\n').count() as u32
        };
        if b == b'-' && bytes.get(i + 1) == Some(&b'-') {
            let end = sql[i..].find('\n').map_or(bytes.len(), |at| i + at);
            i = end;
            continue;
        }
        if b == b'/' && bytes.get(i + 1) == Some(&b'*') {
            let end = sql[i + 2..]
                .find("*/")
                .map_or(bytes.len(), |at| i + 2 + at + 2);
            line += skip(i, end);
            i = end;
            continue;
        }
        if b == b'\n' {
            line += 1;
            i += 1;
            continue;
        }
        if b.is_ascii_whitespace() {
            i += 1;
            continue;
        }
        if b == b';' && trigger_depth == 0 {
            finish(&mut statements, start, i);
            start = None;
            trigger_pending = false;
            i += 1;
            continue;
        }
        if start.is_none() {
            start = Some((i, line));
        }
        if b.is_ascii_alphabetic() || b == b'_' {
            let mut end = i;
            while end < bytes.len() && (bytes[end].is_ascii_alphanumeric() || bytes[end] == b'_') {
                end += 1;
            }
            let word = sql[i..end].to_ascii_uppercase();
            match word.as_str() {
                "TRIGGER" => trigger_pending = true,
                "BEGIN" if trigger_pending && trigger_depth == 0 => trigger_depth = 1,
                "CASE" if trigger_depth > 0 => trigger_depth += 1,
                "END" if trigger_depth > 0 => trigger_depth -= 1,
                _ => {}
            }
            i = end;
            continue;
        }
        if b == b'\'' || b == b'"' || b == b'`' {
            i += 1;
            while i < bytes.len() {
                if bytes[i] == b'\n' {
                    line += 1;
                }
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
            continue;
        }
        if b == b'[' {
            let end = sql[i..].find(']').map_or(bytes.len(), |at| i + at + 1);
            line += skip(i, end);
            i = end;
            continue;
        }
        i += 1;
    }
    finish(&mut statements, start, bytes.len());
    statements
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
