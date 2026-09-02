// Project:  Privatium™  |  File: crates/privatium-core/src/store/materialize.rs
// Authors:  Gabriel Mongefranco (@gabrielmongefranco)
// Created:  2026-09-01  |  Modified: 2026-09-02
// Summary:  spec/protocol.md §4.5 as SQL — the full replay that is the definition, the
//           incremental apply that has to agree with it byte for byte
//           (docs/plans/phase-1.md §2.5), and M4's pieces of §5: staging the envelope once,
//           loading a snapshot tier, applying the log tail, and the checks that decide
//           whether a snapshot applies at all.

use std::collections::BTreeMap;
use std::fmt::Write as _;

use crate::store::schema::{Column, ID_COLUMN, Table};

/// The framework's own schema inside an app's cache database.
///
/// Separate from `main` so nothing here can collide with a table an app declared, and so
/// `spec/data-dictionary.md §5`'s naming rules stay the app author's to obey.
pub(crate) const PV_SCHEMA: &str = "pv";

/// Ids whose winning event is a tombstone (`spec/protocol.md §4.6`).
pub(crate) const TOMBSTONE_TABLE: &str = "pv._tombstone";

/// The staged envelope: every sane event of one app, read from the log **once**.
///
/// A temporary table, so it lives in memory for the length of one restore or one snapshot
/// and never reaches `cache/<slug>.duckdb` on disk. `read_json()` parses the whole log on
/// every statement that names it; staging means the applicability checks of `§5.3`, the
/// tail of every table, the tombstone set and a snapshot's own tables all cost one parse.
pub(crate) const STAGE_TABLE: &str = "_pv_ev";

/// One table's `§4.5` winners while a snapshot is being written. Temporary, like the stage.
pub(crate) const SNAPSHOT_STAGE_TABLE: &str = "_pv_snap";

/// The envelope, as `read_json()` is told to type it (`spec/protocol.md §4.1`).
///
/// Every field is declared, including the two the projection does not select: `§4.5`
/// step 1 filters on `app` **and** `tbl`, so both have to exist as columns. Unknown
/// top-level fields — a `pv/2` peer's `origin`, say — are simply not listed and are
/// ignored, which is `§4.2` holding without anything having to be done about it.
const ENVELOPE_COLUMNS: &str = "{seq:'BIGINT', lam:'BIGINT', ts:'VARCHAR', dev:'VARCHAR', \
                                app:'VARCHAR', op:'VARCHAR', tbl:'VARCHAR', id:'VARCHAR', \
                                d:'JSON'}";

/// Where envelope rows come from.
///
/// `read_json()` refuses a glob that matches no file — "No files found that match the
/// pattern" — and an app whose log directory holds no segment yet is not an error: a
/// schema-less app that has not been written to still has to open, and so does a declared
/// table before its first event. Rather than special-case every statement that reads the
/// log, [`Empty`](Self::Empty) swaps the source for a zero-row relation with the same
/// columns and types, so the filters, the ranking and the projection run unchanged and
/// produce the empty table they would have produced from an empty file.
///
/// [`Table`](Self::Table) is the staged copy ([`STAGE_TABLE`]), which has the same shape.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum Source {
    /// `data/<app>/log/*.jsonl`, through `read_json()`.
    Log(String),
    /// No segment on disk.
    Empty,
    /// A table shaped like the envelope — the stage.
    Table(String),
}

impl Source {
    /// The log, or the empty stand-in when there is no segment to read.
    pub(crate) fn log(glob: &str, has_segments: bool) -> Self {
        if has_segments {
            Self::Log(glob.to_owned())
        } else {
            Self::Empty
        }
    }

    /// The staged envelope.
    pub(crate) fn stage() -> Self {
        Self::Table(STAGE_TABLE.to_owned())
    }

    fn sql(&self) -> String {
        match self {
            Self::Log(glob) => format!(
                "read_json('{glob}', format = 'newline_delimited', columns = {cols})",
                glob = escape_literal(glob),
                cols = ENVELOPE_COLUMNS,
            ),
            Self::Empty => "(SELECT CAST(NULL AS BIGINT) AS seq, CAST(NULL AS BIGINT) AS lam, \
                     CAST(NULL AS VARCHAR) AS ts, CAST(NULL AS VARCHAR) AS dev, \
                     CAST(NULL AS VARCHAR) AS app, CAST(NULL AS VARCHAR) AS op, \
                     CAST(NULL AS VARCHAR) AS tbl, CAST(NULL AS VARCHAR) AS id, \
                     CAST(NULL AS JSON) AS d WHERE false)"
                .to_owned(),
            Self::Table(name) => name.clone(),
        }
    }
}

/// Read the app's sane envelope rows into [`STAGE_TABLE`], once.
///
/// `§4.5` step 1's `app` filter and `§4.4`'s clock hygiene are applied here, so every
/// statement over the stage inherits them. Both are also re-applied by the statements
/// themselves — the filters are idempotent, and keeping them there means the replay reads
/// identically from the log and from the stage.
pub(crate) fn stage_sql(app: &str, source: &Source, cutoff: &str) -> String {
    format!(
        "CREATE OR REPLACE TEMP TABLE {STAGE_TABLE} AS
         SELECT seq, lam, ts, dev, app, op, tbl, id, d
         FROM {source}
         WHERE app = '{app}' AND {sane};",
        source = source.sql(),
        app = escape_literal(app),
        sane = sane_row(cutoff),
    )
}

/// Drop the stage, and a snapshot's per-table stage, if they exist.
pub(crate) fn unstage_sql() -> String {
    format!("DROP TABLE IF EXISTS {STAGE_TABLE}; DROP TABLE IF EXISTS {SNAPSHOT_STAGE_TABLE};")
}

/// The SQL to rebuild one table from the log, per `spec/protocol.md §4.5`.
///
/// `cutoff` is the `§4.4` horizon, passed in rather than read from the clock here so that a
/// replay and an incremental apply compared against each other cannot disagree merely
/// because time passed between them.
pub(crate) fn replay_sql(
    target: &str,
    app: &str,
    table: &Table,
    source: &Source,
    cutoff: &str,
) -> String {
    format!(
        "CREATE OR REPLACE TABLE {target} AS {select};",
        select = replay_select(app, table, source, cutoff),
    )
}

/// `§4.5` steps 1 to 4 for one table, as a `SELECT` — the body of [`replay_sql`], and what a
/// snapshot writes out.
pub(crate) fn replay_select(app: &str, table: &Table, source: &Source, cutoff: &str) -> String {
    format!(
        "WITH ev AS (
           SELECT seq, lam, ts, dev, op, id, d
           FROM {source}
           WHERE {filter}
         ), ranked AS (
           SELECT *, row_number() OVER (PARTITION BY id ORDER BY {order}) AS rn
           FROM ev
         )
         SELECT {projection}
         FROM ranked
         WHERE rn = 1 AND op = 'put'",
        source = source.sql(),
        filter = row_filter(app, &table.name, cutoff),
        order = LWW_ORDER,
        projection = projection(&table.columns),
    )
}

/// The tombstone set, from the same replay (`spec/protocol.md §4.6`).
///
/// The mirror image of [`replay_sql`]: the same winning event per `(tbl, id)`, kept when
/// it is a `del` rather than when it is a `put`. Derived, disposable, and in `cache/` — it
/// is the set M9's data API consults to refuse a client-supplied ULID that has been
/// deleted.
///
/// **Scope is every `tbl` in the app's log, not only the tables `schema.sql` declares.**
/// `spec/data-api.md §2` accepts writes to an app with no `schema.sql` — "`d` is stored
/// as-is" — so a schema-less app's tables exist only as `tbl` values in its log, and
/// `spec/protocol.md §4.6` still requires the data API to refuse a client-supplied `id`
/// that names a tombstoned row there. An earlier version filtered on the declared tables
/// and so answered "not tombstoned" for every table `apps/sketch` has, which left M9
/// nothing to consult. The `app` filter and [`sane_row`] stay: they are `§4.5` step 1 and
/// `§4.4`, not a scope choice.
///
/// **A snapshot never carries this set.** `§5.1`'s artefacts are one Parquet and one CSV
/// per *declared* table, and this set spans undeclared ones too, so a restore from tier 1
/// or 2 rebuilds it from the full log exactly as tier 3 does — over the stage, which is
/// already in memory by then. That is what keeps `docs/plans/phase-1.md §2.5`'s equality
/// true for the tombstones as well as the tables.
pub(crate) fn tombstone_sql(app: &str, source: &Source, cutoff: &str) -> String {
    format!(
        "CREATE OR REPLACE TABLE {TOMBSTONE_TABLE} AS
         WITH ev AS (
           SELECT seq, lam, ts, dev, op, tbl, id
           FROM {source}
           WHERE app = '{app}' AND tbl IS NOT NULL
             AND {sane}
         ), ranked AS (
           SELECT *, row_number() OVER (PARTITION BY tbl, id ORDER BY {order}) AS rn
           FROM ev
         )
         SELECT tbl, id FROM ranked WHERE rn = 1 AND op = 'del';",
        source = source.sql(),
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

// ---------------------------------------------------------------------------------------
// §5 — snapshots and the three-tier read
// ---------------------------------------------------------------------------------------

/// `hi_lam` of the staged log (`spec/protocol.md §5.2`).
///
/// Over the **sane** rows only, like everything else here, so that a `§4.4`-rejected line
/// counts for neither mark — a snapshot describes what it materialized, and it
/// materialized neither that line's `lam` nor its `seq`.
pub(crate) fn hi_lam_sql(source: &Source) -> String {
    format!(
        "SELECT coalesce(max(lam), 0) FROM {source}",
        source = source.sql()
    )
}

/// `hi_seq`: one row per device, `(dev, max(seq))`.
pub(crate) fn hi_seq_sql(source: &Source) -> String {
    format!(
        "SELECT dev, max(seq) FROM {source} WHERE dev IS NOT NULL GROUP BY dev ORDER BY dev",
        source = source.sql()
    )
}

/// `§5.3`'s first applicability condition: the events a snapshot did not see are exactly
/// the events with `lam > hi_lam`.
///
/// Counts the sane rows for which `seq > hi_seq[dev]` and `lam > hi_lam` disagree. A
/// device the manifest never saw has `hi_seq` 0, so all of its rows are "unseen" and every
/// one of them had better be above `hi_lam`. Any row counted here is the `§4.1`
/// cross-device case — an event the snapshot never saw that is not causally after it — and
/// no tier but the replay can place it, because a snapshot row carries no `(lam, ts, dev)`
/// to compare against.
pub(crate) fn non_causal_sql(
    source: &Source,
    hi_lam: u64,
    hi_seq: &BTreeMap<String, u64>,
) -> String {
    format!(
        "SELECT count(*) FROM {source} e LEFT JOIN {heads} h ON e.dev = h.dev
         WHERE (e.seq > coalesce(h.hi_seq, 0)) <> (e.lam > {hi_lam})",
        source = source.sql(),
        heads = heads_relation(hi_seq),
    )
}

/// `§5.3`'s second applicability condition: the log holds everything `hi_seq` claims.
///
/// Returns `(dev, have, claimed)` for every device the snapshot saw further than the log
/// now goes. A snapshot carries no authority (`§5`), so it never resurrects an event the
/// log has lost; a log that moved backwards under a snapshot is replayed as it is.
pub(crate) fn behind_sql(source: &Source, hi_seq: &BTreeMap<String, u64>) -> String {
    format!(
        "SELECT h.dev, coalesce(t.top, 0), h.hi_seq FROM {heads} h
         LEFT JOIN (SELECT dev, max(seq) AS top FROM {source} GROUP BY dev) t ON h.dev = t.dev
         WHERE t.top IS NULL OR t.top < h.hi_seq
         ORDER BY h.dev",
        source = source.sql(),
        heads = heads_relation(hi_seq),
    )
}

/// `hi_seq` as a relation `h(dev, hi_seq)`, or an empty one shaped the same.
fn heads_relation(hi_seq: &BTreeMap<String, u64>) -> String {
    if hi_seq.is_empty() {
        return "(SELECT CAST(NULL AS VARCHAR) AS dev, CAST(NULL AS BIGINT) AS hi_seq \
                 WHERE false)"
            .to_owned();
    }
    let mut rows = String::new();
    for (dev, seq) in hi_seq {
        if !rows.is_empty() {
            rows.push_str(", ");
        }
        let _ = write!(rows, "('{}', {seq})", escape_literal(dev));
    }
    format!("(SELECT * FROM (VALUES {rows}) AS v(dev, hi_seq))")
}

/// The empty table a snapshot tier is loaded into: `id VARCHAR`, then every declared column
/// with its exact type — and **no constraints**, matching what [`replay_sql`]'s
/// `CREATE TABLE AS` produces, so a table restored from a tier is indistinguishable from one
/// replayed. This is also the text of a snapshot's `schema.sql` (`§5.1`, "CREATE TABLE
/// statements with exact types"), which is how tier 2 "creates tables from `schema.sql`"
/// without executing a file: the file is checked equal to this and this is run.
pub(crate) fn create_table_sql(target: &str, table: &Table) -> String {
    format!("CREATE OR REPLACE TABLE {target} ({});", column_defs(table))
}

/// The same statement as `schema.sql` carries it (`§5.1`): plain `CREATE TABLE`, since
/// the file is a description of the tables rather than something run against a cache.
pub(crate) fn declare_table_sql(target: &str, table: &Table) -> String {
    format!("CREATE TABLE {target} ({});", column_defs(table))
}

fn column_defs(table: &Table) -> String {
    let mut columns = format!("{ID_COLUMN} VARCHAR");
    for column in &table.columns {
        let _ = write!(columns, ", {} {}", quote_ident(&column.name), column.ty);
    }
    columns
}

/// Load one table from its Parquet file (`§5.3` tier 1).
///
/// The column list is explicit and cast, so the table's types are the schema's rather than
/// whatever the file's writer chose — the same rule as tier 2, applied where it costs
/// nothing.
pub(crate) fn load_parquet_sql(target: &str, table: &Table, path: &str) -> String {
    format!(
        "INSERT INTO {target} SELECT {columns} FROM read_parquet('{path}');",
        columns = cast_list(table),
        path = escape_literal(path),
    )
}

/// Load one table from its CSV file (`§5.3` tier 2).
///
/// `columns` is the whole of "no CSV type inference": with it given, DuckDB detects
/// nothing, so `header` has to be said too. `allow_quoted_nulls = false` keeps an empty
/// string — which the writer quotes — apart from a NULL, which it leaves bare; the default
/// folds both into NULL, and a tier that did that would not equal the replay.
pub(crate) fn load_csv_sql(target: &str, table: &Table, path: &str) -> String {
    format!(
        "INSERT INTO {target} SELECT {columns}
         FROM read_csv('{path}', header = true, columns = {types}, allow_quoted_nulls = false);",
        columns = cast_list(table),
        path = escape_literal(path),
        types = csv_columns(table),
    )
}

/// `id`, then every declared column, each cast to its declared type.
fn cast_list(table: &Table) -> String {
    let mut out = format!("CAST({ID_COLUMN} AS VARCHAR) AS {ID_COLUMN}");
    for column in &table.columns {
        let name = quote_ident(&column.name);
        let _ = write!(out, ", CAST({name} AS {}) AS {name}", column.ty);
    }
    out
}

/// `read_csv()`'s `columns` struct: `{'id': 'VARCHAR', 'name': 'VARCHAR', ...}`.
fn csv_columns(table: &Table) -> String {
    let mut out = format!("{{'{ID_COLUMN}': 'VARCHAR'");
    for column in &table.columns {
        let _ = write!(
            out,
            ", '{}': '{}'",
            escape_literal(&column.name),
            escape_literal(&column.ty)
        );
    }
    out.push('}');
    out
}

/// The log tail for one table (`§5.3`): every staged event with `lam > hi_lam`.
///
/// Two statements. The first removes every row the tail touches; the second inserts the
/// tail's own `§4.5` winner where that winner is a `put`. Overwriting is correct for the
/// same reason it is in [`apply_sql`]: every tail event is causally after everything the
/// snapshot holds — [`non_causal_sql`] is what guarantees that, and a snapshot for which it
/// does not hold is never loaded.
pub(crate) fn tail_sql(
    target: &str,
    app: &str,
    table: &Table,
    cutoff: &str,
    hi_lam: u64,
) -> (String, String) {
    let filter = row_filter(app, &table.name, cutoff);
    let delete = format!(
        "DELETE FROM {target} WHERE {ID_COLUMN} IN
           (SELECT id FROM {STAGE_TABLE} WHERE {filter} AND lam > {hi_lam});"
    );
    let insert = format!(
        "INSERT INTO {target}
         WITH ev AS (
           SELECT seq, lam, ts, dev, op, id, d
           FROM {STAGE_TABLE}
           WHERE {filter} AND lam > {hi_lam}
         ), ranked AS (
           SELECT *, row_number() OVER (PARTITION BY id ORDER BY {order}) AS rn
           FROM ev
         )
         SELECT {projection}
         FROM ranked
         WHERE rn = 1 AND op = 'put';",
        order = LWW_ORDER,
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
/// One function generates this for the replay, the incremental apply and the tail, which
/// is what makes `docs/plans/phase-1.md §2.5`'s equality structural rather than a
/// coincidence the property test happens to confirm.
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
pub(crate) fn quote_ident(name: &str) -> String {
    format!("\"{}\"", name.replace('"', "\"\""))
}

/// A SQL string literal's body. Table and app names reach here from `app.toml` and from
/// an app's own `schema.sql`, so they are escaped rather than trusted.
pub(crate) fn escape_literal(value: &str) -> String {
    value.replace('\'', "''")
}

// AGENTS.md, Style: unwrap() is permitted in tests. The crate-level deny reaches unit
// tests inside src/, so each one opts out where it is declared.
#[allow(clippy::unwrap_used, clippy::expect_used)]
#[cfg(test)]
mod tests {
    use super::*;

    const CUTOFF: &str = "2026-09-02T00:00:00.000Z";

    fn column(name: &str, ty: &str) -> Column {
        Column {
            name: name.to_owned(),
            ty: ty.to_owned(),
            not_null: false,
        }
    }

    fn profile() -> Table {
        Table {
            name: "profile".to_owned(),
            columns: vec![
                column("display_name", "VARCHAR"),
                column("tags", "VARCHAR[]"),
            ],
            checks: Vec::new(),
        }
    }

    fn log() -> Source {
        Source::log("/root/log/*.jsonl", true)
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

    /// The replay, the incremental apply and the tail must project through the same
    /// expressions, or `docs/plans/phase-1.md §2.5` is a coincidence rather than a property.
    #[test]
    fn all_three_paths_share_one_projection() {
        let table = profile();
        let replay = replay_sql("profile", "hello", &table, &log(), CUTOFF);
        let (_, insert) = apply_sql("profile", &table);
        let (_, tail) = tail_sql("profile", "hello", &table, CUTOFF, 8830);

        let shared = projection(&table.columns);
        assert!(
            replay.contains(&shared),
            "replay lost the shared projection"
        );
        assert!(
            insert.contains(&shared),
            "incremental lost the shared projection"
        );
        assert!(tail.contains(&shared), "tail lost the shared projection");
    }

    /// `§4.5` step 1 names both `app` and `tbl`. The plan's sketch declared the `app`
    /// column and never filtered on it.
    #[test]
    fn the_replay_filters_on_app_and_table() {
        let sql = replay_sql("profile", "hello", &profile(), &log(), CUTOFF);
        assert!(sql.contains("app = 'hello'"), "{sql}");
        assert!(sql.contains("tbl = 'profile'"), "{sql}");
        assert!(sql.contains("rn = 1 AND op = 'put'"), "{sql}");
    }

    /// `spec/data-api.md §2` + `spec/protocol.md §4.6`: the tombstone set covers every
    /// table in the log. A `tbl IN (...)` filter here is the bug this pins.
    #[test]
    fn the_tombstone_set_is_not_scoped_to_declared_tables() {
        let sql = tombstone_sql("sketch", &log(), CUTOFF);
        assert!(sql.contains("app = 'sketch'"), "{sql}");
        assert!(!sql.contains("tbl IN"), "{sql}");
        assert!(sql.contains("PARTITION BY tbl, id"), "{sql}");
        assert!(sql.contains("rn = 1 AND op = 'del'"), "{sql}");
    }

    /// With no segment on disk the log is not read at all, because `read_json()` refuses
    /// a glob that matches nothing; the same statements run over an empty relation.
    #[test]
    fn no_segments_means_the_log_is_not_read() {
        let table = profile();
        let empty = Source::log("/root/log/*.jsonl", false);
        let replay = replay_sql("profile", "hello", &table, &empty, CUTOFF);
        let tombs = tombstone_sql("hello", &empty, CUTOFF);
        let stage = stage_sql("hello", &empty, CUTOFF);
        for sql in [&replay, &tombs, &stage] {
            assert!(!sql.contains("read_json"), "{sql}");
            assert!(sql.contains("WHERE false"), "{sql}");
        }
        assert!(replay.contains(&projection(&table.columns)));
    }

    /// The stage is read where the log would be, with the same filters on top.
    #[test]
    fn the_stage_stands_in_for_the_log() {
        let sql = replay_sql("profile", "hello", &profile(), &Source::stage(), CUTOFF);
        assert!(sql.contains(&format!("FROM {STAGE_TABLE}")), "{sql}");
        assert!(!sql.contains("read_json"), "{sql}");
        assert!(sql.contains("app = 'hello'"), "{sql}");
    }

    /// `§5.3`: the tail is `lam > hi_lam`, applied as delete-then-insert over the stage.
    #[test]
    fn the_tail_is_everything_above_hi_lam() {
        let (delete, insert) = tail_sql("profile", "hello", &profile(), CUTOFF, 8830);
        assert!(delete.contains("lam > 8830"), "{delete}");
        assert!(delete.contains(STAGE_TABLE), "{delete}");
        assert!(insert.contains("lam > 8830"), "{insert}");
        assert!(insert.contains("rn = 1 AND op = 'put'"), "{insert}");
    }

    /// `§5.3`'s applicability checks name `hi_seq` per device and `hi_lam`, and an empty
    /// `hi_seq` is a relation with no rows rather than a syntax error.
    #[test]
    fn the_applicability_checks_carry_the_manifest_marks() {
        let heads = BTreeMap::from([("k7m2q9xf".to_owned(), 1041), ("b3nn8t2q".to_owned(), 87)]);
        let sql = non_causal_sql(&Source::stage(), 8830, &heads);
        assert!(sql.contains("('k7m2q9xf', 1041)"), "{sql}");
        assert!(sql.contains("('b3nn8t2q', 87)"), "{sql}");
        assert!(sql.contains("lam > 8830"), "{sql}");

        let none = behind_sql(&Source::stage(), &BTreeMap::new());
        assert!(none.contains("WHERE false"), "{none}");
    }

    /// The table a tier loads into has `id` first, the declared types, and no constraints —
    /// it has to be indistinguishable from what `CREATE TABLE AS` produced.
    #[test]
    fn a_tier_table_is_typed_and_unconstrained() {
        let sql = create_table_sql("\"profile\"", &profile());
        assert_eq!(
            sql,
            "CREATE OR REPLACE TABLE \"profile\" (id VARCHAR, \"display_name\" VARCHAR, \"tags\" VARCHAR[]);"
        );
        let csv = load_csv_sql("\"profile\"", &profile(), "/snap/profile.csv");
        assert!(csv.contains("header = true"), "{csv}");
        assert!(csv.contains("'tags': 'VARCHAR[]'"), "{csv}");
        assert!(csv.contains("allow_quoted_nulls = false"), "{csv}");
        assert!(!csv.contains("auto"), "{csv}");
    }

    /// `id` is the envelope's. A `d.id` must not be able to reach the row key.
    #[test]
    fn id_is_never_read_from_d() {
        let sql = replay_sql("t", "hello", &profile(), &log(), CUTOFF);
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
