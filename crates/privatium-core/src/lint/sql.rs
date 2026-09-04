// Project:  Privatium™  |  File: crates/privatium-core/src/lint/sql.rs
// Authors:  Gabriel Mongefranco (@gabrielmongefranco)
// Created:  2026-09-05  |  Modified: 2026-09-05
// Summary:  The SQL half of the linter. schema.sql goes through the engine: PV106 asks the
//           catalog for `id VARCHAR PRIMARY KEY`, and PV107 — settled here for SQLite, as
//           docs/plans/phase-1.md M12 left it — prepares each statement under an authorizer
//           that records the actions SQLite reports, so a statement is classified by what
//           the engine would do, never by its first word. App SQL — the literals Lua,
//           templates and JavaScript hand to pv.query and pv.sql, and CREATE VIEW bodies —
//           goes through a small tokenizer for PV303 (no writes) and PV308 (no SUM over a
//           DECIMAL, no + or - on a DATE).

use std::sync::{Arc, Mutex, PoisonError};

use rusqlite::Connection;
use rusqlite::hooks::{AuthAction, AuthContext, Authorization};

use crate::lint::{Columns, Ctx, RuleId, line_of};
use crate::store::{Schema, decimal, params};

const SCHEMA_FILE: &str = "schema.sql";

/// `schema.sql`, if present: parse it as the loader would, then `PV106`, `PV107`,
/// `PV308` over its views and `PV208`.
pub(crate) fn check_schema(ctx: &mut Ctx<'_>) {
    let Some(text) = ctx.read(SCHEMA_FILE) else {
        return;
    };
    super::manifest::check_secrets(ctx, SCHEMA_FILE, &text);
    if text.trim().is_empty() {
        ctx.schema = Some(Schema::empty());
        return;
    }
    match Schema::parse(&text) {
        Ok(schema) => {
            check_id_columns(ctx, &schema);
            classify_statements(ctx, &text);
            check_bookkeeping_names(ctx, &schema);
            let columns = Columns::of(Some(&schema));
            for view in &schema.views {
                let line = text
                    .find(&format!("VIEW {}", view.name))
                    .or_else(|| {
                        text.to_ascii_uppercase()
                            .find(&format!("VIEW {}", view.name.to_ascii_uppercase()))
                    })
                    .map_or(1, |at| line_of(&text, at));
                for problem in arithmetic_problems(&view.sql, &columns) {
                    ctx.push(
                        RuleId::PV308,
                        SCHEMA_FILE,
                        line,
                        format!("view {}: {problem}", view.name),
                    )
                    .fix = Some(problem.fix.clone());
                }
            }
            ctx.schema = Some(schema);
        }
        Err(error) => {
            let message = error.to_string();
            if message.contains("has no `id") {
                ctx.push(RuleId::PV106, SCHEMA_FILE, 0, message).fix =
                    Some("declare `id VARCHAR PRIMARY KEY` on every table".into());
            } else {
                ctx.push(
                    RuleId::PV107,
                    SCHEMA_FILE,
                    0,
                    format!("the engine refused it: {message}"),
                );
            }
        }
    }
}

/// `PV106`: `id` exists (the parser already refused a table without one), is declared
/// `VARCHAR` (or `TEXT`) and is the primary key — read from `pragma_table_info`, the
/// same catalog `Schema::parse` reads.
fn check_id_columns(ctx: &mut Ctx<'_>, schema: &Schema) {
    let Ok(conn) = scratch(schema) else {
        return;
    };
    for table in &schema.tables {
        let mut statement =
            match conn.prepare("SELECT type, pk FROM pragma_table_info(?) WHERE name = 'id'") {
                Ok(statement) => statement,
                Err(_) => continue,
            };
        let row: Option<(String, i64)> = statement
            .query_row(rusqlite::params![table.name], |row| {
                Ok((row.get::<_, String>(0)?, row.get::<_, i64>(1)?))
            })
            .ok();
        let Some((ty, pk)) = row else {
            continue;
        };
        let upper = ty.trim().to_ascii_uppercase();
        let text_like =
            upper.starts_with("VARCHAR") || upper == "TEXT" || upper.starts_with("CHAR");
        if !text_like || pk == 0 {
            let line = line_of_table(ctx, &table.name);
            ctx.push(
                RuleId::PV106,
                SCHEMA_FILE,
                line,
                format!(
                    "table `{}`: id is `{ty}`{} — every table needs `id VARCHAR PRIMARY KEY`",
                    table.name,
                    if pk == 0 {
                        " and not the primary key"
                    } else {
                        ""
                    }
                ),
            )
            .fix = Some("declare the column as `id VARCHAR PRIMARY KEY`".into());
        }
    }
}

/// `PV305` over the schema: a table or column named like outbox bookkeeping.
fn check_bookkeeping_names(ctx: &mut Ctx<'_>, schema: &Schema) {
    const NAMES: &[&str] = &[
        "dedupe",
        "dedup",
        "txid",
        "tx_id",
        "transaction_id",
        "acked",
        "ack_at",
        "acknowledg",
        "outbox",
    ];
    let mut suspects: Vec<(String, String)> = Vec::new();
    for table in &schema.tables {
        let lower = table.name.to_ascii_lowercase();
        if NAMES.iter().any(|n| lower.contains(n)) {
            suspects.push((table.name.clone(), format!("table `{}`", table.name)));
        }
        for column in &table.columns {
            let lower = column.name.to_ascii_lowercase();
            if NAMES.iter().any(|n| lower.contains(n)) {
                suspects.push((
                    table.name.clone(),
                    format!("column `{}.{}`", table.name, column.name),
                ));
            }
        }
    }
    for (table, what) in suspects {
        let line = line_of_table(ctx, &table);
        ctx.push(
            RuleId::PV305,
            SCHEMA_FILE,
            line,
            format!("{what} looks like outbox bookkeeping — a dedupe table, a transaction id, an acknowledgement; ULIDs already make replay idempotent"),
        )
        .fix = Some("drop it: a retry carrying the same id converges under the merge rule".into());
    }
}

fn line_of_table(ctx: &mut Ctx<'_>, table: &str) -> u32 {
    let Some(text) = ctx.read(SCHEMA_FILE) else {
        return 0;
    };
    let upper = text.to_ascii_uppercase();
    let needle = format!("TABLE {}", table.to_ascii_uppercase());
    upper.find(&needle).map_or(0, |at| line_of(&text, at))
}

/// A throwaway in-memory database holding the schema's DDL, for catalog questions.
fn scratch(schema: &Schema) -> rusqlite::Result<Connection> {
    let conn = Connection::open_in_memory()?;
    decimal::register(&conn)?;
    params::register(&conn)?;
    conn.execute_batch(&schema.ddl)?;
    Ok(conn)
}

/// What one statement of a `schema.sql` is, by the actions the engine reports for it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Verdict {
    /// 1-based line of the statement.
    pub line: u32,
    /// `None` when the statement is a declaration; otherwise what it does instead.
    pub problem: Option<String>,
}

/// An action as the recording authorizer keeps it, owned.
#[derive(Debug, Clone, PartialEq, Eq)]
enum Seen {
    Create(&'static str),
    Write(&'static str, String),
    Read,
    Other(String),
}

/// `PV107`: prepare each statement of `sql` under a recording authorizer and classify it.
/// SQLite reports `INSERT`/`UPDATE` on `sqlite_master` for its own bookkeeping of a
/// `CREATE`, and `READ`/`SELECT` while compiling a view's body; those are the
/// declaration's own. Anything else — a row written, an object dropped or altered, a
/// trigger, a temp object, a pragma, a transaction, a bare `SELECT` — is not a
/// declaration and is named.
#[must_use]
pub fn classify(sql: &str) -> Vec<Verdict> {
    let Ok(conn) = Connection::open_in_memory() else {
        return Vec::new();
    };
    if decimal::register(&conn).is_err() || params::register(&conn).is_err() {
        return Vec::new();
    }
    let seen: Arc<Mutex<Vec<Seen>>> = Arc::new(Mutex::new(Vec::new()));
    let sink = Arc::clone(&seen);
    let recorder = move |ctx: AuthContext<'_>| -> Authorization {
        let entry = match ctx.action {
            AuthAction::CreateTable { .. } => Seen::Create("CREATE TABLE"),
            AuthAction::CreateView { .. } => Seen::Create("CREATE VIEW"),
            AuthAction::CreateIndex { .. } => Seen::Create("CREATE INDEX"),
            AuthAction::Insert { table_name } => Seen::Write("INSERT into", table_name.to_owned()),
            AuthAction::Update { table_name, .. } => {
                Seen::Write("UPDATE of", table_name.to_owned())
            }
            AuthAction::Delete { table_name } => Seen::Write("DELETE from", table_name.to_owned()),
            // `CREATE INDEX` on a populated table reports a reindex of its own; a bare
            // `REINDEX` still fails below for creating nothing.
            AuthAction::Read { .. }
            | AuthAction::Select
            | AuthAction::Function { .. }
            | AuthAction::Recursive
            | AuthAction::Reindex { .. } => Seen::Read,
            AuthAction::CreateTempTable { .. }
            | AuthAction::CreateTempView { .. }
            | AuthAction::CreateTempIndex { .. } => {
                Seen::Other("a TEMP object, which vanishes with the connection".into())
            }
            AuthAction::CreateTrigger { .. } | AuthAction::CreateTempTrigger { .. } => {
                Seen::Other("CREATE TRIGGER".into())
            }
            AuthAction::DropTable { .. }
            | AuthAction::DropView { .. }
            | AuthAction::DropIndex { .. }
            | AuthAction::DropTempTable { .. }
            | AuthAction::DropTempView { .. }
            | AuthAction::DropTempIndex { .. }
            | AuthAction::DropTrigger { .. }
            | AuthAction::DropTempTrigger { .. } => Seen::Other("DROP".into()),
            AuthAction::AlterTable { .. } => Seen::Other("ALTER TABLE".into()),
            AuthAction::Pragma { pragma_name, .. } => Seen::Other(format!("PRAGMA {pragma_name}")),
            AuthAction::Transaction { .. } | AuthAction::Savepoint { .. } => {
                Seen::Other("a transaction statement".into())
            }
            AuthAction::Attach { .. } | AuthAction::Detach { .. } => {
                Seen::Other("ATTACH/DETACH".into())
            }
            AuthAction::Analyze { .. } => Seen::Other("ANALYZE".into()),
            AuthAction::CreateVtable { .. } | AuthAction::DropVtable { .. } => {
                Seen::Other("a virtual table".into())
            }
            AuthAction::Unknown { code, .. } => {
                Seen::Other(format!("an action the engine reports as code {code}"))
            }
            _ => Seen::Other("an action this build of the engine does not name".into()),
        };
        sink.lock()
            .unwrap_or_else(PoisonError::into_inner)
            .push(entry);
        Authorization::Allow
    };
    if conn.authorizer(Some(recorder)).is_err() {
        return Vec::new();
    }
    let mut verdicts = Vec::new();
    for statement in params::split_statements(&params::rewrite(sql)) {
        seen.lock().unwrap_or_else(PoisonError::into_inner).clear();
        let prepared = conn.prepare(&statement.sql);
        let actions: Vec<Seen> = seen
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .drain(..)
            .collect();
        let problem = match prepared {
            Err(error) => Some(format!(
                "the engine refused it: {}",
                crate::store::schema::first_line(&error.to_string())
            )),
            Ok(mut prepared) => {
                let verdict = judge(&actions);
                if verdict.is_none() {
                    // Run the declaration so the next statement can refer to it.
                    let _ = prepared.execute([]);
                }
                verdict
            }
        };
        verdicts.push(Verdict {
            line: statement.line,
            problem,
        });
    }
    verdicts
}

fn judge(actions: &[Seen]) -> Option<String> {
    let creates: Vec<&str> = actions
        .iter()
        .filter_map(|a| match a {
            Seen::Create(kind) => Some(*kind),
            _ => None,
        })
        .collect();
    for action in actions {
        match action {
            Seen::Write(verb, table) if !is_catalog(table) => {
                return Some(format!(
                    "{verb} `{table}` — a schema declares tables, views and indexes; rows arrive by append"
                ));
            }
            Seen::Other(what) => return Some(format!("{what} is not a declaration")),
            _ => {}
        }
    }
    if creates.is_empty() {
        return Some("not a CREATE TABLE, CREATE VIEW or CREATE INDEX".to_owned());
    }
    None
}

fn is_catalog(table: &str) -> bool {
    matches!(
        table.to_ascii_lowercase().as_str(),
        "sqlite_master"
            | "sqlite_temp_master"
            | "sqlite_schema"
            | "sqlite_temp_schema"
            | "sqlite_sequence"
            | "sqlite_stat1"
    )
}

fn classify_statements(ctx: &mut Ctx<'_>, text: &str) {
    for verdict in classify(text) {
        if let Some(problem) = verdict.problem {
            ctx.push(RuleId::PV107, SCHEMA_FILE, verdict.line, problem)
                .fix = Some(
                "keep schema.sql to CREATE TABLE, CREATE VIEW, CREATE INDEX and comments".into(),
            );
        }
    }
}

// ---------------------------------------------------------------------------------------
// App SQL: a tokenizer, PV303 and PV308
// ---------------------------------------------------------------------------------------

/// One SQL token.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum Tok {
    /// An identifier or keyword, as written (quotes removed).
    Word(String),
    /// A string literal.
    Str,
    /// A number.
    Num,
    /// Punctuation: `(`, `)`, `,`, `+`, `-`, `*`, `/`, `.`, `||`, `=`, `<`, `>`, `;`, …
    Punct(String),
}

/// Tokenize `sql`, comments dropped, with each token's byte offset.
pub(crate) fn tokens(sql: &str) -> Vec<(usize, Tok)> {
    let bytes = sql.as_bytes();
    let mut out = Vec::new();
    let mut i = 0;
    while i < bytes.len() {
        let b = bytes[i];
        if b.is_ascii_whitespace() {
            i += 1;
            continue;
        }
        if b == b'-' && bytes.get(i + 1) == Some(&b'-') {
            i = sql[i..].find('\n').map_or(bytes.len(), |at| i + at);
            continue;
        }
        if b == b'/' && bytes.get(i + 1) == Some(&b'*') {
            i = sql[i + 2..]
                .find("*/")
                .map_or(bytes.len(), |at| i + 2 + at + 2);
            continue;
        }
        if b == b'\'' {
            let start = i;
            i += 1;
            while i < bytes.len() {
                if bytes[i] == b'\'' {
                    if bytes.get(i + 1) == Some(&b'\'') {
                        i += 2;
                        continue;
                    }
                    i += 1;
                    break;
                }
                i += 1;
            }
            out.push((start, Tok::Str));
            continue;
        }
        if b == b'"' || b == b'`' || b == b'[' {
            let close = if b == b'[' { b']' } else { b };
            let start = i;
            i += 1;
            let name_start = i;
            while i < bytes.len() && bytes[i] != close {
                i += 1;
            }
            out.push((
                start,
                Tok::Word(sql[name_start..i.min(bytes.len())].to_owned()),
            ));
            i += 1;
            continue;
        }
        if b.is_ascii_alphabetic() || b == b'_' || b == b'$' {
            let start = i;
            while i < bytes.len()
                && (bytes[i].is_ascii_alphanumeric() || bytes[i] == b'_' || bytes[i] == b'$')
            {
                i += 1;
            }
            out.push((start, Tok::Word(sql[start..i].to_owned())));
            continue;
        }
        if b.is_ascii_digit() {
            let start = i;
            while i < bytes.len() && (bytes[i].is_ascii_alphanumeric() || bytes[i] == b'.') {
                i += 1;
            }
            out.push((start, Tok::Num));
            continue;
        }
        let two = &sql[i..(i + 2).min(bytes.len())];
        if ["||", "<>", "<=", ">=", "==", "!="].contains(&two) {
            out.push((i, Tok::Punct(two.to_owned())));
            i += 2;
            continue;
        }
        let ch = sql[i..].chars().next().unwrap_or(' ');
        out.push((i, Tok::Punct(ch.to_string())));
        i += ch.len_utf8();
    }
    out
}

/// One problem in a SQL literal.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct SqlProblem {
    /// Byte offset into the literal.
    pub offset: usize,
    /// What is wrong.
    pub message: String,
    /// What to write instead.
    pub fix: String,
}

impl std::fmt::Display for SqlProblem {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.message)
    }
}

/// `PV303`: the first keyword of each statement, `WITH … ` looked through.
pub(crate) fn write_problems(sql: &str) -> Vec<SqlProblem> {
    let toks = tokens(sql);
    let mut out = Vec::new();
    let mut at_start = true;
    let mut depth = 0i32;
    let mut in_with = false;
    for (offset, tok) in &toks {
        match tok {
            Tok::Punct(p) if p == "(" => depth += 1,
            Tok::Punct(p) if p == ")" => depth -= 1,
            Tok::Punct(p) if p == ";" && depth <= 0 => {
                at_start = true;
                in_with = false;
            }
            Tok::Word(word) if depth <= 0 && (at_start || in_with) => {
                let upper = word.to_ascii_uppercase();
                if at_start && upper == "WITH" {
                    in_with = true;
                    at_start = false;
                    continue;
                }
                if matches!(upper.as_str(), "INSERT" | "UPDATE" | "DELETE" | "REPLACE") {
                    out.push(SqlProblem {
                        offset: *offset,
                        message: format!("{upper} in app SQL — writes are appends, reads are SQL"),
                        fix: "pv.append / pv.delete (Lua) or pv.put / pv.del (JavaScript)".into(),
                    });
                }
                if matches!(
                    upper.as_str(),
                    "SELECT" | "INSERT" | "UPDATE" | "DELETE" | "REPLACE"
                ) {
                    in_with = false;
                }
                at_start = false;
            }
            _ => at_start = false,
        }
    }
    out
}

/// `PV308`: `SUM(` over a DECIMAL column, and `+`/`-` beside a DATE column.
pub(crate) fn arithmetic_problems(sql: &str, columns: &Columns) -> Vec<SqlProblem> {
    let toks = tokens(sql);
    let mut out = Vec::new();
    let is_column = |i: usize| -> Option<&str> {
        match toks.get(i) {
            Some((_, Tok::Word(w))) => {
                // Not a function name: `x(`.
                if matches!(toks.get(i + 1), Some((_, Tok::Punct(p))) if p == "(") {
                    return None;
                }
                Some(w.as_str())
            }
            _ => None,
        }
    };
    for i in 0..toks.len() {
        if let (_, Tok::Word(w)) = &toks[i]
            && w.eq_ignore_ascii_case("sum")
            && matches!(toks.get(i + 1), Some((_, Tok::Punct(p))) if p == "(")
        {
            let mut depth = 0;
            let mut j = i + 1;
            while j < toks.len() {
                match &toks[j].1 {
                    Tok::Punct(p) if p == "(" => depth += 1,
                    Tok::Punct(p) if p == ")" => {
                        depth -= 1;
                        if depth == 0 {
                            break;
                        }
                    }
                    Tok::Word(name) if columns.is_decimal(name) => {
                        out.push(SqlProblem {
                            offset: toks[i].0,
                            message: format!("SUM({name}) over a DECIMAL column is a float"),
                            fix: format!("decimal_sum({name})"),
                        });
                        break;
                    }
                    _ => {}
                }
                j += 1;
            }
        }
        if let Some(name) = is_column(i)
            && columns.is_date(name)
        {
            let next = toks.get(i + 1).map(|(_, t)| t);
            let prev = if i > 0 {
                toks.get(i - 1).map(|(_, t)| t)
            } else {
                None
            };
            let beside = |t: Option<&Tok>| matches!(t, Some(Tok::Punct(p)) if p == "+" || p == "-");
            if beside(next) || beside(prev) {
                out.push(SqlProblem {
                    offset: toks[i].0,
                    message: format!("`{name}` is a DATE; + and - treat it as an integer"),
                    fix: format!("date({name}, '+30 days') — the modifier spelling"),
                });
            }
        }
    }
    out
}

// AGENTS.md, Style: unwrap() is permitted in tests. The crate-level deny reaches unit
// tests inside src/, so each one opts out where it is declared.
#[allow(clippy::unwrap_used, clippy::expect_used)]
#[cfg(test)]
mod tests {
    use super::*;

    /// `PV107`, settled for SQLite: the engine's own report of each statement's actions.
    #[test]
    fn test_spec_cli_5_1_pv107_classifies_by_the_engines_actions() {
        let sql = "-- a comment mentioning INSERT\n\
                   CREATE TABLE t (id VARCHAR PRIMARY KEY, n DECIMAL(18,2), d DATE);\n\
                   CREATE VIEW v AS SELECT id FROM t WHERE n > $min;\n\
                   CREATE INDEX t_d ON t (d);\n\
                   INSERT INTO t (id) VALUES ('x');\n\
                   DELETE FROM t;\n\
                   SELECT 1;\n\
                   CREATE TEMP TABLE tmp (id VARCHAR PRIMARY KEY);\n\
                   ALTER TABLE t ADD COLUMN z VARCHAR;\n\
                   CREATE TRIGGER trg AFTER INSERT ON t BEGIN SELECT 1; END;";
        let verdicts = classify(sql);
        let problems: Vec<(u32, Option<String>)> =
            verdicts.into_iter().map(|v| (v.line, v.problem)).collect();
        assert_eq!(problems[0], (2, None));
        assert_eq!(problems[1], (3, None));
        assert_eq!(problems[2], (4, None));
        assert!(
            problems[3]
                .1
                .as_deref()
                .unwrap()
                .contains("INSERT into `t`"),
            "{problems:?}"
        );
        assert!(
            problems[4]
                .1
                .as_deref()
                .unwrap()
                .contains("DELETE from `t`"),
            "{problems:?}"
        );
        assert!(
            problems[5].1.as_deref().unwrap().contains("not a CREATE"),
            "{problems:?}"
        );
        assert!(
            problems[6].1.as_deref().unwrap().contains("TEMP"),
            "{problems:?}"
        );
        assert!(
            problems[7].1.as_deref().unwrap().contains("ALTER"),
            "{problems:?}"
        );
        assert!(problems[8].1.is_some(), "{problems:?}");
        assert_eq!(problems.len(), 9);
    }

    #[test]
    fn writes_and_arithmetic_are_found_in_app_sql() {
        assert!(write_problems("SELECT * FROM t").is_empty());
        assert!(write_problems("WITH x AS (SELECT 1) SELECT * FROM x").is_empty());
        assert_eq!(write_problems("UPDATE t SET a = 1").len(), 1);
        assert_eq!(
            write_problems("WITH x AS (SELECT 1) DELETE FROM t").len(),
            1
        );
        assert!(write_problems("SELECT 'DELETE' FROM t").is_empty());

        let schema = Schema::parse("CREATE TABLE fill (id VARCHAR PRIMARY KEY, copay DECIMAL(18,2), due_on DATE, n BIGINT);").unwrap();
        let columns = Columns::of(Some(&schema));
        assert_eq!(
            arithmetic_problems("SELECT SUM(copay) FROM fill", &columns).len(),
            1
        );
        assert!(
            arithmetic_problems("SELECT decimal_sum(copay), SUM(n) FROM fill", &columns).is_empty()
        );
        assert_eq!(
            arithmetic_problems("SELECT due_on + 30 FROM fill", &columns).len(),
            1
        );
        assert_eq!(
            arithmetic_problems("SELECT 1 - due_on FROM fill", &columns).len(),
            1
        );
        assert!(
            arithmetic_problems(
                "SELECT date(due_on, '+30 days') FROM fill WHERE due_on >= date('now')",
                &columns
            )
            .is_empty()
        );
    }
}
