// Project:  Privatium™  |  File: crates/privatium-core/src/sys.rs
// Authors:  Gabriel Mongefranco (@gabrielmongefranco)
// Created:  2026-09-01  |  Modified: 2026-09-03
// Summary:  The framework's own tables (spec/data-dictionary.md §3), written through the
//           same event log apps use. M1 writes two rows: this node's sys_device entry and
//           its sys_node singleton. M2 adds sys_audit; M4 adds sys_snapshot and the
//           snapshot and restore audit kinds; M5 adds sys_app and the app.* kinds.

use serde::Serialize;

/// `_sys` — a reserved slug (`spec/protocol.md §1.1`), materialized into `cache/_sys.sqlite`
/// (`spec/data-dictionary.md §1`).
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

/// `sys_snapshot` (`spec/data-dictionary.md §3.9`) — the replicated index of what is in
/// `data/<slug>/snap/`.
pub const SNAPSHOT: &str = "sys_snapshot";

/// `sys_setting` (`spec/data-dictionary.md §3.6`).
pub const SETTING: &str = "sys_setting";

/// `sys_app` (`spec/data-dictionary.md §3.4`) — the app index. One row per app folder
/// the node knows about, keyed by slug.
pub const APP: &str = "sys_app";

/// `sys_app.last_error` for a row whose folder is gone (`§3.4`, rules). The row and the
/// app's data stay.
pub const FOLDER_MISSING: &str = "folder missing";

/// An app folder was refused (`spec/app-contract.md §3.1`: "MUST record `app.load_failed`").
pub const KIND_APP_LOAD_FAILED: &str = "app.load_failed";

/// An app loaded cleanly for the first time on this index.
pub const KIND_APP_INSTALLED: &str = "app.installed";

/// An event was refused on ingest. `spec/protocol.md §4.4` requires the rejection to be
/// recorded, and this is the `kind` `§3.10` already reserves for it.
pub const KIND_EVENT_REJECTED: &str = "event.rejected";

/// This node's own clock appears to have moved backwards (`§4.4`, second sentence).
pub const KIND_CLOCK_SKEW: &str = "clock.skew";

/// A snapshot was written (`spec/protocol.md §5`).
pub const KIND_SNAPSHOT_CREATED: &str = "snapshot.created";

/// A snapshot was deleted by retention (`§5.4`).
pub const KIND_SNAPSHOT_PRUNED: &str = "snapshot.pruned";

/// A restore fell back to CSV (`§5.3`, tier 2).
pub const KIND_RESTORE_TIER2: &str = "restore.tier2";

/// A restore fell through to the full replay (`§5.3`, tier 3). `§3.10` makes this an
/// `alert` that MUST surface in the UI.
pub const KIND_RESTORE_TIER3: &str = "restore.tier3";

/// `§3.10` allows a device ID or the literal `system`. Nothing written here is a device's
/// doing.
const ACTOR_SYSTEM: &str = "system";

/// `§3.10`'s three severities. Only `key.mismatch`, `node.admitted`, `cluster.rotated`, and
/// `restore.tier3` MUST be `alert`; inflating anything else would train the owner to ignore
/// alerts.
const SEVERITY_INFO: &str = "info";
const SEVERITY_WARN: &str = "warn";
const SEVERITY_ALERT: &str = "alert";

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
        Self::system(at, kind, subject, detail, SEVERITY_WARN)
    }

    /// An `info` from the framework itself, about `subject`.
    pub(crate) fn info(
        at: &'a str,
        kind: &'a str,
        subject: Option<&'a str>,
        detail: &'a str,
    ) -> Self {
        Self::system(at, kind, subject, detail, SEVERITY_INFO)
    }

    /// An `alert` from the framework itself. `§3.10` reserves this for four kinds;
    /// `restore.tier3` is the one Phase 1 can produce.
    pub(crate) fn alert(
        at: &'a str,
        kind: &'a str,
        subject: Option<&'a str>,
        detail: &'a str,
    ) -> Self {
        Self::system(at, kind, subject, detail, SEVERITY_ALERT)
    }

    fn system(
        at: &'a str,
        kind: &'a str,
        subject: Option<&'a str>,
        detail: &'a str,
        severity: &'a str,
    ) -> Self {
        Self {
            at,
            kind,
            actor: ACTOR_SYSTEM,
            subject,
            detail,
            severity,
        }
    }
}

/// The `d` of a `sys_snapshot` row (`spec/data-dictionary.md §3.9`).
///
/// `id` is the envelope's and is the snapshot id — a caller-chosen key (`§4.1`), so a
/// later `put` with `verified_at` amends the row and a `del` on prune removes it, both
/// blessed by `§4.6`. `hi_lam` and `bytes` are `BIGINT` and therefore JSON **strings**
/// (`§2.1`); `row_counts` is `VARCHAR` holding JSON, like [`AuditRow::detail`].
#[derive(Debug, Serialize)]
pub(crate) struct SnapshotRow<'a> {
    pub app_id: &'a str,
    pub created_at: &'a str,
    pub hi_lam: String,
    pub row_counts: &'a str,
    pub bytes: String,
    pub created_by: &'a str,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub verified_at: Option<&'a str>,
}

impl<'a> SnapshotRow<'a> {
    /// The row for a snapshot as written, or as re-asserted after a verification.
    pub(crate) fn new(
        snapshot: &'a crate::store::Snapshot,
        row_counts: &'a str,
        created_by: &'a str,
        verified_at: Option<&'a str>,
    ) -> Self {
        Self {
            app_id: &snapshot.manifest.app,
            created_at: &snapshot.manifest.created,
            hi_lam: snapshot.manifest.hi_lam.to_string(),
            row_counts,
            bytes: snapshot.bytes.to_string(),
            created_by,
            verified_at,
        }
    }
}

/// The `d` of a `sys_app` row (`spec/data-dictionary.md §3.4`). `id` is the envelope's
/// and is the slug — a caller-chosen key (`spec/protocol.md §4.1`), so every load that
/// changes something is a `put` amending one row.
///
/// Every column, in the dictionary's order, owned rather than borrowed because the loader
/// reads the current row back out of the cache and compares it with the one it would
/// write. `api` and `nav_order` are `INTEGER`, a type `§2.1`'s table does not list; they
/// fit a double exactly and cross as JSON numbers. A NULL is an omitted key.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize)]
pub struct AppRow {
    /// `title`.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub title: Option<String>,
    /// `version`.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub version: Option<String>,
    /// `api`.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub api: Option<i32>,
    /// `tier`.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub tier: Option<String>,
    /// `icon`.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub icon: Option<String>,
    /// `source`: `local` or `bundled`.
    pub source: String,
    /// `enabled`. The owner's; carried forward across loads.
    pub enabled: bool,
    /// `nav_order`.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub nav_order: Option<i32>,
    /// `installed_at`: when the app first loaded cleanly. NULL while it never has.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub installed_at: Option<String>,
    /// `updated_at`: when this row was written.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub updated_at: Option<String>,
    /// `schema_hash`: SHA-256 of `schema.sql`.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub schema_hash: Option<String>,
    /// `manifest_hash`: SHA-256 of `app.toml`.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub manifest_hash: Option<String>,
    /// `advertise`.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub advertise: Option<bool>,
    /// `permissions`: JSON text of the non-default permissions.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub permissions: Option<String>,
    /// `last_error`.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub last_error: Option<String>,
}

impl AppRow {
    /// Whether two rows say the same thing about an app, `updated_at` aside — the test
    /// for whether a load has anything to append.
    #[must_use]
    pub fn same_facts(&self, other: &Self) -> bool {
        let a = Self {
            updated_at: None,
            ..self.clone()
        };
        let b = Self {
            updated_at: None,
            ..other.clone()
        };
        a == b
    }
}

// AGENTS.md, Style: unwrap() is permitted in tests. The crate-level deny reaches unit
// tests inside src/, so each one opts out where it is declared.
#[allow(clippy::unwrap_used, clippy::expect_used)]
#[cfg(test)]
mod tests {
    use super::*;

    /// `§3.4`, every column present when known; `§2.1`, NULL as an omitted key.
    #[test]
    fn the_app_row_carries_every_column_and_omits_nulls() {
        let row = AppRow {
            title: Some("Hello".into()),
            version: Some("1.0.0".into()),
            api: Some(1),
            tier: Some("lua".into()),
            icon: None,
            source: "bundled".into(),
            enabled: true,
            nav_order: Some(10),
            installed_at: Some("2026-09-02T00:00:00.000Z".into()),
            updated_at: Some("2026-09-02T00:00:00.000Z".into()),
            schema_hash: Some("s".into()),
            manifest_hash: Some("m".into()),
            advertise: Some(true),
            permissions: Some("{}".into()),
            last_error: None,
        };
        assert_eq!(
            serde_json::to_string(&row).unwrap(),
            r#"{"title":"Hello","version":"1.0.0","api":1,"tier":"lua","source":"bundled","enabled":true,"nav_order":10,"installed_at":"2026-09-02T00:00:00.000Z","updated_at":"2026-09-02T00:00:00.000Z","schema_hash":"s","manifest_hash":"m","advertise":true,"permissions":"{}"}"#
        );
        let later = AppRow {
            updated_at: Some("2026-09-03T00:00:00.000Z".into()),
            ..row.clone()
        };
        assert!(row.same_facts(&later));
        let changed = AppRow {
            last_error: Some("x".into()),
            ..row.clone()
        };
        assert!(!row.same_facts(&changed));
    }

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

    /// `§2.1`: `BIGINT` crosses as a string, and `row_counts` is a string holding JSON.
    #[test]
    fn the_snapshot_row_encodes_bigints_as_strings() {
        let snapshot = crate::store::Snapshot {
            id: "2026-W35-k7m2q9xf-8830".parse().unwrap(),
            dir: std::path::PathBuf::new(),
            manifest: crate::store::Manifest {
                v: 1,
                snapshot_id: "2026-W35-k7m2q9xf-8830".into(),
                app: "hello".into(),
                created: "2026-08-30T03:00:00.000Z".into(),
                hi_lam: 8830,
                hi_seq: Default::default(),
                engine: "sqlite 3.53.2".into(),
                tables: Vec::new(),
            },
            bytes: 4096,
        };
        let json =
            serde_json::to_string(&SnapshotRow::new(&snapshot, "{}", "k7m2q9xf", None)).unwrap();
        assert_eq!(
            json,
            r#"{"app_id":"hello","created_at":"2026-08-30T03:00:00.000Z","hi_lam":"8830","row_counts":"{}","bytes":"4096","created_by":"k7m2q9xf"}"#
        );
    }
}
