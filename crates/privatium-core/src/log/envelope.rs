// Project:  Privatium™  |  File: crates/privatium-core/src/log/envelope.rs
// Authors:  Gabriel Mongefranco (@gabrielmongefranco)
// Created:  2026-09-01  |  Modified: 2026-09-05
// Summary:  The two halves of spec/protocol.md §4.1 — the struct a writer serializes, and
//           the much smaller struct a reader deserializes. They are separate types on
//           purpose: §4.2 makes preservation a property of the bytes, so nothing here ever
//           reads a line and writes it back out.

use serde::{Deserialize, Serialize};

/// `op` (`spec/protocol.md §4.1`). There are two, and `pv/1` will not grow a third.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Op {
    /// The row is `d`.
    Put,
    /// A tombstone. Permanent, never garbage collected, and the `id` is never reused
    /// (`§4.6`).
    Del,
}

impl Op {
    /// The wire value, for error messages and audit details.
    #[must_use]
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Put => "put",
            Self::Del => "del",
        }
    }
}

/// One event, serialized in the key order of `spec/protocol.md §4.1`.
///
/// The order is what serde emits for a struct's fields, so the declaration order below is
/// load-bearing: §4.1 says readers MUST NOT depend on key order, and also that writers
/// SHOULD emit this one, because a human grepping a log file is the point.
///
/// `d` is `Option` because §4.1 requires it to be **absent** — not null, not `{}` — when
/// `op` is `del`. The type cannot express "present if and only if `op` is `put`", so that
/// pairing is enforced at the two construction sites in `writer.rs`, each with a
/// `debug_assert` naming it.
///
/// `batch` is the marker of §4.1's batch rule: the first line of a batch of `n ≥ 2`
/// events carries `"batch": n`, and no other line carries the key at all — a single
/// event, a tombstone on its own, and a line appended by hand are batches of one and say
/// nothing. It stands before `d` so `d` stays last on the line for a person grepping.
#[derive(Debug, Serialize)]
pub(crate) struct Envelope<'a, D: Serialize> {
    pub seq: u64,
    pub lam: u64,
    pub ts: &'a str,
    pub dev: &'a str,
    pub app: &'a str,
    pub op: Op,
    pub tbl: &'a str,
    pub id: &'a str,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub batch: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub d: Option<&'a D>,
}

/// The fields recovery needs from a line already on disk, and no others.
///
/// **Deliberately not `deny_unknown_fields`.** `config.rs` uses it and is right to; this is
/// the exact inverse case. `§4.2` requires a reader to accept unknown top-level fields, and
/// a node that refused them could not read a log written by a `pv/2` peer — which is the
/// whole mechanism forward compatibility rests on.
///
/// `app`, `tbl`, `id`, and `d` are absent because nothing in M2 needs them. M3 materializes
/// by reading the log files itself (`store::events`), so parsing them here would
/// be the beginning of a second materializer that no one asked for.
///
/// `Cow` rather than `&str`: serde_json can only borrow a string that contains no escapes,
/// and `"dev"` is a legal spelling of `dev`. Borrowing where possible and allocating
/// where not keeps an odd-but-legal line readable instead of turning it into a parse error.
#[derive(Debug, Deserialize)]
pub(crate) struct Meta<'a> {
    pub seq: u64,
    pub lam: u64,
    #[serde(borrow)]
    pub ts: std::borrow::Cow<'a, str>,
    #[serde(borrow)]
    pub dev: std::borrow::Cow<'a, str>,
    /// The batch marker (`§4.1`), on the first line of a batch of that many events.
    #[serde(default)]
    pub batch: Option<u64>,
}

// AGENTS.md, Style: unwrap() is permitted in tests. The crate-level deny reaches unit
// tests inside src/, so each one opts out where it is declared.
#[allow(clippy::unwrap_used, clippy::expect_used)]
#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_put_carries_d_and_a_del_omits_it() {
        let d = serde_json::json!({ "display_name": "Gabriel" });

        let put = Envelope {
            seq: 1,
            lam: 1,
            ts: "2026-08-28T14:03:11.412Z",
            dev: "k7m2q9xf",
            app: "hello",
            op: Op::Put,
            tbl: "profile",
            id: "01J9YQ2W7C8XKF3M0N5RTVB6ZP",
            batch: None,
            d: Some(&d),
        };
        assert_eq!(
            serde_json::to_string(&put).unwrap(),
            r#"{"seq":1,"lam":1,"ts":"2026-08-28T14:03:11.412Z","dev":"k7m2q9xf","app":"hello","op":"put","tbl":"profile","id":"01J9YQ2W7C8XKF3M0N5RTVB6ZP","d":{"display_name":"Gabriel"}}"#
        );

        let del: Envelope<'_, ()> = Envelope {
            seq: 2,
            lam: 2,
            ts: "2026-08-28T14:03:11.412Z",
            dev: "k7m2q9xf",
            app: "hello",
            op: Op::Del,
            tbl: "profile",
            id: "01J9YQ2W7C8XKF3M0N5RTVB6ZP",
            batch: None,
            d: None,
        };
        let line = serde_json::to_string(&del).unwrap();
        assert!(!line.contains("\"d\""), "{line}");
        assert!(line.contains("\"op\":\"del\""), "{line}");
    }

    /// `§4.1`'s batch marker: on the line, between `id` and `d`, and absent when `None`.
    #[test]
    fn the_batch_marker_stands_before_d() {
        let d = serde_json::json!({});
        let first = Envelope {
            seq: 1,
            lam: 1,
            ts: "2026-08-28T14:03:11.412Z",
            dev: "k7m2q9xf",
            app: "hello",
            op: Op::Put,
            tbl: "t",
            id: "x",
            batch: Some(3),
            d: Some(&d),
        };
        let line = serde_json::to_string(&first).unwrap();
        assert_eq!(
            line,
            r#"{"seq":1,"lam":1,"ts":"2026-08-28T14:03:11.412Z","dev":"k7m2q9xf","app":"hello","op":"put","tbl":"t","id":"x","batch":3,"d":{}}"#
        );
        let meta: Meta<'_> = serde_json::from_str(&line).unwrap();
        assert_eq!(meta.batch, Some(3));
    }

    /// `§4.2`. The line below carries a field `pv/1` has never heard of; parsing it must
    /// succeed, because a reader that rejected it could not talk to a later version.
    #[test]
    fn an_unknown_top_level_field_does_not_stop_the_parse() {
        let line = r#"{"seq":3,"lam":9,"ts":"2026-08-28T14:03:11.412Z","dev":"k7m2q9xf","app":"hello","op":"put","tbl":"t","id":"x","d":{},"origin":"pv/2"}"#;
        let meta: Meta<'_> = serde_json::from_str(line).unwrap();
        assert_eq!(meta.seq, 3);
        assert_eq!(meta.lam, 9);
        assert_eq!(meta.dev, "k7m2q9xf");
    }

    /// An escaped value still parses, which is what `Cow` buys over `&str`.
    ///
    /// serde_json cannot hand out a borrowed `&str` for a value it had to unescape. With
    /// `&str` the line below is a hard parse error; with `Cow` it allocates and carries on.
    #[test]
    fn an_escaped_value_still_parses() {
        // `x` is a legal JSON spelling of `x`. The backslash is built rather than
        // written so that no layer of source escaping can quietly turn it into a plain `x`
        // and leave this test passing for the wrong reason.
        let line = format!(
            r#"{{"seq":1,"lam":1,"ts":"2026-08-28T14:03:11.412Z","dev":"k7m2q9{}u0078f"}}"#,
            '\\'
        );

        let meta: Meta<'_> = serde_json::from_str(&line).unwrap();
        assert_eq!(meta.dev, "k7m2q9xf");
        assert!(
            matches!(meta.dev, std::borrow::Cow::Owned(_)),
            "serde_json had to unescape, so it cannot have handed out a borrow"
        );
    }
}
