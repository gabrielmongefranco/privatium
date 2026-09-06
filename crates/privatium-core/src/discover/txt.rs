// Project:  Privatium™  |  File: crates/privatium-core/src/discover/txt.rs
// Authors:  Gabriel Mongefranco (@gabrielmongefranco)
// Created:  2026-09-06  |  Modified: 2026-09-06
// Summary:  The TXT record of spec/protocol.md §6.1 — the eight keys in the table's
//           order, built from the node's facts, with `apps` truncated to `,…` so the
//           whole record stays under 1300 bytes — and the reading of one back into a
//           Discovered.

use std::collections::BTreeMap;
use std::net::IpAddr;

use super::{Discovered, Facts};

/// The budget the whole record SHOULD stay under so it fits one packet (`§6.1`).
pub const BUDGET: usize = 1300;

/// The keys, in the order `§6.1` lists them.
pub const KEYS: [&str; 8] = ["v", "id", "cl", "nm", "apps", "build", "pair", "p"];

/// The marker a truncated `apps` value ends with.
pub const ELLIPSIS: &str = ",…";

/// The record at `now`: `(key, value)` pairs in [`KEYS`] order, never over [`BUDGET`]
/// bytes as DNS encodes them.
#[must_use]
pub fn record(facts: &Facts, now: jiff::Timestamp) -> Vec<(String, String)> {
    let fixed = [
        ("v", "1".to_owned()),
        ("id", facts.id.clone()),
        ("cl", facts.cluster.clone()),
        ("nm", facts.instance_name()),
        ("build", facts.build.clone()),
        ("pair", if facts.pair(now) { "1" } else { "0" }.to_owned()),
        ("p", facts.port.to_string()),
    ];
    let fixed_bytes: usize = fixed.iter().map(|(k, v)| encoded_len(k, v)).sum();
    let room = BUDGET.saturating_sub(fixed_bytes + encoded_len("apps", ""));
    let apps = fit_apps(&facts.apps, room);

    let mut out = Vec::with_capacity(8);
    for key in KEYS {
        let value = match key {
            "apps" => apps.clone(),
            _ => fixed
                .iter()
                .find(|(k, _)| *k == key)
                .map(|(_, v)| v.clone())
                .unwrap_or_default(),
        };
        out.push((key.to_owned(), value));
    }
    out
}

/// The bytes one `key=value` string costs on the wire: a length byte, the key, `=`, the
/// value (RFC 6763 §6.1).
#[must_use]
pub fn encoded_len(key: &str, value: &str) -> usize {
    1 + key.len() + 1 + value.len()
}

/// The DNS-encoded size of a whole record.
#[must_use]
pub fn encoded_size(record: &[(String, String)]) -> usize {
    record.iter().map(|(k, v)| encoded_len(k, v)).sum()
}

/// `apps` joined with commas, cut to `room` bytes: whole slugs only, from the front, and
/// `,…` appended when anything was dropped (`§6.1`).
#[must_use]
pub fn fit_apps(apps: &[String], room: usize) -> String {
    let full = apps.join(",");
    if full.len() <= room {
        return full;
    }
    let mut kept = String::new();
    for slug in apps {
        let candidate_len = if kept.is_empty() {
            slug.len()
        } else {
            kept.len() + 1 + slug.len()
        };
        if candidate_len + ELLIPSIS.len() > room {
            break;
        }
        if !kept.is_empty() {
            kept.push(',');
        }
        kept.push_str(slug);
    }
    kept.push_str(ELLIPSIS);
    kept
}

/// Read a record back — from mDNS or from a UDP answer — into a [`Discovered`] at
/// `addrs`. `None` when it is not a `pv/1` record: no `id`, or a `v` this build does not
/// speak. A missing `p` falls back to `port`, the SRV port an mDNS resolution carries.
#[must_use]
pub fn read(
    txt: &BTreeMap<String, String>,
    addrs: Vec<IpAddr>,
    port: u16,
    instance: &str,
    now: jiff::Timestamp,
) -> Option<Discovered> {
    if txt.get("v").map(String::as_str) != Some("1") {
        return None;
    }
    let id = txt.get("id").filter(|id| !id.is_empty())?.clone();
    let port = txt
        .get("p")
        .and_then(|p| p.parse::<u16>().ok())
        .unwrap_or(port);
    let apps = txt
        .get("apps")
        .map(|apps| {
            apps.split(',')
                .filter(|slug| !slug.is_empty())
                .map(str::to_owned)
                .collect()
        })
        .unwrap_or_default();
    let mut addrs = addrs;
    addrs.sort();
    addrs.dedup();
    Some(Discovered {
        id,
        cluster: txt.get("cl").cloned().unwrap_or_default(),
        name: txt.get("nm").cloned().unwrap_or_default(),
        addrs,
        port,
        apps,
        pair: txt.get("pair").map(String::as_str) == Some("1"),
        seen_at: now,
        instance: instance.to_owned(),
    })
}
