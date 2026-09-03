// Project:  Privatium™  |  File: crates/privatium-core/src/store/sandbox.rs
// Authors:  Gabriel Mongefranco (@gabrielmongefranco)
// Created:  2026-09-03  |  Modified: 2026-09-03
// Summary:  spec/app-contract.md §7 — the connection app SQL runs on. Read-only at the file,
//           `query_only` at the connection, and an authorizer that refuses every write, every
//           PRAGMA, ATTACH, and extension loading, so nothing an app's SQL can say reaches the
//           filesystem or the engine's settings. The framework's own connection is separate
//           and never handed out.

use std::path::Path;
use std::time::Duration;

use rusqlite::hooks::{AuthAction, AuthContext, Authorization};
use rusqlite::{Connection, OpenFlags};

use crate::store::decimal;

/// How long a reader waits for a writer's transaction before giving up. The framework's
/// writes are short — one event, or one rebuild.
const BUSY: Duration = Duration::from_secs(5);

/// Open `path` for app SQL.
///
/// Three layers, each sufficient on its own: the file is opened read-only, the connection
/// is `query_only`, and the authorizer below is installed last — after the one `PRAGMA`
/// this function itself needs, because from then on no `PRAGMA` is allowed at all.
pub(crate) fn open_readonly(path: &Path) -> rusqlite::Result<Connection> {
    let conn = Connection::open_with_flags(
        path,
        OpenFlags::SQLITE_OPEN_READ_ONLY | OpenFlags::SQLITE_OPEN_NO_MUTEX,
    )?;
    conn.busy_timeout(BUSY)?;
    conn.execute_batch("PRAGMA query_only = 1;")?;
    decimal::register(&conn)?;
    conn.authorizer(Some(authorize_query))?;
    Ok(conn)
}

/// What app SQL may do: read, and nothing else.
///
/// `SQLITE_READ` is allowed for every table and view, `pv_%` included — the tombstone set
/// and the health rows are derived facts, not secrets. `load_extension` is refused by name
/// on top of being disabled at the API; the rest of the function set is SQLite's own and
/// has no side effects.
fn authorize_query(ctx: AuthContext<'_>) -> Authorization {
    match ctx.action {
        AuthAction::Select | AuthAction::Read { .. } | AuthAction::Recursive => {
            Authorization::Allow
        }
        AuthAction::Function { function_name } => function(function_name),
        AuthAction::Transaction { .. } | AuthAction::Savepoint { .. } => Authorization::Allow,
        _ => Authorization::Deny,
    }
}

/// What a `schema.sql` may do while it is being read: create tables, views and indexes in
/// a throwaway database, and read. Still no `ATTACH`, no `PRAGMA`, no extension.
pub(crate) fn authorize_ddl(ctx: AuthContext<'_>) -> Authorization {
    match ctx.action {
        AuthAction::CreateTable { .. }
        | AuthAction::CreateView { .. }
        | AuthAction::CreateIndex { .. }
        | AuthAction::CreateTempTable { .. }
        | AuthAction::CreateTempView { .. }
        | AuthAction::CreateTempIndex { .. }
        | AuthAction::Insert { .. }
        | AuthAction::Update { .. }
        | AuthAction::Delete { .. }
        | AuthAction::DropTable { .. }
        | AuthAction::DropView { .. }
        | AuthAction::DropIndex { .. }
        | AuthAction::AlterTable { .. }
        | AuthAction::Reindex { .. }
        | AuthAction::Analyze { .. }
        | AuthAction::Select
        | AuthAction::Read { .. }
        | AuthAction::Recursive
        | AuthAction::Transaction { .. }
        | AuthAction::Savepoint { .. } => Authorization::Allow,
        AuthAction::Function { function_name } => function(function_name),
        _ => Authorization::Deny,
    }
}

fn function(name: &str) -> Authorization {
    if name.eq_ignore_ascii_case("load_extension") {
        Authorization::Deny
    } else {
        Authorization::Allow
    }
}

// AGENTS.md, Style: unwrap() is permitted in tests. The crate-level deny reaches unit
// tests inside src/, so each one opts out where it is declared.
#[allow(clippy::unwrap_used, clippy::expect_used)]
#[cfg(test)]
mod tests {
    use super::*;

    /// `spec/app-contract.md §7`, one refusal per way out.
    #[test]
    fn the_app_connection_can_only_read() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("t.sqlite");
        let owner = Connection::open(&path).unwrap();
        owner
            .execute_batch(
                "CREATE TABLE t (id TEXT PRIMARY KEY, x TEXT); INSERT INTO t VALUES ('a', '1');",
            )
            .unwrap();

        let app = open_readonly(&path).unwrap();
        let x: String = app
            .query_row("SELECT x FROM t WHERE id = 'a'", [], |r| r.get(0))
            .unwrap();
        assert_eq!(x, "1");
        let sum: String = app
            .query_row("SELECT decimal_sum(x) FROM t", [], |r| r.get(0))
            .unwrap();
        assert_eq!(sum, "1");

        let leak = dir
            .path()
            .join("leak.sqlite")
            .display()
            .to_string()
            .replace('\\', "/");
        for sql in [
            "INSERT INTO t VALUES ('b', '2')".to_owned(),
            "UPDATE t SET x = '3'".to_owned(),
            "DELETE FROM t".to_owned(),
            "CREATE TABLE u (id TEXT)".to_owned(),
            "DROP TABLE t".to_owned(),
            "PRAGMA journal_mode = WAL".to_owned(),
            "PRAGMA query_only = 0".to_owned(),
            format!("ATTACH '{leak}' AS leak"),
            format!("VACUUM INTO '{leak}'"),
            "SELECT load_extension('x')".to_owned(),
        ] {
            assert!(
                app.execute_batch(&sql).is_err(),
                "app SQL was allowed to run: {sql}"
            );
        }
        assert!(!dir.path().join("leak.sqlite").exists());

        // The owner still writes, with the app's connection open — no window, no seal.
        owner
            .execute_batch("INSERT INTO t VALUES ('b', '2')")
            .unwrap();
        let n: i64 = app
            .query_row("SELECT count(*) FROM t", [], |r| r.get(0))
            .unwrap();
        assert_eq!(n, 2);
    }
}
