// Project:  Privatium™  |  File: crates/privatium-core/tests/pair.rs
// Authors:  Gabriel Mongefranco (@gabrielmongefranco)
// Created:  2026-09-05  |  Modified: 2026-09-05
// Summary:  Pairing against spec/protocol.md §7: the code and its renderings, the SPAKE2
//           vectors both languages read, the six messages of /ws/pair through the node,
//           the window's TTL, attempt cap and rate limit, the audit rows, and the device
//           row a success writes.

#![allow(clippy::unwrap_used, clippy::expect_used)]

mod common;

use std::net::{IpAddr, Ipv4Addr};
use std::time::Duration;

use base64::{Engine as _, engine::general_purpose::STANDARD};
use common::{at, sys_row};
use privatium_core::pair::handshake::{Client, Exchange, Paired};
use privatium_core::pair::spake2::{self, Identities, Side, State};
use privatium_core::pair::{self, Code, CodeError, GLYPHS, PairError, Pairing, WORDS};
use privatium_core::{Error, Identity, Node};
use serde_json::{Value, json};
use x25519_dalek::StaticSecret;

const FIXTURE: &str = concat!(
    env!("CARGO_MANIFEST_DIR"),
    "/tests/fixtures/pake-vectors.json"
);

/// TEST-NET-1 (RFC 5737): never loopback, never a real peer.
fn source(last: u8) -> IpAddr {
    IpAddr::V4(Ipv4Addr::new(192, 0, 2, last))
}

fn now() -> jiff::Timestamp {
    at("2026-09-05T12:00:00.000Z")
}

fn later(secs: i64) -> jiff::Timestamp {
    now()
        .checked_add(jiff::SignedDuration::from_secs(secs))
        .unwrap()
}

fn open(root: &tempfile::TempDir) -> Node {
    Node::open(root.path()).unwrap()
}

fn fixture() -> Value {
    serde_json::from_str(&std::fs::read_to_string(FIXTURE).unwrap()).unwrap()
}

fn b64(value: &Value) -> Vec<u8> {
    STANDARD.decode(value.as_str().unwrap()).unwrap()
}

fn wide(value: &Value) -> [u8; 64] {
    b64(value).try_into().unwrap()
}

/// Every message of one run, in order, for the tests that read the wire.
#[derive(Debug, Default)]
struct Transcript {
    texts: Vec<String>,
    sealed: Vec<Vec<u8>>,
}

/// One full run of `§7.4.2` between a Rust client and the node, with the code the node
/// shows, from `source`, at `when`.
fn run(
    node: &mut Node,
    code: Code,
    source: IpAddr,
    when: jiff::Timestamp,
) -> Result<(Paired, Transcript), Error> {
    let mut wire = Transcript::default();
    let hello = node.pairing_hello(when);
    wire.texts.push(hello.clone());
    let (mut client, start) = Client::start(&hello, code, "browser")?;
    wire.texts.push(start.clone());
    let (exchange, reply) = node.pairing_begin(source, when, &start)?;
    wire.texts.push(reply.clone());
    // A client that finds `cB` wrong says so and closes without sending `cA`; the
    // transport then tells the node the attempt is over, as `wire::channel` will.
    let confirm = match client.reply(&reply) {
        Ok(confirm) => confirm,
        Err(error) => {
            node.pairing_abandon(client.device(), source)?;
            return Err(error.into());
        }
    };
    wire.texts.push(confirm.clone());
    let (sealed, node_sealed) = node.pairing_confirm(exchange, &confirm)?;
    wire.sealed.push(node_sealed.clone());
    let (_pins, client_sealed) = client.finish(
        &node_sealed,
        Some("Pixel 9"),
        Some("Mozilla/5.0 synthetic"),
        when,
    )?;
    wire.sealed.push(client_sealed.clone());
    let paired = node.pairing_finish(sealed, &client_sealed, when)?;
    Ok((paired, wire))
}

/// Every `sys_audit` row of one kind, after `_sys` is rematerialized from the log.
fn audit_rows(node: &mut Node, kind: &str) -> Vec<Value> {
    node.refresh().unwrap();
    common::audit_rows(node, kind)
}

fn code_of(node: &Node) -> Code {
    node.pairing().unwrap().code()
}

// -------------------------------------------------------------------------------------
// §7.2, §7.3 — the code and its renderings
// -------------------------------------------------------------------------------------

#[test]
fn test_spec_7_2_code_is_16_bits_rendered_as_four_glyphs_and_two_words() {
    let code = Code::from_u16(0x7F2E);
    let glyphs = code.glyphs();
    assert_eq!(
        glyphs.map(|g| g.label),
        ["Fox", "Strawberry", "Pizza", "Game Die"]
    );
    assert_eq!(code.words(), [WORDS[0x7F], WORDS[0x2E]]);
    assert_eq!(code.bytes(), [0x7F, 0x2E]);
    // Every one of the 65,536 codes renders and reads back both ways.
    for value in 0..=u16::MAX {
        let code = Code::from_u16(value);
        let emoji: String = code
            .glyphs()
            .iter()
            .map(|g| g.glyph)
            .collect::<Vec<_>>()
            .join(" ");
        assert_eq!(Code::parse(&emoji), Ok(code), "{value:#06x}");
        let words = code.words().join(" ");
        assert_eq!(Code::parse(&words), Ok(code), "{value:#06x}");
    }
    let mut seen = std::collections::BTreeSet::new();
    for _ in 0..64 {
        seen.insert(Code::random().as_u16());
    }
    assert!(
        seen.len() > 1,
        "the CSPRNG produced one code sixty-four times"
    );
}

#[test]
fn test_spec_7_2_word_input_is_case_and_punctuation_insensitive() {
    let code = Code::from_u16(0x0105);
    let [first, second] = code.words();
    for text in [
        format!("{first} {second}"),
        format!("{}-{}", first.to_uppercase(), second),
        format!("  {first},{second}. "),
        format!("{first}{second}"),
        format!("{}\t{}", &first[..3], &second[..3]),
        format!("{} {}", &first[..3], second),
    ] {
        assert_eq!(Code::parse(&text), Ok(code), "{text:?}");
    }
    assert_eq!(Code::parse(""), Err(CodeError::Empty));
    assert_eq!(Code::parse(" - ,"), Err(CodeError::Empty));
    for text in [
        first.to_owned(),
        format!("{first} {second} {first}"),
        format!("{} {}", &first[..2], second),
        "zzzzz zzzzz".to_owned(),
        format!("{first} 12345"),
    ] {
        assert_eq!(Code::parse(&text), Err(CodeError::Unrecognized), "{text:?}");
        assert!(!CodeError::Unrecognized.to_string().contains(&text));
    }
}

#[test]
fn test_spec_7_2_glyph_labels_are_accepted_as_input() {
    // 🦊 🍕 ⚡️ 🎲 — the spec's own example row.
    let code = Code::from_u16(0x728E);
    for text in [
        "fox pizza lightning die",
        "Fox, Pizza, Lightning, Game Die",
        "FOX PIZZA LIGHTNING GAMEDIE",
        "foxpizzalightninggamedie",
    ] {
        assert_eq!(Code::parse(text), Ok(code), "{text:?}");
    }
    let two_word = Code::from_u16(0x9BD9);
    for text in [
        "hot pepper artist palette maple leaf hot pepper",
        "pepper palette leaf pepper",
        "Hot-Pepper / Artist-Palette / Maple-Leaf / Hot-Pepper",
    ] {
        assert_eq!(Code::parse(text), Ok(two_word), "{text:?}");
    }
    // The glyphs themselves, with and without their variation selectors, and mixed with
    // spaces — but never mixed with letters, and never three or five of them.
    let emoji = "\u{1F98A}\u{1F355}\u{26A1}\u{1F3B2}";
    assert_eq!(Code::parse(emoji), Ok(code));
    assert_eq!(
        Code::parse("\u{1F98A} \u{1F355} \u{26A1}\u{FE0F} \u{1F3B2}"),
        Ok(code)
    );
    assert_eq!(
        Code::parse("\u{1F98A}\u{1F355}\u{26A1}"),
        Err(CodeError::Unrecognized)
    );
    assert_eq!(
        Code::parse("\u{1F98A} fox \u{26A1} \u{1F3B2}"),
        Err(CodeError::Unrecognized)
    );
    // `guitar` is a word and a label; two tokens are words, four are labels.
    let words = Code::parse("guitar acid").unwrap();
    assert_eq!(words.words(), ["guitar", "acid"]);
    assert_eq!(
        Code::parse("guitar guitar guitar guitar").unwrap().glyphs()[0].label,
        "Guitar"
    );
}

#[test]
fn test_spec_7_3_glyph_table_is_normative_and_keeps_variation_selectors() {
    assert!(GLYPHS[8].glyph.ends_with('\u{FE0F}'));
    assert!(GLYPHS[9].glyph.ends_with('\u{FE0F}'));
    assert_eq!(GLYPHS[8].glyph.as_bytes(), "\u{26A1}\u{FE0F}".as_bytes());
    // The table in the spec, row by row: index, glyph, codepoints, label.
    let spec = std::fs::read_to_string(concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/../../spec/protocol.md"
    ))
    .unwrap();
    let mut rows = 0;
    for line in spec.lines() {
        let cells: Vec<&str> = line.split('|').map(str::trim).collect();
        if cells.len() != 6 || !cells[3].starts_with("U+") {
            continue;
        }
        let Ok(index) = cells[1].parse::<usize>() else {
            continue;
        };
        let glyph = &GLYPHS[index];
        let from_codepoints: String = cells[3]
            .split_whitespace()
            .map(|cp| char::from_u32(u32::from_str_radix(&cp[2..], 16).unwrap()).unwrap())
            .collect();
        assert_eq!(glyph.glyph, from_codepoints, "row {index}");
        assert_eq!(
            glyph.glyph, cells[2],
            "row {index}: the spec's own glyph bytes"
        );
        assert_eq!(glyph.label, cells[4], "row {index}");
        rows += 1;
    }
    assert_eq!(rows, 16);
}

#[test]
fn test_spec_7_2_word_list_has_256_distinct_words_with_unique_three_letter_prefixes() {
    let file = std::fs::read_to_string(concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/../../spec/pairing-words.txt"
    ))
    .unwrap();
    let lines: Vec<&str> = file.lines().collect();
    assert_eq!(lines, WORDS.to_vec());
    let distinct: std::collections::BTreeSet<&str> = WORDS.iter().copied().collect();
    assert_eq!(distinct.len(), 256);
    let prefixes: std::collections::BTreeSet<&str> = WORDS.iter().map(|w| &w[..3]).collect();
    assert_eq!(prefixes.len(), 256);
    assert_eq!(WORDS[0], "abyss");
    let mut sorted = WORDS.to_vec();
    sorted.sort_unstable();
    assert_eq!(sorted, WORDS.to_vec(), "index order is alphabetical");
}

// -------------------------------------------------------------------------------------
// §7.4.1 — SPAKE2
// -------------------------------------------------------------------------------------

#[test]
fn test_spec_7_4_spake2_matches_the_checked_in_vectors() {
    let v = fixture();
    let code = Code::from_u16(v["code"].as_u64().unwrap() as u16);
    let w = spake2::password(code).unwrap();
    assert_eq!(STANDARD.encode(w.to_bytes()), v["w"]);
    let ids = Identities::new(
        v["device_ed25519_public"].as_str().unwrap(),
        v["node_ed25519_public"].as_str().unwrap(),
    );
    let a = State::start_with(Side::A, &w, &wide(&v["x"])).unwrap();
    let b = State::start_with(Side::B, &w, &wide(&v["y"])).unwrap();
    assert_eq!(STANDARD.encode(a.message()), v["pA"]);
    assert_eq!(STANDARD.encode(b.message()), v["pB"]);
    let (pa, pb) = (a.message(), b.message());
    let a = a.finish(&pb, &ids).unwrap();
    let b = b.finish(&pa, &ids).unwrap();
    assert_eq!(STANDARD.encode(&*a.transcript), v["TT"]);
    assert_eq!(STANDARD.encode(&*b.transcript), v["TT"]);
    assert_eq!(STANDARD.encode(*a.ke), v["Ke"]);
    assert_eq!(STANDARD.encode(*b.ke), v["Ke"]);
    assert_eq!(STANDARD.encode(a.confirm_send), v["cA"]);
    assert_eq!(STANDARD.encode(b.confirm_send), v["cB"]);
    assert!(a.verify(&b64(&v["cB"])));
    assert!(b.verify(&b64(&v["cA"])));
    assert!(!a.verify(&b64(&v["cA"])));
    assert!(!b.verify(&[0; 32]));
    // Ke ‖ Ka = SHA-256(TT); KcA ‖ KcB = HKDF(Ka, "ConfirmationKeys").
    use sha2::Digest as _;
    let digest = sha2::Sha256::digest(b64(&v["TT"]));
    assert_eq!(STANDARD.encode(&digest[..16]), v["Ke"]);
    assert_eq!(STANDARD.encode(&digest[16..]), v["Ka"]);
    let mut kc = [0u8; 32];
    hkdf::Hkdf::<sha2::Sha256>::new(None, &digest[16..])
        .expand(b"ConfirmationKeys", &mut kc)
        .unwrap();
    assert_eq!(STANDARD.encode(&kc[..16]), v["KcA"]);
    assert_eq!(STANDARD.encode(&kc[16..]), v["KcB"]);
    // The constants are the RFC's, spelled in hex in the fixture for a reader.
    assert_eq!(hex(&spake2::M), v["M"]);
    assert_eq!(hex(&spake2::N), v["N"]);
}

fn hex(bytes: &[u8]) -> String {
    bytes.iter().map(|b| format!("{b:02x}")).collect()
}

#[test]
fn test_spec_7_4_a_wrong_code_derives_nothing() {
    let ids = Identities::new("QQ==", "Qg==");
    let right = spake2::password(Code::from_u16(0x1234)).unwrap();
    let wrong = spake2::password(Code::from_u16(0x1235)).unwrap();
    let a = State::start(Side::A, &right).unwrap();
    let b = State::start(Side::B, &wrong).unwrap();
    let (pa, pb) = (a.message(), b.message());
    let a = a.finish(&pb, &ids).unwrap();
    let b = b.finish(&pa, &ids).unwrap();
    assert!(!a.verify(&b.confirm_send));
    assert!(!b.verify(&a.confirm_send));
    assert_ne!(*a.ke, *b.ke);
    assert_ne!(*a.transcript, *b.transcript);
    // Two identical runs with fresh secrets never repeat a message.
    let again = State::start(Side::A, &right).unwrap();
    assert_ne!(again.message(), pa);
}

// -------------------------------------------------------------------------------------
// §7.4.2 — the messages, through the node
// -------------------------------------------------------------------------------------

#[test]
fn test_spec_7_4_pairing_completes_and_writes_the_device_row() {
    let root = tempfile::tempdir().unwrap();
    let mut node = open(&root);
    let snapshot = node.pair_at(Duration::from_secs(120), now()).unwrap();
    let code = Code::parse(&snapshot.words.join(" ")).unwrap();
    let (paired, _) = run(&mut node, code, source(10), later(5)).unwrap();
    assert_eq!(paired.kind, "browser");
    assert_eq!(paired.label.as_deref(), Some("Pixel 9"));
    assert_eq!(paired.source, source(10));

    let row = sys_row(&node, "sys_device", &paired.device).unwrap();
    assert_eq!(row["id"], paired.device);
    assert_eq!(row["kind"], "browser");
    assert_eq!(row["replica"], false);
    assert_eq!(row["label"], "Pixel 9");
    assert_eq!(row["ed25519_pub"], paired.ed25519_pub);
    assert_eq!(row["x25519_pub"], paired.x25519_pub);
    assert_eq!(row["paired_at"], "2026-09-05T12:00:05.000Z");
    assert_eq!(row["paired_via"], "lan");
    assert_eq!(row["user_agent"], "Mozilla/5.0 synthetic");
    assert_eq!(row["last_seen_at"], Value::Null);
    assert_eq!(row["revoked_at"], Value::Null);
    assert_eq!(row["revoked_reason"], Value::Null);
    // The device's key decodes, and its ID is the ID §2.2 derives from it.
    let key =
        ed25519_dalek::VerifyingKey::from_bytes(&b64(&row["ed25519_pub"]).try_into().unwrap())
            .unwrap();
    assert_eq!(privatium_core::NodeId::derive(&key).as_str(), paired.device);
    // The node's own row is untouched, and no cluster private key travelled.
    let own = sys_row(&node, "sys_device", node.id().as_str()).unwrap();
    assert_eq!(own["kind"], "node");

    let success = audit_rows(&mut node, "pair.success");
    assert_eq!(success.len(), 1);
    assert_eq!(success[0]["subject"], paired.device);
    assert_eq!(success[0]["severity"], "info");
    let detail: Value = serde_json::from_str(success[0]["detail"].as_str().unwrap()).unwrap();
    assert_eq!(detail["source"], "192.0.2.10");
    assert_eq!(detail["kind"], "browser");

    let after = node.refresh_pairing(later(6)).unwrap().unwrap();
    assert_eq!(after.consumed_by.as_deref(), Some(paired.device.as_str()));
    assert_eq!(
        after.consumed_at.as_deref(),
        Some("2026-09-05T12:00:05.000Z")
    );
    assert!(!node.pairing_open(later(6)));
}

#[test]
fn test_spec_7_4_the_client_pins_the_cluster_key_and_the_node_certificate() {
    let root = tempfile::tempdir().unwrap();
    let mut node = open(&root);
    node.pair_at(Duration::from_secs(120), now()).unwrap();
    let code = code_of(&node);
    let hello = node.pairing_hello(now());
    let (mut client, start) = Client::start(&hello, code, "mobile").unwrap();
    let (exchange, reply) = node.pairing_begin(source(1), now(), &start).unwrap();
    let confirm = client.reply(&reply).unwrap();
    let (sealed, node_sealed) = node.pairing_confirm(exchange, &confirm).unwrap();
    let (pins, client_sealed) = client.finish(&node_sealed, None, None, now()).unwrap();
    let paired = node.pairing_finish(sealed, &client_sealed, now()).unwrap();
    assert_eq!(paired.kind, "mobile");
    assert_eq!(paired.label, None);
    assert_eq!(paired.user_agent, None);
    assert_eq!(pins.node_id, node.id().as_str());
    assert_eq!(pins.cluster_id, node.identity().cluster_id().as_str());
    assert_eq!(pins.cluster_pub, node.identity().cluster_public());
    assert_eq!(&pins.certificate, node.identity().certificate());
    assert_eq!(
        STANDARD.encode(pins.node_x25519.as_bytes()),
        node.identity().x25519_public_base64()
    );
    assert_eq!(pins.device, paired.device);
    // The pins are exactly what the channel handshake of §8 takes.
    let session_pins = privatium_core::session::handshake::NodePins {
        id: pins.node_id.clone(),
        cluster: pins.cluster_pub,
        x25519: pins.node_x25519,
    };
    assert!(
        privatium_core::session::handshake::ClientHandshake::start(
            &pins.device,
            pins.x25519,
            session_pins
        )
        .is_ok()
    );
}

#[test]
fn test_spec_7_0_the_code_never_crosses_the_wire() {
    let root = tempfile::tempdir().unwrap();
    let mut node = open(&root);
    node.pair_at(Duration::from_secs(120), now()).unwrap();
    let code = code_of(&node);
    let (_, wire) = run(&mut node, code, source(3), now()).unwrap();
    assert_eq!(wire.texts.len(), 4);
    assert_eq!(wire.sealed.len(), 2);
    let w = spake2::password(code).unwrap();
    let w_bytes = w.to_bytes();
    let w_b64 = STANDARD.encode(w_bytes);
    let hex = format!("{:04x}", code.as_u16());
    for text in &wire.texts {
        for word in code.words() {
            assert!(!text.to_lowercase().contains(word), "{text}");
        }
        for glyph in code.glyphs() {
            assert!(!text.contains(glyph.glyph), "{text}");
            assert!(
                !text.to_lowercase().contains(&glyph.label.to_lowercase()),
                "{text}"
            );
        }
        assert!(!text.contains(&w_b64) && !text.contains(&hex));
        // Every text frame is JSON whose values are keys, IDs, kinds and points; the
        // code is not among them as a number either.
        let value: Value = serde_json::from_str(text).unwrap();
        assert!(!value.to_string().contains(&format!(":{}", code.as_u16())));
    }
    for frame in &wire.sealed {
        assert!(!frame.windows(32).any(|window| window == w_bytes));
    }
    // The base64 of `w` is absent from every sealed frame too.
    let all: Vec<u8> = wire.sealed.concat();
    assert!(
        !all.windows(w_b64.len())
            .any(|window| window == w_b64.as_bytes())
    );
}

#[test]
fn test_spec_7_1_pairing_is_closed_until_opened_and_closes_on_first_success() {
    let root = tempfile::tempdir().unwrap();
    let mut node = open(&root);
    let hello: Value = serde_json::from_str(&node.pairing_hello(now())).unwrap();
    assert_eq!(hello["open"], false);
    assert_eq!(hello["v"], 1);
    assert_eq!(hello["id"], node.id().as_str());
    assert_eq!(hello["pub"], node.identity().public_key_base64());
    assert!(!node.pairing_open(now()));
    assert!(node.pairing().is_none());
    // A client with any code, against a closed node.
    let closed = Client::start(&node.pairing_hello(now()), Code::from_u16(1), "browser");
    assert!(matches!(closed, Err(PairError::Closed)));
    let start = json!({"v":1,"dev":"b3nn8t2q","pub":STANDARD.encode([9;32]),"kind":"browser","pA":STANDARD.encode([1;32])}).to_string();
    let refused = node.pairing_begin(source(1), now(), &start).unwrap_err();
    assert!(matches!(refused, Error::Pair(PairError::Closed)));
    assert!(audit_rows(&mut node, "pair.attempt").is_empty());
    assert!(audit_rows(&mut node, "pair.failed").is_empty());

    node.pair_at(Duration::from_secs(120), now()).unwrap();
    assert!(node.pairing_open(now()));
    let hello: Value = serde_json::from_str(&node.pairing_hello(now())).unwrap();
    assert_eq!(hello["open"], true);
    let code = code_of(&node);
    run(&mut node, code, source(1), now()).unwrap();
    assert!(!node.pairing_open(now()));
    let hello: Value = serde_json::from_str(&node.pairing_hello(now())).unwrap();
    assert_eq!(hello["open"], false);
    let second = run(&mut node, code, source(2), later(3));
    assert!(matches!(second, Err(Error::Pair(PairError::Closed))));
    assert_eq!(audit_rows(&mut node, "pair.success").len(), 1);
    // The owner opens it again: a new window, a new code.
    let again = node.pair_at(Duration::from_secs(120), later(10)).unwrap();
    assert!(node.pairing_open(later(10)));
    assert_eq!(again.attempts, 0);
    assert_eq!(again.consumed_by, None);
}

#[test]
fn test_spec_7_5_code_expires_at_120s_and_five_attempts_issue_a_new_one() {
    let root = tempfile::tempdir().unwrap();
    let mut node = open(&root);
    let snapshot = node.pair_at(Duration::from_secs(3600), now()).unwrap();
    assert_eq!(
        snapshot.expires_at, "2026-09-05T12:02:00.000Z",
        "clamped to 120 s"
    );
    assert!(node.pairing_open(later(119)));
    assert!(!node.pairing_open(later(120)));
    assert_eq!(
        node.refresh_pairing(later(119)).unwrap().unwrap().id,
        snapshot.id
    );
    assert!(node.refresh_pairing(later(120)).unwrap().is_none());
    assert!(node.pairing().is_none());
    let expired = audit_rows(&mut node, "pair.expired");
    assert_eq!(expired.len(), 1);
    assert_eq!(expired[0]["subject"], snapshot.id);
    // An attempt after expiry is refused as closed, and audited as nothing.
    let refused = run(&mut node, Code::from_u16(7), source(1), later(121));
    assert!(matches!(refused, Err(Error::Pair(PairError::Closed))));
    assert!(audit_rows(&mut node, "pair.attempt").is_empty());

    // Five wrong codes replace the code inside the same window.
    let opened = node.pair_at(Duration::from_secs(120), later(200)).unwrap();
    let right = code_of(&node);
    let wrong = Code::from_u16(right.as_u16().wrapping_add(1));
    for attempt in 1..=5u8 {
        let when = later(200 + i64::from(attempt));
        let outcome = run(&mut node, wrong, source(attempt), when);
        assert!(
            matches!(outcome, Err(Error::Pair(PairError::WrongCode))),
            "attempt {attempt}"
        );
        let window = node.refresh_pairing(when).unwrap().unwrap();
        if attempt < 5 {
            assert_eq!(window.attempts, attempt);
            assert_eq!(window.generation, 0);
        } else {
            assert_eq!(window.attempts, 0, "the fifth failure issues a new code");
            assert_eq!(window.generation, 1);
        }
        assert_eq!(
            window.expires_at, opened.expires_at,
            "the window is unchanged"
        );
        assert_eq!(window.id, opened.id);
    }
    let renewed = code_of(&node);
    assert!(node.pairing_open(later(210)));
    let failed = audit_rows(&mut node, "pair.failed");
    assert_eq!(failed.len(), 5);
    let last: Value = serde_json::from_str(failed[4]["detail"].as_str().unwrap()).unwrap();
    assert_eq!(last["new_code"], true);
    assert_eq!(last["reason"], "abandoned before confirmation");
    let fourth: Value = serde_json::from_str(failed[3]["detail"].as_str().unwrap()).unwrap();
    assert_eq!(fourth["new_code"], false);
    // The old code is dead even if it is guessed now; the new one works.
    let stale = run(&mut node, right, source(20), later(211));
    assert!(matches!(stale, Err(Error::Pair(PairError::WrongCode))));
    run(&mut node, renewed, source(21), later(214)).unwrap();

    // Attempts that never send cA count too: five silent peers exhaust a code.
    // Bound to a name: a directory dropped here would vanish under the open node on
    // Linux and macOS, which delete a directory whose files are still open.
    let silent = tempfile::tempdir().unwrap();
    let mut node = open(&silent);
    node.pair_at(Duration::from_secs(120), now()).unwrap();
    let code = code_of(&node);
    for n in 1..=5u8 {
        let hello = node.pairing_hello(later(i64::from(n)));
        let (_, start) = Client::start(&hello, code, "browser").unwrap();
        node.pairing_begin(source(n), later(i64::from(n)), &start)
            .unwrap();
    }
    assert_eq!(node.refresh_pairing(later(6)).unwrap().unwrap().attempts, 5);
    let hello = node.pairing_hello(later(6));
    let (_, start) = Client::start(&hello, code, "browser").unwrap();
    let sixth = node.pairing_begin(source(6), later(6), &start).unwrap_err();
    assert!(matches!(sixth, Error::Pair(PairError::Exhausted)));
    let window = node.refresh_pairing(later(6)).unwrap().unwrap();
    assert_eq!((window.attempts, window.generation), (0, 1));
    let exhausted = audit_rows(&mut node, "pair.failed");
    assert_eq!(exhausted.len(), 1);
    let detail: Value = serde_json::from_str(exhausted[0]["detail"].as_str().unwrap()).unwrap();
    assert_eq!(detail["new_code"], true);
    // A peer that goes away is reported by the transport and counts as a failure.
    node.pairing_abandon("b3nn8t2q", source(6)).unwrap();
    assert_eq!(audit_rows(&mut node, "pair.failed").len(), 2);
}

#[test]
fn test_spec_7_5_attempts_are_rate_limited_per_source() {
    let root = tempfile::tempdir().unwrap();
    let mut node = open(&root);
    node.pair_at(Duration::from_secs(120), now()).unwrap();
    let code = code_of(&node);
    let wrong = Code::from_u16(code.as_u16() ^ 0x0F0F);
    let start = |node: &Node, when: jiff::Timestamp| {
        Client::start(&node.pairing_hello(when), wrong, "browser")
            .unwrap()
            .1
    };
    // The first attempt from a source is accepted; a second inside two seconds is not.
    let first = start(&node, now());
    node.pairing_begin(source(1), now(), &first).unwrap();
    let second = start(&node, later(1));
    let refused = node
        .pairing_begin(source(1), later(1), &second)
        .unwrap_err();
    assert!(matches!(refused, Error::Pair(PairError::RateLimited)));
    // Another source is unaffected; the same source is fine two seconds on.
    let other = start(&node, later(1));
    node.pairing_begin(source(2), later(1), &other).unwrap();
    let third = start(&node, later(2));
    node.pairing_begin(source(1), later(2), &third).unwrap();
    // Only the accepted attempts were counted and audited.
    assert_eq!(node.refresh_pairing(later(2)).unwrap().unwrap().attempts, 3);
    assert_eq!(audit_rows(&mut node, "pair.attempt").len(), 3);
    assert!(
        audit_rows(&mut node, "pair.failed").is_empty(),
        "a refusal is not a failure"
    );
    // Close codes, as §7.4.2 assigns them.
    assert_eq!(PairError::RateLimited.close_code(), 4429);
    assert_eq!(PairError::Exhausted.close_code(), 4429);
    assert_eq!(PairError::Closed.close_code(), 4404);
    assert_eq!(PairError::WrongCode.close_code(), 4401);
    assert_eq!(PairError::Format.close_code(), 4400);
    assert_eq!(PairError::NodeKind.close_code(), 4403);
}

#[test]
fn test_spec_7_5_every_attempt_writes_an_audit_row_without_the_code() {
    let root = tempfile::tempdir().unwrap();
    let mut node = open(&root);
    let snapshot = node.pair_at(Duration::from_secs(90), now()).unwrap();
    let code = code_of(&node);
    let wrong = Code::from_u16(code.as_u16() ^ 1);
    run(&mut node, wrong, source(1), later(1)).unwrap_err();
    run(&mut node, code, source(2), later(2)).unwrap();
    node.refresh().unwrap();
    let opened = audit_rows(&mut node, "pair.opened");
    assert_eq!(opened.len(), 1);
    assert_eq!(opened[0]["subject"], snapshot.id);
    let detail: Value = serde_json::from_str(opened[0]["detail"].as_str().unwrap()).unwrap();
    assert_eq!(detail["ttl"], 90);
    assert_eq!(audit_rows(&mut node, "pair.attempt").len(), 2);
    assert_eq!(audit_rows(&mut node, "pair.failed").len(), 1);
    assert_eq!(audit_rows(&mut node, "pair.success").len(), 1);
    let mut every = Vec::new();
    for kind in ["pair.opened", "pair.attempt", "pair.failed", "pair.success"] {
        every.extend(audit_rows(&mut node, kind));
    }
    let words = code.words();
    for row in &every {
        let text = row.to_string().to_lowercase();
        for word in words {
            assert!(!text.contains(word), "{text}");
        }
        for glyph in code.glyphs() {
            assert!(!text.contains(glyph.glyph), "{text}");
        }
        assert!(!text.contains(&format!("{:04x}", code.as_u16())));
        assert_eq!(row["actor"], "system");
        if row["kind"] != "pair.opened" {
            assert!(
                row["detail"].as_str().unwrap().contains("192.0.2."),
                "the source is named"
            );
        }
    }
    // Nothing about the window reached a file: not the code, not `w`.
    let w = STANDARD.encode(spake2::password(code).unwrap().to_bytes());
    for entry in walkdir(root.path()) {
        // `local/lock` is held exclusively by the node and is empty; nothing else is.
        let Ok(bytes) = std::fs::read(&entry) else {
            assert!(entry.ends_with("lock"), "{}", entry.display());
            continue;
        };
        let text = String::from_utf8_lossy(&bytes).to_lowercase();
        assert!(!text.contains(&w), "{}", entry.display());
        assert!(
            !words.iter().all(|word| text.contains(word)),
            "{}",
            entry.display()
        );
    }
}

fn walkdir(dir: &std::path::Path) -> Vec<std::path::PathBuf> {
    let mut out = Vec::new();
    for entry in std::fs::read_dir(dir).unwrap() {
        let path = entry.unwrap().path();
        if path.is_dir() {
            out.extend(walkdir(&path));
        } else {
            out.push(path);
        }
    }
    out
}

#[test]
fn test_spec_7_4_a_node_kind_is_refused_naming_phase_3() {
    let root = tempfile::tempdir().unwrap();
    let mut node = open(&root);
    node.pair_at(Duration::from_secs(120), now()).unwrap();
    let code = code_of(&node);
    let (_, start) = Client::start(&node.pairing_hello(now()), code, "node").unwrap();
    let refused = node.pairing_begin(source(1), now(), &start).unwrap_err();
    assert!(matches!(refused, Error::Pair(PairError::NodeKind)));
    assert!(refused.to_string().contains("Phase 3"));
    assert!(refused.to_string().contains("spec/protocol.md §2.3.1"));
    // Not an attempt: nothing counted, nothing audited, the window untouched.
    assert_eq!(node.refresh_pairing(now()).unwrap().unwrap().attempts, 0);
    assert!(audit_rows(&mut node, "pair.attempt").is_empty());
    // Every other kind the dictionary names is accepted; anything else is malformed.
    for (n, kind) in ["browser", "desktop", "mobile"].into_iter().enumerate() {
        let when = later(3 + n as i64);
        let (_, start) = Client::start(&node.pairing_hello(when), code, kind).unwrap();
        node.pairing_begin(source(2 + n as u8), when, &start)
            .unwrap();
        node.pairing_abandon("b3nn8t2q", source(2 + n as u8))
            .unwrap();
    }
    let (_, start) = Client::start(&node.pairing_hello(now()), code, "toaster").unwrap();
    let refused = node.pairing_begin(source(9), later(9), &start).unwrap_err();
    assert!(matches!(refused, Error::Pair(PairError::Format)));
}

#[test]
fn test_spec_7_4_2_malformed_messages_and_a_mismatched_device_id_are_refused() {
    let root = tempfile::tempdir().unwrap();
    let mut node = open(&root);
    node.pair_at(Duration::from_secs(120), now()).unwrap();
    let code = code_of(&node);
    let (_, good) = Client::start(&node.pairing_hello(now()), code, "browser").unwrap();
    let good: Value = serde_json::from_str(&good).unwrap();
    let mutate = |f: &dyn Fn(&mut Value)| {
        let mut v = good.clone();
        f(&mut v);
        v.to_string()
    };
    let bad = [
        String::new(),
        "[]".to_owned(),
        "x".repeat(8193),
        mutate(&|v| v["v"] = json!(2)),
        mutate(&|v| v["dev"] = json!("00000000")),
        mutate(&|v| v["dev"] = json!("../etc")),
        mutate(&|v| v["pub"] = json!("bad")),
        mutate(&|v| v["pub"] = json!(STANDARD.encode([0; 32]))),
        mutate(&|v| v["pA"] = json!(STANDARD.encode([0; 31]))),
        mutate(&|v| v["dev"] = json!(node.id().as_str())),
        mutate(&|v| {
            v.as_object_mut().unwrap().remove("kind");
        }),
    ];
    for (n, text) in bad.iter().enumerate() {
        let refused = node
            .pairing_begin(source(n as u8 + 1), later(n as i64), text)
            .unwrap_err();
        assert!(
            matches!(refused, Error::Pair(PairError::Format)),
            "{n}: {refused}"
        );
        assert!(!refused.to_string().contains("00000000"));
    }
    // None of those counted: they were refused before the PAKE.
    assert_eq!(node.refresh_pairing(now()).unwrap().unwrap().attempts, 0);
    assert!(audit_rows(&mut node, "pair.attempt").is_empty());
    // A pA that decodes but is no point spends an attempt: it reached the PAKE.
    let identity = {
        let mut bytes = [0u8; 32];
        bytes[0] = 1;
        bytes
    };
    let no_point = mutate(&|v| v["pA"] = json!(STANDARD.encode(identity)));
    let refused = node
        .pairing_begin(source(40), later(40), &no_point)
        .unwrap_err();
    assert!(matches!(refused, Error::Pair(PairError::Format)));
    assert_eq!(node.refresh_pairing(now()).unwrap().unwrap().attempts, 1);
    assert_eq!(audit_rows(&mut node, "pair.failed").len(), 1);

    // A wrong cA is one failed attempt with close code 4401 and nothing sealed.
    let (mut client, start) = Client::start(&node.pairing_hello(now()), code, "browser").unwrap();
    let (exchange, reply) = node.pairing_begin(source(50), later(50), &start).unwrap();
    client.reply(&reply).unwrap();
    let forged = json!({"cA": STANDARD.encode([7; 32])}).to_string();
    let refused = node.pairing_confirm(exchange, &forged).unwrap_err();
    assert!(matches!(refused, Error::Pair(PairError::WrongCode)));
    assert_eq!(audit_rows(&mut node, "pair.failed").len(), 2);
    assert_eq!(node.refresh_pairing(now()).unwrap().unwrap().attempts, 2);
    // The client side refuses a forged cB the same way, and sends nothing after it.
    let (mut client, start) = Client::start(&node.pairing_hello(now()), code, "browser").unwrap();
    let (_, reply) = node.pairing_begin(source(51), later(51), &start).unwrap();
    let mut reply: Value = serde_json::from_str(&reply).unwrap();
    reply["cB"] = json!(STANDARD.encode([7; 32]));
    assert_eq!(client.reply(&reply.to_string()), Err(PairError::WrongCode));
    assert!(client.reply("{}").is_err(), "a state is single use");

    // The sealed messages: a tampered node message fails on the client; a tampered client
    // message fails on the node, and neither writes a row.
    let (mut client, start) = Client::start(&node.pairing_hello(now()), code, "browser").unwrap();
    let (exchange, reply) = node.pairing_begin(source(52), later(52), &start).unwrap();
    let confirm = client.reply(&reply).unwrap();
    let (sealed, mut node_sealed) = node.pairing_confirm(exchange, &confirm).unwrap();
    node_sealed[3] ^= 1;
    assert!(matches!(
        client.finish(&node_sealed, None, None, now()),
        Err(PairError::Format)
    ));
    let refused = node.pairing_finish(sealed, &[0; 64], now()).unwrap_err();
    assert!(matches!(refused, Error::Pair(PairError::Format)));
    assert!(sys_row(&node, "sys_device", "b3nn8t2q").is_none());
    assert!(audit_rows(&mut node, "pair.success").is_empty());
    assert!(
        node.pairing_open(later(53)),
        "a refused registration leaves the window open"
    );
}

#[test]
fn test_spec_7_6_a_registered_device_key_cannot_pair_again() {
    let root = tempfile::tempdir().unwrap();
    let mut node = open(&root);
    node.pair_at(Duration::from_secs(120), now()).unwrap();
    let code = code_of(&node);
    let signing = ed25519_dalek::SigningKey::from_bytes(&[5; 32]);
    let pair_with_key = |node: &mut Node, when: jiff::Timestamp, code: Code| {
        let hello = node.pairing_hello(when);
        let (mut client, start) = Client::start_with(
            &hello,
            code,
            "browser",
            signing.clone(),
            StaticSecret::from([8; 32]),
            &[9; 64],
        )?;
        let (exchange, reply) = node.pairing_begin(source(1), when, &start)?;
        let confirm = client.reply(&reply)?;
        let (sealed, node_sealed) = node.pairing_confirm(exchange, &confirm)?;
        let (_, client_sealed) = client.finish(&node_sealed, None, None, when)?;
        node.pairing_finish(sealed, &client_sealed, when)
    };
    let first = pair_with_key(&mut node, now(), code).unwrap();
    // A new window, the same key: refused, audited, and the row unchanged.
    node.pair_at(Duration::from_secs(120), later(10)).unwrap();
    let code = code_of(&node);
    let refused = pair_with_key(&mut node, later(10), code).unwrap_err();
    assert!(matches!(refused, Error::Pair(PairError::DeviceKnown)));
    assert_eq!(refused.to_string(), PairError::DeviceKnown.to_string());
    let failed = audit_rows(&mut node, "pair.failed");
    assert_eq!(failed.len(), 1);
    assert_eq!(failed[0]["subject"], first.device);
    let row = sys_row(&node, "sys_device", &first.device).unwrap();
    assert_eq!(row["paired_at"], "2026-09-05T12:00:00.000Z");
    assert!(node.pairing_open(later(11)));
    assert_eq!(audit_rows(&mut node, "pair.success").len(), 1);
}

#[test]
fn test_spec_app_contract_6_pair_opens_a_window_and_returns_the_code() {
    let root = tempfile::tempdir().unwrap();
    let mut node = open(&root);
    // A Phase 1 build refused this call; now it opens the window of §7.1.
    let snapshot = node.pair_at(Duration::from_secs(120), now()).unwrap();
    assert_eq!(snapshot.emoji.len(), 4);
    assert_eq!(snapshot.labels.len(), 4);
    for (glyph, label) in snapshot.emoji.iter().zip(snapshot.labels) {
        let entry = GLYPHS.iter().find(|g| g.glyph == *glyph).unwrap();
        assert_eq!(entry.label, label);
    }
    for word in snapshot.words {
        assert!(WORDS.contains(&word));
    }
    let from_words = Code::parse(&snapshot.words.join(" ")).unwrap();
    let from_emoji = Code::parse(&snapshot.emoji.join("")).unwrap();
    assert_eq!(from_words, from_emoji);
    assert_eq!(from_words, code_of(&node));
    assert_eq!(
        snapshot.url,
        format!("http://127.0.0.1:{}", node.config().node.port)
    );
    assert_eq!(snapshot.created_at, "2026-09-05T12:00:00.000Z");
    assert_eq!(snapshot.expires_at, "2026-09-05T12:02:00.000Z");
    assert_eq!(snapshot.attempts, 0);
    assert_eq!(snapshot.consumed_by, None);
    // The JSON shape `POST /api/v1/pair` answers with.
    let json = serde_json::to_value(&snapshot).unwrap();
    for key in [
        "id",
        "emoji",
        "labels",
        "words",
        "url",
        "created_at",
        "expires_at",
        "attempts",
    ] {
        assert!(json.get(key).is_some(), "{key}");
    }
    assert!(json.get("consumed_by").is_none());
    // One window at a time: a second call while it is open returns the same window.
    let again = node.pair_at(Duration::from_secs(30), later(5)).unwrap();
    assert_eq!(again, snapshot);
    // Closing it is explicit and audited; a zero TTL is refused.
    assert!(node.close_pairing(later(6)).unwrap());
    assert!(!node.close_pairing(later(6)).unwrap());
    assert!(node.pairing().is_none());
    assert_eq!(audit_rows(&mut node, "pair.expired").len(), 1);
    assert!(matches!(
        node.pair_at(Duration::ZERO, later(7)),
        Err(Error::Pair(PairError::Ttl))
    ));
    // The other two §6 network methods still refuse, naming their phase.
    assert!(matches!(
        node.serve_discovery(),
        Err(Error::Unimplemented { phase: "2", .. })
    ));
    assert!(matches!(
        node.start_sync(),
        Err(Error::Unimplemented { phase: "3", .. })
    ));
}

// -------------------------------------------------------------------------------------
// The vector file both languages read (docs/plans/phase-2.md §2.4, risk R9)
// -------------------------------------------------------------------------------------

/// The identity fixtures of `tests/fixtures/identity/` as a loaded `Identity`: the
/// checked-in node key and certificate, and the cluster key the README names.
fn fixture_identity(dir: &std::path::Path) -> Identity {
    let fixtures = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/identity");
    std::fs::copy(fixtures.join("node.key"), dir.join("node.key")).unwrap();
    std::fs::write(dir.join("cluster.key"), [42u8; 32]).unwrap();
    std::fs::write(
        dir.join("node.cert"),
        std::fs::read_to_string(fixtures.join("node.cert"))
            .unwrap()
            .trim(),
    )
    .unwrap();
    Identity::load_or_create_at(dir, at("2026-09-05T00:00:00.000Z")).unwrap()
}

#[test]
fn test_spec_7_4_2_the_transcript_matches_the_checked_in_vectors() {
    let v = fixture();
    let dir = tempfile::tempdir().unwrap();
    let identity = fixture_identity(dir.path());
    let when = at(v["now"].as_str().unwrap());
    let code = Code::from_u16(v["code"].as_u64().unwrap() as u16);
    let mut window = Pairing::open_with(code, Duration::from_secs(120), when).unwrap();
    let t = &v["transcript"];
    let hello = pair::handshake::node_hello(&identity, true);
    assert_eq!(hello, t["node_hello"]);
    let (mut client, start) = Client::start_with(
        &hello,
        code,
        "browser",
        ed25519_dalek::SigningKey::from_bytes(
            &b64(&v["device_ed25519_secret"]).try_into().unwrap(),
        ),
        StaticSecret::from(<[u8; 32]>::try_from(b64(&v["device_x25519_secret"])).unwrap()),
        &wide(&v["x"]),
    )
    .unwrap();
    assert_eq!(start, t["client_start"]);
    assert_eq!(client.device(), v["device_id"]);
    let (exchange, reply) = Exchange::begin_with(
        &identity,
        &mut window,
        source(1),
        when,
        &start,
        &wide(&v["y"]),
    )
    .unwrap();
    assert_eq!(reply, t["node_reply"]);
    let confirm = client.reply(&reply).unwrap();
    assert_eq!(confirm, t["client_confirm"]);
    let (sealed, node_sealed) = exchange.confirm(&identity, &mut window, &confirm).unwrap();
    assert_eq!(STANDARD.encode(&node_sealed), t["node_sealed"]);
    let (pins, client_sealed) = client
        .finish(&node_sealed, v["label"].as_str(), v["ua"].as_str(), when)
        .unwrap();
    assert_eq!(STANDARD.encode(&client_sealed), t["client_sealed"]);
    let paired = sealed.finish(&identity, &client_sealed).unwrap();
    assert_eq!(paired.device, v["device_id"]);
    assert_eq!(paired.label.as_deref(), v["label"].as_str());
    assert_eq!(pins.cluster_id, identity.cluster_id().as_str());
    // The parse cases the browser reads too.
    for case in v["parse_cases"].as_array().unwrap() {
        let input = case["input"].as_str().unwrap();
        match case.get("code") {
            Some(code) => assert_eq!(
                Code::parse(input).map(Code::as_u16),
                Ok(code.as_u64().unwrap() as u16),
                "{input:?}"
            ),
            None => assert!(Code::parse(input).is_err(), "{input:?}"),
        }
    }
}

#[test]
#[ignore = "explicitly regenerate synthetic cross-language fixtures"]
fn generate_pake_vectors() {
    let dir = tempfile::tempdir().unwrap();
    let identity = fixture_identity(dir.path());
    let when = at("2026-09-05T00:00:00.000Z");
    let code = Code::from_u16(0x728E); // 🦊 🍕 ⚡️ 🎲
    let x = [6u8; 64];
    let y = [7u8; 64];
    let device_secret = [5u8; 32];
    let device_x25519 = [8u8; 32];
    let signing = ed25519_dalek::SigningKey::from_bytes(&device_secret);
    let device_public = STANDARD.encode(signing.verifying_key().as_bytes());
    let w = spake2::password(code).unwrap();
    let ids = Identities::new(&device_public, &identity.public_key_base64());
    let a = State::start_with(Side::A, &w, &x).unwrap();
    let b = State::start_with(Side::B, &w, &y).unwrap();
    let (pa, pb) = (a.message(), b.message());
    let a = a.finish(&pb, &ids).unwrap();
    let b = b.finish(&pa, &ids).unwrap();
    use sha2::Digest as _;
    let digest = sha2::Sha256::digest(&*a.transcript);
    let mut kc = [0u8; 32];
    hkdf::Hkdf::<sha2::Sha256>::new(None, &digest[16..])
        .expand(b"ConfirmationKeys", &mut kc)
        .unwrap();

    let mut window = Pairing::open_with(code, Duration::from_secs(120), when).unwrap();
    let hello = pair::handshake::node_hello(&identity, true);
    let (mut client, start) = Client::start_with(
        &hello,
        code,
        "browser",
        signing.clone(),
        StaticSecret::from(device_x25519),
        &x,
    )
    .unwrap();
    let (exchange, reply) =
        Exchange::begin_with(&identity, &mut window, source(1), when, &start, &y).unwrap();
    let confirm = client.reply(&reply).unwrap();
    let (sealed, node_sealed) = exchange.confirm(&identity, &mut window, &confirm).unwrap();
    let (_, client_sealed) = client
        .finish(
            &node_sealed,
            Some("Synthetic Phone"),
            Some("SyntheticBrowser/1.0"),
            when,
        )
        .unwrap();
    sealed.finish(&identity, &client_sealed).unwrap();

    let parse_cases = json!([
        {"input": "fox pizza lightning die", "code": 0x728E},
        {"input": "\u{1F98A}\u{1F355}\u{26A1}\u{FE0F}\u{1F3B2}", "code": 0x728E},
        {"input": "\u{1F98A} \u{1F355} \u{26A1} \u{1F3B2}", "code": 0x728E},
        {"input": format!("{} {}", WORDS[0x72], WORDS[0x8E]), "code": 0x728E},
        {"input": format!("{}-{}", WORDS[0x72].to_uppercase(), WORDS[0x8E]), "code": 0x728E},
        {"input": format!("{}{}", WORDS[0x72], WORDS[0x8E]), "code": 0x728E},
        {"input": format!("{} {}", &WORDS[0x72][..3], &WORDS[0x8E][..3]), "code": 0x728E},
        {"input": "Hot Pepper, Artist Palette, Maple Leaf, Game Die", "code": 0x9BDE},
        {"input": "pepper palette leaf die", "code": 0x9BDE},
        {"input": "abyss abyss", "code": 0},
        {"input": "guitar acid", "code": (0x99 << 8) | 0x01},
        {"input": ""},
        {"input": "fox pizza lightning"},
        {"input": "\u{1F98A} fox \u{26A1} \u{1F3B2}"},
        {"input": "zzzzz zzzzz"},
        {"input": format!("{} 12345", WORDS[0x72])}
    ]);
    // `guitar` is WORDS[0x99]; a change to the list would move it, so look it up.
    let guitar = WORDS.iter().position(|w| *w == "guitar").unwrap();
    let acid = WORDS.iter().position(|w| *w == "acid").unwrap();
    let mut parse_cases = parse_cases;
    parse_cases[10]["code"] = json!((guitar << 8) | acid);

    let fixture = json!({
        "code": code.as_u16(),
        "code_emoji": code.glyphs().map(|g| g.glyph),
        "code_words": code.words(),
        "M": hex(&spake2::M),
        "N": hex(&spake2::N),
        "w": STANDARD.encode(w.to_bytes()),
        "device_ed25519_secret": STANDARD.encode(device_secret),
        "device_ed25519_public": device_public,
        "device_id": privatium_core::NodeId::derive(&signing.verifying_key()).as_str(),
        "device_x25519_secret": STANDARD.encode(device_x25519),
        "node_ed25519_public": identity.public_key_base64(),
        "node_id": identity.id().as_str(),
        "node_x25519_public": identity.x25519_public_base64(),
        "cluster_public": STANDARD.encode(identity.cluster_public().as_bytes()),
        "cluster_id": identity.cluster_id().as_str(),
        "x": STANDARD.encode(x),
        "y": STANDARD.encode(y),
        "pA": STANDARD.encode(pa),
        "pB": STANDARD.encode(pb),
        "TT": STANDARD.encode(&*a.transcript),
        "Ke": STANDARD.encode(&digest[..16]),
        "Ka": STANDARD.encode(&digest[16..]),
        "KcA": STANDARD.encode(&kc[..16]),
        "KcB": STANDARD.encode(&kc[16..]),
        "cA": STANDARD.encode(a.confirm_send),
        "cB": STANDARD.encode(b.confirm_send),
        "now": "2026-09-05T00:00:00.000Z",
        "label": "Synthetic Phone",
        "ua": "SyntheticBrowser/1.0",
        "transcript": {
            "node_hello": hello,
            "client_start": start,
            "node_reply": reply,
            "client_confirm": confirm,
            "node_sealed": STANDARD.encode(&node_sealed),
            "client_sealed": STANDARD.encode(&client_sealed),
        },
        "parse_cases": parse_cases,
    });
    std::fs::write(
        FIXTURE,
        serde_json::to_string_pretty(&fixture).unwrap() + "\n",
    )
    .unwrap();
}
