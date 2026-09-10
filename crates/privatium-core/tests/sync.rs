// This file is part of Privatium
// crates/privatium-core/tests/sync.rs
// Author(s): Gabriel Mongefranco
// Created: 2026-09-07
// Last Modified: 2026-09-07
// Summary: Foreign log integrity, causal ordering, and synchronization contracts in spec/protocol.md
//          §10.2 and §4.3.
// Notes: See README file for documentation and full license information.
//
// Copyright © 2026 Gabriel Mongefranco
//
// This program is free software: you can redistribute it and/or modify
// it under the terms of the GNU General Public License as published by
// the Free Software Foundation, either version 3 of the License, or (at your option) any later version.
//
// This program is distributed in the hope that it will be useful,
// but WITHOUT ANY WARRANTY; without even the implied warranty of
// MERCHANTABILITY or FITNESS FOR A PARTICULAR PURPOSE. See the
// GNU General Public License for more details.
//
// You should have received a copy of the GNU General Public License along
// with this program. If not, see <https://www.gnu.org/licenses/>.

#![allow(clippy::unwrap_used, clippy::expect_used)]

mod common;

use std::fs;

use privatium_core::log::{Durability, foreign::Receiver};
use privatium_core::{Error, Node};
use sha2::{Digest, Sha256};

const APP: &str = "hello";
const DEV: &str = "aaaaaaaa";
const LIMIT: usize = 4096;
const DDL: &str = "CREATE TABLE profile (id TEXT PRIMARY KEY, display_name TEXT);";

/// `spec/lua-api.md §3.4`: an app reacts to what other devices wrote, not only to its
/// own writes, and `pv.device()` names whoever wrote it — "when a fill lands, recompute
/// the refill window" only works if the callback fires for a fill that arrived by sync.
#[tokio::test]
async fn test_spec_lua_3_4_on_append_fires_for_synced_events_with_the_origin_device() {
    let root = tempfile::tempdir().unwrap();
    let mut node = Node::open(root.path()).unwrap();
    common::write_lua_app(
        &node.paths().apps_dir(),
        APP,
        &[
            (
                "schema.sql",
                "CREATE TABLE profile (id TEXT PRIMARY KEY, display_name TEXT); CREATE TABLE origin (id TEXT PRIMARY KEY, device TEXT);",
            ),
            (
                "app.lua",
                "local pv = require 'privatium'\npv.on('append', function(ev) if ev.tbl == 'profile' then pv.append('origin', ev.id, {device=pv.device()}) end end)\n",
            ),
        ],
    );
    let report = node
        .load_apps(&[privatium_core::AppRoot::local(node.paths().apps_dir())])
        .unwrap();
    node.receive(APP, DEV, &event(1)).unwrap();
    let handler = privatium_core::Handler::new(node, report);
    tokio::time::timeout(std::time::Duration::from_secs(5), handler.fire_pending(APP))
        .await
        .unwrap();
    let shared = handler.node();
    let node = shared.lock().unwrap();
    let origin: String = node
        .app(APP)
        .unwrap()
        .store()
        .conn()
        .query_row("SELECT device FROM origin", [], |row| row.get(0))
        .unwrap();
    assert_eq!(origin, DEV);
}

/// `spec/data-api.md §3`: a subscriber sees an event that arrived from another device
/// even though its counter is below the mark the subscriber resumed from. Causal order
/// is not arrival order once more than one device writes, so a filter on `lam` would
/// drop exactly the events sync exists to deliver.
#[tokio::test]
async fn test_spec_data_3_live_sse_delivers_below_the_resume_mark() {
    use futures_util::StreamExt as _;
    let root = tempfile::tempdir().unwrap();
    let mut node = Node::open(root.path()).unwrap();
    common::write_lua_app(&node.paths().apps_dir(), APP, &[("schema.sql", DDL)]);
    let report = node
        .load_apps(&[privatium_core::AppRoot::local(node.paths().apps_dir())])
        .unwrap();
    for _ in 0..5 {
        node.append(APP, privatium_core::Event::del("profile", "own"))
            .unwrap();
    }
    let handler = privatium_core::Handler::new(node, report);
    let response = handler
        .handle(
            axum::http::Request::builder()
                .uri("/a/hello/api/stream?after=5")
                .body(privatium_core::Body::empty())
                .unwrap(),
        )
        .await;
    assert_eq!(response.status(), 200);
    let mut body = response.into_body().into_data_stream();
    handler
        .node()
        .lock()
        .unwrap()
        .receive(APP, DEV, &event(1))
        .unwrap();
    let data = tokio::time::timeout(std::time::Duration::from_secs(2), body.next())
        .await
        .unwrap()
        .unwrap()
        .unwrap();
    assert!(data.starts_with(b"event: append\n"));
    assert!(data.windows(7).any(|w| w == b"\"lam\":1"));
}

/// `spec/protocol.md §10.2`, `§4.1`: a line without its newline is a line the origin
/// has not finished. It survives a restart untouched and reaches the tables only once
/// the origin sends the rest — never truncated, never half-read.
#[test]
fn test_spec_10_2_foreign_torn_tail_survives_restart_and_is_not_materialized() {
    let root = tempfile::tempdir().unwrap();
    let mut node = Node::open(root.path()).unwrap();
    node.open_app(APP, DDL).unwrap();
    let bytes = event(1);
    fs::write(path(&node), &bytes[..bytes.len() - 1]).unwrap();
    drop(node);
    let mut node = Node::open(root.path()).unwrap();
    node.open_app(APP, DDL).unwrap();
    assert_eq!(count(&node, "profile"), 0);
    node.receive(APP, DEV, &bytes).unwrap();
    assert_eq!(count(&node, "profile"), 1);
    assert_eq!(fs::read(path(&node)).unwrap(), bytes);
}

fn count(node: &Node, table: &str) -> i64 {
    node.app(APP)
        .unwrap()
        .store()
        .conn()
        .query_row(&format!("SELECT count(*) FROM {table}"), [], |row| {
            row.get(0)
        })
        .unwrap()
}

fn audit_count(node: &Node, kind: &str) -> i64 {
    node.store()
        .conn()
        .query_row(
            "SELECT count(*) FROM sys_audit WHERE kind = ?",
            [kind],
            |row| row.get(0),
        )
        .unwrap()
}

/// `spec/protocol.md §4.1`, `§10.2`: what a crash left behind is copied to the peer
/// exactly as the origin holds it, and both nodes then skip the same lines for the same
/// reason. The owner hears about it once, not once per drain and once per restart.
#[test]
fn test_spec_10_2_a_short_batch_and_a_non_envelope_line_are_copied_and_skipped_everywhere() {
    let root = tempfile::tempdir().unwrap();
    let mut node = Node::open(root.path()).unwrap();
    node.open_app(APP, DDL).unwrap();
    let header = String::from_utf8(event(1))
        .unwrap()
        .replacen('{', "{\"batch\":3,", 1)
        .into_bytes();
    let bytes = [header, b"not json\n".to_vec(), event(2)].concat();
    node.receive(APP, DEV, &bytes).unwrap();
    assert_eq!(fs::read(path(&node)).unwrap(), bytes);
    assert_eq!(count(&node, "profile"), 0);
    assert_eq!(audit_count(&node, "batch.incomplete"), 1);
    node.refresh_app(APP).unwrap();
    assert_eq!(audit_count(&node, "batch.incomplete"), 1);
    drop(node);
    let mut node = Node::open(root.path()).unwrap();
    node.open_app(APP, DDL).unwrap();
    assert_eq!(count(&node, "profile"), 0);
    assert_eq!(audit_count(&node, "batch.incomplete"), 1);
}

/// `spec/protocol.md §10.2`: a node holds every app of its cluster, including apps it
/// has no folder for. Those logs are still checked and still audited, and receiving one
/// creates no cache and opens no writer of this node's own.
#[test]
fn test_spec_10_2_unmounted_logs_are_audited_without_a_local_writer() {
    let root = tempfile::tempdir().unwrap();
    let mut node = Node::open(root.path()).unwrap();
    let header = String::from_utf8(event(1))
        .unwrap()
        .replacen('{', "{\"batch\":3,", 1)
        .into_bytes();
    let bytes = [header, event(2)].concat();
    node.receive(APP, DEV, &bytes).unwrap();
    assert!(node.app(APP).is_none());
    assert!(
        !node
            .paths()
            .app_log_dir(APP)
            .join(format!("{}.jsonl", node.id()))
            .exists()
    );
    assert_eq!(audit_count(&node, "batch.incomplete"), 1);
    drop(node);
    let mut node = Node::open(root.path()).unwrap();
    node.open_app(APP, DDL).unwrap();
    assert_eq!(audit_count(&node, "batch.incomplete"), 1);
    assert_eq!(count(&node, "profile"), 0);
}

/// `spec/protocol.md §4.4`, `§10.2`: a line stamped in the future is kept as the origin
/// wrote it — the log is a record, not a judgement — while the tables leave it out and
/// the owner is told once. Discarding it would make the two nodes' files differ forever.
#[test]
fn test_spec_4_4_a_future_dated_synced_line_is_stored_skipped_and_audited_once() {
    let root = tempfile::tempdir().unwrap();
    let mut node = Node::open(root.path()).unwrap();
    node.open_app(APP, DDL).unwrap();
    let future = privatium_core::log::format_ts(
        jiff::Timestamp::now()
            .checked_add(jiff::SignedDuration::from_secs(25 * 3600))
            .unwrap(),
    );
    let bytes = String::from_utf8(event(1))
        .unwrap()
        .replace("2026-09-01T00:00:00.000Z", &future)
        .into_bytes();
    node.receive(APP, DEV, &bytes).unwrap();
    assert_eq!(fs::read(path(&node)).unwrap(), bytes);
    assert_eq!(count(&node, "profile"), 0);
    assert_eq!(audit_count(&node, "event.rejected"), 1);
    node.refresh_app(APP).unwrap();
    drop(node);
    let mut node = Node::open(root.path()).unwrap();
    node.open_app(APP, DDL).unwrap();
    assert_eq!(audit_count(&node, "event.rejected"), 1);
    assert_eq!(count(&node, "profile"), 0);
}

/// `spec/protocol.md §4.3`: a received event lifts this node's counter, so a later
/// local write always ranks after what it had already seen. That has to survive a
/// restart, or the node would mint counters a peer has already used.
#[test]
fn test_spec_4_3_lamport_folds_received_events_and_stays_monotonic_across_restart() {
    let root = tempfile::tempdir().unwrap();
    let mut node = Node::open(root.path()).unwrap();
    node.open_app(APP, DDL).unwrap();
    let bytes = String::from_utf8(event(1))
        .unwrap()
        .replace("\"lam\":1", "\"lam\":100")
        .into_bytes();
    node.receive(APP, DEV, &bytes).unwrap();
    let own = node
        .append(
            APP,
            privatium_core::Event::put(
                "profile",
                "own",
                serde_json::json!({"display_name":"local"}),
            ),
        )
        .unwrap();
    assert!(own.lam > 100);
    drop(node);
    let mut node = Node::open(root.path()).unwrap();
    node.open_app(APP, DDL).unwrap();
    assert!(
        node.append(APP, privatium_core::Event::del("profile", "own"))
            .unwrap()
            .lam
            > own.lam
    );
}

/// `spec/data-api.md §3`, `spec/protocol.md §4.1`: an open page learns about another
/// device's writes as they land. The lines of a batch a crash left short are not among
/// them: no reader serves those to an app.
#[test]
fn test_spec_data_3_stream_carries_synced_events() {
    let root = tempfile::tempdir().unwrap();
    let mut node = Node::open(root.path()).unwrap();
    node.open_app(APP, DDL).unwrap();
    for _ in 0..5 {
        node.append(APP, privatium_core::Event::del("profile", "own"))
            .unwrap();
    }
    let mut stream = node.subscribe(APP).unwrap();
    node.receive(APP, DEV, &event(1)).unwrap();
    assert!(matches!(
        stream.try_recv().unwrap(),
        privatium_core::StreamEvent::Append { lam: 1, .. }
    ));
    let header = String::from_utf8(event(2))
        .unwrap()
        .replacen('{', "{\"batch\":3,", 1)
        .into_bytes();
    node.receive(APP, DEV, &[header, event(3)].concat())
        .unwrap();
    assert!(stream.try_recv().is_err());
}

/// `spec/protocol.md §4.1`, `§10.2`: a batch crosses to a peer whole and inside one
/// bounded request, so an append past that bound is refused before it is written —
/// bytes in the log that no page could carry would never converge.
#[test]
fn test_spec_10_2_an_append_past_the_page_bound_is_refused_before_it_is_written() {
    let root = tempfile::tempdir().unwrap();
    let mut node = Node::open(root.path()).unwrap();
    node.open_app(APP, DDL).unwrap();
    set_body_bound(&mut node, 16 * 1024);
    let own = node
        .paths()
        .app_log_dir(APP)
        .join(format!("{}.jsonl", node.id()));
    let fits = privatium_core::Event::put(
        "profile",
        "small",
        serde_json::json!({"display_name": "x".repeat(1024)}),
    );
    node.append(APP, fits).unwrap();
    let before = fs::read(&own).unwrap();

    // One row too large for a page, and a batch whose lines are together too large.
    let huge = privatium_core::Event::put(
        "profile",
        "huge",
        serde_json::json!({"display_name": "x".repeat(32 * 1024)}),
    );
    let many: Vec<_> = (0..64)
        .map(|n| {
            privatium_core::Event::put(
                "profile",
                format!("many-{n}"),
                serde_json::json!({"display_name": "x".repeat(1024)}),
            )
        })
        .collect();
    for events in [vec![huge], many] {
        let refused = node.append_batch(APP, events).unwrap_err();
        assert!(
            matches!(refused, Error::AppendTooLarge { limit, .. } if limit == 16 * 1024),
            "{refused}"
        );
        assert_eq!(fs::read(&own).unwrap(), before, "{refused}");
    }
    assert_eq!(count(&node, "profile"), 1);

    // The bound is the setting, so raising it lets the same batch through.
    set_body_bound(&mut node, 4 * 1024 * 1024);
    node.append(
        APP,
        privatium_core::Event::put(
            "profile",
            "huge",
            serde_json::json!({"display_name": "x".repeat(32 * 1024)}),
        ),
    )
    .unwrap();
    assert_eq!(count(&node, "profile"), 2);
}

/// Move `api.max_body` (`spec/data-dictionary.md §3.6`) and rebuild `_sys`.
fn set_body_bound(node: &mut Node, bytes: u64) {
    let at = privatium_core::log::now();
    node.sys_log_mut()
        .put(
            privatium_core::sys::SETTING,
            "api.max_body",
            &serde_json::json!({ "value": bytes.to_string(), "updated_at": at }),
        )
        .unwrap();
    node.refresh().unwrap();
}

fn event(seq: u64) -> Vec<u8> {
    format!(
        "{{\"seq\":{seq},\"lam\":{seq},\"ts\":\"2026-09-01T00:00:00.000Z\",\"dev\":\"{DEV}\",\"app\":\"{APP}\",\"op\":\"put\",\"tbl\":\"profile\",\"id\":\"synthetic-{seq}\",\"d\":{{\"display_name\":\"example\",\"future\":1}},\"unknown\": [1, 2]}}\n"
    ).into_bytes()
}

fn receiver(node: &Node) -> Receiver {
    Receiver::open(node.paths(), APP, DEV, node.id(), Durability::Sync, LIMIT).unwrap()
}

fn path(node: &Node) -> std::path::PathBuf {
    node.paths().app_log_dir(APP).join(format!("{DEV}.jsonl"))
}

/// `spec/protocol.md §10.2`: everything a peer sends is untrusted. A range that names
/// the wrong device or app, skips or repeats a sequence, or carries something that is
/// not an envelope is refused whole, and a device or app name that is not shaped like
/// one is refused before any path is built from it.
#[test]
fn test_spec_10_2_push_validates_dev_seq_app_and_envelope() {
    let root = tempfile::tempdir().unwrap();
    let node = Node::open(root.path()).unwrap();
    let mut receiver = receiver(&node);
    receiver.append(&event(1)).unwrap();
    let before = fs::read(path(&node)).unwrap();
    for bytes in [
        event(3),
        event(1),
        String::from_utf8(event(2))
            .unwrap()
            .replace(DEV, "bbbbbbbb")
            .into_bytes(),
        String::from_utf8(event(2))
            .unwrap()
            .replace(APP, "other")
            .into_bytes(),
        b"{\"seq\":2}\n".to_vec(),
        [vec![b'x'; LIMIT], vec![b'\n']].concat(),
        [event(2), event(4)].concat(),
    ] {
        assert!(receiver.append(&bytes).is_err());
        assert_eq!(receiver.head(), 1);
        assert_eq!(fs::read(path(&node)).unwrap(), before);
    }
    assert_eq!(receiver.append(b"").unwrap(), 1);
    for (app, dev) in [("_lint", DEV), ("../escape", DEV), (APP, "../x")] {
        assert!(
            Receiver::open(node.paths(), app, dev, node.id(), Durability::Sync, LIMIT).is_err()
        );
    }
    assert!(!root.path().join("data/_lint").exists());
    assert!(!root.path().join("escape").exists());
}

/// `spec/protocol.md §10.2`: a hole in a log is never left to be filled in later. The
/// range is refused until the missing part arrives, and only then does the file grow.
#[test]
fn test_spec_10_2_a_seq_gap_is_refused_and_the_range_is_pulled() {
    let root = tempfile::tempdir().unwrap();
    let node = Node::open(root.path()).unwrap();
    let mut receiver = receiver(&node);
    let prefix = (1..=3).flat_map(event).collect::<Vec<_>>();
    receiver.append(&prefix).unwrap();
    assert!(matches!(
        receiver.append(&event(5)),
        Err(Error::ForeignSeq {
            expected: 4,
            found: 5,
            ..
        })
    ));
    receiver.append(&[event(4), event(5)].concat()).unwrap();
    assert_eq!(
        Sha256::digest(fs::read(path(&node)).unwrap()),
        Sha256::digest((1..=5).flat_map(event).collect::<Vec<_>>())
    );
}

/// `spec/protocol.md §4.2`, `§10.2`: the copy is the original, byte for byte, including
/// fields this build does not know. Re-serializing would change what a later reader — or
/// a person with `grep` — sees, and no two nodes would agree on the bytes.
#[test]
fn test_spec_10_2_received_lines_land_in_the_origin_devices_file_byte_for_byte() {
    let root = tempfile::tempdir().unwrap();
    let node = Node::open(root.path()).unwrap();
    let bytes = [event(1), b"not json\n\n".to_vec(), event(2)].concat();
    receiver(&node).append(&bytes).unwrap();
    assert_eq!(
        Sha256::digest(fs::read(path(&node)).unwrap()),
        Sha256::digest(&bytes)
    );
    assert!(!bytes.contains(&b'\r'));
    assert_eq!(receiver(&node).head(), 2);
}

/// `AGENTS.md` 2, `spec/protocol.md §10.2`: one device appends to one file, forever.
/// Sync is the single exception, and it copies rather than writes. The search over the
/// crate makes a fourth append path a failing test rather than a discovery years later.
#[test]
fn test_spec_10_2_the_receiver_is_the_only_writer_of_another_devices_file() {
    let root = tempfile::tempdir().unwrap();
    let node = Node::open(root.path()).unwrap();
    assert!(
        Receiver::open(
            node.paths(),
            APP,
            node.id().as_str(),
            node.id(),
            Durability::Sync,
            LIMIT
        )
        .is_err()
    );
    assert!(matches!(
        privatium_core::log::Writer::open(path(&node), APP, node.id(), 0, Durability::Sync),
        Err(Error::LogNotOurs { .. })
    ));
}

/// `spec/protocol.md §10.2`, `§3.1`: a copy cut mid-line is finished by adding the part
/// that is missing, never by cutting back to the last whole line. Bytes that do not
/// continue what is already there are refused and the owner is told where to look.
#[test]
fn test_spec_10_2_a_torn_foreign_segment_is_completed_by_its_suffix_never_truncated() {
    let root = tempfile::tempdir().unwrap();
    let node = Node::open(root.path()).unwrap();
    let first = event(1);
    let second = event(2);
    let cut = second.len() / 2;
    fs::create_dir_all(node.paths().app_log_dir(APP)).unwrap();
    let prefix = [first.clone(), second[..cut].to_vec()].concat();
    fs::write(path(&node), &prefix).unwrap();
    let mut receiver = receiver(&node);
    assert_eq!(receiver.head(), 1);
    assert_eq!(receiver.torn_tail(), &second[..cut]);
    assert!(receiver.append(&second).is_err());
    assert!(matches!(
        receiver.complete_torn(&event(3)),
        Err(Error::ForeignDiverged { .. })
    ));
    assert!(receiver.complete_torn(b"").is_err());
    assert_eq!(fs::read(path(&node)).unwrap(), prefix);
    receiver.complete_torn(&second).unwrap();
    assert_eq!(
        fs::read(path(&node)).unwrap(),
        [first.clone(), second].concat()
    );
    assert_eq!(receiver.head(), 2);

    // A segment torn before its first newline holds no complete line, so its head is 0
    // and the line it holds is the tail an origin completes.
    let other = tempfile::tempdir().unwrap();
    let node = Node::open(other.path()).unwrap();
    fs::create_dir_all(node.paths().app_log_dir(APP)).unwrap();
    let whole = first.len() - 1;
    fs::write(path(&node), &first[..whole]).unwrap();
    let mut torn = self::receiver(&node);
    assert_eq!(torn.head(), 0);
    assert_eq!(torn.torn_tail(), &first[..whole]);
    torn.complete_torn(&first).unwrap();
    assert_eq!(fs::read(path(&node)).unwrap(), first);
    assert_eq!(torn.head(), 1);
}
