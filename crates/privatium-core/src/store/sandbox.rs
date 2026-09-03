// Project:  Privatium™  |  File: crates/privatium-core/src/store/sandbox.rs
// Authors:  Gabriel Mongefranco (@gabrielmongefranco)
// Created:  2026-09-03  |  Modified: 2026-09-03
// Summary:  spec/app-contract.md §7 — the connection app SQL runs on. Read-only at the file,
//           `query_only` at the connection, and an authorizer that refuses every write, every
//           PRAGMA, ATTACH, and extension loading, so nothing an app's SQL can say reaches the
//           filesystem or the engine's settings. The framework's own connection is separate
//           and never handed out. The framework attaches cache/_sys.sqlite as `sys` before
//           the authorizer goes on (spec/data-dictionary.md §4): read-only, like main.

use std::path::Path;
use std::time::Duration;

use rusqlite::hooks::{AuthAction, AuthContext, Authorization};
use rusqlite::{Connection, OpenFlags};

use crate::store::{decimal, params};

/// How long a reader waits for a writer's transaction before giving up. The framework's
/// writes are short — one event, or one rebuild.
const BUSY: Duration = Duration::from_secs(5);

/// The name `cache/_sys.sqlite` is attached under (`spec/data-dictionary.md §1`, `§4`).
pub const SYS_ALIAS: &str = "sys";

/// Open `path` for app SQL, with `sys` — `cache/_sys.sqlite` — attached read-only when it
/// exists, and the `pv_param` table the data API binds `$name` through.
///
/// Three layers, each sufficient on its own: the file is opened read-only, the connection
/// is `query_only`, and the authorizer below is installed last — after the one `PRAGMA`
/// and the one `ATTACH` this function itself needs, because from then on neither is
/// allowed at all. An attached database takes the main connection's flags, so `sys` is
/// read-only at the file too.
pub(crate) fn open_readonly(
    path: &Path,
    sys: Option<&Path>,
) -> rusqlite::Result<(Connection, params::Params)> {
    let conn = Connection::open_with_flags(
        path,
        OpenFlags::SQLITE_OPEN_READ_ONLY | OpenFlags::SQLITE_OPEN_NO_MUTEX,
    )?;
    conn.busy_timeout(BUSY)?;
    conn.execute_batch("PRAGMA query_only = 1;")?;
    decimal::register(&conn)?;
    let params = params::register(&conn)?;
    if let Some(sys) = sys.filter(|sys| sys.is_file()) {
        conn.execute(
            &format!("ATTACH ? AS {SYS_ALIAS}"),
            [sys.to_string_lossy().as_ref()],
        )?;
    }
    conn.authorizer(Some(authorize_query))?;
    Ok((conn, params))
}

/// What app SQL may do: read, and nothing else.
///
/// `SQLITE_READ` is allowed for every table and view, `pv_%` included — the tombstone set
/// and the health rows are derived facts, not secrets — and for the attached `sys`, whose
/// tables carry nothing a log line does not (`AGENTS.md` 5). `load_extension` is refused by
/// name on top of being disabled at the API; the rest of the function set is SQLite's own
/// and has no side effects.
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

        let (app, _) = open_readonly(&path, None).unwrap();
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

    /// `spec/data-dictionary.md §4` — `sys` is attached read-only: its views answer, a
    /// write to it is refused, and it cannot be detached to make room for another file.
    #[test]
    fn sys_is_attached_read_only() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("t.sqlite");
        let sys = dir.path().join("_sys.sqlite");
        Connection::open(&path)
            .unwrap()
            .execute_batch("CREATE TABLE t (id TEXT PRIMARY KEY);")
            .unwrap();
        Connection::open(&sys)
            .unwrap()
            .execute_batch(
                "CREATE TABLE sys_app (id TEXT PRIMARY KEY, enabled INTEGER);
                 INSERT INTO sys_app VALUES ('hello', 1);
                 CREATE VIEW v_app_nav AS SELECT id FROM sys_app WHERE enabled = 1;",
            )
            .unwrap();

        let (app, _) = open_readonly(&path, Some(&sys)).unwrap();
        let slug: String = app
            .query_row("SELECT id FROM sys.v_app_nav", [], |r| r.get(0))
            .unwrap();
        assert_eq!(slug, "hello");
        for sql in [
            "INSERT INTO sys.sys_app VALUES ('x', 1)",
            "DELETE FROM sys.sys_app",
            "DETACH sys",
        ] {
            assert!(app.execute_batch(sql).is_err(), "{sql}");
        }
        let still: i64 = app
            .query_row("SELECT count(*) FROM sys.sys_app", [], |r| r.get(0))
            .unwrap();
        assert_eq!(still, 1);

        // No `_sys` file: nothing is attached and the connection still opens.
        let (alone, _) = open_readonly(&path, Some(&dir.path().join("absent.sqlite"))).unwrap();
        assert!(alone.prepare("SELECT * FROM sys.v_app_nav").is_err());
    }
}
