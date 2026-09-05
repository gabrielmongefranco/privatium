// Project:  Privatium™  |  File: crates/privatium-core/tests/identity.rs
// Authors:  Gabriel Mongefranco (@gabrielmongefranco)
// Created:  2026-09-01  |  Modified: 2026-09-05
// Summary:  Node and cluster identities, certificate validation and renewal, secret
//           exclusion, and identity selection after restore (spec/protocol.md §2).

// AGENTS.md, Style: unwrap() is permitted in tests, and a test that hides a failure
// behind `?` is worse than one that panics with a line number.
#![allow(clippy::unwrap_used, clippy::expect_used)]

use std::fs;
use std::path::{Path, PathBuf};

use base64::{Engine as _, engine::general_purpose::STANDARD};
use privatium_core::identity::{Certificate, CertificateError, Identity};
use privatium_core::{Node, NodeId};

fn instant() -> jiff::Timestamp {
    "2026-09-04T12:00:00.000Z".parse().unwrap()
}

fn days(n: i64) -> jiff::SignedDuration {
    jiff::SignedDuration::from_secs(n * 86_400)
}

fn events(node: &Node) -> Vec<serde_json::Value> {
    fs::read_to_string(node.paths().app_log("_sys", node.id()))
        .unwrap()
        .lines()
        .map(|line| serde_json::from_str(line).unwrap())
        .collect()
}

#[test]
fn test_spec_2_3_first_start_founds_a_cluster() {
    let root = tempfile::tempdir().unwrap();
    let node = Node::open(root.path()).unwrap();
    let identity = node.identity();
    assert_ne!(identity.cluster_id().as_str(), node.id().as_str());
    assert_eq!(
        fs::read(node.paths().identity_dir().join("cluster.pub")).unwrap(),
        identity.cluster_public().as_bytes()
    );
    let lines = events(&node);
    assert_eq!(lines.len(), 4);
    assert_eq!(lines[0]["batch"], 3);
    assert_eq!(lines[2]["tbl"], "sys_cluster");
    assert_eq!(lines[2]["id"], identity.cluster_id().as_str());
    assert_eq!(lines[3]["d"]["kind"], "cluster.created");
    assert_eq!(lines[3]["d"]["severity"], "info");
    let cert: String = node
        .store()
        .conn()
        .query_row("SELECT cert FROM sys_node", [], |r| r.get(0))
        .unwrap();
    assert_eq!(
        Certificate::from_base64(&cert).unwrap(),
        *identity.certificate()
    );
    assert_eq!(lines[0]["d"]["ed25519_pub"], identity.public_key_base64());
    assert_eq!(lines[0]["d"]["x25519_pub"], identity.x25519_public_base64());
}

#[test]
fn test_spec_2_3_a_phase_1_root_founds_a_cluster_on_its_next_start() {
    let root = root_with_fixture_key();
    let log_dir = root.path().join("data/_sys/log");
    fs::create_dir_all(&log_dir).unwrap();
    let original = concat!(
        "{\"seq\":1,\"lam\":1,\"ts\":\"2026-01-01T00:00:00.000Z\",\"dev\":\"as3nn9tm\",\"app\":\"_sys\",\"op\":\"put\",\"tbl\":\"sys_device\",\"id\":\"as3nn9tm\",\"batch\":2,\"d\":{\"kind\":\"node\",\"replica\":true,\"label\":\"Study\",\"future\":{\"x\":1}}}\n",
        "{\"seq\":2,\"lam\":2,\"ts\":\"2026-01-01T00:00:00.000Z\",\"dev\":\"as3nn9tm\",\"app\":\"_sys\",\"op\":\"put\",\"tbl\":\"sys_node\",\"id\":\"as3nn9tm\",\"d\":{\"display_name\":\"Study\",\"created_at\":\"2026-01-01T00:00:00.000Z\",\"protocol\":\"pv/1\",\"build\":\"custom\",\"future\":42}}\n"
    );
    let path = log_dir.join("as3nn9tm.jsonl");
    fs::write(&path, original).unwrap();
    let node = Node::open(root.path()).unwrap();
    assert_eq!(node.id().as_str(), "as3nn9tm");
    assert!(fs::read_to_string(&path).unwrap().starts_with(original));
    let lines = events(&node);
    assert_eq!(lines[2]["d"]["future"], serde_json::json!({"x": 1}));
    assert_eq!(lines[3]["d"]["future"], 42);
    assert_eq!(lines[3]["d"]["display_name"], "Study");
    assert_eq!(lines[3]["d"]["created_at"], "2026-01-01T00:00:00.000Z");
    let before = fs::read(&path).unwrap();
    drop(node);
    drop(Node::open(root.path()).unwrap());
    assert_eq!(fs::read(path).unwrap(), before);
}

#[test]
fn test_spec_2_3_1_certificate_verifies_against_the_cluster_key_and_expires_at_180_days() {
    let root = root_with_fixture_key();
    let identity = Identity::load_or_create_at(&root.path().join("identity"), instant()).unwrap();
    let cert = identity.certificate();
    cert.verify(&identity.cluster_public(), instant()).unwrap();
    assert_eq!(
        cert.expires_at.parse::<jiff::Timestamp>().unwrap(),
        instant() + days(180)
    );
    cert.verify(
        &identity.cluster_public(),
        instant() + days(180) - jiff::SignedDuration::from_millis(1),
    )
    .unwrap();
    assert!(matches!(
        cert.verify(&identity.cluster_public(), instant() + days(180)),
        Err(CertificateError::Expired)
    ));
    let wrong = ed25519_dalek::SigningKey::from_bytes(&[42; 32]).verifying_key();
    assert!(cert.verify(&wrong, instant()).is_err());
    let mut tampered = cert.clone();
    tampered.node_id = "00000000".into();
    assert!(
        tampered
            .verify(&identity.cluster_public(), instant())
            .is_err()
    );
    tampered = cert.clone();
    tampered.sig = STANDARD.encode([0; 64]);
    assert!(matches!(
        tampered.verify(&identity.cluster_public(), instant()),
        Err(CertificateError::Signature)
    ));
    for invalid in ["", "{}", "null", "not base64"] {
        assert!(Certificate::from_base64(invalid).is_err());
    }
}

#[test]
fn test_spec_2_3_1_certificate_signed_bytes_are_canonical() {
    let cert: Certificate =
        serde_json::from_str(include_str!("fixtures/identity/node.cert")).unwrap();
    let cluster = ed25519_dalek::SigningKey::from_bytes(&[42; 32]).verifying_key();
    cert.verify(&cluster, instant()).unwrap();
    assert_eq!(
        String::from_utf8(cert.signed_bytes().unwrap()).unwrap(),
        include_str!("fixtures/identity/certificate-message.txt").trim_end()
    );
}

#[test]
fn test_spec_2_3_1_certificate_renews_under_ninety_days() {
    let root = root_with_fixture_key();
    let dir = root.path().join("identity");
    let first = Identity::load_or_create_at(&dir, instant()).unwrap();
    let cert = first.certificate().clone();
    let at_boundary = Identity::load_or_create_at(&dir, instant() + days(90)).unwrap();
    assert_eq!(at_boundary.certificate(), &cert);
    let renewed = Identity::load_or_create_at(
        &dir,
        instant() + days(90) + jiff::SignedDuration::from_millis(1),
    )
    .unwrap();
    assert_ne!(renewed.certificate(), &cert);
    assert_eq!(renewed.cluster_id(), first.cluster_id());
    assert_eq!(renewed.id(), first.id());
    fs::write(dir.join("node.cert"), serde_json::to_vec(&cert).unwrap()).unwrap();
    assert!(Identity::load_or_create_at(&dir, instant() + days(180)).is_err());
    assert_eq!(
        serde_json::from_slice::<Certificate>(&fs::read(dir.join("node.cert")).unwrap()).unwrap(),
        cert
    );
}

#[test]
fn test_spec_2_3_1_startup_renewal_is_audited_once() {
    let root = tempfile::tempdir().unwrap();
    let node = Node::open(root.path()).unwrap();
    let old = node
        .identity()
        .sign_certificate(
            &node.identity().verifying_key(),
            jiff::Timestamp::now() - days(91),
        )
        .unwrap();
    fs::write(
        node.paths().identity_dir().join("node.cert"),
        serde_json::to_vec(&old).unwrap(),
    )
    .unwrap();
    drop(node);
    let node = Node::open(root.path()).unwrap();
    assert_eq!(
        events(&node)
            .iter()
            .filter(|e| e["d"]["kind"] == "cert.renewed")
            .count(),
        1
    );
    let before = events(&node);
    drop(node);
    let node = Node::open(root.path()).unwrap();
    assert_eq!(events(&node), before);
}

#[test]
fn test_spec_2_1_x25519_static_is_derived_and_stable() {
    let root = root_with_fixture_key();
    let node = Node::open(root.path()).unwrap();
    let secret = node.identity().x25519_static();
    let mut expected = [0; 32];
    hkdf::Hkdf::<sha2::Sha256>::new(None, &fs::read(fixture_key()).unwrap())
        .expand(b"privatium/x25519/v1", &mut expected)
        .unwrap();
    assert_eq!(secret.as_bytes(), &expected);
    assert_ne!(secret.as_bytes(), node.identity().csrf_key().as_bytes());
    let public = node.identity().x25519_public_base64();
    drop(node);
    let node = Node::open(root.path()).unwrap();
    assert_eq!(node.identity().x25519_public_base64(), public);
    assert_eq!(
        fs::read_dir(node.paths().identity_dir()).unwrap().count(),
        5
    );
}

#[test]
fn test_spec_3_1b_pkarr_name_is_zbase32_of_the_cluster_public_key() {
    let root = root_with_fixture_key();
    fs::write(root.path().join("identity/cluster.key"), [42; 32]).unwrap();
    let node = Node::open(root.path()).unwrap();
    let name: String = node
        .store()
        .conn()
        .query_row("SELECT pkarr_name FROM sys_cluster", [], |r| r.get(0))
        .unwrap();
    assert_eq!(
        name,
        include_str!("fixtures/identity/pkarr-name.txt").trim_end()
    );
}

#[test]
fn test_identity_second_run_keeps_the_cluster_and_the_node_id() {
    let root = tempfile::tempdir().unwrap();
    let node = Node::open(root.path()).unwrap();
    let id = node.id().clone();
    let cluster = node.identity().cluster_id().clone();
    let cert = node.identity().certificate().clone();
    let before = events(&node);
    drop(node);
    let node = Node::open(root.path()).unwrap();
    assert_eq!(node.id(), &id);
    assert_eq!(node.identity().cluster_id(), &cluster);
    assert_eq!(node.identity().certificate(), &cert);
    assert_eq!(events(&node), before);
}

#[test]
fn test_spec_2_3_invalid_cluster_material_is_refused_without_replacement() {
    for bytes in [Vec::new(), vec![1; 31], vec![1; 33]] {
        let root = root_with_fixture_key();
        let key = root.path().join("identity/cluster.key");
        fs::write(&key, &bytes).unwrap();
        assert!(Node::open(root.path()).is_err());
        assert_eq!(fs::read(key).unwrap(), bytes);
    }
    let root = tempfile::tempdir().unwrap();
    let node = Node::open(root.path()).unwrap();
    let dir = node.paths().identity_dir();
    let log = node.paths().app_log("_sys", node.id());
    let before = fs::read(&log).unwrap();
    drop(node);
    fs::write(dir.join("node.cert"), b"{}").unwrap();
    assert!(Node::open(root.path()).is_err());
    assert_eq!(fs::read(log).unwrap(), before);
    assert_eq!(fs::read(dir.join("node.cert")).unwrap(), b"{}");
}

#[test]
fn test_spec_2_3_1_signed_invalid_certificate_fields_are_refused() {
    use ed25519_dalek::Signer as _;
    let original: Certificate =
        serde_json::from_str(include_str!("fixtures/identity/node.cert")).unwrap();
    let key = ed25519_dalek::SigningKey::from_bytes(&[42; 32]);
    for field in [
        "node_id",
        "cluster_id",
        "node_pub",
        "issued_at",
        "expires_at",
    ] {
        let mut value = serde_json::to_value(&original).unwrap();
        value[field] = "invalid".into();
        let mut cert: Certificate = serde_json::from_value(value).unwrap();
        cert.sig = STANDARD.encode(key.sign(&cert.signed_bytes().unwrap()).to_bytes());
        assert!(
            cert.verify(&key.verifying_key(), instant()).is_err(),
            "{field}"
        );
    }
    for expiry in [
        "2027-03-02T12:00:00.000Z",
        "2027-03-04T12:00:00.000Z",
        "2027-03-03T12:00:00Z",
    ] {
        let mut cert = original.clone();
        cert.expires_at = expiry.into();
        cert.sig = STANDARD.encode(key.sign(&cert.signed_bytes().unwrap()).to_bytes());
        assert!(cert.verify(&key.verifying_key(), instant()).is_err());
    }
    assert!(
        original
            .verify(&key.verifying_key(), instant() - days(1))
            .is_err()
    );
    assert!(Certificate::from_base64(&"A".repeat(8193)).is_err());
}

#[test]
fn test_spec_2_3_1_startup_refuses_another_nodes_certificate() {
    let root = tempfile::tempdir().unwrap();
    let node = Node::open(root.path()).unwrap();
    let other = ed25519_dalek::SigningKey::from_bytes(&[43; 32]).verifying_key();
    let cert = node
        .identity()
        .sign_certificate(&other, jiff::Timestamp::now())
        .unwrap();
    let path = node.paths().identity_dir().join("node.cert");
    let bytes = serde_json::to_vec(&cert).unwrap();
    fs::write(&path, &bytes).unwrap();
    drop(node);
    assert!(Node::open(root.path()).is_err());
    assert_eq!(fs::read(path).unwrap(), bytes);
}

#[test]
fn test_spec_2_3_missing_cluster_key_is_not_silently_replaced() {
    let root = tempfile::tempdir().unwrap();
    let node = Node::open(root.path()).unwrap();
    let key = node.paths().identity_dir().join("cluster.key");
    drop(node);
    fs::remove_file(&key).unwrap();
    assert!(Node::open(root.path()).is_err());
    assert!(!key.exists());
}

#[test]
fn test_spec_3_1b_data_only_restore_preserves_records_and_selects_local_identity() {
    let source = tempfile::tempdir().unwrap();
    let mut original = Node::open(source.path()).unwrap();
    let mut row = events(&original)[1]["d"].clone();
    row["display_name"] = serde_json::json!("Study");
    let original_id = original.id().clone();
    original
        .sys_log_mut()
        .put("sys_node", original_id.as_str(), &row)
        .unwrap();
    let original_log = original.paths().app_log("_sys", original.id());
    let before = fs::read(&original_log).unwrap();
    for started in [false, true] {
        let backup = tempfile::tempdir().unwrap();
        if started {
            drop(Node::open(backup.path()).unwrap());
        }
        let paths = privatium_core::Paths::rooted(backup.path());
        privatium_core::backup::Plan::build(source.path(), &paths, None)
            .unwrap()
            .apply()
            .unwrap();
        assert_eq!(paths.identity_dir().exists(), started);
        let restored = Node::open(backup.path()).unwrap();
        assert_ne!(restored.id(), original.id());
        assert_ne!(
            restored.identity().cluster_id(),
            original.identity().cluster_id()
        );
        assert_local_identity_rows(&restored);
        assert_eq!(
            privatium_core::http::api::display_name(&restored).unwrap(),
            None
        );
        let counts: (i64, i64) = restored
            .store()
            .conn()
            .query_row(
                "SELECT (SELECT count(*) FROM sys_node), (SELECT count(*) FROM sys_cluster)",
                [],
                |row| Ok((row.get(0)?, row.get(1)?)),
            )
            .unwrap();
        assert_eq!(counts, (2, 2));
        assert_eq!(
            fs::read(paths.app_log("_sys", original.id())).unwrap(),
            before
        );
        assert!(events(&restored).iter().all(|event| event["op"] != "del"));
        assert!(
            original
                .identity()
                .certificate()
                .verify(
                    &restored.identity().cluster_public(),
                    jiff::Timestamp::now(),
                )
                .is_err()
        );
        let id = restored.id().clone();
        let cluster = restored.identity().cluster_id().clone();
        let local_log = fs::read(paths.app_log("_sys", &id)).unwrap();
        drop(restored);
        let restored = Node::open(backup.path()).unwrap();
        assert_eq!(restored.id(), &id);
        assert_eq!(restored.identity().cluster_id(), &cluster);
        assert_eq!(fs::read(paths.app_log("_sys", &id)).unwrap(), local_log);
        drop(restored);
        fs::remove_file(paths.app_cache_db("_sys")).unwrap();
        let restored = Node::open(backup.path()).unwrap();
        assert_local_identity_rows(&restored);
        assert_eq!(
            restored
                .store()
                .conn()
                .query_row("SELECT count(*) FROM sys_cluster", [], |row| row
                    .get::<_, i64>(0),)
                .unwrap(),
            2
        );
    }
    assert_eq!(fs::read(original_log).unwrap(), before);
}

fn assert_local_identity_rows(node: &Node) {
    let (pubkey, cluster, cert): (String, String, String) = node
        .store()
        .conn()
        .query_row(
            "SELECT pubkey, cluster_id, cert FROM sys_node WHERE id = ?",
            [node.id().as_str()],
            |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)),
        )
        .unwrap();
    assert_eq!(pubkey, node.identity().public_key_base64());
    assert_eq!(cluster, node.identity().cluster_id().as_str());
    assert_eq!(
        Certificate::from_base64(&cert).unwrap(),
        *node.identity().certificate()
    );
    let public: String = node
        .store()
        .conn()
        .query_row(
            "SELECT pubkey FROM sys_cluster WHERE id = ?",
            [cluster],
            |row| row.get(0),
        )
        .unwrap();
    assert_eq!(
        public,
        STANDARD.encode(node.identity().cluster_public().as_bytes())
    );
}

#[test]
fn test_spec_3_1b_restored_keys_select_the_original_cluster() {
    let source = tempfile::tempdir().unwrap();
    let original = Node::open(source.path()).unwrap();
    let id = original.id().clone();
    let cluster = original.identity().cluster_id().clone();
    let before = events(&original);
    drop(original);
    let target = tempfile::tempdir().unwrap();
    drop(Node::open(target.path()).unwrap());
    let paths = privatium_core::Paths::rooted(target.path());
    privatium_core::backup::Plan::build(source.path(), &paths, None)
        .unwrap()
        .apply()
        .unwrap();
    for file in [
        "node.key",
        "node.pub",
        "cluster.key",
        "cluster.pub",
        "node.cert",
    ] {
        fs::copy(
            source.path().join("identity").join(file),
            paths.identity_dir().join(file),
        )
        .unwrap();
    }
    let restored = Node::open(target.path()).unwrap();
    assert_eq!(restored.id(), &id);
    assert_eq!(restored.identity().cluster_id(), &cluster);
    assert_local_identity_rows(&restored);
    assert!(events(&restored).starts_with(&before));
    assert!(events(&restored).iter().all(|event| event["op"] != "del"));
    assert_eq!(
        restored
            .store()
            .conn()
            .query_row("SELECT count(*) FROM sys_cluster", [], |row| row
                .get::<_, i64>(0),)
            .unwrap(),
        2
    );
}

#[test]
fn test_spec_3_1b_replayed_rows_cannot_change_local_cluster_identity() {
    let root = tempfile::tempdir().unwrap();
    let mut node = Node::open(root.path()).unwrap();
    let id = node.id().clone();
    let cluster = node.identity().cluster_id().clone();
    let mut cluster_row = events(&node)[2]["d"].clone();
    cluster_row["pubkey"] = serde_json::json!(STANDARD.encode([0; 32]));
    cluster_row["pkarr_name"] = serde_json::json!("invalid");
    cluster_row["label"] = serde_json::json!("Study");
    cluster_row["future"] = serde_json::json!({"x": 1});
    let mut node_row = events(&node)[1]["d"].clone();
    node_row["cluster_id"] = serde_json::json!("00000000");
    node_row["display_name"] = serde_json::json!("Study");
    node.sys_log_mut()
        .put("sys_cluster", cluster.as_str(), &cluster_row)
        .unwrap();
    node.sys_log_mut()
        .put("sys_node", id.as_str(), &node_row)
        .unwrap();
    let before = fs::read(node.paths().app_log("_sys", &id)).unwrap();
    drop(node);
    let node = Node::open(root.path()).unwrap();
    assert_eq!(node.identity().cluster_id(), &cluster);
    assert_local_identity_rows(&node);
    assert_eq!(
        privatium_core::http::api::display_name(&node)
            .unwrap()
            .as_deref(),
        Some("Study")
    );
    assert!(
        fs::read(node.paths().app_log("_sys", &id))
            .unwrap()
            .starts_with(&before)
    );
    let repaired = events(&node)
        .into_iter()
        .rev()
        .find(|event| event["tbl"] == "sys_cluster")
        .unwrap();
    assert_eq!(repaired["d"]["created_at"], cluster_row["created_at"]);
    assert_eq!(repaired["d"]["created_by"], cluster_row["created_by"]);
    assert_eq!(repaired["d"]["label"], "Study");
    assert_eq!(repaired["d"]["future"], serde_json::json!({"x": 1}));
}

#[test]
fn test_spec_2_3_interrupted_founding_keeps_the_cluster_key() {
    let root = root_with_fixture_key();
    let dir = root.path().join("identity");
    fs::write(dir.join("cluster.key"), [42; 32]).unwrap();
    let node = Node::open(root.path()).unwrap();
    assert_eq!(
        node.identity().cluster_public(),
        ed25519_dalek::SigningKey::from_bytes(&[42; 32]).verifying_key()
    );
    let cert = node.identity().certificate().clone();
    drop(node);
    fs::remove_file(dir.join("cluster.pub")).unwrap();
    let node = Node::open(root.path()).unwrap();
    assert_eq!(node.identity().certificate(), &cert);
    assert_eq!(fs::read(dir.join("cluster.key")).unwrap(), [42; 32]);
}

#[test]
fn test_spec_4_2_identity_amendment_preserves_unknown_json_bytes() {
    let root = tempfile::tempdir().unwrap();
    let mut node = Node::open(root.path()).unwrap();
    let id = node.id().as_str().to_owned();
    let raw = r#"{"future": {"n": 1.2300000000000000000000001e+40, "text":"\u0061"},"display_name":"Study"}"#;
    let row: &serde_json::value::RawValue = serde_json::from_str(raw).unwrap();
    node.sys_log_mut().put("sys_node", &id, &row).unwrap();
    let path = node.paths().app_log("_sys", node.id());
    let before = fs::read(&path).unwrap();
    drop(node);
    let node = Node::open(root.path()).unwrap();
    let after = fs::read(&path).unwrap();
    assert!(after.starts_with(&before));
    let appended = std::str::from_utf8(&after[before.len()..]).unwrap();
    assert!(
        appended.contains(r#""future":{"n": 1.2300000000000000000000001e+40, "text":"\u0061"}"#)
    );
    assert_eq!(events(&node).last().unwrap()["d"]["display_name"], "Study");
}

#[cfg(unix)]
#[test]
fn test_spec_2_3_cluster_key_mode_0600() {
    use std::os::unix::fs::PermissionsExt as _;
    let root = tempfile::tempdir().unwrap();
    let node = Node::open(root.path()).unwrap();
    assert_eq!(
        fs::metadata(node.paths().identity_dir().join("cluster.key"))
            .unwrap()
            .permissions()
            .mode()
            & 0o777,
        0o600
    );
}

#[test]
fn test_spec_2_3_3_cluster_private_key_is_absent_from_every_event_snapshot_and_backup() {
    fn check(path: &Path, secrets: &[Vec<u8>]) {
        for entry in fs::read_dir(path).unwrap() {
            let path = entry.unwrap().path();
            if path.is_dir() {
                check(&path, secrets);
            } else {
                let bytes = fs::read(path).unwrap();
                for secret in secrets {
                    assert!(!bytes.windows(secret.len()).any(|w| w == secret));
                }
            }
        }
    }
    let root = tempfile::tempdir().unwrap();
    let mut node = Node::open(root.path()).unwrap();
    node.snapshot("_sys").unwrap();
    let key = fs::read(node.paths().identity_dir().join("cluster.key")).unwrap();
    let secrets = [key.clone(), STANDARD.encode(&key).into_bytes()];
    check(&node.paths().data_dir(), &secrets);
    let target = tempfile::tempdir().unwrap();
    let paths = privatium_core::Paths::rooted(target.path());
    privatium_core::backup::Plan::build(root.path(), &paths, None)
        .unwrap()
        .apply()
        .unwrap();
    check(target.path(), &secrets);
    let debug = format!("{:?}", node.identity());
    assert!(!debug.contains(&STANDARD.encode(&key)));
    assert!(!debug.contains(&format!("{key:?}")));
}

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
