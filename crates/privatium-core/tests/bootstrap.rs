// Project:  Privatium™  |  File: crates/privatium-core/tests/bootstrap.rs
// Authors:  Gabriel Mongefranco (@gabrielmongefranco)
// Created:  2026-09-01  |  Modified: 2026-09-01
// Summary:  First run: the §3 directory tree, and the two _sys rows a node writes about
//           itself before any app exists (docs/plans/phase-1.md §2.6, steps 1 to 3).

// AGENTS.md, Style: unwrap() is permitted in tests.
#![allow(clippy::unwrap_used, clippy::expect_used)]

use std::collections::BTreeSet;
use std::fs;
use std::path::Path;

use privatium_core::Node;
use serde_json::Value;

/// Every directory below the root, relative and slash-separated.
fn tree(root: &Path) -> BTreeSet<String> {
    fn walk(base: &Path, dir: &Path, into: &mut BTreeSet<String>) {
        for entry in fs::read_dir(dir).unwrap() {
            let entry = entry.unwrap();
            let path = entry.path();
            let relative = path
                .strip_prefix(base)
                .unwrap()
                .to_string_lossy()
                .replace('\\', "/");
            if entry.file_type().unwrap().is_dir() {
                into.insert(format!("{relative}/"));
                walk(base, &path, into);
            } else {
                into.insert(relative);
            }
        }
    }

    let mut found = BTreeSet::new();
    walk(root, root, &mut found);
    found
}

/// Read the `_sys` log as parsed events.
fn sys_events(node: &Node) -> Vec<Value> {
    let log = node.paths().app_log("_sys", node.id());
    let raw = fs::read_to_string(log).unwrap();
    raw.lines()
        .map(|line| serde_json::from_str(line).unwrap())
        .collect()
}

/// `spec/protocol.md §3` — first run creates that tree, and only that tree.
///
/// Exhaustive on purpose. A `contains` assertion would pass just as happily if the node
/// had also written a CSRF secret into `local/`, which `docs/plans/phase-1.md §2.2`
/// forbids: no secret is stored anywhere, and the key is derived at startup instead. That
/// decision has no code of its own to test, so this is where it is enforced.
#[test]
fn test_spec_3_layout_created() {
    let root = tempfile::tempdir().unwrap();
    let node = Node::open(root.path()).unwrap();
    let dev = node.id().as_str();

    let expected: BTreeSet<String> = [
        "apps/".to_owned(),
        "cache/".to_owned(),
        "data/".to_owned(),
        "data/_sys/".to_owned(),
        "data/_sys/log/".to_owned(),
        format!("data/_sys/log/{dev}.jsonl"),
        "data/_sys/snap/".to_owned(),
        "identity/".to_owned(),
        "identity/node.key".to_owned(),
        "identity/node.pub".to_owned(),
        "local/".to_owned(),
    ]
    .into_iter()
    .collect();

    assert_eq!(tree(root.path()), expected);
}

/// `docs/plans/phase-1.md §1` — cluster identity is out of scope, and absent is a valid
/// state rather than an unfinished one.
///
/// Written down as a test because "we did not build it" and "we built it wrong" look
/// identical from outside, and because the next reader's instinct on seeing a node with no
/// certificate will be to generate one.
#[test]
fn test_no_cluster_identity_in_phase_1() {
    let root = tempfile::tempdir().unwrap();
    let node = Node::open(root.path()).unwrap();
    let identity = node.paths().identity_dir();

    for absent in ["cluster.key", "cluster.pub", "node.cert"] {
        assert!(
            !identity.join(absent).exists(),
            "identity/{absent} should not exist in Phase 1"
        );
    }
}

/// `spec/data-dictionary.md §3.2` and `docs/plans/phase-1.md §2.2` — the node is the
/// device. One `sys_device` row, `kind = 'node'`, `replica = true`, and no fabricated
/// pairing metadata.
#[test]
fn test_sys_device_self_row_kind_is_node() {
    let root = tempfile::tempdir().unwrap();
    let node = Node::open(root.path()).unwrap();

    let events = sys_events(&node);
    let device = events
        .iter()
        .find(|event| event["tbl"] == "sys_device")
        .expect("no sys_device row was written");

    assert_eq!(
        device["id"],
        node.id().as_str(),
        "the row is keyed by this node's ID"
    );
    assert_eq!(device["d"]["kind"], "node");
    assert_eq!(device["d"]["replica"], true);

    // §3.2's pairing columns. This node paired with nobody, and `lan | iroh | onion |
    // tunnel` are four wrong answers rather than four candidates.
    for null in [
        "paired_at",
        "paired_via",
        "ed25519_pub",
        "x25519_pub",
        "user_agent",
    ] {
        assert!(
            device["d"].get(null).is_none(),
            "{null} was populated; it must be NULL for a node that paired with nobody"
        );
    }

    // Exactly one device row, and it is this node's.
    let devices = events
        .iter()
        .filter(|event| event["tbl"] == "sys_device")
        .count();
    assert_eq!(devices, 1);
}

/// `spec/data-dictionary.md §3.1` — the singleton describing this installation, keyed by
/// Node ID, carrying the public key that derives it.
#[test]
fn test_sys_node_row_describes_this_installation() {
    let root = tempfile::tempdir().unwrap();
    let node = Node::open(root.path()).unwrap();

    let events = sys_events(&node);
    let row = events
        .iter()
        .find(|event| event["tbl"] == "sys_node")
        .expect("no sys_node row was written");

    assert_eq!(row["id"], node.id().as_str());
    assert_eq!(row["d"]["protocol"], "pv/1");
    assert_eq!(row["d"]["build"], "custom");
    assert_eq!(row["d"]["pubkey"], node.identity().public_key_base64());

    // Phase 1 has no cluster, so §3.1's three certificate columns stay NULL.
    for null in ["cluster_id", "cert", "cert_expires_at", "display_name"] {
        assert!(
            row["d"].get(null).is_none(),
            "{null} should be NULL in Phase 1"
        );
    }
}

/// `spec/protocol.md §2.6` order and `§4.1` envelope invariants, on the only two events
/// Phase 1's bootstrap writes.
///
/// `seq` starts at 1 and is gapless, `lam` follows `§4.3` from a counter that has seen
/// nothing, and `dev` equals the log filename — which is the invariant that makes
/// `AGENTS.md` 2 checkable at all.
#[test]
fn test_bootstrap_writes_two_events_in_order() {
    let root = tempfile::tempdir().unwrap();
    let node = Node::open(root.path()).unwrap();
    let dev = node.id().as_str();

    let events = sys_events(&node);
    assert_eq!(
        events.len(),
        2,
        "bootstrap wrote {} events, not 2",
        events.len()
    );

    // §2.6 step 3: the device row, then the node row.
    assert_eq!(events[0]["tbl"], "sys_device");
    assert_eq!(events[1]["tbl"], "sys_node");

    for (index, event) in events.iter().enumerate() {
        let n = u64::try_from(index).unwrap() + 1;
        assert_eq!(event["seq"], n, "seq is not gapless from 1");
        assert_eq!(
            event["lam"], n,
            "lam does not follow §4.3 from a fresh counter"
        );
        assert_eq!(event["dev"], dev, "dev must equal the log filename (§4.1)");
        assert_eq!(event["app"], "_sys");
        assert_eq!(event["op"], "put");
        assert!(
            event["ts"].as_str().unwrap().ends_with('Z'),
            "ts is not UTC with a literal Z"
        );
    }

    // Both rows describe one moment: this node coming into existence.
    assert_eq!(events[0]["ts"], events[1]["ts"]);
    assert_eq!(events[1]["d"]["created_at"], events[1]["ts"]);
}

/// `spec/protocol.md §4.1` — lines are `\n` terminated, `0x0A`, never `\r\n`.
///
/// Checked on the bytes rather than through a line iterator, which would hide a `\r`. This
/// is the property that keeps `echo >> log.jsonl` working, and it is the one most likely
/// to rot on Windows.
#[test]
fn test_spec_4_1_lines_are_lf_terminated() {
    let root = tempfile::tempdir().unwrap();
    let node = Node::open(root.path()).unwrap();

    let raw = fs::read(node.paths().app_log("_sys", node.id())).unwrap();

    assert!(!raw.contains(&b'\r'), "the log contains a CR byte");
    assert_eq!(
        raw.last(),
        Some(&b'\n'),
        "the log does not end with a newline"
    );
    assert_eq!(raw.iter().filter(|byte| **byte == b'\n').count(), 2);
}

/// The event envelope emits keys in the order `spec/protocol.md §4.1` asks for.
///
/// §4.1 says readers MUST NOT depend on key order and writers SHOULD emit this one. The
/// reason is greppability: a human reading a log file with `less` should see the same
/// shape on every line.
#[test]
fn test_spec_4_1_key_order_is_greppable() {
    let root = tempfile::tempdir().unwrap();
    let node = Node::open(root.path()).unwrap();

    let raw = fs::read_to_string(node.paths().app_log("_sys", node.id())).unwrap();
    let first = raw.lines().next().unwrap();

    let order = [
        "\"seq\"", "\"lam\"", "\"ts\"", "\"dev\"", "\"app\"", "\"op\"", "\"tbl\"", "\"id\"",
        "\"d\"",
    ];

    let mut cursor = 0;
    for key in order {
        let at = first[cursor..]
            .find(key)
            .map(|offset| offset + cursor)
            .unwrap_or_else(|| panic!("{key} is missing or out of order in {first}"));
        cursor = at + key.len();
    }
}

/// Opening a node twice never re-runs the bootstrap, and the writer that could is
/// incapable of attaching to an existing log.
///
/// `docs/plans/phase-1.md §2.6` puts step 2 in M1 while the log itself is M2, so M1's
/// writer can only create. The guard above it is the log file's existence; this checks
/// that the guard and the writer agree, because if the guard ever regressed, an
/// `open`-capable writer would emit `seq: 1` over a live stream.
#[test]
fn test_bootstrap_does_not_re_open_an_existing_log() {
    let root = tempfile::tempdir().unwrap();

    let first = Node::open(root.path()).unwrap();
    let log = first.paths().app_log("_sys", first.id());
    drop(first);

    // A third and fourth open, to be sure the guard is not merely "the tree was absent".
    let _ = Node::open(root.path()).unwrap();
    let node = Node::open(root.path()).unwrap();

    assert_eq!(sys_events(&node).len(), 2);
    assert_eq!(fs::read_to_string(&log).unwrap().lines().count(), 2);
}

/// `config.toml` is optional (`docs/backup-and-restore.md §1`), and a node that starts
/// without one does not write one.
#[test]
fn test_config_is_optional_and_not_created() {
    let root = tempfile::tempdir().unwrap();
    let node = Node::open(root.path()).unwrap();

    assert!(!node.paths().config_file().exists());
    assert_eq!(node.config().node.port, 8420);
    assert_eq!(node.config().node.mode, privatium_core::Mode::Host);
}

/// A `config.toml` that is present is read, and `--config` may point outside the root.
#[test]
fn test_config_is_read_from_the_configured_path() {
    let root = tempfile::tempdir().unwrap();
    let elsewhere = tempfile::tempdir().unwrap();
    let config = elsewhere.path().join("elsewhere.toml");
    fs::write(&config, "[node]\nport = 9310\n\n[lua]\npool_size = 3\n").unwrap();

    let node = Node::open_with(Some(root.path()), Some(config.as_path())).unwrap();

    assert_eq!(node.config().node.port, 9310);
    assert_eq!(node.config().lua.pool_size, 3);
    assert_eq!(
        node.config().lua.max_memory_mb,
        64,
        "an unset key keeps its default"
    );
}

/// A config the node cannot make sense of stops it, and says which file and why.
#[test]
fn test_solo_mode_without_an_app_is_refused() {
    let root = tempfile::tempdir().unwrap();
    fs::write(root.path().join("config.toml"), "[node]\nmode = \"solo\"\n").unwrap();

    let error = Node::open(root.path()).unwrap_err().to_string();

    assert!(error.contains("config.toml"), "{error}");
    assert!(error.contains("solo"), "{error}");
}
