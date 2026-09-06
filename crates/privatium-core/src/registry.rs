// Project:  Privatium™  |  File: crates/privatium-core/src/registry.rs
// Authors:  Gabriel Mongefranco (@gabrielmongefranco)
// Created:  2026-09-06  |  Modified: 2026-09-06
// Summary:  What the owner edits in the public registry (spec/data-dictionary.md §3.1,
//           §3.2): the node's display name, a device's label, its revocation, and the
//           hourly last_seen_at mark. Every change is a put of the whole current row
//           read from the log, so owner-set and unknown fields survive (spec/protocol.md
//           §4.2), and a revocation is never a del.

use std::collections::BTreeMap;

use serde_json::value::{RawValue, to_raw_value};

use crate::store::events::{Op, read_log, winners};
use crate::{Error, Node, Result, StoreError, log, new_ulid, store, sys};

/// The most characters a display name may hold. The mDNS instance name is cut to 63
/// bytes (`spec/protocol.md §6.1`); a name this long already reads badly on a phone.
pub const DISPLAY_NAME_MAX: usize = 63;

/// The most characters a device label may hold — the same bound the pairing message
/// applies to the label a device suggests (`spec/protocol.md §7.4.2`).
pub const LABEL_MAX: usize = 80;

/// How long a session may run before its next request writes `last_seen_at` again
/// (`spec/data-dictionary.md §3.2`): at most hourly, never per request.
pub const SEEN_INTERVAL: jiff::SignedDuration = jiff::SignedDuration::from_secs(60 * 60);

/// The owner revoked a device (`spec/data-dictionary.md §3.10`).
pub const KIND_DEVICE_REVOKED: &str = "device.revoked";

/// The owner changed a setting the registry carries — the display name
/// (`spec/data-dictionary.md §3.10`).
pub const KIND_CONFIG_CHANGED: &str = "config.changed";

/// A row's `d` as the log holds it: every key, known or not, as raw JSON text.
type Row = BTreeMap<String, Box<RawValue>>;

/// Owner-typed text cut to what a column may hold: control characters dropped,
/// whitespace trimmed, at most `max` characters. `None` when nothing is left.
#[must_use]
pub fn clean_text(value: &str, max: usize) -> Option<String> {
    let kept: String = value
        .chars()
        .filter(|c| !c.is_control())
        .take(max)
        .collect();
    let kept = kept.trim();
    (!kept.is_empty()).then(|| kept.to_owned())
}

impl Node {
    /// Set `sys_node.display_name` — the mDNS instance name and the manifest's `name`
    /// (`spec/protocol.md §6.1`, `§9.2`). Empty unsets it, so the Node ID stands in
    /// again. Longer than [`DISPLAY_NAME_MAX`] characters is refused. Writes
    /// `config.changed` naming the key, never the name, and hands the running discovery
    /// mechanisms the new facts.
    pub fn set_display_name(&mut self, name: &str) -> Result<()> {
        let trimmed = name.trim();
        if trimmed.chars().count() > DISPLAY_NAME_MAX {
            return Err(Error::InvalidText {
                field: "display name",
                problem: format!("at most {DISPLAY_NAME_MAX} characters"),
            });
        }
        let name = clean_text(trimmed, DISPLAY_NAME_MAX);
        let id = self.id().as_str().to_owned();
        let changed = self.amend_sys_row(sys::NODE, &id, |row| {
            match &name {
                Some(name) => {
                    row.insert("display_name".into(), to_raw_value(name)?);
                }
                None => {
                    row.remove("display_name");
                }
            }
            Ok(())
        })?;
        if changed {
            let detail = serde_json::to_string(&serde_json::json!({
                "key": "sys_node.display_name",
            }))?;
            self.audit(KIND_CONFIG_CHANGED, false, None, &detail)?;
            self.refresh()?;
            self.publish_facts()?;
        }
        Ok(())
    }

    /// Set a paired device's `label` (`spec/data-dictionary.md §3.2`). Empty clears it.
    /// This node's own row and an unknown device are refused.
    pub fn label_device(&mut self, device: &str, label: &str) -> Result<()> {
        if label.trim().chars().count() > LABEL_MAX {
            return Err(Error::InvalidText {
                field: "label",
                problem: format!("at most {LABEL_MAX} characters"),
            });
        }
        self.require_other_device(device)?;
        let label = clean_text(label, LABEL_MAX);
        if self.amend_sys_row(sys::DEVICE, device, |row| {
            match &label {
                Some(label) => {
                    row.insert("label".into(), to_raw_value(label)?);
                }
                None => {
                    row.remove("label");
                }
            }
            Ok(())
        })? {
            self.refresh()?;
        }
        Ok(())
    }

    /// Revoke a device now (`spec/data-dictionary.md §3.2`): a `put` with `revoked_at`
    /// and `revoked_reason` set on the row as it stands, never a `del`, so the record of
    /// what was paired survives; then `device.revoked`. The device's next request is
    /// refused (`spec/protocol.md §8.3`); the channel closes its open socket. This
    /// node's own row and an unknown device are refused; revoking twice is a no-op.
    pub fn revoke_device(
        &mut self,
        device: &str,
        reason: Option<&str>,
        now: jiff::Timestamp,
    ) -> Result<()> {
        self.require_other_device(device)?;
        let at = log::format_ts(now);
        let reason = reason.and_then(|reason| clean_text(reason, LABEL_MAX));
        let changed = self.amend_sys_row(sys::DEVICE, device, |row| {
            if row.contains_key("revoked_at") {
                return Ok(());
            }
            row.insert("revoked_at".into(), to_raw_value(&at)?);
            match &reason {
                Some(reason) => {
                    row.insert("revoked_reason".into(), to_raw_value(reason)?);
                }
                None => {
                    row.remove("revoked_reason");
                }
            }
            Ok(())
        })?;
        if changed {
            let detail = serde_json::to_string(&serde_json::json!({ "by": "owner" }))?;
            self.audit(KIND_DEVICE_REVOKED, false, Some(device), &detail)?;
            self.refresh()?;
        }
        Ok(())
    }

    /// Mark a device seen at `now` — at the channel handshake and on a request — writing
    /// `last_seen_at` only when the row holds none or one more than [`SEEN_INTERVAL`]
    /// old (`spec/data-dictionary.md §3.2`): at most hourly, never per request, and
    /// never per page navigation, each of which opens a channel of its own. Returns
    /// whether a row was written. An unknown or revoked device writes nothing.
    pub fn note_device_seen(&mut self, device: &str, now: jiff::Timestamp) -> Result<bool> {
        let sql = format!(
            "SELECT last_seen_at FROM {} WHERE id = ? AND revoked_at IS NULL",
            sys::DEVICE
        );
        let last: Option<String> =
            match self
                .store()
                .conn()
                .query_row(&sql, rusqlite::params![device], |row| {
                    row.get::<_, Option<String>>(0)
                }) {
                Ok(last) => last,
                Err(rusqlite::Error::QueryReturnedNoRows) => return Ok(false),
                Err(error) => return Err(Error::Store(Box::new(StoreError::Sql(error)))),
            };
        let due = match last.and_then(|text| text.parse::<jiff::Timestamp>().ok()) {
            Some(last) => now.duration_since(last) >= SEEN_INTERVAL,
            None => true,
        };
        if !due {
            return Ok(false);
        }
        let at = log::format_ts(now);
        let written = self.amend_sys_row(sys::DEVICE, device, |row| {
            row.insert("last_seen_at".into(), to_raw_value(&at)?);
            Ok(())
        })?;
        if written {
            self.refresh()?;
        }
        Ok(written)
    }

    /// The number of paired nodes other than this one — active `sys_device` rows with
    /// `kind = 'node'` (`spec/lua-api.md §3.4`, `spec/data-api.md §4`). Zero until a
    /// second node is admitted.
    pub fn paired_node_count(&self) -> Result<u64> {
        let sql = "SELECT count(*) FROM v_device_active WHERE kind = 'node' AND id <> ?";
        self.store()
            .conn()
            .query_row(sql, rusqlite::params![self.id().as_str()], |row| {
                row.get::<_, i64>(0)
            })
            .map(|count| count.try_into().unwrap_or(0))
            .map_err(|error| Error::Store(Box::new(StoreError::Sql(error))))
    }

    /// Whether any device but this node has ever paired — a `sys_device` row other than
    /// its own, revoked or not. The first-run rule of `spec/protocol.md §7.1` opens a
    /// pairing window on `--open` only while this is false.
    pub fn has_paired_device(&self) -> Result<bool> {
        let sql = format!("SELECT count(*) FROM {} WHERE id <> ?", sys::DEVICE);
        self.store()
            .conn()
            .query_row(&sql, rusqlite::params![self.id().as_str()], |row| {
                row.get::<_, i64>(0)
            })
            .map(|count| count > 0)
            .map_err(|error| Error::Store(Box::new(StoreError::Sql(error))))
    }

    /// The winning `d` of `table`/`id` as the log holds it, every key kept.
    fn sys_row(&self, table: &str, id: &str) -> Result<Option<Row>> {
        let events = read_log(self.sys_log().log_dir(), sys::SLUG, &store::cutoff_now())
            .map_err(|error| Error::Store(Box::new(error)))?;
        winners(&events)
            .get(&(table, id))
            .filter(|event| event.op == Op::Put)
            .and_then(|event| event.d.as_deref())
            .map(serde_json::from_str)
            .transpose()
            .map_err(|_| Error::IdentityRow)
    }

    /// Put the row as it stands with `edit` applied, or nothing when `edit` changed
    /// nothing. Returns whether an event was appended. A row the log does not hold is
    /// [`Error::DeviceUnknown`] for `sys_device` and [`Error::IdentityRow`] otherwise.
    fn amend_sys_row(
        &mut self,
        table: &str,
        id: &str,
        edit: impl FnOnce(&mut Row) -> Result<()>,
    ) -> Result<bool> {
        let Some(current) = self.sys_row(table, id)? else {
            return Err(if table == sys::DEVICE {
                Error::DeviceUnknown {
                    device: id.to_owned(),
                }
            } else {
                Error::IdentityRow
            });
        };
        let mut next = current.clone();
        edit(&mut next)?;
        let same = current.len() == next.len()
            && next
                .iter()
                .all(|(key, value)| current.get(key).is_some_and(|old| old.get() == value.get()));
        if same {
            return Ok(false);
        }
        self.sys_log_mut().put(table, id, &next)?;
        Ok(true)
    }

    /// A device row that is not this node's own, or the refusal.
    fn require_other_device(&self, device: &str) -> Result<()> {
        if device == self.id().as_str() {
            return Err(Error::OwnDevice);
        }
        if self.sys_row(sys::DEVICE, device)?.is_none() {
            return Err(Error::DeviceUnknown {
                device: device.to_owned(),
            });
        }
        Ok(())
    }

    fn audit(&mut self, kind: &str, warn: bool, subject: Option<&str>, detail: &str) -> Result<()> {
        let at = log::now();
        let row = if warn {
            sys::AuditRow::warn(&at, kind, subject, detail)
        } else {
            sys::AuditRow::info(&at, kind, subject, detail)
        };
        self.sys_log_mut().put(sys::AUDIT, &new_ulid(), &row)?;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn owner_text_is_cleaned_and_bounded() {
        assert_eq!(
            clean_text("  Pixel\u{7} 9 \n", 80).as_deref(),
            Some("Pixel 9")
        );
        assert_eq!(clean_text("   ", 80), None);
        assert_eq!(clean_text("", 80), None);
        assert_eq!(clean_text("abcdef", 3).as_deref(), Some("abc"));
    }
}
