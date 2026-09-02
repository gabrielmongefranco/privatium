// Project:  Privatium™  |  File: crates/privatium-core/src/store/materialize.rs
// Authors:  Gabriel Mongefranco (@gabrielmongefranco)
// Created:  2026-09-01  |  Modified: 2026-09-01
// Summary:  spec/protocol.md §4.5 as SQL — the full replay that is the definition, and the
//           incremental apply that has to agree with it byte for byte
//           (docs/plans/phase-1.md §2.5).

use std::fmt::Write as _;

use crate::store::schema::{Column, ID_COLUMN, Table};

/// The framework's own schema inside an app's cache database.
///
/// Separate from `main` so nothing here can collide with a table an app declared, and so
/// `spec/data-dictionary.md §5`'s naming rules stay the app author's to obey.
pub(crate) const PV_SCHEMA: &str = "pv";

/// Ids whose winning event is a tombstone (`spec/protocol.md §4.6`).
pub(crate) const TOMBSTONE_TABLE: &str = "pv._tombstone";

/// The envelope, as `read_json()` is told to type it (`spec/protocol.md §4.1`).
///
/// Every field is declared, including the two the projection does not select: `§4.5`
/// step 1 filters on `app` **and** `tbl`, so both have to exist as columns. Unknown
/// top-level fields — a `pv/2` peer's `origin`, say — are simply not listed and are
/// ignored, which is `§4.2` holding without anything having to be done about it.
const ENVELOPE_COLUMNS: &str = "{seq:'BIGINT', lam:'BIGINT', ts:'VARCHAR', dev:'VARCHAR', \
                                app:'VARCHAR', op:'VARCHAR', tbl:'VARCHAR', id:'VARCHAR', \
                                d:'JSON'}";

/// The SQL to rebuild one table from the log, per `spec/protocol.md §4.5`.
///
/// `log_glob` is `data/<app>/log/*.jsonl` — every device's segments, which is what §4.5
/// step 1 asks for. `cutoff` is the `§4.4` horizon, passed in rather than read from the
/// clock here so that a replay and an incremental apply compared against each other
/// cannot disagree merely because time passed between them.
pub(crate) fn replay_sql(
    target: &str,
    app: &str,
    table: &Table,
    log_glob: &str,
    cutoff: &str,
) -> String {
    format!(
        "CREATE OR REPLACE TABLE {target} AS
         WITH ev AS (
           SELECT seq, lam, ts, dev, op, id, d
           FROM read_json('{glob}', format = 'newline_delimited', columns = {cols})
           WHERE {filter}
         ), ranked AS (
           SELECT *, row_number() OVER (PARTITION BY id ORDER BY {order}) AS rn
           FROM ev
         )
         SELECT {projection}
         FROM ranked
         WHERE rn = 1 AND op = 'put';",
        glob = escape_literal(log_glob),
        cols = ENVELOPE_COLUMNS,
        filter = row_filter(app, &table.name, cutoff),
        order = LWW_ORDER,
        projection = projection(&table.columns),
    )
}

/// The tombstone set, from the same replay (`spec/protocol.md §4.6`).
///
/// The mirror image of [`replay_sql`]: the same winning event per `id`, kept when it is a
/// `del` rather than when it is a `put`. Derived, disposable, and in `cache/` — it is the
/// set M9's data API consults to refuse a client-supplied ULID that has been deleted.
pub(crate) fn tombstone_sql(app: &str, tables: &[Table], log_glob: &str, cutoff: &str) -> String {
    if tables.is_empty() {
        return format!("CREATE OR REPLACE TABLE {TOMBSTONE_TABLE} (tbl VARCHAR, id VARCHAR);");
    }
    let names = tables
        .iter()
        .map(|table| format!("'{}'", escape_literal(&table.name)))
        .collect::<Vec<_>>()
        .join(", ");
    format!(
        "CREATE OR REPLACE TABLE {TOMBSTONE_TABLE} AS
         WITH ev AS (
           SELECT seq, lam, ts, dev, op, tbl, id
           FROM read_json('{glob}', format = 'newline_delimited', columns = {cols})
           WHERE app = '{app}' AND tbl IN ({names})
             AND {sane}
         ), ranked AS (
           SELECT *, row_number() OVER (PARTITION BY tbl, id ORDER BY {order}) AS rn
           FROM ev
         )
         SELECT tbl, id FROM ranked WHERE rn = 1 AND op = 'del';",
        glob = escape_literal(log_glob),
        cols = ENVELOPE_COLUMNS,
        app = escape_literal(app),
        sane = sane_row(cutoff),
        order = LWW_ORDER,
    )
}

/// Apply one event this node just wrote, without re-reading the log
/// (`docs/plans/phase-1.md §2.3`).
///
/// **This is a `DELETE` followed by an `INSERT`, and that is not a violation of
/// `AGENTS.md` invariant 3.** Invariant 3 governs **log files**, which are never touched
/// here; `cache/<slug>.duckdb` is a derived artefact that `spec/protocol.md §3.1` says may
/// be deleted entirely without losing anything. Do not "fix" this into an append.
///
/// **Why it is correct to overwrite blindly.** The event was written by this node a moment
/// ago, so `§4.3` gives it `max(lam_local, lam_max_seen) + 1` — the highest `lam` in the
/// app — and `§4.5` therefore makes it the winner for its `id` no matter what else the log
/// holds. That reasoning is the whole licence for skipping the replay, and it expires the
/// moment a second writer exists: Phase 3's sync receiver ingests events whose `lam` may
/// be lower than ours, and must either compare `(lam, ts, dev)` here or fall back to a
/// full rematerialize.
///
/// Returns the two statements in the order they must run.
pub(crate) fn apply_sql(target: &str, table: &Table) -> (String, String) {
    let delete = format!("DELETE FROM {target} WHERE {ID_COLUMN} = ?;");
    let insert = format!(
        "INSERT INTO {target}
         WITH ev AS (SELECT CAST(? AS VARCHAR) AS {ID_COLUMN}, CAST(? AS JSON) AS d)
         SELECT {projection} FROM ev;",
        projection = projection(&table.columns),
    );
    (delete, insert)
}

/// `§4.5` step 3: order by `(lam, ts, dev)` ascending and take the last — which is
/// `row_number() = 1` over the same keys descending.
///
/// `NULLS LAST` is explicit belt to the `WHERE` clause's braces. DuckDB's
/// `default_null_order` already puts NULLs last in both directions, so this changes
/// nothing today; it is here so that a line which is not an envelope cannot win a row if
/// that setting is ever changed underneath us.
const LWW_ORDER: &str = "lam DESC NULLS LAST, ts DESC NULLS LAST, dev DESC NULLS LAST";

/// `§4.5` step 1, plus the two things a file that anyone may append to makes necessary.
fn row_filter(app: &str, table: &str, cutoff: &str) -> String {
    format!(
        "app = '{app}' AND tbl = '{table}' AND {sane}",
        app = escape_literal(app),
        table = escape_literal(table),
        sane = sane_row(cutoff),
    )
}

/// What disqualifies a line from `§4.5` entirely.
///
/// Two clauses, two different reasons.
///
/// The NULL guards drop a line that is not an envelope. `read_json()` with an explicit
/// `columns` list yields NULL for a field it cannot find, so a stray line becomes a row of
/// NULLs rather than an error — and a row with no `lam` has no place in a causal ordering.
///
/// The `ts` clause is `§4.4`. An event more than 24 hours ahead of this node's clock is
/// rejected on ingest, and `read_json()` sees the line whether or not M2's reader folded
/// its `lam` in. Letting it materialize would hand it the row permanently, which is
/// precisely the harm `§4.4` exists to prevent — a rejection that only withholds a counter
/// increment is not a rejection. It mirrors M2's reader including the mercy: a `ts` this
/// node cannot parse carries no information and is **accepted**, because rejecting it
/// would be gap rejection by another name and `§4.1` forbids a reader that.
///
/// No audit row is written from here. M2's `recover()` already reports each rejection
/// once, against the head recorded in `local/state.jsonl`; a second report per
/// materialization would append to `sys_audit` forever.
fn sane_row(cutoff: &str) -> String {
    format!(
        "seq IS NOT NULL AND lam IS NOT NULL AND id IS NOT NULL
         AND (try_cast(ts AS TIMESTAMPTZ) IS NULL
              OR try_cast(ts AS TIMESTAMPTZ) <= TIMESTAMPTZ '{cutoff}')",
        cutoff = escape_literal(cutoff),
    )
}

/// `id`, then one expression per declared column.
///
/// `id` comes from the envelope (`§4.1`), never from `d`. A `d` that happens to carry an
/// `id` key is an unprojected key like any other, which is `§4.5`'s own paragraph: keys no
/// column matches are simply not projected and are still in the log next time.
///
/// One function generates this for both the replay and the incremental apply, which is
/// what makes `docs/plans/phase-1.md §2.5`'s equality structural rather than a coincidence
/// the property test happens to confirm.
fn projection(columns: &[Column]) -> String {
    let mut out = String::from(ID_COLUMN);
    for column in columns {
        let _ = write!(
            out,
            ", {} AS {}",
            extract(column),
            quote_ident(&column.name)
        );
    }
    out
}

/// Read one column out of `d`, typed as `schema.sql` declared it.
///
/// `spec/data-dictionary.md §2.1` encodes `DECIMAL` and `BIGINT` as JSON **strings**
/// precisely so that a parser cannot round them through an IEEE 754 double, so the scalar
/// path goes through `json_extract_string` and casts the text. That also rescues the case
/// where a client wrongly sent a JSON number: `json_extract_string` hands back the number's
/// own text, and `VARCHAR → DECIMAL` parses it exactly, so `"12.34"` and `12.34` land
/// identically rather than one of them quietly losing a cent.
///
/// Structured types cannot survive that — a list is not a string — so `VARCHAR[]` and its
/// relatives go through `json_extract` and cast the JSON value directly.
fn extract(column: &Column) -> String {
    let path = json_path(&column.name);
    if is_structured(&column.ty) {
        format!("CAST(json_extract(d, '{path}') AS {ty})", ty = column.ty)
    } else {
        format!(
            "CAST(json_extract_string(d, '{path}') AS {ty})",
            ty = column.ty
        )
    }
}

/// Whether a DuckDB type needs the JSON value rather than its text.
fn is_structured(ty: &str) -> bool {
    ty.ends_with(']')
        || ty.starts_with("STRUCT")
        || ty.starts_with("MAP")
        || ty.starts_with("UNION")
}

/// `$."name"` — a quoted JSON path, so a column named `a.b` or `x[0]` addresses itself
/// rather than a path expression.
fn json_path(name: &str) -> String {
    format!("$.\"{}\"", name.replace('"', "\\\""))
}

/// A SQL identifier, double-quoted. `spec/data-dictionary.md §5` forbids the reserved
/// words that would otherwise need this, and an app author who ignores that still gets a
/// working table rather than a syntax error.
fn quote_ident(name: &str) -> String {
    format!("\"{}\"", name.replace('"', "\"\""))
}

/// A SQL string literal's body. Table and app names reach here from `app.toml` and from
/// an app's own `schema.sql`, so they are escaped rather than trusted.
fn escape_literal(value: &str) -> String {
    value.replace('\'', "''")
}

// AGENTS.md, Style: unwrap() is permitted in tests. The crate-level deny reaches unit
// tests inside src/, so each one opts out where it is declared.
#[allow(clippy::unwrap_used, clippy::expect_used)]
#[cfg(test)]
mod tests {
    use super::*;

    fn column(name: &str, ty: &str) -> Column {
        Column {
            name: name.to_owned(),
            ty: ty.to_owned(),
            not_null: false,
        }
    }

    /// `§2.1`: a scalar reads its text, a list reads its value. Getting this backwards is
    /// invisible until a `VARCHAR[]` column silently materializes as NULL.
    #[test]
    fn scalars_read_text_and_lists_read_the_json_value() {
        assert!(extract(&column("copay_amount", "DECIMAL(18,2)")).contains("json_extract_string"));
        assert!(extract(&column("n", "BIGINT")).contains("json_extract_string"));
        assert!(extract(&column("ok", "BOOLEAN")).contains("json_extract_string"));

        let tags = extract(&column("tags", "VARCHAR[]"));
        assert!(tags.contains("json_extract(d"), "{tags}");
        assert!(!tags.contains("json_extract_string"), "{tags}");
    }

    /// The replay and the incremental apply must project through the same expressions, or
    /// `docs/plans/phase-1.md §2.5` is a coincidence rather than a property.
    #[test]
    fn both_paths_share_one_projection() {
        let table = Table {
            name: "profile".to_owned(),
            columns: vec![
                column("display_name", "VARCHAR"),
                column("tags", "VARCHAR[]"),
            ],
            checks: Vec::new(),
        };

        let replay = replay_sql(
            "profile",
            "hello",
            &table,
            "/root/log/*.jsonl",
            "2026-09-02T00:00:00.000Z",
        );
        let (_, insert) = apply_sql("profile", &table);

        let shared = projection(&table.columns);
        assert!(
            replay.contains(&shared),
            "replay lost the shared projection"
        );
        assert!(
            insert.contains(&shared),
            "incremental lost the shared projection"
        );
    }

    /// `§4.5` step 1 names both `app` and `tbl`. The plan's sketch declared the `app`
    /// column and never filtered on it.
    #[test]
    fn the_replay_filters_on_app_and_table() {
        let table = Table {
            name: "profile".to_owned(),
            columns: vec![column("display_name", "VARCHAR")],
            checks: Vec::new(),
        };
        let sql = replay_sql(
            "profile",
            "hello",
            &table,
            "/root/log/*.jsonl",
            "2026-09-02T00:00:00.000Z",
        );
        assert!(sql.contains("app = 'hello'"), "{sql}");
        assert!(sql.contains("tbl = 'profile'"), "{sql}");
        assert!(sql.contains("rn = 1 AND op = 'put'"), "{sql}");
    }

    /// `id` is the envelope's. A `d.id` must not be able to reach the row key.
    #[test]
    fn id_is_never_read_from_d() {
        let table = Table {
            name: "t".to_owned(),
            columns: vec![column("display_name", "VARCHAR")],
            checks: Vec::new(),
        };
        let sql = replay_sql(
            "t",
            "hello",
            &table,
            "/root/log/*.jsonl",
            "2026-09-02T00:00:00.000Z",
        );
        assert!(!sql.contains("json_extract_string(d, '$.\"id\"')"), "{sql}");
    }

    /// A quote in a table name or a data root must not end the literal it is inside.
    #[test]
    fn literals_and_identifiers_are_escaped() {
        assert_eq!(escape_literal("it's"), "it''s");
        assert_eq!(quote_ident("od\"d"), "\"od\"\"d\"");
        assert_eq!(json_path("plain"), "$.\"plain\"");
    }
}
