// Project:  Privatium™  |  File: crates/privatium-core/tests/identity.rs
// Authors:  Gabriel Mongefranco (@gabrielmongefranco)
// Created:  2026-09-01  |  Modified: 2026-09-01
// Summary:  Node identity against spec/protocol.md §2.1 — the derivation, the file mode,
//           and the property that matters more than either: opening the same data root
//           twice yields the same node.

// AGENTS.md, Style: unwrap() is permitted in tests, and a test that hides a failure
// behind `?` is worse than one that panics with a line number.
#![allow(clippy::unwrap_used, clippy::expect_used)]

use std::fs;
use std::path::{Path, PathBuf};

use privatium_core::{Node, NodeId};

/// The checked-in keypair, whose bytes are `0x00..0x1f` — synthetic on purpose, so nobody
/// mistakes it for key material (`AGENTS.md`, Security expectations).
///
/// `.gitattributes` marks `*.key` as `-text` so git never rewrites a byte of it, and
/// `.gitignore` re-includes `crates/*/tests/**/*.key` from the blanket `*.key` rule. Both
/// are load-bearing for this file: without them the fixture would be absent or mangled and
/// the test below would look flaky rather than broken.
fn fixture_key() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/identity/node.key")
}

/// Install the fixture keypair into a fresh data root, so the node derives a known ID.
fn root_with_fixture_key() -> tempfile::TempDir {
    let root = tempfile::tempdir().unwrap();
    let identity = root.path().join("identity");
    fs::create_dir_all(&identity).unwrap();
    fs::copy(fixture_key(), identity.join("node.key")).unwrap();
    root
}

/// `spec/protocol.md §2.1` — the Node ID is the first 40 bits of `SHA-256(public_key)` as
/// 8 lowercase Crockford Base32 characters.
///
/// The expectation is a literal rather than a recomputation. A test that derives the value
/// the same way the code does proves only that the code is self-consistent; this one fails
/// if the derivation ever changes, which is the whole point — a node's ID is permanent and
/// is the filename of its log.
#[test]
fn test_spec_2_1_node_id_derivation() {
    let root = root_with_fixture_key();
    let node = Node::open(root.path()).unwrap();

    assert_eq!(node.id().as_str(), "as3nn9tm");
}

/// The alphabet is Crockford's, which excludes `i`, `l`, `o`, and `u`, and the ID is
/// exactly 8 characters — 40 bits at 5 bits each, with no padding.
#[test]
fn test_spec_2_1_node_id_is_crockford_lower() {
    let root = tempfile::tempdir().unwrap();
    let node = Node::open(root.path()).unwrap();
    let id = node.id().as_str();

    assert_eq!(id.len(), 8, "{id}");
    for character in id.chars() {
        assert!(
            character.is_ascii_digit() || character.is_ascii_lowercase(),
            "{id}: {character} is not lowercase Crockford Base32"
        );
        assert!(
            !"ilou".contains(character),
            "{id}: {character} is not in the alphabet"
        );
    }
}

/// `spec/protocol.md §2.1` — `identity/node.key` is mode `0600`.
///
/// Unix only. Windows has no mode; the file inherits its parent's ACL, which under
/// `%LOCALAPPDATA%` is already restricted to the owning user.
#[cfg(unix)]
#[test]
fn test_spec_2_1_key_mode_0600() {
    use std::os::unix::fs::PermissionsExt as _;

    let root = tempfile::tempdir().unwrap();
    let node = Node::open(root.path()).unwrap();

    let mode = fs::metadata(node.paths().node_key())
        .unwrap()
        .permissions()
        .mode()
        & 0o777;
    assert_eq!(mode, 0o600, "node.key is {mode:o}, not 600");

    // The directory holding it, too. Not pinned by §2.1, but a 0600 key inside a
    // world-readable directory still leaks its existence and its mtime.
    let dir = fs::metadata(node.paths().identity_dir())
        .unwrap()
        .permissions()
        .mode()
        & 0o777;
    assert_eq!(dir, 0o700, "identity/ is {dir:o}, not 700");
}

/// Opening the same data root twice is the same node, and the second open writes nothing.
///
/// Two separate properties, and the second is the one that would break quietly: an
/// identity that reloaded correctly but re-ran the `_sys` bootstrap would duplicate the
/// node's own device row on every start.
#[test]
fn test_identity_second_run_is_stable() {
    let root = tempfile::tempdir().unwrap();

    let first = Node::open(root.path()).unwrap();
    let id = first.id().clone();
    let log = first.paths().app_log("_sys", &id);
    let after_first = fs::read(&log).unwrap();
    drop(first);

    let second = Node::open(root.path()).unwrap();

    assert_eq!(
        second.id(),
        &id,
        "the node changed identity across a restart"
    );
    assert_eq!(
        fs::read(&log).unwrap(),
        after_first,
        "the second open appended to _sys; bootstrap is not idempotent"
    );
}

/// A `node.pub` deleted by hand is rewritten, and the ID does not move.
///
/// The public key is derivable from the private one, so treating its absence as an error
/// would turn a recoverable state into a dead node.
#[test]
fn test_public_key_file_is_rebuilt_from_the_private_key() {
    let root = root_with_fixture_key();

    let first = Node::open(root.path()).unwrap();
    let public = first.paths().node_pub();
    let bytes = fs::read(&public).unwrap();
    fs::remove_file(&public).unwrap();
    drop(first);

    let second = Node::open(root.path()).unwrap();

    assert_eq!(second.id().as_str(), "as3nn9tm");
    assert_eq!(fs::read(&public).unwrap(), bytes);
    assert_eq!(
        bytes.len(),
        32,
        "node.pub holds the raw key, not an encoding of it"
    );
}

/// A truncated or overwritten `node.key` fails loudly and names the file.
///
/// The alternative — deriving an ID from whatever bytes are there — would silently give
/// the node a new identity and orphan its own log.
#[test]
fn test_a_malformed_key_is_an_error_not_a_new_identity() {
    let root = tempfile::tempdir().unwrap();
    let identity = root.path().join("identity");
    fs::create_dir_all(&identity).unwrap();
    fs::write(identity.join("node.key"), b"too short").unwrap();

    let error = Node::open(root.path()).unwrap_err();
    let message = error.to_string();

    assert!(message.contains("node.key"), "{message}");
    assert!(message.contains("found 9 bytes"), "{message}");
}

/// Two different keys give two different IDs. Trivially true unless the derivation
/// ignores its input, which is exactly the bug a fixed-fixture test cannot catch.
#[test]
fn test_distinct_keys_give_distinct_ids() {
    let first = tempfile::tempdir().unwrap();
    let second = tempfile::tempdir().unwrap();

    let one = Node::open(first.path()).unwrap();
    let two = Node::open(second.path()).unwrap();

    assert_ne!(one.id(), two.id());
}

/// `NodeId::derive` is pure: the same public key, the same ID, no filesystem involved.
#[test]
fn test_derivation_is_pure() {
    let root = root_with_fixture_key();
    let node = Node::open(root.path()).unwrap();

    let again = NodeId::derive(&node.identity().verifying_key());
    assert_eq!(&again, node.id());
}
