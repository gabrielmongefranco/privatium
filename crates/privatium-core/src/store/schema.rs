// Project:  Privatium™  |  File: crates/privatium-core/src/store/schema.rs
// Authors:  Gabriel Mongefranco (@gabrielmongefranco)
// Created:  2026-09-01  |  Modified: 2026-09-02
// Summary:  What a schema.sql declares, learned from DuckDB's own catalog rather than
//           from a parser we wrote. Tables, column types, NOT NULL and CHECK, and views —
//           everything spec/protocol.md §4.5's projection is generated from.

use std::fmt::Write as _;

use duckdb::Connection;
use sha2::{Digest, Sha256};

use crate::store::StoreError;

/// The column every table must have (`spec/app-contract.md §4.5`).
pub const ID_COLUMN: &str = "id";

/// One column of one table, as DuckDB understands it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Column {
    /// The column name, as written.
    pub name: String,
    /// The DuckDB type, spelled as the engine spells it — `DECIMAL(18,2)`, `VARCHAR[]`.
    ///
    /// Taken from the catalog rather than from the source text so that the `CAST` in the
    /// projection and the type the engine will enforce cannot drift apart.
    pub ty: String,
    /// Whether the column is `NOT NULL`.
    ///
    /// Metadata only in M3. `spec/data-api.md §2` enforces this **before** an append, and
    /// the materialized table deliberately carries no constraints at all — see
    /// [`super::materialize`].
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
    /// §4.1`), not from `d`. Keeping it out of this list means no generated expression can
    /// accidentally read `d.id` and let an app overwrite its own row key.
    pub columns: Vec<Column>,
    /// `CHECK` expressions, as DuckDB re-renders them.
    ///
    /// Metadata only in M3, for the same reason as [`Column::not_null`].
    pub checks: Vec<String>,
}

/// One view of an app's schema (`spec/data-api.md §1`, `spec/app-contract.md §5`).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct View {
    /// The view name.
    pub name: String,
    /// The `CREATE VIEW` statement, as DuckDB re-renders it.
    pub sql: String,
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

    /// Learn what `sql` declares by running it and asking DuckDB.
    ///
    /// **Why executing it is the parser.** The plan called for `json_serialize_sql()`, and
    /// that function refuses anything but a `SELECT` — handed DDL it returns
    /// `{"error":true,"error_message":"Only SELECT statements can be serialized to
    /// json!"}`. DuckDB exposes no other parser to safe Rust. So the DDL is executed and
    /// the answer read out of the catalog, which is still "types from the engine that will
    /// execute them" and is neither a regex nor a second SQL implementation.
    ///
    /// **Why running an app's DDL is safe.** The instance is in-memory and is sealed
    /// *before* the file runs, so `COPY`, `ATTACH`, `INSTALL` and any `SET` fail, and an
    /// `INSERT` touches memory that is dropped when this function returns. Nothing here
    /// can reach the filesystem — which matters, because the real cache database has to
    /// keep external access on to read the logs.
    pub fn parse(sql: &str) -> Result<Self, StoreError> {
        let conn = Connection::open_in_memory().map_err(StoreError::Duck)?;
        // The order is `spec/app-contract.md §7`'s, and `lock_configuration` is last
        // because it is what makes the other three unrepealable.
        conn.execute_batch(
            "SET enable_external_access = false;
             SET autoinstall_known_extensions = false;
             SET autoload_known_extensions = false;
             SET lock_configuration = true;",
        )
        .map_err(StoreError::Duck)?;

        conn.execute_batch(sql)
            .map_err(|source| StoreError::Schema {
                problem: first_line(&source.to_string()),
            })?;

        let mut schema = Self {
            tables: read_tables(&conn)?,
            views: read_views(&conn)?,
            hash: hash_of(sql),
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

/// Read the tables and their columns out of the catalog.
///
/// **`schema_name = 'main' AND NOT internal` is load-bearing.** `duckdb_columns()`
/// describes the entire catalog: `pg_catalog`, `information_schema`, the `duckdb_*`
/// functions themselves — some four hundred rows before an app's own table appears.
/// Without the filter, the first "table" in an app's schema is `character_sets`.
fn read_tables(conn: &Connection) -> Result<Vec<Table>, StoreError> {
    let mut columns = conn
        .prepare(
            "SELECT table_name, column_name, data_type, is_nullable
             FROM duckdb_columns()
             WHERE schema_name = 'main' AND NOT internal
             ORDER BY table_name, column_index",
        )
        .map_err(StoreError::Duck)?;
    let rows = columns
        .query_map([], |row| {
            Ok((
                row.get::<_, String>(0)?,
                row.get::<_, String>(1)?,
                row.get::<_, String>(2)?,
                row.get::<_, bool>(3)?,
            ))
        })
        .map_err(StoreError::Duck)?;

    // A view's columns come back from `duckdb_columns()` too, so tables are established
    // from `duckdb_tables()` first and anything else is ignored.
    let table_names = read_table_names(conn)?;
    let mut tables: Vec<Table> = Vec::new();
    for row in rows {
        let (table, column, ty, nullable) = row.map_err(StoreError::Duck)?;
        if !table_names.contains(&table) {
            continue;
        }
        let entry = match tables.iter_mut().find(|t| t.name == table) {
            Some(entry) => entry,
            None => {
                tables.push(Table {
                    name: table.clone(),
                    columns: Vec::new(),
                    checks: Vec::new(),
                });
                // The push cannot fail and the entry is the one just added.
                match tables.last_mut() {
                    Some(entry) => entry,
                    None => continue,
                }
            }
        };
        if column == ID_COLUMN {
            // Present, deliberately unprojected. See `Table::columns`.
            continue;
        }
        entry.columns.push(Column {
            name: column,
            ty,
            not_null: !nullable,
        });
    }

    // `spec/app-contract.md §4.5`: every table needs `id VARCHAR PRIMARY KEY`. This is a
    // load refusal rather than lint (`PV106`, M12) because `§4.5` groups events by `id`:
    // a table without one cannot be materialized at all, so there is nothing to warn about.
    for name in &table_names {
        if !has_id_column(conn, name)? {
            return Err(StoreError::Schema {
                problem: format!(
                    "table `{name}` has no `id VARCHAR` column; \
                     spec/app-contract.md §4.5 requires one and §4.5 keys every row by it"
                ),
            });
        }
    }

    for table in &mut tables {
        table.checks = read_checks(conn, &table.name)?;
    }
    Ok(tables)
}

fn read_table_names(conn: &Connection) -> Result<Vec<String>, StoreError> {
    let mut statement = conn
        .prepare(
            "SELECT table_name FROM duckdb_tables()
             WHERE schema_name = 'main' AND NOT internal
             ORDER BY table_name",
        )
        .map_err(StoreError::Duck)?;
    let rows = statement
        .query_map([], |row| row.get::<_, String>(0))
        .map_err(StoreError::Duck)?;
    let mut names = Vec::new();
    for row in rows {
        names.push(row.map_err(StoreError::Duck)?);
    }
    Ok(names)
}

fn has_id_column(conn: &Connection, table: &str) -> Result<bool, StoreError> {
    let found: i64 = conn
        .query_row(
            "SELECT count(*) FROM duckdb_columns()
             WHERE schema_name = 'main' AND NOT internal
               AND table_name = ? AND column_name = ?",
            duckdb::params![table, ID_COLUMN],
            |row| row.get(0),
        )
        .map_err(StoreError::Duck)?;
    Ok(found > 0)
}

/// `CHECK` expressions for one table.
///
/// `duckdb_constraints()` reports `NOT NULL` and `PRIMARY KEY` here too; those are read
/// from `duckdb_columns()` and from the `id` requirement respectively, so only `CHECK`
/// rows are wanted. A `NOT NULL` row's `constraint_text` is the literal string
/// `NOT NULL` with no column in it — the column is in `constraint_column_names` — which
/// is exactly the trap this filter avoids.
fn read_checks(conn: &Connection, table: &str) -> Result<Vec<String>, StoreError> {
    let mut statement = conn
        .prepare(
            "SELECT constraint_text FROM duckdb_constraints()
             WHERE schema_name = 'main' AND table_name = ? AND constraint_type = 'CHECK'
             ORDER BY constraint_index",
        )
        .map_err(StoreError::Duck)?;
    let rows = statement
        .query_map(duckdb::params![table], |row| row.get::<_, String>(0))
        .map_err(StoreError::Duck)?;
    let mut checks = Vec::new();
    for row in rows {
        checks.push(row.map_err(StoreError::Duck)?);
    }
    Ok(checks)
}

fn read_views(conn: &Connection) -> Result<Vec<View>, StoreError> {
    let mut statement = conn
        .prepare(
            "SELECT view_name, sql FROM duckdb_views()
             WHERE schema_name = 'main' AND NOT internal
             ORDER BY view_name",
        )
        .map_err(StoreError::Duck)?;
    let rows = statement
        .query_map([], |row| {
            Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?))
        })
        .map_err(StoreError::Duck)?;
    let mut views = Vec::new();
    for row in rows {
        let (name, sql) = row.map_err(StoreError::Duck)?;
        views.push(View { name, sql });
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

/// DuckDB errors carry a stack of context; the first line is the part a person reads.
fn first_line(message: &str) -> String {
    message.lines().next().unwrap_or(message).to_owned()
}

// AGENTS.md, Style: unwrap() is permitted in tests. The crate-level deny reaches unit
// tests inside src/, so each one opts out where it is declared.
#[allow(clippy::unwrap_used, clippy::expect_used)]
#[cfg(test)]
mod tests {
    use super::*;

    /// The whole point of introspecting rather than matching text: a schema written the
    /// way a person writes one, with the words `CREATE TABLE` appearing in a comment and
    /// inside a string literal, still yields the right columns.
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
                 CHECK (kind IN ('q', 'a'))
             );
             COMMENT ON TABLE node IS 'the words CREATE TABLE inside a string literal';
             CREATE VIEW v_leaves AS SELECT id FROM node WHERE kind = 'a';",
        )
        .unwrap();

        let node = schema.table("node").unwrap();
        let names: Vec<&str> = node.columns.iter().map(|c| c.name.as_str()).collect();
        assert_eq!(
            names,
            vec!["kind", "text", "amount", "tags"],
            "`id` is not projected"
        );

        let by = |name: &str| node.columns.iter().find(|c| c.name == name).unwrap();
        assert_eq!(
            by("amount").ty,
            "DECIMAL(18,2)",
            "the engine's own spelling"
        );
        assert_eq!(by("tags").ty, "VARCHAR[]");
        assert!(by("kind").not_null);
        assert!(!by("amount").not_null);

        assert_eq!(node.checks.len(), 1, "{:?}", node.checks);
        assert!(node.checks[0].contains("kind"), "{:?}", node.checks);

        assert_eq!(schema.views.len(), 1);
        assert_eq!(schema.views[0].name, "v_leaves");
    }

    /// `duckdb_columns()` describes the whole catalog. If the filter were ever dropped,
    /// this schema would come back with hundreds of tables starting at `character_sets`.
    #[test]
    fn the_system_catalog_is_not_mistaken_for_the_app_schema() {
        let schema = Schema::parse("CREATE TABLE profile (id VARCHAR PRIMARY KEY);").unwrap();
        assert_eq!(schema.tables.len(), 1);
        assert_eq!(schema.tables[0].name, "profile");
    }

    /// `spec/app-contract.md §4.5`. Refused at load, because `§4.5` has nothing to group by.
    #[test]
    fn a_table_without_an_id_column_is_refused() {
        let error = Schema::parse("CREATE TABLE bad (name VARCHAR);").unwrap_err();
        assert!(error.to_string().contains("bad"), "{error}");
        assert!(error.to_string().contains("id"), "{error}");
    }

    /// The introspection instance is sealed before the file runs, so a `schema.sql` that
    /// tries to reach the filesystem fails there rather than against the real database.
    #[test]
    fn a_schema_that_touches_the_filesystem_is_refused() {
        let error = Schema::parse("COPY (SELECT 1) TO 'leak.csv';").unwrap_err();
        assert!(
            error.to_string().to_lowercase().contains("file system")
                || error.to_string().to_lowercase().contains("permission"),
            "{error}"
        );
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
    /// a parser error and none of them says which line. That is how the reserved `at`
    /// column was found.
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

        // §3.10 names the column `at`, and DuckDB reserves the word. The quoting in
        // sys.sql is what makes the dictionary's name survive.
        let audit = schema.table("sys_audit").unwrap();
        assert!(audit.columns.iter().any(|c| c.name == "at"), "{audit:?}");

        // §4's views, less `v_health`, which the materializer creates over `pv.health`
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
