// Project:  Privatium™  |  File: crates/privatium-core/src/store/validate.rs
// Authors:  Gabriel Mongefranco (@gabrielmongefranco)
// Created:  2026-09-03  |  Modified: 2026-09-03
// Summary:  NOT NULL and CHECK before an append (spec/data-api.md §2, spec/lua-api.md §3.3).
//           The author's schema.sql runs in a throwaway in-memory database and every put of
//           the batch is inserted there, so the engine judges each row by the constraints
//           exactly as they were written — no second parser, no list of constraint kinds.
//           A violation names the event's index in the batch; nothing has reached the log.

use rusqlite::Connection;

use crate::store::schema::Schema;
use crate::store::{decimal, materialize, params, sandbox};

/// A row the schema's constraints refuse.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Violation {
    /// The event's position in the batch, from 0.
    pub index: usize,
    /// The table.
    pub tbl: String,
    /// What SQLite said — `NOT NULL constraint failed: fill.drug`, `CHECK constraint
    /// failed: …`.
    pub problem: String,
}

/// Check every put in `rows` — `(tbl, id, d)` — against the schema's `NOT NULL` and
/// `CHECK` constraints. A row for a table the schema does not declare is not checked
/// (`spec/app-contract.md §5.3`: stored as it is), and an app with no tables has nothing
/// to check.
///
/// Each put is a delete-then-insert of the row in the check database, so a batch that
/// writes one id twice — an amend of an event earlier in the same batch — is not a key
/// collision. Only the row's own constraints can fail.
pub fn validate<'a>(
    schema: &Schema,
    rows: impl IntoIterator<Item = (&'a str, &'a str, Option<&'a serde_json::Value>)>,
) -> Result<(), Violation> {
    if schema.tables.is_empty() {
        return Ok(());
    }
    let mut checker: Option<Connection> = None;
    for (index, (tbl, id, d)) in rows.into_iter().enumerate() {
        let (Some(table), Some(d)) = (schema.table(tbl), d) else {
            continue;
        };
        let conn = match checker.as_ref() {
            Some(conn) => conn,
            None => checker.insert(open(schema).map_err(|error| Violation {
                index,
                tbl: tbl.to_owned(),
                problem: error.to_string(),
            })?),
        };
        let text = d.to_string();
        let checked = conn
            .execute(
                &format!(
                    "DELETE FROM main.{} WHERE id = ?",
                    materialize::quote_ident(tbl)
                ),
                rusqlite::params![id],
            )
            .and_then(|_| {
                conn.execute(
                    &materialize::insert_sql(table),
                    rusqlite::params_from_iter(materialize::project(table, id, Some(&text))),
                )
            });
        if let Err(error) = checked {
            return Err(Violation {
                index,
                tbl: tbl.to_owned(),
                problem: crate::store::schema::first_line(&error.to_string()),
            });
        }
    }
    Ok(())
}

/// The check database: the author's DDL, in memory, under the same authorizer a
/// `schema.sql` is parsed with.
fn open(schema: &Schema) -> rusqlite::Result<Connection> {
    let conn = Connection::open_in_memory()?;
    decimal::register(&conn)?;
    params::register(&conn)?;
    conn.authorizer(Some(sandbox::authorize_ddl))?;
    conn.execute_batch(&schema.ddl)?;
    Ok(conn)
}

// AGENTS.md, Style: unwrap() is permitted in tests. The crate-level deny reaches unit
// tests inside src/, so each one opts out where it is declared.
#[allow(clippy::unwrap_used, clippy::expect_used)]
#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn schema() -> Schema {
        Schema::parse(
            "CREATE TABLE fill (
                 id VARCHAR PRIMARY KEY,
                 drug VARCHAR NOT NULL,
                 copay_amount DECIMAL(18,2) CHECK (copay_amount >= 0),
                 note VARCHAR
             );
             CREATE VIEW v_all AS SELECT * FROM fill WHERE drug = $drug;",
        )
        .unwrap()
    }

    /// `spec/data-api.md §2`: the offending index, and the engine's own words.
    #[test]
    fn a_violation_names_the_index_and_the_constraint() {
        let ok = json!({ "drug": "Example", "copay_amount": "12.50" });
        let no_drug = json!({ "copay_amount": "1.00" });
        let negative = json!({ "drug": "X", "copay_amount": "-1.00" });
        assert!(validate(&schema(), [("fill", "a", Some(&ok))]).is_ok());

        let refused = validate(
            &schema(),
            [("fill", "a", Some(&ok)), ("fill", "b", Some(&no_drug))],
        )
        .unwrap_err();
        assert_eq!(refused.index, 1);
        assert_eq!(refused.tbl, "fill");
        assert!(refused.problem.contains("NOT NULL"), "{}", refused.problem);
        assert!(refused.problem.contains("fill.drug"), "{}", refused.problem);

        let refused = validate(&schema(), [("fill", "c", Some(&negative))]).unwrap_err();
        assert_eq!(refused.index, 0);
        assert!(refused.problem.contains("CHECK"), "{}", refused.problem);
    }

    /// An amend of a row written earlier in the same batch, a tombstone, and a table the
    /// schema does not declare are all fine; an app with no schema checks nothing.
    #[test]
    fn what_is_not_a_violation() {
        let ok = json!({ "drug": "Example" });
        let loose = json!({ "anything": 1 });
        assert!(
            validate(
                &schema(),
                [
                    ("fill", "a", Some(&ok)),
                    ("fill", "a", Some(&ok)),
                    ("fill", "a", None),
                    ("undeclared", "x", Some(&loose)),
                ]
            )
            .is_ok()
        );
        assert!(validate(&Schema::empty(), [("fill", "a", Some(&loose))]).is_ok());
    }
}
