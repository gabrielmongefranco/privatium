// Project:  Privatium™  |  File: crates/privatium-core/tests/session.rs
// Authors:  Gabriel Mongefranco (@gabrielmongefranco)
// Created:  2026-09-05  |  Modified: 2026-09-05
// Summary:  Session key agreement, authenticated framing, and handshake refusals
//           against spec/protocol.md §8 and shared browser fixtures.

#![allow(clippy::unwrap_used, clippy::expect_used)]

use base64::{Engine as _, engine::general_purpose::STANDARD};
use privatium_core::Identity;
use privatium_core::session::handshake::{ClientHandshake, DevicePins, Handshake, NodePins};
use privatium_core::session::{Direction, Frame, Keys, Role, SessionError};
use serde_json::{Value, json};
use x25519_dalek::{PublicKey, StaticSecret};

fn keys(role: Role) -> Keys {
    let (mine, theirs, eph, other_eph) = match role {
        Role::Client => (1, 2, 3, 4),
        Role::Node => (2, 1, 4, 3),
    };
    Keys::derive(
        role,
        &StaticSecret::from([mine; 32]),
        &PublicKey::from(&StaticSecret::from([theirs; 32])),
        &StaticSecret::from([eph; 32]),
        &PublicKey::from(&StaticSecret::from([other_eph; 32])),
        "k7m2q9xf",
        "b3nn8t2q",
    )
    .unwrap()
}

#[test]
fn test_spec_8_key_schedule_matches_the_checked_in_vectors() {
    let fixture: Value = serde_json::from_str(
        &std::fs::read_to_string(concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/tests/fixtures/session-vectors.json"
        ))
        .unwrap(),
    )
    .unwrap();
    let keys = keys(Role::Client);
    for (direction, name) in [(Direction::C2s, "c2s"), (Direction::S2c, "s2c")] {
        assert_eq!(STANDARD.encode(keys.key(direction)), fixture[name]["key"]);
        let mut frame = Frame::new(*keys.key(direction), direction);
        let mut receive = Frame::new(*keys.key(direction), direction);
        for entry in fixture[name]["frames"].as_array().unwrap() {
            let plain = STANDARD
                .decode(entry["plaintext"].as_str().unwrap())
                .unwrap();
            assert_eq!(
                STANDARD.encode(frame.seal(&plain).unwrap()),
                entry["ciphertext"]
            );
            assert_eq!(
                receive
                    .open(
                        &STANDARD
                            .decode(entry["ciphertext"].as_str().unwrap())
                            .unwrap()
                    )
                    .unwrap(),
                plain
            );
        }
    }
}

#[test]
fn test_spec_8_frames_round_trip_and_the_counter_never_repeats() {
    let (mut send, _) = keys(Role::Client).into_frames();
    let (_, mut receive) = keys(Role::Node).into_frames();
    let first = send.seal(b"").unwrap();
    let second = send.seal(b"").unwrap();
    assert_ne!(first, second);
    assert_eq!(receive.open(&first).unwrap(), b"");
    assert_eq!(receive.open(&second).unwrap(), b"");
    assert!(receive.open(&first).is_err());
    assert!(receive.open(&send.seal(b"later").unwrap()).is_err());
}

#[test]
fn test_spec_8_a_tampered_frame_is_refused() {
    for index in [0, 5, 19] {
        let (mut send, _) = keys(Role::Client).into_frames();
        let (_, mut receive) = keys(Role::Node).into_frames();
        let original = send.seal(b"synthetic message").unwrap();
        let mut bad = original.clone();
        bad[index] ^= 1;
        assert_eq!(
            receive.open(&bad).unwrap_err(),
            SessionError::Authentication
        );
        assert_eq!(receive.open(&original).unwrap_err(), SessionError::Closed);
    }
}

#[test]
fn test_spec_8_wrong_direction_truncated_and_out_of_order_frames_are_refused() {
    let (mut send, _) = keys(Role::Client).into_frames();
    let first = send.seal(b"synthetic").unwrap();
    let second = send.seal(b"synthetic").unwrap();
    for bad in [
        &first[..0],
        &first[..15],
        &first[..first.len() - 1],
        &second,
    ] {
        let (_, mut receive) = keys(Role::Node).into_frames();
        assert!(receive.open(bad).is_err());
    }
    let mut wrong = Frame::new(*keys(Role::Client).key(Direction::C2s), Direction::S2c);
    assert!(wrong.open(&first).is_err());
}

fn setup() -> (tempfile::TempDir, Identity, NodePins, DevicePins) {
    let root = tempfile::tempdir().unwrap();
    let identity = Identity::load_or_create(root.path()).unwrap();
    let node_pins = NodePins {
        id: identity.id().to_string(),
        cluster: identity.cluster_public(),
        x25519: PublicKey::from(&identity.x25519_static()),
    };
    let device = DevicePins {
        x25519: Some(STANDARD.encode(PublicKey::from(&StaticSecret::from([1; 32])).as_bytes())),
        revoked: false,
    };
    (root, identity, node_pins, device)
}

#[test]
fn test_spec_8_handshake_derives_the_same_keys_on_both_sides() {
    let (_root, identity, pins, device) = setup();
    let (client, hello) =
        ClientHandshake::start("b3nn8t2q", StaticSecret::from([1; 32]), pins).unwrap();
    let (pending, node_hello) = Handshake::node(&identity, |_| Some(device), &hello).unwrap();
    let (mut client, confirm) = client.finish(&node_hello, jiff::Timestamp::now()).unwrap();
    let mut node = pending.confirm(&confirm).unwrap();
    assert_eq!(node.device, "b3nn8t2q");
    assert_eq!(
        node.receive
            .open(&client.send.seal(b"request").unwrap())
            .unwrap(),
        b"request"
    );
    assert_eq!(
        client
            .receive
            .open(&node.send.seal(b"response").unwrap())
            .unwrap(),
        b"response"
    );
}

#[test]
fn test_spec_8_1_a_static_key_that_is_not_the_pinned_one_fails_the_confirm() {
    let (_root, identity, mut pins, device) = setup();
    pins.x25519 = PublicKey::from(&StaticSecret::from([9; 32]));
    let (client, hello) =
        ClientHandshake::start("b3nn8t2q", StaticSecret::from([1; 32]), pins).unwrap();
    let (pending, node_hello) = Handshake::node(&identity, |_| Some(device), &hello).unwrap();
    let (_, confirm) = client.finish(&node_hello, jiff::Timestamp::now()).unwrap();
    assert!(matches!(
        pending.confirm(&confirm),
        Err(SessionError::Authentication)
    ));
}

#[test]
fn test_spec_8_3_unknown_revoked_and_missing_device_keys_are_refused() {
    let (_root, identity, pins, device) = setup();
    let (_, hello) = ClientHandshake::start("b3nn8t2q", StaticSecret::from([1; 32]), pins).unwrap();
    for entry in [
        None,
        Some(DevicePins {
            revoked: true,
            ..device
        }),
        Some(DevicePins {
            revoked: false,
            x25519: None,
        }),
        Some(DevicePins {
            revoked: false,
            x25519: Some("invalid".into()),
        }),
        Some(DevicePins {
            revoked: false,
            x25519: Some(STANDARD.encode([0; 32])),
        }),
    ] {
        let result = Handshake::node(&identity, |_| entry, &hello);
        assert!(result.is_err());
        assert_eq!(result.err().unwrap().close_code(), 4403);
    }
}

#[test]
fn test_spec_8_3_malformed_hello_and_wrong_version_are_refused() {
    let (_root, identity, _, device) = setup();
    for hello in [
        "".to_owned(),
        "{}".into(),
        "[]".into(),
        "x".repeat(8193),
        json!({"v":2,"dev":"b3nn8t2q","e":STANDARD.encode([3;32])}).to_string(),
        json!({"v":1,"dev":"../bad","e":STANDARD.encode([3;32])}).to_string(),
        json!({"v":1,"dev":"b3nn8t2q","e":STANDARD.encode([0;32])}).to_string(),
        "{\"v\":1,\"v\":1,\"dev\":\"b3nn8t2q\",\"e\":\"bad\"}".into(),
    ] {
        assert!(Handshake::node(&identity, |_| Some(device.clone()), &hello).is_err());
    }
}

#[test]
fn test_spec_8_3_confirm_binds_the_exact_hello_bytes() {
    let (_root, identity, pins, device) = setup();
    let (client, hello) =
        ClientHandshake::start("b3nn8t2q", StaticSecret::from([1; 32]), pins).unwrap();
    let (pending, node_hello) =
        Handshake::node(&identity, |_| Some(device), &(hello + " ")).unwrap();
    let (_, confirm) = client.finish(&node_hello, jiff::Timestamp::now()).unwrap();
    assert!(pending.confirm(&confirm).is_err());
}

#[test]
fn test_spec_8_1_certificate_mismatch_and_expiry_are_refused_before_confirm() {
    for mode in 0..3 {
        let (_root, identity, mut pins, device) = setup();
        if mode == 0 {
            pins.cluster = ed25519_dalek::SigningKey::from_bytes(&[9; 32]).verifying_key();
        }
        let (client, hello) =
            ClientHandshake::start("b3nn8t2q", StaticSecret::from([1; 32]), pins).unwrap();
        let (_, mut node_hello) = Handshake::node(&identity, |_| Some(device), &hello).unwrap();
        if mode == 1 {
            let mut value: Value = serde_json::from_str(&node_hello).unwrap();
            value["id"] = json!("00000000");
            node_hello = value.to_string();
        }
        let now = if mode == 2 {
            identity.certificate().expires_at.parse().unwrap()
        } else {
            jiff::Timestamp::now()
        };
        assert!(matches!(
            client.finish(&node_hello, now),
            Err(SessionError::PinnedKey)
        ));
    }
}

#[test]
fn test_spec_8_noncontributory_keys_and_invalid_ids_are_refused() {
    for id in ["", "../bad", "AAAAAAAA", "b3nn8t2q "] {
        assert!(
            Keys::derive(
                Role::Client,
                &StaticSecret::from([1; 32]),
                &PublicKey::from([2; 32]),
                &StaticSecret::from([3; 32]),
                &PublicKey::from([4; 32]),
                id,
                "b3nn8t2q"
            )
            .is_err()
        );
    }
    for (static_key, ephemeral) in [([0; 32], [4; 32]), ([2; 32], [0; 32])] {
        assert!(
            Keys::derive(
                Role::Client,
                &StaticSecret::from([1; 32]),
                &PublicKey::from(static_key),
                &StaticSecret::from([3; 32]),
                &PublicKey::from(ephemeral),
                "k7m2q9xf",
                "b3nn8t2q"
            )
            .is_err()
        );
    }
}

#[test]
#[ignore = "explicitly regenerate synthetic cross-language fixtures"]
fn generate_session_vectors() {
    let keys = keys(Role::Client);
    let mut fixture = json!({"node_id":"k7m2q9xf","device_id":"b3nn8t2q",
        "client_static":STANDARD.encode([1;32]),"node_static":STANDARD.encode([2;32]),
        "client_ephemeral":STANDARD.encode([3;32]),"node_ephemeral":STANDARD.encode([4;32])});
    for (direction, name) in [(Direction::C2s, "c2s"), (Direction::S2c, "s2c")] {
        let mut frame = Frame::new(*keys.key(direction), direction);
        let frames: Vec<_> = (0..10).map(|n| {
            let plain = if n == 0 { Vec::new() } else { format!("synthetic {name} frame {n}").into_bytes() };
            json!({"plaintext":STANDARD.encode(&plain),"ciphertext":STANDARD.encode(frame.seal(&plain).unwrap())})
        }).collect();
        fixture[name] = json!({"key":STANDARD.encode(keys.key(direction)),"frames":frames});
    }
    let mut node_static = [0; 32];
    hkdf::Hkdf::<sha2::Sha256>::new(None, include_bytes!("fixtures/identity/node.key"))
        .expand(b"privatium/x25519/v1", &mut node_static)
        .unwrap();
    let node_static = StaticSecret::from(node_static);
    let client_hello = json!({"v":1,"dev":"b3nn8t2q","e":STANDARD.encode(PublicKey::from(&StaticSecret::from([3;32])).as_bytes())}).to_string();
    let node_hello = json!({"v":1,"id":"as3nn9tm","e":STANDARD.encode(PublicKey::from(&StaticSecret::from([4;32])).as_bytes()),
        "cert":STANDARD.encode(include_str!("fixtures/identity/node.cert").trim())}).to_string();
    let handshake_keys = Keys::derive(
        Role::Client,
        &StaticSecret::from([1; 32]),
        &PublicKey::from(&node_static),
        &StaticSecret::from([3; 32]),
        &PublicKey::from(&StaticSecret::from([4; 32])),
        "as3nn9tm",
        "b3nn8t2q",
    )
    .unwrap();
    use sha2::Digest as _;
    let transcript: String = sha2::Sha256::digest(format!("{client_hello}{node_hello}"))
        .iter()
        .map(|b| format!("{b:02x}"))
        .collect();
    let (mut send, _) = handshake_keys.into_frames();
    fixture["handshake"] = json!({"client_hello":client_hello,"node_hello":node_hello,
        "node_static_public":STANDARD.encode(PublicKey::from(&node_static).as_bytes()),
        "cluster_public":STANDARD.encode(ed25519_dalek::SigningKey::from_bytes(&[42;32]).verifying_key().as_bytes()),
        "now":"2026-09-05T00:00:00.000Z",
        "confirm":STANDARD.encode(send.seal(json!({"confirm":transcript}).to_string().as_bytes()).unwrap())});
    std::fs::write(
        concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/tests/fixtures/session-vectors.json"
        ),
        serde_json::to_string_pretty(&fixture).unwrap() + "\n",
    )
    .unwrap();
}
