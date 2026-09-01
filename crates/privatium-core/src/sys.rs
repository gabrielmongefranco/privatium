// Project:  Privatium™  |  File: crates/privatium-core/src/sys.rs
// Authors:  Gabriel Mongefranco (@gabrielmongefranco)
// Created:  2026-09-01  |  Modified: 2026-09-01
// Summary:  The framework's own tables (spec/data-dictionary.md §3), written through the
//           same event log apps use. M1 writes two rows: this node's sys_device entry and
//           its sys_node singleton.

use serde::Serialize;

/// `_sys` — a reserved slug (`spec/protocol.md §1.1`), materialized into the DuckDB
/// schema `sys`.
///
/// The app loader skips it exactly as it skips the lint fixture corpus: `_sys` is not
/// discoverable, not mountable, and not lintable (`docs/plans/phase-1.md §2.6`).
pub const SLUG: &str = "_sys";

/// `sys_device` (`spec/data-dictionary.md §3.2`).
pub const DEVICE: &str = "sys_device";

/// `sys_node` (`spec/data-dictionary.md §3.1`).
pub const NODE: &str = "sys_node";

/// `sys_audit` (`spec/data-dictionary.md §3.10`).
pub const AUDIT: &str = "sys_audit";

/// An event was refused on ingest. `spec/protocol.md §4.4` requires the rejection to be
/// recorded, and this is the `kind` `§3.10` already reserves for it.
pub const KIND_EVENT_REJECTED: &str = "event.rejected";

/// This node's own clock appears to have moved backwards (`§4.4`, second sentence).
pub const KIND_CLOCK_SKEW: &str = "clock.skew";

/// `§3.10` allows a device ID or the literal `system`. Neither of these is a device's doing.
const ACTOR_SYSTEM: &str = "system";

/// `§3.10`'s three severities. Only `key.mismatch`, `node.admitted`, `cluster.rotated`, and
/// `restore.tier3` MUST be `alert`; neither kind written here is one of them, and inflating
/// a warning into an alert would train the owner to ignore alerts.
const SEVERITY_WARN: &str = "warn";

/// The `d` of a `sys_device` row.
///
/// Every column of `§3.2` appears here, including the ones Phase 1 leaves NULL, so that
/// the shape of the row is auditable against the data dictionary in one place. A NULL is
/// written by omitting the key, which `spec/data-dictionary.md §2.1` defines as equivalent
/// to `null`.
///
/// `id` is not a field: it is the envelope's `id` (`spec/protocol.md §4.1`), and for this
/// table it is the device's Node ID rather than a ULID.
#[derive(Debug, Serialize)]
pub(crate) struct DeviceRow<'a> {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub label: Option<&'a str>,
    pub kind: &'a str,
    pub replica: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub ed25519_pub: Option<&'a str>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub x25519_pub: Option<&'a str>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub paired_at: Option<&'a str>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub paired_via: Option<&'a str>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub last_seen_at: Option<&'a str>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub user_agent: Option<&'a str>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub revoked_at: Option<&'a str>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub revoked_reason: Option<&'a str>,
}

impl DeviceRow<'_> {
    /// This node's own row.
    ///
    /// `spec/data-dictionary.md §3.1` states that every node appears in `sys_device` with
    /// `kind = 'node'`, including itself, and `spec/protocol.md §1` makes a node a replica
    /// always. There is no console device and no second keypair: the node is the device
    /// (`docs/plans/phase-1.md §2.2`).
    ///
    /// Everything else is NULL, and deliberately. `paired_at`, `paired_via`,
    /// `ed25519_pub`, `x25519_pub`, and `user_agent` all describe a pairing, and this node
    /// paired with nobody — `lan | iroh | onion | tunnel` are four wrong answers rather
    /// than four candidates, and no X25519 key exists in Phase 1 at all. `label` is
    /// owner-set and there is no surface to set it on yet. `last_seen_at` is written by
    /// the request path (`§3.2`: at most hourly, never per request), which is M6; a value
    /// stamped here would be permanently stale, because this row is written once.
    pub(crate) fn this_node() -> Self {
        Self {
            label: None,
            kind: "node",
            replica: true,
            ed25519_pub: None,
            x25519_pub: None,
            paired_at: None,
            paired_via: None,
            last_seen_at: None,
            user_agent: None,
            revoked_at: None,
            revoked_reason: None,
        }
    }
}

/// The `d` of the `sys_node` singleton (`spec/data-dictionary.md §3.1`).
///
/// As with [`DeviceRow`], `id` is the envelope's and is this node's ID.
#[derive(Debug, Serialize)]
pub(crate) struct NodeRow<'a> {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub display_name: Option<&'a str>,
    pub pubkey: &'a str,
    pub created_at: &'a str,
    pub protocol: &'a str,
    pub build: &'a str,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub cluster_id: Option<&'a str>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub cert: Option<&'a str>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub cert_expires_at: Option<&'a str>,
}

impl<'a> NodeRow<'a> {
    /// This installation's row.
    ///
    /// `protocol` is `pv/1` — the protocol implemented. The `(partial: phase 1)`
    /// qualifier `spec/cli.md §1` requires belongs to the `--version` string, which is a
    /// claim about conformance rather than about which wire format this speaks.
    ///
    /// `build` is `custom`, one of the three values `§3.1` allows. A locally compiled
    /// binary cannot assert `official`, and there is nothing here that could tell it
    /// apart from one.
    ///
    /// `display_name` is owner-set and used as the mDNS instance name; there is no owner
    /// input surface until M6 and no discovery until Phase 2, so it stays NULL rather
    /// than becoming a hostname nobody chose. `cluster_id`, `cert`, and `cert_expires_at`
    /// are NULL because Phase 1 has no cluster identity — `identity/cluster.*` and
    /// `identity/node.cert` are absent and `sys_cluster` has zero rows
    /// (`docs/plans/phase-1.md §1`).
    pub(crate) fn this_installation(pubkey: &'a str, created_at: &'a str) -> Self {
        Self {
            display_name: None,
            pubkey,
            created_at,
            protocol: crate::PROTOCOL,
            build: "custom",
            cluster_id: None,
            cert: None,
            cert_expires_at: None,
        }
    }
}

/// The `d` of a `sys_audit` row (`spec/data-dictionary.md §3.10`).
///
/// One trap worth naming, because it is invisible until something reads the table: `detail`
/// is typed `VARCHAR` and described as "JSON object", and `spec/data-dictionary.md §2.1`
/// encodes `VARCHAR` as a **string**. So `detail` is a JSON string that *contains* JSON —
/// `"detail":"{\"dev\":\"…\"}"` — not a nested object. `§3.9`'s `row_counts` is the same
/// shape. Emitting a real object here would type the column as `JSON` in M3 and diverge
/// from the dictionary on the one table whose job is to be trustworthy.
///
/// `at` is this row's own timestamp and will differ from the envelope's `ts` by whatever
/// time passes between building the row and appending it — under a millisecond. They are
/// two different facts (`§3.10`'s column, `§4.1`'s envelope field), and collapsing them
/// would mean handing the writer a caller-supplied `ts` again.
#[derive(Debug, Serialize)]
pub(crate) struct AuditRow<'a> {
    pub at: &'a str,
    pub kind: &'a str,
    pub actor: &'a str,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub subject: Option<&'a str>,
    pub detail: &'a str,
    pub severity: &'a str,
}

impl<'a> AuditRow<'a> {
    /// A `warn` from the framework itself, about `subject`.
    pub(crate) fn warn(
        at: &'a str,
        kind: &'a str,
        subject: Option<&'a str>,
        detail: &'a str,
    ) -> Self {
        Self {
            at,
            kind,
            actor: ACTOR_SYSTEM,
            subject,
            detail,
            severity: SEVERITY_WARN,
        }
    }
}

// AGENTS.md, Style: unwrap() is permitted in tests. The crate-level deny reaches unit
// tests inside src/, so each one opts out where it is declared.
#[allow(clippy::unwrap_used, clippy::expect_used)]
#[cfg(test)]
mod tests {
    use super::*;

    /// A NULL column is an absent key, not `"key": null`. Both are legal
    /// (`spec/data-dictionary.md §2.1`); this pins which one is emitted, because the log
    /// line is what a person greps.
    #[test]
    fn null_columns_are_omitted_rather_than_written_as_null() {
        let json = serde_json::to_string(&DeviceRow::this_node()).unwrap();
        assert_eq!(json, r#"{"kind":"node","replica":true}"#);
    }

    #[test]
    fn the_node_row_carries_only_what_phase_1_knows() {
        let json = serde_json::to_string(&NodeRow::this_installation(
            "QUJD",
            "2026-09-01T00:00:00.000Z",
        ))
        .unwrap();
        assert_eq!(
            json,
            r#"{"pubkey":"QUJD","created_at":"2026-09-01T00:00:00.000Z","protocol":"pv/1","build":"custom"}"#
        );
    }
}
