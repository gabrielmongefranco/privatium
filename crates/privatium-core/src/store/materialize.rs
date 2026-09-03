// Project:  Privatium™  |  File: crates/privatium-core/src/store/materialize.rs
// Authors:  Gabriel Mongefranco (@gabrielmongefranco)
// Created:  2026-09-01  |  Modified: 2026-09-03
// Summary:  spec/protocol.md §4.5 in Rust over the staged log — the full replay that is the
//           definition, the incremental apply that has to agree with it byte for byte
//           (docs/plans/phase-1.md §2.5), the log tail a restore applies over a snapshot
//           (§5.3), and the one projection from a JSON `d` to typed columns
//           (spec/data-dictionary.md §2.1) that all three share.

use std::collections::BTreeMap;
use std::fmt::Write as _;

use rusqlite::Connection;
use rusqlite::types::Value;
use serde_json::value::RawValue;

use crate::store::StoreError;
use crate::store::decimal::Decimal;
use crate::store::events::{Event, Op, winners};
use crate::store::schema::{Column, ID_COLUMN, Kind, Table};

/// Ids whose winning event is a tombstone (`spec/protocol.md §4.6`).
///
/// Derived, disposable, and in `cache/` — the set M9's data API consults to refuse a
/// client-supplied ULID that has been deleted. Prefixed `pv_` so it cannot collide with a
/// table an app declared under `spec/data-dictionary.md §5`'s naming rules.
pub(crate) const TOMBSTONE_TABLE: &str = "pv_tombstone";

/// `CREATE TABLE` for one declared table: `id` first, then every column with the storage
/// its kind wants — and no constraint but the key, so a table loaded from a snapshot tier
/// is indistinguishable from one replayed. This is also the text of a snapshot's
/// `schema.sql` (`§5.1`), which is how tier 2 "creates tables from `schema.sql`" without
/// executing a file: the file is checked equal to this rendering and this is run.
pub(crate) fn create_table_sql(table: &Table) -> String {
    let mut out = format!(
        "CREATE TABLE {} ({ID_COLUMN} TEXT PRIMARY KEY",
        quote_ident(&table.name)
    );
    for column in &table.columns {
        let _ = write!(
            out,
            ", {} {} /* {} */",
            quote_ident(&column.name),
            column.kind.storage(),
            column.ty.replace("*/", "* /")
        );
    }
    out.push_str(");");
    out
}

/// Drop and recreate every declared table, empty.
///
/// Every write here names `main.` explicitly. While a snapshot file is attached for a tier
/// 1 load, an unqualified name that is absent from `main` — which is every name in a
/// freshly created cache — resolves to the attached database, and the first thing this
/// would do is try to drop the snapshot's own table.
pub(crate) fn create_tables(conn: &Connection, tables: &[Table]) -> Result<(), StoreError> {
    for table in tables {
        conn.execute_batch(&format!(
            "DROP TABLE IF EXISTS main.{};\n{}",
            quote_ident(&table.name),
            create_table_sql(table)
        ))
        .map_err(StoreError::Sql)?;
    }
    Ok(())
}

/// `INSERT` for one table, `id` then the declared columns, all bound.
pub(crate) fn insert_sql(table: &Table) -> String {
    let mut columns = String::from(ID_COLUMN);
    let mut marks = String::from("?");
    for column in &table.columns {
        let _ = write!(columns, ", {}", quote_ident(&column.name));
        marks.push_str(", ?");
    }
    format!(
        "INSERT INTO main.{} ({columns}) VALUES ({marks})",
        quote_ident(&table.name)
    )
}

/// `§4.5` steps 1 to 4 for every declared table over `events`, into freshly created
/// tables. The definition; everything else must agree with it.
pub(crate) fn replay(
    conn: &Connection,
    tables: &[Table],
    events: &[Event],
) -> Result<(), StoreError> {
    let winners = winners(events);
    create_tables(conn, tables)?;
    for table in tables {
        let mut insert = conn.prepare(&insert_sql(table)).map_err(StoreError::Sql)?;
        for ((tbl, id), event) in &winners {
            if *tbl != table.name || event.op != Op::Put {
                continue;
            }
            let row = project(table, id, event.d.as_deref());
            insert
                .execute(rusqlite::params_from_iter(row))
                .map_err(StoreError::Sql)?;
        }
    }
    Ok(())
}

/// The tombstone set, from the same ranking (`spec/protocol.md §4.6`): every `(tbl, id)`
/// whose winner is a `del`.
///
/// **Scope is every `tbl` in the app's log, not only the tables `schema.sql` declares.**
/// `spec/data-api.md §2` accepts writes to an app with no `schema.sql`, so a schema-less
/// app's tables exist only as `tbl` values in its log, and `§4.6` still requires the data
/// API to refuse a client-supplied `id` that names a tombstoned row there.
///
/// **A snapshot never carries this set.** `§5.1`'s artefacts are one file per *declared*
/// table, and this set spans undeclared ones too, so a restore from tier 1 or 2 rebuilds it
/// from the whole log exactly as tier 3 does — which is what keeps `docs/plans/phase-1.md
/// §2.5`'s equality true for the tombstones as well as the tables.
pub(crate) fn rebuild_tombstones(conn: &Connection, events: &[Event]) -> Result<(), StoreError> {
    conn.execute_batch(&format!(
        "DROP TABLE IF EXISTS main.{TOMBSTONE_TABLE};
         CREATE TABLE {TOMBSTONE_TABLE} (tbl TEXT NOT NULL, id TEXT NOT NULL, PRIMARY KEY (tbl, id));"
    ))
    .map_err(StoreError::Sql)?;
    let mut insert = conn
        .prepare(&format!(
            "INSERT INTO main.{TOMBSTONE_TABLE} (tbl, id) VALUES (?, ?)"
        ))
        .map_err(StoreError::Sql)?;
    for ((tbl, id), event) in winners(events) {
        if event.op == Op::Del {
            insert
                .execute(rusqlite::params![tbl, id])
                .map_err(StoreError::Sql)?;
        }
    }
    Ok(())
}

/// The log tail over loaded tables (`§5.3`): every event with `lam > hi_lam`, applied as
/// delete-then-insert of each table's `§4.5` winner among the tail.
///
/// Overwriting is correct for the same reason it is in [`apply`]: every tail event is
/// causally after everything the snapshot holds — the applicability checks in `restore`
/// guarantee that, and a snapshot for which they do not hold is never loaded.
pub(crate) fn apply_tail(
    conn: &Connection,
    tables: &[Table],
    events: &[Event],
    hi_lam: u64,
) -> Result<(), StoreError> {
    let tail: Vec<Event> = events.iter().filter(|e| e.lam > hi_lam).cloned().collect();
    let winners = winners(&tail);
    for table in tables {
        let mut delete = conn
            .prepare(&format!(
                "DELETE FROM main.{} WHERE {ID_COLUMN} = ?",
                quote_ident(&table.name)
            ))
            .map_err(StoreError::Sql)?;
        let mut insert = conn.prepare(&insert_sql(table)).map_err(StoreError::Sql)?;
        for ((tbl, id), event) in &winners {
            if *tbl != table.name {
                continue;
            }
            delete
                .execute(rusqlite::params![id])
                .map_err(StoreError::Sql)?;
            if event.op == Op::Put {
                insert
                    .execute(rusqlite::params_from_iter(project(
                        table,
                        id,
                        event.d.as_deref(),
                    )))
                    .map_err(StoreError::Sql)?;
            }
        }
    }
    Ok(())
}

/// Apply one event this node just wrote, without re-reading the log
/// (`docs/plans/phase-1.md §2.3`).
///
/// **This is a `DELETE` followed by an `INSERT`, and that is not a violation of
/// `AGENTS.md` invariant 3.** Invariant 3 governs **log files**, which are never touched
/// here; `cache/<slug>.sqlite` is a derived artefact that `spec/protocol.md §3.1` says may
/// be deleted entirely without losing anything. Do not "fix" this into an append.
///
/// **Why it is correct to overwrite blindly.** The event was written by this node a moment
/// ago, so `§4.3` gives it `max(lam_local, lam_max_seen) + 1` — the highest `lam` in the
/// app — and `§4.5` therefore makes it the winner for its `id` no matter what else the log
/// holds. That reasoning is the whole licence for skipping the replay, and it expires the
/// moment a second writer exists: Phase 3's sync receiver ingests events whose `lam` may
/// be lower than ours, and must either compare `(lam, ts, dev)` here or fall back to a
/// full rematerialize.
pub(crate) fn apply(
    conn: &Connection,
    table: Option<&Table>,
    tbl: &str,
    id: &str,
    d: Option<&str>,
) -> Result<(), StoreError> {
    // The tombstone set first, and for **every** table, declared or not — it mirrors
    // `rebuild_tombstones`, whose scope is every `tbl` in the log. Delete before insert,
    // so a second `del` on the same id does not add a second row: the replay produces one
    // row per *currently* tombstoned id, and `docs/plans/phase-1.md §2.5` requires this
    // path to match it.
    conn.execute(
        &format!("DELETE FROM main.{TOMBSTONE_TABLE} WHERE tbl = ? AND id = ?"),
        rusqlite::params![tbl, id],
    )
    .map_err(StoreError::Sql)?;
    if d.is_none() {
        conn.execute(
            &format!("INSERT INTO main.{TOMBSTONE_TABLE} (tbl, id) VALUES (?, ?)"),
            rusqlite::params![tbl, id],
        )
        .map_err(StoreError::Sql)?;
    }

    // The table itself, only if `schema.sql` declares one. An event for a table it does
    // not is ordinary: a schema-less app has none at all (`spec/app-contract.md §5.3`).
    let Some(table) = table else {
        return Ok(());
    };
    conn.execute(
        &format!(
            "DELETE FROM main.{} WHERE {ID_COLUMN} = ?",
            quote_ident(&table.name)
        ),
        rusqlite::params![id],
    )
    .map_err(StoreError::Sql)?;
    if d.is_some() {
        conn.execute(
            &insert_sql(table),
            rusqlite::params_from_iter(project(table, id, d)),
        )
        .map_err(StoreError::Sql)?;
    }
    Ok(())
}

// ---------------------------------------------------------------------------------------
// The projection (spec/data-dictionary.md §2.1)
// ---------------------------------------------------------------------------------------

/// The row for `id` from `d`: `id`, then one typed value per declared column.
///
/// `id` comes from the envelope (`§4.1`), never from `d`. A `d` that happens to carry an
/// `id` key is an unprojected key like any other, which is `§4.5`'s own paragraph: keys no
/// column matches are simply not projected and are still in the log next time. A `d` that
/// is not a JSON object projects as all NULLs.
///
/// One function serves the replay, the incremental apply and the tail, which is what
/// makes `docs/plans/phase-1.md §2.5`'s equality structural rather than a coincidence.
pub(crate) fn project(table: &Table, id: &str, d: Option<&str>) -> Vec<Value> {
    let fields: BTreeMap<String, Box<RawValue>> = d
        .and_then(|d| serde_json::from_str(d).ok())
        .unwrap_or_default();
    let mut row = Vec::with_capacity(table.columns.len() + 1);
    row.push(Value::Text(id.to_owned()));
    for column in &table.columns {
        row.push(typed(column, fields.get(&column.name).map(|raw| raw.get())));
    }
    row
}

/// One JSON value, as its raw text, typed by the column it lands in (`§2.1`).
///
/// A value that does not parse as its declared type materializes as NULL rather than
/// failing the replay: the log line cannot be corrected (`§3.1`), so a rejection here would
/// stop the app forever over one hand-typed field. The data API validates before it
/// appends, which is where a wrong value is refused.
pub(crate) fn typed(column: &Column, raw: Option<&str>) -> Value {
    let Some(raw) = raw.map(str::trim) else {
        return Value::Null;
    };
    if raw == "null" {
        return Value::Null;
    }
    let scalar = Scalar::of(raw);
    match column.kind {
        Kind::Json => Value::Text(raw.to_owned()),
        Kind::Text => match scalar {
            Scalar::Str(s) => Value::Text(s),
            Scalar::Bool(b) => Value::Text(b.to_string()),
            Scalar::Number(n) => Value::Text(n.to_owned()),
            Scalar::Structured => Value::Text(raw.to_owned()),
        },
        Kind::Integer => match scalar {
            Scalar::Str(s) => s.trim().parse::<i64>().map_or(Value::Null, Value::Integer),
            Scalar::Number(n) => n.parse::<i64>().map_or(Value::Null, Value::Integer),
            Scalar::Bool(b) => Value::Integer(i64::from(b)),
            Scalar::Structured => Value::Null,
        },
        Kind::Decimal { scale } => {
            let text = match scalar {
                Scalar::Str(s) => s,
                Scalar::Number(n) => n.to_owned(),
                _ => return Value::Null,
            };
            Decimal::parse(&text).map_or(Value::Null, |d| {
                Value::Text(d.with_scale(scale).to_string())
            })
        }
        Kind::Boolean => match scalar {
            Scalar::Bool(b) => Value::Integer(i64::from(b)),
            Scalar::Str(s) => match s.trim().to_ascii_lowercase().as_str() {
                "true" | "1" => Value::Integer(1),
                "false" | "0" => Value::Integer(0),
                _ => Value::Null,
            },
            Scalar::Number(n) => match n {
                "1" => Value::Integer(1),
                "0" => Value::Integer(0),
                _ => Value::Null,
            },
            Scalar::Structured => Value::Null,
        },
    }
}

/// A text value read back from a CSV file, typed the same way (`§5.3`, tier 2: no
/// inference — the schema types it).
pub(crate) fn typed_text(column: &Column, text: Option<&str>) -> Value {
    let Some(text) = text else {
        return Value::Null;
    };
    match column.kind {
        Kind::Json | Kind::Text => Value::Text(text.to_owned()),
        Kind::Integer => text
            .trim()
            .parse::<i64>()
            .map_or(Value::Null, Value::Integer),
        Kind::Decimal { scale } => Decimal::parse(text).map_or(Value::Null, |d| {
            Value::Text(d.with_scale(scale).to_string())
        }),
        Kind::Boolean => match text.trim().to_ascii_lowercase().as_str() {
            "true" | "1" => Value::Integer(1),
            "false" | "0" => Value::Integer(0),
            _ => Value::Null,
        },
    }
}

/// What a raw JSON value is, by its first character — which is all `§2.1` needs.
enum Scalar<'a> {
    Str(String),
    Bool(bool),
    /// The number's own digits, untouched: `12.34` stays `12.34`, `9007199254740993`
    /// stays itself, and neither goes through a double.
    Number(&'a str),
    Structured,
}

impl<'a> Scalar<'a> {
    fn of(raw: &'a str) -> Self {
        match raw.as_bytes().first() {
            Some(b'"') => serde_json::from_str::<String>(raw).map_or(Self::Structured, Self::Str),
            Some(b't') => Self::Bool(true),
            Some(b'f') => Self::Bool(false),
            Some(b'[' | b'{') => Self::Structured,
            _ => Self::Number(raw),
        }
    }
}

/// A SQL identifier, double-quoted. `spec/data-dictionary.md §5` forbids the reserved
/// words that would otherwise need this, and an app author who ignores that still gets a
/// working table rather than a syntax error.
pub(crate) fn quote_ident(name: &str) -> String {
    format!("\"{}\"", name.replace('"', "\"\""))
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
            kind: Kind::of(ty),
            not_null: false,
        }
    }

    fn thing() -> Table {
        Table {
            name: "thing".to_owned(),
            columns: vec![
                column("name", "VARCHAR"),
                column("copay_amount", "DECIMAL(18,2)"),
                column("count", "BIGINT"),
                column("ok", "BOOLEAN"),
                column("tags", "VARCHAR[]"),
            ],
        }
    }

    /// `§2.1`, one assertion per row of the encoding table — and the rescue of a number a
    /// client wrongly sent as a number, which must land exactly and not through a double.
    #[test]
    fn the_projection_types_each_column_from_its_declaration() {
        let row = project(
            &thing(),
            "x",
            Some(
                r#"{"name":"Gabriel","copay_amount":"12.34","count":"9007199254740993","ok":true,"tags":["a","b"],"id":"evil","extra":1}"#,
            ),
        );
        assert_eq!(row[0], Value::Text("x".into()), "id is the envelope's");
        assert_eq!(row[1], Value::Text("Gabriel".into()));
        assert_eq!(row[2], Value::Text("12.34".into()));
        assert_eq!(row[3], Value::Integer(9_007_199_254_740_993));
        assert_eq!(row[4], Value::Integer(1));
        assert_eq!(row[5], Value::Text(r#"["a","b"]"#.into()));

        let numbers = project(
            &thing(),
            "n",
            Some(r#"{"copay_amount":12.34,"count":7,"ok":1}"#),
        );
        assert_eq!(numbers[2], Value::Text("12.34".into()));
        assert_eq!(numbers[3], Value::Integer(7));
        assert_eq!(numbers[4], Value::Integer(1));

        let scaled = project(&thing(), "s", Some(r#"{"copay_amount":"12.3"}"#));
        assert_eq!(scaled[2], Value::Text("12.30".into()), "the declared scale");

        // An omitted key is NULL (§2.1), and so is a value that is not its type.
        let empty = project(&thing(), "e", Some("{}"));
        assert!(empty[1..].iter().all(|v| *v == Value::Null));
        let bad = project(
            &thing(),
            "b",
            Some(r#"{"copay_amount":"abc","count":"x","ok":"maybe"}"#),
        );
        assert!(bad[2..=4].iter().all(|v| *v == Value::Null));
        let none = project(&thing(), "d", None);
        assert!(none[1..].iter().all(|v| *v == Value::Null));
    }

    /// The CSV reader hands text back; the same typing applies.
    #[test]
    fn csv_text_is_typed_the_same_way() {
        let t = thing();
        assert_eq!(
            typed_text(&t.columns[1], Some("12.3")),
            Value::Text("12.30".into())
        );
        assert_eq!(typed_text(&t.columns[2], Some("42")), Value::Integer(42));
        assert_eq!(typed_text(&t.columns[3], Some("true")), Value::Integer(1));
        assert_eq!(typed_text(&t.columns[3], Some("0")), Value::Integer(0));
        assert_eq!(
            typed_text(&t.columns[4], Some("[]")),
            Value::Text("[]".into())
        );
        assert_eq!(typed_text(&t.columns[0], None), Value::Null);
    }

    /// The table a tier loads into has `id` first, storage types, the declared type as a
    /// comment, and no constraint but the key.
    #[test]
    fn the_cache_table_declares_storage_not_affinity() {
        assert_eq!(
            create_table_sql(&thing()),
            "CREATE TABLE \"thing\" (id TEXT PRIMARY KEY, \"name\" TEXT /* VARCHAR */, \
             \"copay_amount\" TEXT COLLATE decimal /* DECIMAL(18,2) */, \"count\" INTEGER /* BIGINT */, \
             \"ok\" INTEGER /* BOOLEAN */, \"tags\" TEXT /* VARCHAR[] */);"
        );
        assert_eq!(
            insert_sql(&thing()),
            "INSERT INTO main.\"thing\" (id, \"name\", \"copay_amount\", \"count\", \"ok\", \"tags\") \
             VALUES (?, ?, ?, ?, ?, ?)"
        );
    }

    #[test]
    fn identifiers_are_quoted() {
        assert_eq!(quote_ident("od\"d"), "\"od\"\"d\"");
    }
}
