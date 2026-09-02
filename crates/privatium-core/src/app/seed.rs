// Project:  Privatium™  |  File: crates/privatium-core/src/app/seed.rs
// Authors:  Gabriel Mongefranco (@gabrielmongefranco)
// Created:  2026-09-02  |  Modified: 2026-09-02
// Summary:  sample/seed.jsonl (spec/app-contract.md §9) — synthetic events an owner may
//           load into an empty app. Parsed here; appended by Node::load_seed through the
//           app's own log as this node's events, never copied as another device's file.

use serde_json::Value;
use thiserror::Error;

use crate::log::Op;

/// Where a seed lives, relative to the app folder.
pub const SEED_PATH: &str = "sample/seed.jsonl";

/// One seed line, reduced to what this node re-emits.
///
/// `seq`, `lam`, `ts`, `dev` and `app` are discarded on purpose: the seed was written on
/// some other machine at some other time, and appending those values would make this
/// node's log claim to be that device's (`AGENTS.md` 2). The writer mints all five. Keys
/// inside `d` survive, because `d` is appended as the value that was parsed.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SeedEvent {
    /// `put` or `del`.
    pub op: Op,
    /// The table.
    pub tbl: String,
    /// The row key. Trusted, as any server-side caller's is (`spec/protocol.md §4.1`).
    pub id: String,
    /// The row, present exactly when `op` is `put`.
    pub d: Option<Value>,
}

/// A seed line that is not an event.
#[derive(Debug, Error)]
#[error("line {line}: {problem}")]
pub struct SeedError {
    /// 1-based line number.
    pub line: usize,
    /// What is wrong with it.
    pub problem: String,
}

/// Parse a whole seed file. Blank lines are skipped; anything else must be an envelope
/// with `op`, `tbl`, `id`, and `d` present iff `op` is `put` (`spec/protocol.md §4.1`).
pub fn parse(text: &str) -> Result<Vec<SeedEvent>, SeedError> {
    let mut events = Vec::new();
    for (index, raw) in text.lines().enumerate() {
        let line = index + 1;
        if raw.trim().is_empty() {
            continue;
        }
        let bad = |problem: String| SeedError { line, problem };
        let value: Value = serde_json::from_str(raw).map_err(|e| bad(e.to_string()))?;
        let Some(object) = value.as_object() else {
            return Err(bad("not a JSON object".into()));
        };
        let string = |key: &str| -> Result<String, SeedError> {
            match object.get(key).and_then(Value::as_str) {
                Some(s) if !s.is_empty() => Ok(s.to_owned()),
                _ => Err(bad(format!("`{key}` must be a non-empty string"))),
            }
        };
        let op = match string("op")?.as_str() {
            "put" => Op::Put,
            "del" => Op::Del,
            other => return Err(bad(format!("`op` must be put or del, found {other:?}"))),
        };
        let tbl = string("tbl")?;
        let id = string("id")?;
        let d = match (op, object.get("d")) {
            (Op::Put, Some(d)) if d.is_object() => Some(d.clone()),
            (Op::Put, Some(_)) => return Err(bad("`d` must be an object".into())),
            (Op::Put, None) => return Err(bad("a put needs `d`".into())),
            (Op::Del, Some(_)) => return Err(bad("a del carries no `d`".into())),
            (Op::Del, None) => None,
        };
        events.push(SeedEvent { op, tbl, id, d });
    }
    Ok(events)
}

// AGENTS.md, Style: unwrap() is permitted in tests. The crate-level deny reaches unit
// tests inside src/, so each one opts out where it is declared.
#[allow(clippy::unwrap_used, clippy::expect_used)]
#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn envelope_fields_are_discarded_and_d_survives() {
        let text = r#"{"seq":9,"lam":9,"ts":"2020-01-01T00:00:00.000Z","dev":"zzzzzzzz","app":"other","op":"put","tbl":"profile","id":"p1","d":{"display_name":"Sample","extra":[1]}}

{"op":"del","tbl":"profile","id":"p1"}
"#;
        let events = parse(text).unwrap();
        assert_eq!(events.len(), 2);
        assert_eq!(events[0].op, Op::Put);
        assert_eq!(
            events[0].d.as_ref().unwrap()["extra"],
            serde_json::json!([1])
        );
        assert_eq!(events[1].op, Op::Del);
        assert!(events[1].d.is_none());
    }

    #[test]
    fn a_bad_line_is_named_by_number() {
        for (text, needle) in [
            ("{\"op\":\"put\",\"tbl\":\"t\",\"id\":\"x\"}", "needs `d`"),
            (
                "{\"op\":\"del\",\"tbl\":\"t\",\"id\":\"x\",\"d\":{}}",
                "no `d`",
            ),
            ("{\"op\":\"up\",\"tbl\":\"t\",\"id\":\"x\"}", "put or del"),
            (
                "{\"op\":\"put\",\"tbl\":\"\",\"id\":\"x\",\"d\":{}}",
                "`tbl`",
            ),
            (
                "{\"op\":\"put\",\"tbl\":\"t\",\"id\":\"x\",\"d\":1}",
                "object",
            ),
            ("[]", "object"),
            ("not json", "expected"),
        ] {
            let error = parse(&format!("\n{text}\n")).unwrap_err();
            assert_eq!(error.line, 2, "{text}");
            assert!(error.to_string().contains(needle), "{text}: {error}");
        }
    }
}
