// Project:  Privatium™  |  File: crates/privatium-core/tests/admission.rs
// Authors:  Gabriel Mongefranco (@gabrielmongefranco)
// Created:  2026-09-07  |  Modified: 2026-09-07
// Summary:  Node admission against spec/protocol.md §2.3–§2.3.4, §6.1 and §7.4.2: two
//           nodes in one process, each in its own data root, the eight messages driven as
//           data through Node::pairing_* on the window's side and Node::join_* on the
//           dialer's, in both directions; disposability, the swap, the expired state,
//           re-admission, renewal at runtime, revocation, and the cluster filter.
//           See main README.md for full license information.

// AGENTS.md, Style: unwrap() is permitted in tests, and a test that hides a failure
// behind `?` is worse than one that panics with a line number.
#![allow(clippy::unwrap_used, clippy::expect_used)]

mod common;

use std::fs;
use std::net::{IpAddr, Ipv4Addr};
use std::path::Path;
use std::time::Duration;

use base64::{Engine as _, engine::general_purpose::STANDARD};
use common::sys_row;
use privatium_core::discover::Discovered;
use privatium_core::identity::Identity;
use privatium_core::pair::handshake::Client;
use privatium_core::pair::join::JoinStep;
use privatium_core::pair::{Code, Joined, PairError, PairOutcome};
use privatium_core::{Error, Node, Standing, sys};
use serde_json::{Value, json};

/// TEST-NET-1 (RFC 5737): never loopback, never a real peer.
fn source(last: u8) -> IpAddr {
    IpAddr::V4(Ipv4Addr::new(192, 0, 2, last))
}

fn now() -> jiff::Timestamp {
    jiff::Timestamp::now()
}

/// A source address no earlier attempt in this process used, so the two-second rule of
/// `spec/protocol.md §7.5` never refuses a test's second exchange.
fn fresh_source() -> IpAddr {
    use std::sync::atomic::{AtomicU16, Ordering};
    static NEXT: AtomicU16 = AtomicU16::new(1);
    let n = NEXT.fetch_add(1, Ordering::Relaxed);
    IpAddr::V4(Ipv4Addr::new(198, 51, (n >> 8) as u8, n as u8))
}

fn days(n: i64) -> jiff::SignedDuration {
    jiff::SignedDuration::from_secs(n * 86_400)
}

fn open(root: &tempfile::TempDir) -> Node {
    Node::open(root.path()).unwrap()
}

fn code_of(node: &Node) -> Code {
    node.pairing().unwrap().code()
}

/// A synthetic browser row, as pairing would have written it, so a node counts as having
/// paired something (`spec/protocol.md §2.3`).
fn pair_a_browser(node: &mut Node, id: &str) {
    node.sys_log_mut()
        .put(
            sys::DEVICE,
            id,
            &json!({
                "kind": "browser",
                "replica": false,
                "ed25519_pub": "AAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAA=",
                "x25519_pub": "AAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAA=",
                "paired_at": "2026-09-06T10:00:00.000Z",
                "paired_via": "lan",
            }),
        )
        .unwrap();
    node.refresh().unwrap();
}

/// Back-date a node's certificate so that it expired `days_ago` days before `at`, as a
/// node offline past 180 days holds one; the identity reloads in the expired state.
fn expire_certificate(root: &tempfile::TempDir, at: jiff::Timestamp, days_ago: i64) {
    let node = open(root);
    let cert = node
        .identity()
        .sign_certificate(&node.identity().verifying_key(), at - days(180 + days_ago))
        .unwrap();
    let path = node.paths().identity_dir().join("node.cert");
    drop(node);
    fs::write(path, serde_json::to_vec(&cert).unwrap()).unwrap();
}

/// The outcome of one exchange, as each side reports it.
#[derive(Debug)]
struct Exchanged {
    /// What the dialer's `join_apply` answered.
    client: Joined,
    /// What the window's side answered: the row written for the dialer, or the adoption.
    node_joined: bool,
    node_readmitted: bool,
}

/// Drive the whole admission as data (`spec/protocol.md §7.4.2`): `node` opened a window
/// for a node and `client` dials it at `url`. Both sides go through the same functions
/// the sockets call, so what the wire would carry is exactly what crosses here.
fn exchange(
    node: &mut Node,
    client: &mut Node,
    url: &str,
    when: jiff::Timestamp,
) -> Result<Exchanged, Error> {
    let hello = node.pairing_hello(when);
    let code = code_of(node);
    let (mut joining, start) = client.join_start_at(url, code, &hello, when)?;
    let (attempt, reply) = node.pairing_begin(fresh_source(), when, &start)?;
    let confirm = joining.reply(&reply)?;
    let (sealed, node_sealed) = node.pairing_confirm(attempt, &confirm, when)?;
    let (decided, client_sealed) = joining.finish(&node_sealed, when)?;
    let outcome = node.pairing_finish(sealed, &client_sealed, when)?;
    let step = decided.next(when)?;
    match (outcome, step) {
        (PairOutcome::Admit(admission), JoinStep::AwaitAdmit(mut awaiting)) => {
            let (pending, admit) = node.pairing_admit(admission, when)?;
            let applied = awaiting.verify(&admit, when)?;
            let joined = client.join_apply_at(applied, when)?;
            let message = awaiting.joined()?;
            let paired = node.pairing_admitted(pending, &message, when)?;
            assert_eq!(paired.device, client.id().as_str());
            Ok(Exchanged {
                client: joined,
                node_joined: false,
                node_readmitted: false,
            })
        }
        (PairOutcome::Join(admission), JoinStep::SendAdmit { bytes, pending }) => {
            let (node_side, message) = node.pairing_adopt(admission, &bytes, when)?;
            let applied = pending.admitted(&message)?;
            let joined = client.join_apply_at(applied, when)?;
            Ok(Exchanged {
                client: joined,
                node_joined: node_side.joined,
                node_readmitted: node_side.readmitted,
            })
        }
        (outcome, _) => panic!("the two sides disagree on the direction: {outcome:?}"),
    }
}

/// Open a window for a node on `node` and run the exchange from `client`.
fn admit(node: &mut Node, client: &mut Node, when: jiff::Timestamp) -> Result<Exchanged, Error> {
    node.pair_node_at(Duration::from_secs(120), when)?;
    exchange(node, client, "http://192.0.2.1:8420", when)
}

/// Every `sys_audit` row of one kind, after `_sys` is rematerialized from the log.
fn audit_rows(node: &mut Node, kind: &str) -> Vec<Value> {
    node.refresh().unwrap();
    common::audit_rows(node, kind)
}

fn cluster_of(node: &Node) -> String {
    node.identity().cluster_id().as_str().to_owned()
}

fn cluster_key(root: &tempfile::TempDir) -> Vec<u8> {
    fs::read(root.path().join("identity/cluster.key")).unwrap()
}

/// Every file under `path` is free of every secret.
fn check_absent(path: &Path, secrets: &[Vec<u8>]) {
    for entry in fs::read_dir(path).unwrap() {
        let path = entry.unwrap().path();
        if path.is_dir() {
            check_absent(&path, secrets);
        } else if path.file_name().is_some_and(|name| name == "lock") {
            // `local/lock` is held by the node itself (spec/protocol.md §3.1).
            continue;
        } else {
            let bytes = fs::read(&path).unwrap();
            for secret in secrets {
                assert!(
                    !bytes.windows(secret.len()).any(|w| w == secret),
                    "{} holds a cluster secret",
                    path.display()
                );
            }
        }
    }
}

fn device_columns(row: &Value) -> Vec<(&str, Value)> {
    [
        "kind",
        "replica",
        "ed25519_pub",
        "x25519_pub",
        "paired_at",
        "paired_via",
        "label",
        "user_agent",
    ]
    .into_iter()
    .map(|key| (key, row.get(key).cloned().unwrap_or(Value::Null)))
    .collect()
}

// ---------------------------------------------------------------------------------------
// §2.3.1 — admission in either direction
// ---------------------------------------------------------------------------------------

/// `spec/protocol.md §2.3.1`, `§7.4.2` — a fresh node dials an established one's window:
/// it receives the cluster key and a certificate for its own key, adopts the cluster,
/// the admitter writes its row and `node.admitted`, and the joiner remembers the
/// admitter as a peer hint.
#[test]
fn test_spec_2_3_1_a_node_is_admitted_by_pairing_and_receives_the_cluster_key_and_a_certificate() {
    let (root_a, root_b) = (tempfile::tempdir().unwrap(), tempfile::tempdir().unwrap());
    let mut a = open(&root_a);
    let mut b = open(&root_b);
    pair_a_browser(&mut a, "b3nn8t2q");
    a.set_display_name("Desk").unwrap();
    let founded_by_b = cluster_of(&b);
    assert_ne!(cluster_of(&a), founded_by_b);

    let outcome = admit(&mut a, &mut b, now()).unwrap();
    assert!(outcome.client.joined, "the fresh node joined");
    assert!(!outcome.client.readmitted);
    assert_eq!(outcome.client.cluster_id, cluster_of(&a));
    assert_eq!(outcome.client.peer, a.id().as_str());
    assert_eq!(cluster_of(&b), cluster_of(&a));
    assert_eq!(cluster_key(&root_b), cluster_key(&root_a));
    assert_eq!(b.identity().certificate().cluster_id, cluster_of(&a));
    assert_eq!(b.identity().certificate().node_id, b.id().as_str());
    assert_eq!(b.standing(now()).unwrap(), Standing::Member);

    // The admitter's row for the joiner, and the alert.
    let row = sys_row(&a, "sys_device", b.id().as_str()).unwrap();
    assert_eq!(row["kind"], "node");
    assert_eq!(row["replica"], true);
    assert_eq!(row["paired_via"], "lan");
    assert_eq!(row["ed25519_pub"], b.identity().public_key_base64());
    assert_eq!(row["x25519_pub"], b.identity().x25519_public_base64());
    assert!(row.get("user_agent").is_none_or(Value::is_null));
    let admitted = audit_rows(&mut a, "node.admitted");
    assert_eq!(admitted.len(), 1);
    assert_eq!(admitted[0]["severity"], "alert");
    assert_eq!(admitted[0]["subject"], b.id().as_str());
    assert!(
        admitted[0]["detail"]
            .as_str()
            .unwrap()
            .contains("\"readmitted\":false")
    );
    assert_eq!(a.paired_node_count().unwrap(), 1);
    assert!(a.pairing().unwrap().consumed_by() == Some(b.id().as_str()));

    // The joiner: its sys_node amended, its own row re-asserted, the founded cluster's
    // row tombstoned, the hint written, and the admitter's public key held.
    let node_row = sys_row(&b, "sys_node", b.id().as_str()).unwrap();
    assert_eq!(node_row["cluster_id"], cluster_of(&a));
    assert_eq!(
        node_row["cert"],
        b.identity().certificate().to_base64().unwrap()
    );
    assert!(sys_row(&b, "sys_cluster", &founded_by_b).is_none());
    let hints = b.peer_hints();
    assert_eq!(hints.len(), 1);
    assert_eq!(hints[0].id, a.id().as_str());
    assert_eq!(hints[0].x25519_pub, a.identity().x25519_public_base64());
    assert_eq!(hints[0].url.as_deref(), Some("http://192.0.2.1:8420"));
    let state = fs::read_to_string(root_b.path().join("local/state.jsonl")).unwrap();
    assert!(state.contains("\"peers\""), "{state}");
    // The window is consumed as for any kind; a second dial finds it closed.
    assert!(!a.pairing_open(now()));
}

/// `§2.3.1` — the established node admits the disposable one whichever dialed: the
/// fresh node dialing the established one, and the established one dialing the fresh
/// one, end the same way, with `admit` from the established side both times.
#[test]
fn test_spec_2_3_1_the_established_node_admits_the_disposable_one_whichever_dialed() {
    // The fresh node dials.
    let (root_a, root_b) = (tempfile::tempdir().unwrap(), tempfile::tempdir().unwrap());
    let mut a = open(&root_a);
    let mut b = open(&root_b);
    pair_a_browser(&mut a, "b3nn8t2q");
    let outcome = admit(&mut a, &mut b, now()).unwrap();
    assert!(outcome.client.joined);
    assert!(!outcome.node_joined);
    assert_eq!(cluster_of(&b), cluster_of(&a));
    assert!(sys_row(&a, "sys_device", b.id().as_str()).is_some());
    assert_eq!(audit_rows(&mut a, "node.admitted").len(), 1);
    assert_eq!(audit_rows(&mut b, "node.admitted").len(), 0);

    // The established node dials the fresh one's window — the always-on case.
    let (root_c, root_d) = (tempfile::tempdir().unwrap(), tempfile::tempdir().unwrap());
    let mut established = open(&root_c);
    let mut fresh = open(&root_d);
    pair_a_browser(&mut established, "b3nn8t2q");
    let before = cluster_of(&established);
    let outcome = admit(&mut fresh, &mut established, now()).unwrap();
    assert!(!outcome.client.joined, "the dialer admitted");
    assert!(outcome.node_joined, "the window's node joined");
    assert_eq!(outcome.client.cluster_id, before);
    assert_eq!(cluster_of(&fresh), before);
    assert_eq!(cluster_key(&root_d), cluster_key(&root_c));
    // The admitter — the dialer this time — wrote the row and the alert; the joiner
    // remembered the admitter, without a URL since it was dialed.
    let row = sys_row(&established, "sys_device", fresh.id().as_str()).unwrap();
    assert_eq!(row["kind"], "node");
    assert_eq!(row["replica"], true);
    assert_eq!(audit_rows(&mut established, "node.admitted").len(), 1);
    assert_eq!(established.paired_node_count().unwrap(), 1);
    let hints = fresh.peer_hints();
    assert_eq!(hints.len(), 1);
    assert_eq!(hints[0].id, established.id().as_str());
    assert!(hints[0].url.is_none());
}

/// `§2.3.1` — two fresh nodes: the dialer joins the node whose owner opened the window.
/// Two established nodes are refused with 4403 naming rotation, one audited failure,
/// and neither identity changes.
#[test]
fn test_spec_2_3_1_two_fresh_nodes_join_toward_the_window_and_two_established_ones_are_refused() {
    let (root_a, root_b) = (tempfile::tempdir().unwrap(), tempfile::tempdir().unwrap());
    let mut a = open(&root_a);
    let mut b = open(&root_b);
    assert!(a.is_disposable().unwrap() && b.is_disposable().unwrap());
    let window_cluster = cluster_of(&a);
    let outcome = admit(&mut a, &mut b, now()).unwrap();
    assert!(outcome.client.joined);
    assert_eq!(cluster_of(&b), window_cluster);
    assert_eq!(cluster_of(&a), window_cluster);

    let (root_c, root_d) = (tempfile::tempdir().unwrap(), tempfile::tempdir().unwrap());
    let mut c = open(&root_c);
    let mut d = open(&root_d);
    pair_a_browser(&mut c, "b3nn8t2q");
    pair_a_browser(&mut d, "c4mm9t3r");
    let (before_c, before_d) = (cluster_of(&c), cluster_of(&d));
    let refused = admit(&mut c, &mut d, now()).unwrap_err();
    assert!(
        matches!(refused, Error::Pair(PairError::TwoClusters)),
        "{refused}"
    );
    assert_eq!(PairError::TwoClusters.close_code(), 4403);
    assert!(refused.to_string().contains("§2.3.5"));
    assert_eq!(cluster_of(&c), before_c);
    assert_eq!(cluster_of(&d), before_d);
    assert!(sys_row(&c, "sys_device", d.id().as_str()).is_none());
    let failed = audit_rows(&mut c, "pair.failed");
    assert_eq!(failed.len(), 1);
    assert!(
        failed[0]["detail"]
            .as_str()
            .unwrap()
            .contains("both nodes already belong")
    );
    assert!(audit_rows(&mut c, "node.admitted").is_empty());
}

/// `§7.4.2` — `admit` is sealed only after the joiner's signature verified: a missing
/// signature, one by another key and one over other bytes are each 4403, one audited
/// failure, and nothing sealed after them — on the node's side for the client's
/// message, and on the client's side for the node's.
#[test]
fn test_spec_2_3_1_admit_is_sent_only_after_the_joiners_signature_verifies() {
    use ed25519_dalek::Signer as _;
    use privatium_core::pair::handshake::verify_signature;

    // End to end: a client that declares the node kind and signs nothing — a browser's
    // client driven with `kind = "node"` — is refused at its sealed message with 4403,
    // one audited failure, nothing sealed after it, and the window still open.
    let root_a = tempfile::tempdir().unwrap();
    let mut a = open(&root_a);
    pair_a_browser(&mut a, "b3nn8t2q");
    a.pair_node_at(Duration::from_secs(120), now()).unwrap();
    let hello = a.pairing_hello(now());
    let (mut client, start) = Client::start(&hello, code_of(&a), "node").unwrap();
    let (attempt, reply) = a.pairing_begin(source(20), now(), &start).unwrap();
    let confirm = client.reply(&reply).unwrap();
    let (sealed, node_sealed) = a.pairing_confirm(attempt, &confirm, now()).unwrap();
    let (_, unsigned) = client.finish(&node_sealed, None, None, now()).unwrap();
    let refused = a.pairing_finish(sealed, &unsigned, now()).unwrap_err();
    assert!(
        matches!(refused, Error::Pair(PairError::Signature)),
        "{refused}"
    );
    let failed = audit_rows(&mut a, "pair.failed");
    assert_eq!(failed.len(), 1);
    assert!(failed[0]["detail"].as_str().unwrap().contains("signature"));
    assert!(audit_rows(&mut a, "node.admitted").is_empty());
    assert!(
        a.pairing_open(now()),
        "a refused signature leaves the window open"
    );
    assert!(sys_row(&a, "sys_device", client_id(&start)).is_none());

    // The verifier both sides call, over every case: missing, by another key, over
    // other bytes, and a byte flipped.
    let signing = ed25519_dalek::SigningKey::from_bytes(&[3; 32]);
    let public = STANDARD.encode(signing.verifying_key().as_bytes());
    let transcript = b"pv/1 pake transcript".to_vec();
    let good = STANDARD.encode(signing.sign(&transcript).to_bytes());
    assert!(verify_signature(&public, &transcript, Some(&good)).is_ok());
    assert_eq!(
        verify_signature(&public, &transcript, None),
        Err(PairError::Signature)
    );
    let other = ed25519_dalek::SigningKey::from_bytes(&[4; 32]);
    let by_other = STANDARD.encode(other.sign(&transcript).to_bytes());
    assert_eq!(
        verify_signature(&public, &transcript, Some(&by_other)),
        Err(PairError::Signature)
    );
    assert_eq!(
        verify_signature(&public, b"other bytes", Some(&good)),
        Err(PairError::Signature)
    );
    let mut bad = STANDARD.decode(&good).unwrap();
    bad[9] ^= 1;
    assert_eq!(
        verify_signature(&public, &transcript, Some(&STANDARD.encode(bad))),
        Err(PairError::Signature)
    );
    assert_eq!(
        verify_signature(&public, &transcript, Some("not base64")),
        Err(PairError::Signature)
    );
    assert_eq!(PairError::Signature.close_code(), 4403);
}

/// The `dev` a client start message names.
fn client_id(start: &str) -> &str {
    let value: Value = serde_json::from_str(start).unwrap();
    let id = value["dev"].as_str().unwrap();
    // The ID is eight characters at a fixed place in the text; return a slice of the
    // original so the borrow is of `start`.
    let at = start.find(id).unwrap();
    &start[at..at + id.len()]
}

/// `§2.3` — disposability is judged from this node's own segments: a paired browser
/// makes a node non-disposable; a restored `_sys` full of other devices' rows does not;
/// a re-founded node is disposable again; an empty log is.
#[test]
fn test_spec_2_3_disposability_is_judged_from_this_nodes_own_segments() {
    let root = tempfile::tempdir().unwrap();
    let mut node = open(&root);
    assert!(
        node.is_disposable().unwrap(),
        "a first start paired nothing"
    );
    pair_a_browser(&mut node, "b3nn8t2q");
    assert!(!node.is_disposable().unwrap(), "a paired browser counts");
    let id = node.id().as_str().to_owned();
    drop(node);

    // Rows another device wrote, restored into `data/_sys/log/` under that device's
    // name: pairings of some other node, which count for nothing here.
    let other = tempfile::tempdir().unwrap();
    let mut other_node = open(&other);
    pair_a_browser(&mut other_node, "c4mm9t3r");
    pair_a_browser(&mut other_node, "d5nn0t4s");
    let other_id = other_node.id().as_str().to_owned();
    drop(other_node);
    let fresh = tempfile::tempdir().unwrap();
    let fresh_node = open(&fresh);
    let log_dir = fresh_node.paths().app_log_dir("_sys");
    drop(fresh_node);
    fs::copy(
        other.path().join(format!("data/_sys/log/{other_id}.jsonl")),
        log_dir.join(format!("{other_id}.jsonl")),
    )
    .unwrap();
    let fresh_node = open(&fresh);
    assert!(
        sys_row(&fresh_node, "sys_device", "c4mm9t3r").is_some(),
        "the restored rows are in the cache"
    );
    assert!(
        fresh_node.is_disposable().unwrap(),
        "rows another device wrote count for nothing"
    );
    drop(fresh_node);

    // Re-founding (§2.3.5): delete the cluster files and start again — the node's own
    // `sys_node` event sets a new cluster, and the browser it paired before is behind
    // that boundary.
    for file in ["cluster.key", "cluster.pub", "node.cert"] {
        fs::remove_file(root.path().join("identity").join(file)).unwrap();
    }
    let node = open(&root);
    assert_eq!(node.id().as_str(), id);
    assert!(
        node.is_disposable().unwrap(),
        "a re-founded node is disposable again"
    );
    let mut node = node;
    pair_a_browser(&mut node, "e6pp1t5t");
    assert!(!node.is_disposable().unwrap());
}

/// `§2.3.3` — the cluster key goes to a node and never to a browser: a browser's sealed
/// message carries the public key alone and no `admit` follows it, while a node's
/// exchange carries the seed exactly once, sealed, and it lands in `identity/` alone.
#[test]
fn test_spec_2_3_3_the_cluster_key_goes_to_a_node_and_never_to_a_browser() {
    let (root_a, root_b) = (tempfile::tempdir().unwrap(), tempfile::tempdir().unwrap());
    let mut a = open(&root_a);
    let mut b = open(&root_b);
    pair_a_browser(&mut a, "b3nn8t2q");
    let key = cluster_key(&root_a);
    let key_b64 = STANDARD.encode(&key);

    // A browser: the six messages, and what it pins is the public key.
    a.pair_at(Duration::from_secs(120), now()).unwrap();
    let hello = a.pairing_hello(now());
    let (mut client, start) = Client::start(&hello, code_of(&a), "browser").unwrap();
    let (attempt, reply) = a.pairing_begin(source(1), now(), &start).unwrap();
    let confirm = client.reply(&reply).unwrap();
    let (sealed, node_sealed) = a.pairing_confirm(attempt, &confirm, now()).unwrap();
    let (pinned, client_sealed) = client
        .finish(&node_sealed, Some("Phone"), None, now())
        .unwrap();
    assert_eq!(pinned.cluster_pub, a.identity().cluster_public());
    let outcome = a.pairing_finish(sealed, &client_sealed, now()).unwrap();
    assert!(matches!(outcome, PairOutcome::Device(_)));
    for text in [&start, &reply, &confirm] {
        assert!(!text.contains(&key_b64));
    }
    a.close_pairing(now()).unwrap();

    // A node: the seed crosses once, under K_pair, and lands in identity/ alone.
    let outcome = admit(&mut a, &mut b, now()).unwrap();
    assert!(outcome.client.joined);
    assert_eq!(cluster_key(&root_b), key);
    let secrets = [key.clone(), key_b64.into_bytes()];
    for root in [&root_a, &root_b] {
        check_absent(&root.path().join("data"), &secrets);
        check_absent(&root.path().join("local"), &secrets);
    }
}

/// `§2.3.3` — the Phase 2 walk, again over both nodes' roots after admission: the key
/// is in no event, no snapshot and no backup export; the hint holds no secret; and
/// `Debug` of neither identity prints it.
#[test]
fn test_spec_2_3_3_cluster_private_key_is_absent_from_every_event_snapshot_and_backup() {
    let (root_a, root_b) = (tempfile::tempdir().unwrap(), tempfile::tempdir().unwrap());
    let mut a = open(&root_a);
    let mut b = open(&root_b);
    pair_a_browser(&mut a, "b3nn8t2q");
    admit(&mut a, &mut b, now()).unwrap();
    a.snapshot("_sys").unwrap();
    b.snapshot("_sys").unwrap();
    let key = cluster_key(&root_a);
    assert_eq!(cluster_key(&root_b), key);
    let secrets = [key.clone(), STANDARD.encode(&key).into_bytes()];
    for (root, node) in [(&root_a, &a), (&root_b, &b)] {
        check_absent(&node.paths().data_dir(), &secrets);
        check_absent(&root.path().join("local"), &secrets);
        let target = tempfile::tempdir().unwrap();
        let paths = privatium_core::Paths::rooted(target.path());
        privatium_core::backup::Plan::build(root.path(), &paths, None)
            .unwrap()
            .apply()
            .unwrap();
        check_absent(target.path(), &secrets);
        let debug = format!("{:?}", node.identity());
        assert!(!debug.contains(&STANDARD.encode(&key)));
        assert!(!debug.contains(&format!("{key:?}")));
    }
}

/// `§2.3.1`, `spec/data-dictionary.md §3.2` — the joiner and the admitter write the same
/// device facts: the two rows materialize to the same columns whichever wins; the
/// joiner's re-assertion outranks its bootstrap row; the folded `lam` is past the
/// admitter's.
#[test]
fn test_spec_2_3_1_the_joiner_and_the_admitter_write_the_same_device_facts() {
    let (root_a, root_b) = (tempfile::tempdir().unwrap(), tempfile::tempdir().unwrap());
    let mut a = open(&root_a);
    let mut b = open(&root_b);
    pair_a_browser(&mut a, "b3nn8t2q");
    b.set_display_name("Laptop").unwrap();
    let admitter_lam_before = a.sys_log().lam();
    admit(&mut a, &mut b, now()).unwrap();

    let by_admitter = sys_row(&a, "sys_device", b.id().as_str()).unwrap();
    let by_joiner = sys_row(&b, "sys_device", b.id().as_str()).unwrap();
    assert_eq!(device_columns(&by_admitter), device_columns(&by_joiner));
    assert_eq!(by_joiner["label"], "Laptop");
    assert_eq!(by_joiner["replica"], true);
    assert!(by_joiner["paired_at"].is_string());

    // The joiner's own log: its re-assertion is later than its bootstrap row and past
    // the admitter's counter at admission.
    let lines: Vec<Value> = fs::read_to_string(b.paths().app_log("_sys", b.id()))
        .unwrap()
        .lines()
        .map(|line| serde_json::from_str(line).unwrap())
        .collect();
    let own_rows: Vec<&Value> = lines
        .iter()
        .filter(|line| line["tbl"] == "sys_device" && line["id"] == b.id().as_str())
        .collect();
    assert!(own_rows.len() >= 2, "bootstrap row and re-assertion");
    let last = own_rows.last().unwrap();
    assert!(last["lam"].as_u64().unwrap() > admitter_lam_before);
    assert_eq!(last["d"]["paired_via"], "lan");
    assert!(b.sys_log().lam() > admitter_lam_before);
}

/// `§2.3.1` — a `joined` that never arrives writes no row and no success on the
/// admitter; the joiner holds the key and, still disposable, joins again against a new
/// window.
#[test]
fn test_spec_2_3_1_a_lost_joined_leaves_no_row_and_the_join_is_retried() {
    let (root_a, root_b) = (tempfile::tempdir().unwrap(), tempfile::tempdir().unwrap());
    let mut a = open(&root_a);
    let mut b = open(&root_b);
    pair_a_browser(&mut a, "b3nn8t2q");
    let when = now();
    a.pair_node_at(Duration::from_secs(120), when).unwrap();
    let hello = a.pairing_hello(when);
    let (mut joining, start) = b
        .join_start_at("http://192.0.2.1:8420", code_of(&a), &hello, when)
        .unwrap();
    let (attempt, reply) = a.pairing_begin(source(7), when, &start).unwrap();
    let confirm = joining.reply(&reply).unwrap();
    let (sealed, node_sealed) = a.pairing_confirm(attempt, &confirm, when).unwrap();
    let (decided, client_sealed) = joining.finish(&node_sealed, when).unwrap();
    let PairOutcome::Admit(admission) = a.pairing_finish(sealed, &client_sealed, when).unwrap()
    else {
        panic!("the established node admits");
    };
    let JoinStep::AwaitAdmit(mut awaiting) = decided.next(when).unwrap() else {
        panic!("the fresh node awaits admit");
    };
    let (pending, admit_message) = a.pairing_admit(admission, when).unwrap();
    let applied = awaiting.verify(&admit_message, when).unwrap();
    b.join_apply_at(applied, when).unwrap();
    // The connection is cut here: `joined` is never delivered.
    let refused = a.pairing_admitted(pending, &[0; 32], when).unwrap_err();
    assert!(matches!(refused, Error::Pair(PairError::Format)));
    assert!(sys_row(&a, "sys_device", b.id().as_str()).is_none());
    assert!(audit_rows(&mut a, "node.admitted").is_empty());
    assert_eq!(audit_rows(&mut a, "pair.failed").len(), 1);
    assert_eq!(cluster_of(&b), cluster_of(&a), "the joiner holds the key");
    assert!(b.is_disposable().unwrap(), "and is still disposable");
    assert!(
        !a.pairing_open(when),
        "the window was consumed at the sealed message"
    );

    // A second join against a new window succeeds and writes the row.
    let later = when + jiff::SignedDuration::from_secs(5);
    let outcome = admit(&mut a, &mut b, later).unwrap();
    assert!(outcome.client.joined);
    assert!(sys_row(&a, "sys_device", b.id().as_str()).is_some());
    assert_eq!(audit_rows(&mut a, "node.admitted").len(), 1);
}

/// `spec/data-dictionary.md §3.1b` — a joined node writes no `sys_cluster` row for the
/// cluster it joined and audits no founding; the founder's row is the founder's.
#[test]
fn test_spec_2_3_1_a_joined_node_writes_no_cluster_row_and_audits_no_founding() {
    let (root_a, root_b) = (tempfile::tempdir().unwrap(), tempfile::tempdir().unwrap());
    let mut a = open(&root_a);
    let mut b = open(&root_b);
    pair_a_browser(&mut a, "b3nn8t2q");
    assert_eq!(
        audit_rows(&mut b, "cluster.created").len(),
        1,
        "founding is audited once"
    );
    admit(&mut a, &mut b, now()).unwrap();
    let joined = cluster_of(&a);
    let id_b = b.id().as_str().to_owned();
    drop(b);
    let mut b = open(&root_b);
    assert_eq!(cluster_of(&b), joined);
    assert!(
        sys_row(&b, "sys_cluster", &joined).is_none(),
        "the founder's row arrives by sync, not from the joiner"
    );
    assert_eq!(
        audit_rows(&mut b, "cluster.created").len(),
        1,
        "no second founding"
    );
    let lines = fs::read_to_string(b.paths().app_log("_sys", b.id())).unwrap();
    let cluster_puts = lines
        .lines()
        .map(|line| serde_json::from_str::<Value>(line).unwrap())
        .filter(|line| line["tbl"] == "sys_cluster" && line["op"] == "put")
        .count();
    assert_eq!(
        cluster_puts, 1,
        "the founded cluster's row alone, later tombstoned"
    );
    assert_eq!(
        sys_row(&b, "sys_node", &id_b).unwrap()["cluster_id"],
        joined
    );
    // Discovery facts read the cluster ID from identity/ meanwhile.
    assert_eq!(b.discovery_facts().unwrap().cluster, joined);
}

/// `§2.3` — a join interrupted mid-swap completes at the next start: with the three
/// files staged, with one, two and all three already moved into place, the node opens
/// with the joined cluster and never the founded one; an uncommitted staging directory
/// is discarded and the founded cluster kept.
#[test]
fn test_spec_2_3_a_join_interrupted_mid_swap_completes_at_the_next_start() {
    let other = tempfile::tempdir().unwrap();
    let other_identity = other.path().join("identity");
    fs::create_dir_all(&other_identity).unwrap();
    let other_node = Identity::load_or_create_at(&other_identity, now()).unwrap();
    let joined_cluster = other_node.cluster_id().as_str().to_owned();
    let joined_key = fs::read(other_identity.join("cluster.key")).unwrap();
    let joined_pub = fs::read(other_identity.join("cluster.pub")).unwrap();

    for moved in 0..=3usize {
        let root = tempfile::tempdir().unwrap();
        let node = open(&root);
        let founded = cluster_of(&node);
        let cert = other_node
            .sign_certificate(&node.identity().verifying_key(), now())
            .unwrap();
        let id = node.id().as_str().to_owned();
        drop(node);
        let identity = root.path().join("identity");
        let staged = identity.join("adopt");
        fs::create_dir(&staged).unwrap();
        let files: [(&str, Vec<u8>); 3] = [
            ("node.cert", serde_json::to_vec(&cert).unwrap()),
            ("cluster.pub", joined_pub.clone()),
            ("cluster.key", joined_key.clone()),
        ];
        for (n, (name, bytes)) in files.iter().enumerate() {
            // The renames happen in this order; the first `moved` are already done.
            if n < moved {
                fs::write(identity.join(name), bytes).unwrap();
            } else {
                fs::write(staged.join(name), bytes).unwrap();
            }
        }
        let node = open(&root);
        assert_eq!(node.id().as_str(), id);
        assert_eq!(cluster_of(&node), joined_cluster, "moved {moved}");
        assert_ne!(cluster_of(&node), founded);
        assert!(!staged.exists(), "the staging directory is gone");
        assert_eq!(fs::read(identity.join("cluster.key")).unwrap(), joined_key);
        assert!(node.identity().certificate() == &cert);
        assert!(!node.identity().founded());
    }

    // Uncommitted: `adopt.tmp/` is discarded and the founded cluster stays.
    let root = tempfile::tempdir().unwrap();
    let node = open(&root);
    let founded = cluster_of(&node);
    drop(node);
    let staging = root.path().join("identity/adopt.tmp");
    fs::create_dir(&staging).unwrap();
    fs::write(staging.join("cluster.key"), &joined_key).unwrap();
    let node = open(&root);
    assert_eq!(cluster_of(&node), founded);
    assert!(!staging.exists());
}

// ---------------------------------------------------------------------------------------
// §2.3.1 — the expired state, re-admission, renewal at runtime
// ---------------------------------------------------------------------------------------

/// `§2.3.1` — an expired node starts for its owner alone: it opens, its standing says
/// expired, `cert.expired` is written once across restarts, it opens no window for
/// devices and advertises `pair = 0`, and it refuses to admit anyone.
#[test]
fn test_spec_2_3_1_an_expired_node_starts_for_its_owner_and_refuses_every_channel() {
    let root = tempfile::tempdir().unwrap();
    expire_certificate(&root, now(), 1);
    let mut node = open(&root);
    assert_eq!(node.standing(now()).unwrap(), Standing::Expired);
    assert!(node.identity().is_expired(now()));
    let expired = audit_rows(&mut node, "cert.expired");
    assert_eq!(expired.len(), 1);
    assert_eq!(expired[0]["severity"], "warn");
    assert_eq!(expired[0]["subject"], node.id().as_str());
    // The owner is served: a query, an append, the settings pages work as before.
    node.open_app("notes", "").unwrap();
    assert!(node.query("notes", "SELECT 1 AS one", &[]).is_ok());
    // No device window; a node window for re-admission, and `pair = 0` regardless.
    let refused = node.pair_at(Duration::from_secs(120), now()).unwrap_err();
    assert!(matches!(refused, Error::CertificateExpired), "{refused}");
    node.pair_node_at(Duration::from_secs(120), now()).unwrap();
    assert!(node.pairing_open(now()));
    assert!(!node.discovery_facts().unwrap().pair(now()));
    // An expired node never admits: a fresh node dialing it is refused by the rule.
    let fresh_root = tempfile::tempdir().unwrap();
    let mut fresh = open(&fresh_root);
    let refused = exchange(&mut node, &mut fresh, "http://192.0.2.1:8420", now()).unwrap_err();
    assert!(
        matches!(refused, Error::Pair(PairError::NoAdmitter(_))),
        "{refused}"
    );
    assert_ne!(cluster_of(&fresh), cluster_of(&node));
    drop(node);
    // Once, not once per start.
    let mut node = open(&root);
    assert_eq!(audit_rows(&mut node, "cert.expired").len(), 1);
    assert!(!node.identity().certificate().expires_at.is_empty());
}

/// `§2.3.1`, row 30 — an expired node is re-admitted with its own key and no new row,
/// from either side; an unexpired registered key and a revoked one are refused.
#[test]
fn test_spec_2_3_1_an_expired_node_is_readmitted_with_its_own_key_and_no_new_row() {
    let (root_a, root_b) = (tempfile::tempdir().unwrap(), tempfile::tempdir().unwrap());
    let mut a = open(&root_a);
    let mut b = open(&root_b);
    pair_a_browser(&mut a, "b3nn8t2q");
    admit(&mut a, &mut b, now()).unwrap();
    // B pairs a phone of its own, so it is not disposable — and is still re-admitted.
    pair_a_browser(&mut b, "c4mm9t3r");
    let row_before = sys_row(&a, "sys_device", b.id().as_str()).unwrap();
    let id_b = b.id().as_str().to_owned();
    // A's registry says B's certificate expired: B's own row says so, since that is
    // what B wrote at admission and A holds after sync — here, written as sync would.
    let expired_at = privatium_core::log::format_ts(now() - days(1));
    let mut node_row_b = sys_row(&b, "sys_node", &id_b).unwrap();
    node_row_b["cert_expires_at"] = Value::String(expired_at.clone());
    a.sys_log_mut().put(sys::NODE, &id_b, &node_row_b).unwrap();
    a.refresh().unwrap();
    drop(b);
    expire_certificate(&root_b, now(), 1);
    let mut b = open(&root_b);
    assert_eq!(b.standing(now()).unwrap(), Standing::Expired);
    let cert_before = b.identity().certificate().clone();

    // The expired node dials the member's window.
    let outcome = admit(&mut a, &mut b, now()).unwrap();
    assert!(outcome.client.joined);
    assert!(outcome.client.readmitted);
    assert_eq!(b.standing(now()).unwrap(), Standing::Member);
    assert_ne!(b.identity().certificate(), &cert_before);
    assert_eq!(cluster_of(&b), cluster_of(&a));
    assert_eq!(
        device_columns(&sys_row(&a, "sys_device", &id_b).unwrap()),
        device_columns(&row_before),
        "no new row: the same facts as before"
    );
    let admitted = audit_rows(&mut a, "node.admitted");
    assert_eq!(admitted.len(), 2);
    assert!(
        admitted[1]["detail"]
            .as_str()
            .unwrap()
            .contains("\"readmitted\":true")
    );
    assert_eq!(
        audit_rows(&mut a, "cert.expired").len(),
        0,
        "the peer's expiry is B's own row"
    );

    // The other way round: the member dials the expired node's window.
    a.sys_log_mut().put(sys::NODE, &id_b, &node_row_b).unwrap();
    a.refresh().unwrap();
    drop(b);
    expire_certificate(&root_b, now(), 1);
    let mut b = open(&root_b);
    assert_eq!(b.standing(now()).unwrap(), Standing::Expired);
    let outcome = admit(&mut b, &mut a, now()).unwrap();
    assert!(!outcome.client.joined, "the member admitted");
    assert!(outcome.node_joined && outcome.node_readmitted);
    assert_eq!(b.standing(now()).unwrap(), Standing::Member);
    assert_eq!(audit_rows(&mut a, "node.admitted").len(), 3);

    // A registered key whose certificate has not expired is refused; so is a revoked
    // one, even when its certificate has expired.
    let mut c = open(&tempfile::tempdir().unwrap());
    let refused = admit(&mut a, &mut b, now()).unwrap_err();
    assert!(
        matches!(refused, Error::Pair(PairError::TwoClusters)),
        "two members: {refused}"
    );
    let _ = &mut c;
    a.revoke_node(&id_b, Some("lost"), now()).unwrap();
    a.sys_log_mut().put(sys::NODE, &id_b, &node_row_b).unwrap();
    a.refresh().unwrap();
    drop(b);
    expire_certificate(&root_b, now(), 1);
    let mut b = open(&root_b);
    let refused = admit(&mut a, &mut b, now()).unwrap_err();
    assert!(
        matches!(refused, Error::Pair(PairError::Revoked)),
        "{refused}"
    );
    assert_eq!(b.standing(now()).unwrap(), Standing::Expired);
}

/// `§2.3.1` — the certificate renews at runtime under ninety days and never at expiry:
/// a fake clock at 91 days sees no renewal, at 89 days a renewal with the `sys_node`
/// amendment and `cert.renewed`, at 180 days a refusal.
#[test]
fn test_spec_2_3_1_certificate_renews_at_runtime_under_ninety_days_and_never_at_expiry() {
    let root = tempfile::tempdir().unwrap();
    let mut node = open(&root);
    let issued: jiff::Timestamp = node.identity().certificate().issued_at.parse().unwrap();
    let before = node.identity().certificate().clone();
    let renewed_before = audit_rows(&mut node, "cert.renewed").len();
    assert!(!node.renew_certificate_if_due(issued + days(89)).unwrap());
    assert_eq!(node.identity().certificate(), &before);
    assert!(node.renew_certificate_if_due(issued + days(91)).unwrap());
    let after = node.identity().certificate().clone();
    assert_ne!(after, before);
    assert_eq!(
        after.issued_at,
        privatium_core::log::format_ts(issued + days(91))
    );
    assert_eq!(
        sys_row(&node, "sys_node", node.id().as_str()).unwrap()["cert"],
        after.to_base64().unwrap()
    );
    assert_eq!(
        audit_rows(&mut node, "cert.renewed").len(),
        renewed_before + 1
    );
    assert_eq!(
        serde_json::from_slice::<privatium_core::identity::Certificate>(
            &fs::read(node.paths().identity_dir().join("node.cert")).unwrap()
        )
        .unwrap(),
        after
    );
    let refused = node
        .renew_certificate_if_due(issued + days(91 + 180))
        .unwrap_err();
    assert!(
        matches!(
            refused,
            Error::Certificate(privatium_core::identity::CertificateError::Expired)
        ),
        "{refused}"
    );
    assert_eq!(node.identity().certificate(), &after);
}

// ---------------------------------------------------------------------------------------
// §2.3.4 — revocation of a node
// ---------------------------------------------------------------------------------------

/// `§2.3.4`, `spec/data-dictionary.md §3.1c` — revoking a node writes its device row
/// revoked, the revocation row and `node.revoked` (alert); the channel's lookup refuses
/// it; a device row that is not a node's is refused by name; twice writes nothing new.
#[test]
fn test_spec_2_3_4_revoking_a_node_writes_the_revocation_row_and_closes_its_channel() {
    let (root_a, root_b) = (tempfile::tempdir().unwrap(), tempfile::tempdir().unwrap());
    let mut a = open(&root_a);
    let mut b = open(&root_b);
    pair_a_browser(&mut a, "b3nn8t2q");
    admit(&mut a, &mut b, now()).unwrap();
    let id_b = b.id().as_str().to_owned();
    assert!(a.device_is_node(&id_b).unwrap());
    assert!(!a.device_is_node("b3nn8t2q").unwrap());
    let when = now();
    a.revoke_node(&id_b, Some("laptop sold"), when).unwrap();
    assert!(a.node_revoked(&id_b).unwrap());
    let revocation = sys_row(&a, "sys_node_revocation", &id_b).unwrap();
    assert_eq!(revocation["revoked_by"], a.id().as_str());
    assert_eq!(revocation["reason"], "laptop sold");
    assert_eq!(
        revocation["revoked_at"],
        privatium_core::log::format_ts(when)
    );
    let row = sys_row(&a, "sys_device", &id_b).unwrap();
    assert!(row["revoked_at"].is_string());
    let alerts = audit_rows(&mut a, "node.revoked");
    assert_eq!(alerts.len(), 1);
    assert_eq!(alerts[0]["severity"], "alert");
    assert_eq!(alerts[0]["subject"], id_b);
    assert_eq!(a.paired_node_count().unwrap(), 0);
    // Twice: nothing new.
    a.revoke_node(&id_b, None, when).unwrap();
    assert_eq!(audit_rows(&mut a, "node.revoked").len(), 1);
    // A browser is not a node.
    let refused = a.revoke_node("b3nn8t2q", None, when).unwrap_err();
    assert!(matches!(refused, Error::DeviceUnknown { .. }), "{refused}");
    assert!(sys_row(&a, "sys_node_revocation", "b3nn8t2q").is_none());
    // Its channel: the handshake's lookup answers revoked (the live close is the
    // broadcast the devices page sends, held by the channel tests).
    use privatium_core::session::handshake::Handshake;
    let refused = Handshake::node(
        a.identity(),
        |dev| {
            assert_eq!(dev, id_b);
            Some(privatium_core::session::handshake::DevicePins {
                x25519: Some(b.identity().x25519_public_base64()),
                revoked: a.node_revoked(dev).unwrap(),
            })
        },
        &format!(
            "{{\"v\":1,\"dev\":\"{id_b}\",\"e\":\"{}\"}}",
            STANDARD.encode([9; 32])
        ),
    );
    assert!(refused.is_err());
}

/// `§2.3.4` — a node refuses a peer named in its revocation table: on the client side
/// of a join, before anything is sent; on the node side, at the registry check.
#[test]
fn test_spec_2_3_4_a_node_refuses_a_peer_named_in_its_revocation_table() {
    let (root_a, root_b) = (tempfile::tempdir().unwrap(), tempfile::tempdir().unwrap());
    let mut a = open(&root_a);
    let mut b = open(&root_b);
    pair_a_browser(&mut a, "b3nn8t2q");
    admit(&mut a, &mut b, now()).unwrap();
    let id_a = a.id().as_str().to_owned();
    // B learns that A is revoked — by sync in the ordinary course; written here.
    b.sys_log_mut()
        .put(
            sys::REVOCATION,
            &id_a,
            &json!({ "revoked_at": "2026-09-07T00:00:00.000Z", "revoked_by": "as3nn9tm" }),
        )
        .unwrap();
    b.refresh().unwrap();
    assert!(b.node_revoked(&id_a).unwrap());
    // As the client: B dialing A refuses at A's hello, before its first message.
    a.pair_node_at(Duration::from_secs(120), now()).unwrap();
    let hello = a.pairing_hello(now());
    let refused = b
        .join_start_at("http://192.0.2.1:8420", code_of(&a), &hello, now())
        .unwrap_err();
    assert!(matches!(refused, Error::PeerRevoked { .. }), "{refused}");
    assert_eq!(a.refresh_pairing(now()).unwrap().unwrap().attempts, 0);
    a.close_pairing(now()).unwrap();
    // As the node: A dialing B's window is refused at the registry check, audited.
    pair_a_browser(&mut b, "c4mm9t3r");
    let refused = admit(&mut b, &mut a, now()).unwrap_err();
    assert!(
        matches!(
            refused,
            Error::Pair(PairError::TwoClusters) | Error::Pair(PairError::Revoked)
        ),
        "{refused}"
    );
}

/// `§2.3.4` — a node that finds its own ID in `sys_node_revocation` stops: it opens
/// for its owner, its standing says revoked, it tells the owner once, it opens no
/// window of either kind and joins nothing.
#[test]
fn test_spec_2_3_4_a_node_that_finds_itself_revoked_stops_and_tells_the_owner() {
    let root = tempfile::tempdir().unwrap();
    let mut node = open(&root);
    let id = node.id().as_str().to_owned();
    // The revocation reaches the node's log — by sync, or a restore; written here.
    node.sys_log_mut()
        .put(
            sys::REVOCATION,
            &id,
            &json!({ "revoked_at": "2026-09-07T00:00:00.000Z", "revoked_by": "as3nn9tm" }),
        )
        .unwrap();
    drop(node);
    let mut node = open(&root);
    assert_eq!(node.standing(now()).unwrap(), Standing::Revoked);
    let told = audit_rows(&mut node, "node.revoked");
    assert_eq!(told.len(), 1);
    assert_eq!(told[0]["severity"], "alert");
    assert!(
        told[0]["detail"]
            .as_str()
            .unwrap()
            .contains("\"self\":true")
    );
    assert!(matches!(
        node.pair_at(Duration::from_secs(120), now()).unwrap_err(),
        Error::NodeRevoked
    ));
    assert!(matches!(
        node.pair_node_at(Duration::from_secs(120), now())
            .unwrap_err(),
        Error::NodeRevoked
    ));
    assert!(!node.discovery_facts().unwrap().pair(now()));
    let hello = format!(
        "{{\"v\":1,\"id\":\"as3nn9tm\",\"pub\":\"{}\",\"open\":true}}",
        STANDARD.encode(
            ed25519_dalek::SigningKey::from_bytes(&[2; 32])
                .verifying_key()
                .as_bytes()
        )
    );
    let refused = node
        .join_start_at(
            "http://192.0.2.1:8420",
            Code::parse(&format!(
                "{} {}",
                privatium_core::pair::WORDS[0],
                privatium_core::pair::WORDS[1]
            ))
            .unwrap(),
            &hello,
            now(),
        )
        .unwrap_err();
    assert!(matches!(refused, Error::NodeRevoked), "{refused}");
    drop(node);
    let mut node = open(&root);
    assert_eq!(audit_rows(&mut node, "node.revoked").len(), 1, "told once");
}

/// `§7.4.2`, `§2.3.4` — a revoked node's key cannot be admitted again: fresh or expired,
/// dialing or dialed.
#[test]
fn test_spec_7_4_2_a_revoked_nodes_key_cannot_be_admitted_again() {
    let (root_a, root_b) = (tempfile::tempdir().unwrap(), tempfile::tempdir().unwrap());
    let mut a = open(&root_a);
    let mut b = open(&root_b);
    pair_a_browser(&mut a, "b3nn8t2q");
    admit(&mut a, &mut b, now()).unwrap();
    let id_b = b.id().as_str().to_owned();
    a.revoke_node(&id_b, None, now()).unwrap();
    // B re-founds — a fresh cluster, disposable — and tries again with the same key.
    drop(b);
    for file in ["cluster.key", "cluster.pub", "node.cert"] {
        fs::remove_file(root_b.path().join("identity").join(file)).unwrap();
    }
    let mut b = open(&root_b);
    assert!(b.is_disposable().unwrap());
    let refused = admit(&mut a, &mut b, now()).unwrap_err();
    assert!(
        matches!(refused, Error::Pair(PairError::Revoked)),
        "{refused}"
    );
    let failed = audit_rows(&mut a, "pair.failed");
    assert!(
        failed.last().unwrap()["detail"]
            .as_str()
            .unwrap()
            .contains("revoked")
    );
    assert_ne!(cluster_of(&b), cluster_of(&a));
    // Dialed by A, B's window: A is the dialer and refuses at its own registry check.
    let refused = admit(&mut b, &mut a, now()).unwrap_err();
    assert!(refused.to_string().contains("revoked"), "{refused}");
    assert_ne!(cluster_of(&b), cluster_of(&a));
}

// ---------------------------------------------------------------------------------------
// §6.1 — the cluster filter
// ---------------------------------------------------------------------------------------

/// `§6.1` — peers are the cluster's nodes and strangers are kept apart, by ID: a
/// stranger's record with another `cl`, an invalid `cl` (never kept), this node's own
/// registration, and an empty table.
#[test]
fn test_spec_6_1_peers_are_the_clusters_nodes_and_strangers_are_kept_apart_by_id() {
    let root = tempfile::tempdir().unwrap();
    let mut node = open(&root);
    assert!(node.peers().is_empty() && node.strangers().is_empty());
    assert!(!node.absorb_discovered(record("z9z9z9z9", "q4w8rt2n")));
    for key in ["discovery.mdns", "discovery.udp"] {
        node.sys_log_mut()
            .put(
                sys::SETTING,
                key,
                &json!({ "value": "false", "updated_at": privatium_core::log::now() }),
            )
            .unwrap();
    }
    node.refresh().unwrap();
    node.serve_discovery().unwrap();
    assert!(
        node.peers().is_empty() && node.strangers().is_empty(),
        "empty table"
    );
    let own = cluster_of(&node);
    assert!(node.absorb_discovered(record("k7m2q9xf", &own)));
    assert!(node.absorb_discovered(record("b3nn8t2q", &own)));
    assert!(node.absorb_discovered(record("z9z9z9z9", "q4w8rt2n")));
    assert!(node.absorb_discovered(record(node.id().as_str(), &own)));
    let peers = node.peers();
    assert_eq!(
        peers.iter().map(|p| p.id.as_str()).collect::<Vec<_>>(),
        ["b3nn8t2q", "k7m2q9xf"]
    );
    let strangers = node.strangers();
    assert_eq!(
        strangers.iter().map(|p| p.id.as_str()).collect::<Vec<_>>(),
        ["z9z9z9z9"]
    );
    assert_eq!(
        node.discovered().len(),
        4,
        "the table itself keeps every record"
    );
    // A record whose `cl` is not shaped as an ID is not a `pv/1` record and is refused
    // off the wire (`§6.1`), so it can never be a stranger either.
    let txt: std::collections::BTreeMap<String, String> = [
        ("v", "1"),
        ("id", "c4mm9t3r"),
        ("cl", "not-an-id"),
        ("p", "8420"),
    ]
    .into_iter()
    .map(|(k, v)| (k.to_owned(), v.to_owned()))
    .collect();
    assert!(
        privatium_core::discover::txt::read(
            &txt,
            vec![source(9)],
            8420,
            "x._privatium._tcp.local.",
            now()
        )
        .is_none()
    );
}

fn record(id: &str, cluster: &str) -> Discovered {
    Discovered {
        id: id.to_owned(),
        cluster: cluster.to_owned(),
        name: id.to_owned(),
        addrs: vec![source(9)],
        port: 8420,
        apps: vec![],
        pair: false,
        seen_at: now(),
        instance: format!("{id}._privatium._tcp.local."),
    }
}
