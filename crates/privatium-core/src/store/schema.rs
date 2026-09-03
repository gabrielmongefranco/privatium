// Project:  Privatium™  |  File: crates/privatium-core/src/store/schema.rs
// Authors:  Gabriel Mongefranco (@gabrielmongefranco)
// Created:  2026-09-01  |  Modified: 2026-09-03
// Summary:  What a schema.sql declares, learned from SQLite's own catalog rather than from a
//           parser we wrote: tables, the declared type of every column, NOT NULL, and views.
//           The declared type decides how the materializer stores a column
//           (spec/data-dictionary.md §2), because SQLite would otherwise decide by affinity
//           and turn a DECIMAL into a float.

use std::fmt::Write as _;

use rusqlite::Connection;
use sha2::{Digest, Sha256};

use crate::store::{StoreError, decimal, params, sandbox};

/// The column every table must have (`spec/app-contract.md §4.5`).
pub const ID_COLUMN: &str = "id";

/// How the materializer stores a column, decided from its declared type
/// (`spec/data-dictionary.md §2`).
///
/// SQLite has five storage classes and column *affinity*, not types: a column declared
/// `DECIMAL(18,2)` gets NUMERIC affinity and stores `12.34` as an eight-byte float. So the
/// cache table is declared with the storage the dictionary wants, and the value is typed
/// here, on the way in, from the type the author wrote.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Kind {
    /// `VARCHAR`, `TEXT`, `DATE`, `TIME`, `TIMESTAMPTZ`, `INTERVAL`, and anything unknown:
    /// stored as text, exactly as the event spelled it.
    Text,
    /// `BIGINT`, `INTEGER` and their relatives: a 64-bit integer, parsed from the string
    /// `§2.1` says it crosses as.
    Integer,
    /// `DECIMAL(p,s)` / `NUMERIC`: text at the declared scale, with the `decimal` collation,
    /// never a float.
    Decimal {
        /// Places after the point.
        scale: u8,
    },
    /// `BOOLEAN`: `1` or `0`.
    Boolean,
    /// `JSON`, `VARCHAR[]` and any other structured type: the value's own JSON text.
    Json,
}

impl Kind {
    /// From the declared type, as `PRAGMA table_info` reports it.
    #[must_use]
    pub fn of(declared: &str) -> Self {
        let upper = declared.trim().to_ascii_uppercase();
        if upper.ends_with(']')
            || upper == "JSON"
            || upper.starts_with("STRUCT")
            || upper.starts_with("MAP")
            || upper.starts_with("LIST")
        {
            return Self::Json;
        }
        if let Some(scale) = decimal::declared_scale(&upper) {
            return Self::Decimal { scale };
        }
        if upper == "BOOLEAN" || upper == "BOOL" {
            return Self::Boolean;
        }
        if upper.starts_with("INTERVAL") {
            return Self::Text;
        }
        if upper.contains("INT") {
            return Self::Integer;
        }
        Self::Text
    }

    /// The type name the cache table declares, so SQLite's affinity agrees with the kind.
    #[must_use]
    pub fn storage(self) -> &'static str {
        match self {
            Self::Text | Self::Json => "TEXT",
            Self::Integer | Self::Boolean => "INTEGER",
            Self::Decimal { .. } => "TEXT COLLATE decimal",
        }
    }
}

/// One column of one table.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Column {
    /// The column name, as written.
    pub name: String,
    /// The declared type, as written — `DECIMAL(18,2)`, `VARCHAR[]`. What the data API
    /// reports (`spec/data-api.md §1`) and what [`Kind::of`] reads.
    pub ty: String,
    /// How it is stored and typed.
    pub kind: Kind,
    /// Whether the column is `NOT NULL`.
    ///
    /// Metadata only. `spec/data-api.md §2` enforces this **before** an append, and the
    /// materialized table carries no constraint — a log line that omits the key still
    /// materializes, with NULL (`spec/app-contract.md §4.5`).
    pub not_null: bool,
}

/// One table of an app's schema.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Table {
    /// The table name.
    pub name: String,
    /// Its columns, in declaration order, **excluding** `id`.
    ///
    /// `id` is not a projected column: it comes from the envelope (`spec/protocol.md
    /// §4.1`), not from `d`. Keeping it out of this list means nothing can read `d.id` and
    /// let an app overwrite its own row key.
    pub columns: Vec<Column>,
}

/// One view of an app's schema (`spec/data-api.md §1`, `spec/app-contract.md §5`).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct View {
    /// The view name.
    pub name: String,
    /// The `CREATE VIEW` statement as SQLite holds it: the author's text with every
    /// `$name` rewritten to `pv_param('name')` (`params`).
    pub sql: String,
    /// The `$name` placeholders the view reads, in order of first appearance — what
    /// `/api/q/<view>` binds from the query string (`spec/data-api.md §1`).
    pub params: Vec<String>,
}

/// Everything a `schema.sql` declares.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Schema {
    /// Tables, in name order.
    pub tables: Vec<Table>,
    /// Views, in name order.
    pub views: Vec<View>,
    /// SHA-256 of the source text.
    ///
    /// This is `sys_app.schema_hash` (`spec/data-dictionary.md §3.4`), and a change to it
    /// is what forces a full rematerialization (`spec/app-contract.md §4.5`).
    pub hash: String,
    /// The DDL as it ran: the source with `$name` rewritten (`params::rewrite`). What the
    /// constraint check of `spec/data-api.md §2` executes in its throwaway database.
    pub ddl: String,
}

impl Schema {
    /// An app with no `schema.sql` at all — the `sketch` case.
    ///
    /// `spec/app-contract.md §4.5` makes the file optional and `§5.3` makes the event log
    /// a document store without it. Not an error, and not a special case anywhere above:
    /// an empty schema materializes zero tables and everything else behaves.
    #[must_use]
    pub fn empty() -> Self {
        Self {
            hash: hash_of(""),
            ..Self::default()
        }
    }

    /// Learn what `sql` declares by running it in a throwaway in-memory database and
    /// asking the catalog.
    ///
    /// **Why executing it is the parser.** SQLite exposes no parser to safe Rust, and
    /// text-matching `CREATE TABLE` is a second SQL implementation. So the DDL runs — under
    /// the same authorizer an app's connection gets, plus permission to create — and the
    /// answer is read out of `sqlite_master` and `PRAGMA table_info`, which is "types from
    /// the engine that will execute them".
    ///
    /// **Why running an app's DDL is safe.** The database is in memory and gone when this
    /// returns; the authorizer refuses `ATTACH`, every `PRAGMA` and `load_extension()`, so
    /// nothing in the file can reach the filesystem or the real cache.
    pub fn parse(sql: &str) -> Result<Self, StoreError> {
        let conn = Connection::open_in_memory().map_err(StoreError::Sql)?;
        decimal::register(&conn).map_err(StoreError::Sql)?;
        params::register(&conn).map_err(StoreError::Sql)?;
        conn.authorizer(Some(sandbox::authorize_ddl))
            .map_err(StoreError::Sql)?;

        // SQLite refuses a `$name` inside a view; the framework's `pv_param('name')` is
        // what runs in its place (`spec/data-api.md §1`, `params`).
        let ddl = params::rewrite(sql);
        conn.execute_batch(&ddl)
            .map_err(|source| StoreError::Schema {
                problem: first_line(&source.to_string()),
            })?;
        // The file has run; the introspection below uses `pragma_table_info`, which the
        // authorizer would refuse as a PRAGMA, and needs no sandbox of its own.
        conn.authorizer::<fn(rusqlite::hooks::AuthContext<'_>) -> rusqlite::hooks::Authorization>(
            None,
        )
        .map_err(StoreError::Sql)?;

        let mut schema = Self {
            tables: read_tables(&conn)?,
            views: read_views(&conn)?,
            hash: hash_of(sql),
            ddl,
        };
        schema.tables.sort_by(|a, b| a.name.cmp(&b.name));
        schema.views.sort_by(|a, b| a.name.cmp(&b.name));
        Ok(schema)
    }

    /// The table of that name, if the schema has one.
    #[must_use]
    pub fn table(&self, name: &str) -> Option<&Table> {
        self.tables.iter().find(|table| table.name == name)
    }
}

/// Read the tables and their columns out of the catalog. `sqlite_%` tables are SQLite's
/// own and are not an app's.
fn read_tables(conn: &Connection) -> Result<Vec<Table>, StoreError> {
    let names = read_names(conn, "table")?;
    let mut tables = Vec::with_capacity(names.len());
    for name in names {
        let mut statement = conn
            .prepare("SELECT name, type, \"notnull\" FROM pragma_table_info(?) ORDER BY cid")
            .map_err(StoreError::Sql)?;
        let rows = statement
            .query_map(rusqlite::params![name], |row| {
                Ok((
                    row.get::<_, String>(0)?,
                    row.get::<_, String>(1)?,
                    row.get::<_, i64>(2)?,
                ))
            })
            .map_err(StoreError::Sql)?;
        let mut columns = Vec::new();
        let mut has_id = false;
        for row in rows {
            let (column, ty, not_null) = row.map_err(StoreError::Sql)?;
            if column == ID_COLUMN {
                // Present, deliberately unprojected. See `Table::columns`.
                has_id = true;
                continue;
            }
            columns.push(Column {
                kind: Kind::of(&ty),
                name: column,
                ty,
                not_null: not_null != 0,
            });
        }
        // `spec/app-contract.md §4.5`: every table needs `id VARCHAR PRIMARY KEY`. A load
        // refusal rather than lint (`PV106`, M12), because `§4.5` groups events by `id`: a
        // table without one cannot be materialized at all.
        if !has_id {
            return Err(StoreError::Schema {
                problem: format!(
                    "table `{name}` has no `id VARCHAR` column; \
                     spec/app-contract.md §4.5 requires one and §4.5 keys every row by it"
                ),
            });
        }
        tables.push(Table { name, columns });
    }
    Ok(tables)
}

fn read_names(conn: &Connection, kind: &str) -> Result<Vec<String>, StoreError> {
    let mut statement = conn
        .prepare(
            "SELECT name FROM sqlite_master WHERE type = ? AND name NOT LIKE 'sqlite_%' \
             ORDER BY name",
        )
        .map_err(StoreError::Sql)?;
    let rows = statement
        .query_map(rusqlite::params![kind], |row| row.get::<_, String>(0))
        .map_err(StoreError::Sql)?;
    let mut names = Vec::new();
    for row in rows {
        names.push(row.map_err(StoreError::Sql)?);
    }
    Ok(names)
}

fn read_views(conn: &Connection) -> Result<Vec<View>, StoreError> {
    let mut statement = conn
        .prepare("SELECT name, sql FROM sqlite_master WHERE type = 'view' ORDER BY name")
        .map_err(StoreError::Sql)?;
    let rows = statement
        .query_map([], |row| {
            Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?))
        })
        .map_err(StoreError::Sql)?;
    let mut views = Vec::new();
    for row in rows {
        let (name, sql) = row.map_err(StoreError::Sql)?;
        let params = params::placeholders(&sql);
        views.push(View { name, sql, params });
    }
    Ok(views)
}

/// SHA-256, lowercase hex. `sys_app.schema_hash` (`spec/data-dictionary.md §3.4`).
pub(crate) fn hash_of(sql: &str) -> String {
    let digest = Sha256::digest(sql.as_bytes());
    let mut out = String::with_capacity(digest.len() * 2);
    for byte in digest {
        // Writing to a String cannot fail; the result is discarded rather than unwrapped
        // because `AGENTS.md` forbids unwrap outside tests.
        let _ = write!(out, "{byte:02x}");
    }
    out
}

/// The first line of an engine error is the part a person reads.
pub(crate) fn first_line(message: &str) -> String {
    message.lines().next().unwrap_or(message).to_owned()
}

// AGENTS.md, Style: unwrap() is permitted in tests. The crate-level deny reaches unit
// tests inside src/, so each one opts out where it is declared.
#[allow(clippy::unwrap_used, clippy::expect_used)]
#[cfg(test)]
mod tests {
    use super::*;

    /// The whole point of introspecting rather than matching text: a schema written the
    /// way a person writes one, with the words `CREATE TABLE` in a comment and inside a
    /// string literal, still yields the right columns, with their declared types verbatim.
    #[test]
    fn a_schema_is_read_from_the_catalog_not_from_the_text() {
        let schema = Schema::parse(
            "-- this comment says CREATE TABLE and must not be read as one
             CREATE TABLE node (
                 id     VARCHAR PRIMARY KEY,
                 kind   VARCHAR NOT NULL,
                 \"text\" VARCHAR NOT NULL,
                 amount DECIMAL(18,2),
                 tags   VARCHAR[],
                 ok     BOOLEAN,
                 n      BIGINT,
                 CHECK (kind IN ('q', 'a'))
             );
             CREATE VIEW v_leaves AS SELECT id FROM node WHERE kind = 'CREATE TABLE';",
        )
        .unwrap();

        let node = schema.table("node").unwrap();
        let names: Vec<&str> = node.columns.iter().map(|c| c.name.as_str()).collect();
        assert_eq!(
            names,
            vec!["kind", "text", "amount", "tags", "ok", "n"],
            "`id` is not projected"
        );

        let by = |name: &str| node.columns.iter().find(|c| c.name == name).unwrap();
        assert_eq!(by("amount").ty, "DECIMAL(18,2)", "as the author spelled it");
        assert_eq!(by("amount").kind, Kind::Decimal { scale: 2 });
        assert_eq!(by("tags").ty, "VARCHAR[]");
        assert_eq!(by("tags").kind, Kind::Json);
        assert_eq!(by("ok").kind, Kind::Boolean);
        assert_eq!(by("n").kind, Kind::Integer);
        assert_eq!(by("kind").kind, Kind::Text);
        assert!(by("kind").not_null);
        assert!(!by("amount").not_null);

        assert_eq!(schema.views.len(), 1);
        assert_eq!(schema.views[0].name, "v_leaves");
        assert!(schema.views[0].sql.starts_with("CREATE VIEW v_leaves"));
        assert!(schema.views[0].params.is_empty());
    }

    /// `spec/data-api.md §1` — a view may read `$name`, which SQLite alone refuses
    /// ("parameters are not allowed in views"); the schema loads with the placeholder
    /// rewritten to the framework's function and the view knows its names.
    #[test]
    fn a_view_may_read_a_named_placeholder() {
        let source = "CREATE TABLE fill (id VARCHAR PRIMARY KEY, due_on DATE);
             CREATE VIEW v_upcoming AS SELECT id FROM fill
               WHERE due_on <= date('now', '+' || $days || ' days') AND $days IS NOT NULL;";
        let schema = Schema::parse(source).unwrap();
        assert_eq!(schema.views[0].params, vec!["days"]);
        assert!(
            schema.views[0].sql.contains("pv_param('days')"),
            "{}",
            schema.views[0].sql
        );
        assert!(!schema.ddl.contains("$days"));
        assert_eq!(
            schema.hash,
            hash_of(source),
            "the hash is over the author's text"
        );
    }

    #[test]
    fn kinds_follow_the_dictionary() {
        assert_eq!(Kind::of("VARCHAR"), Kind::Text);
        assert_eq!(Kind::of("DATE"), Kind::Text);
        assert_eq!(Kind::of("TIMESTAMPTZ"), Kind::Text);
        assert_eq!(Kind::of("INTERVAL"), Kind::Text);
        assert_eq!(Kind::of("TIME"), Kind::Text);
        assert_eq!(Kind::of("BIGINT"), Kind::Integer);
        assert_eq!(Kind::of("integer"), Kind::Integer);
        assert_eq!(Kind::of("DECIMAL(9,4)"), Kind::Decimal { scale: 4 });
        assert_eq!(Kind::of("BOOLEAN"), Kind::Boolean);
        assert_eq!(Kind::of("JSON"), Kind::Json);
        assert_eq!(Kind::of("VARCHAR[]"), Kind::Json);
        assert_eq!(Kind::Decimal { scale: 2 }.storage(), "TEXT COLLATE decimal");
        assert_eq!(Kind::Boolean.storage(), "INTEGER");
    }

    /// `spec/app-contract.md §4.5`. Refused at load, because `§4.5` has nothing to group by.
    #[test]
    fn a_table_without_an_id_column_is_refused() {
        let error = Schema::parse("CREATE TABLE bad (name VARCHAR);").unwrap_err();
        assert!(error.to_string().contains("bad"), "{error}");
        assert!(error.to_string().contains("id"), "{error}");
    }

    /// The introspection database is sandboxed, so a `schema.sql` that tries to reach the
    /// filesystem or the engine's settings fails there rather than against the real cache.
    #[test]
    fn a_schema_that_reaches_outside_is_refused() {
        for sql in [
            "ATTACH 'leak.sqlite' AS leak;",
            "PRAGMA journal_mode = WAL;",
            "SELECT load_extension('x');",
        ] {
            let error = Schema::parse(sql).unwrap_err();
            assert!(
                error.to_string().to_lowercase().contains("not authorized"),
                "{sql}: {error}"
            );
        }
    }

    /// `spec/app-contract.md §4.5` — the file is optional, and its absence is ordinary.
    #[test]
    fn an_app_with_no_schema_has_no_tables() {
        let schema = Schema::empty();
        assert!(schema.tables.is_empty());
        assert!(schema.views.is_empty());
    }

    /// The framework's own schema parses, and holds exactly the tables
    /// `spec/data-dictionary.md §3` says are replicated.
    ///
    /// A unit test rather than something the bootstrap discovers: a bad `sys.sql` breaks
    /// `Node::open`, so without this every test in `tests/bootstrap.rs` fails at once with
    /// a parser error and none of them says which line.
    #[test]
    fn the_framework_schema_parses_and_matches_the_dictionary() {
        let schema = Schema::parse(crate::store::SYS_DDL).unwrap();

        let names: Vec<&str> = schema.tables.iter().map(|t| t.name.as_str()).collect();
        assert_eq!(
            names,
            vec![
                "sys_app",
                "sys_app_grant",
                "sys_audit",
                "sys_cluster",
                "sys_device",
                "sys_node",
                "sys_node_revocation",
                "sys_setting",
                "sys_snapshot",
            ]
        );

        // §3.3, §3.7, §3.7b and §3.8 are local-store-only and are not event-sourced into
        // data/_sys/, so they must not appear; §3.11 is reserved and unimplemented.
        for absent in [
            "sys_pairing",
            "sys_peer",
            "sys_endpoint",
            "sys_sync_state",
            "sys_migration",
        ] {
            assert!(schema.table(absent).is_none(), "{absent} should not exist");
        }

        let audit = schema.table("sys_audit").unwrap();
        assert!(audit.columns.iter().any(|c| c.name == "at"), "{audit:?}");

        // §4's views, less `v_health`, which the materializer creates over `pv_health`
        // rather than this file (see sys.sql).
        let views: Vec<&str> = schema.views.iter().map(|v| v.name.as_str()).collect();
        assert_eq!(
            views,
            vec!["v_app_nav", "v_audit_recent", "v_device_active"]
        );
    }

    /// `spec/data-dictionary.md §3.4` — the hash is what triggers rematerialization, so it
    /// has to change when the text does and not otherwise.
    #[test]
    fn the_hash_follows_the_text() {
        let a = Schema::parse("CREATE TABLE t (id VARCHAR PRIMARY KEY);").unwrap();
        let b = Schema::parse("CREATE TABLE t (id VARCHAR PRIMARY KEY);").unwrap();
        let c = Schema::parse("CREATE TABLE t (id VARCHAR PRIMARY KEY, x VARCHAR);").unwrap();
        assert_eq!(a.hash, b.hash);
        assert_ne!(a.hash, c.hash);
        assert_eq!(a.hash.len(), 64);
    }
}
