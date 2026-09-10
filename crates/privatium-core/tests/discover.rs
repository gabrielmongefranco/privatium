// This file is part of Privatium
// crates/privatium-core/tests/discover.rs
// Author(s): Gabriel Mongefranco
// Created: 2026-09-06
// Last Modified: 2026-09-06
// Summary: spec/protocol.md §6 — the TXT record and its budget (§6.1), the instance name, the subtype
//          rule, the pair flag from one source, nodes keyed by id; the UDP responder and probe
//          over loopback with the source check and the rate limit (§6.4); both mechanisms
//          started together and stopped together (§6.5); the sys_setting switches; and a real
//          daemon browsing its own registration.
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

// AGENTS.md, Style: unwrap() is permitted in tests, and a test that hides a failure
// behind `?` is worse than one that panics with a line number.
#![allow(clippy::unwrap_used, clippy::expect_used)]

mod common;

use std::collections::BTreeMap;
use std::net::{IpAddr, Ipv4Addr, Ipv6Addr, SocketAddr, UdpSocket};
use std::time::{Duration, Instant};

use common::{lua_manifest, repo_apps_dir, sys_row, write_app};
use privatium_core::discover::mdns::{SERVICE_TYPE, Seen};
use privatium_core::discover::{
    Discovered, Discovery, Facts, INSTANCE_NAME_MAX, Options, Outcome, Switch, txt, udp,
};
use privatium_core::http::api;
use privatium_core::{AppRoot, Node, sys};
use serde_json::{Value, json};

fn now() -> jiff::Timestamp {
    jiff::Timestamp::now()
}

/// Facts a test controls entirely: no node, no clock.
fn facts(id: &str, name: &str, apps: &[&str]) -> Facts {
    Facts {
        id: id.to_owned(),
        cluster: "q4w8rt2n".to_owned(),
        name: name.to_owned(),
        apps: apps.iter().map(|s| (*s).to_owned()).collect(),
        advertised: apps.iter().map(|s| (*s).to_owned()).collect(),
        build: "custom".to_owned(),
        pair_until: None,
        port: 8420,
    }
}

/// A node with the reference apps mounted.
fn node_with_reference_apps(root: &tempfile::TempDir) -> Node {
    let mut node = Node::open(root.path()).unwrap();
    let roots = [
        AppRoot::local(node.paths().apps_dir()),
        AppRoot::bundled(repo_apps_dir()),
    ];
    node.load_apps(&roots).unwrap();
    node
}

/// Set a `sys_setting` on `node` (`spec/data-dictionary.md §3.6`).
fn set_setting(node: &mut Node, key: &str, value: &str) {
    let at = privatium_core::log::now();
    node.sys_log_mut()
        .put(
            sys::SETTING,
            key,
            &json!({ "value": value, "updated_at": at }),
        )
        .unwrap();
    node.refresh().unwrap();
}

/// Every `discovery.method` audit row as `(severity, detail)`, from the materialized
/// `_sys`; `_sys` is not an app the query API serves, so the store is read directly.
fn discovery_audits(node: &Node) -> Vec<(String, String)> {
    let mut statement = node
        .store()
        .conn()
        .prepare("SELECT severity, detail FROM sys_audit WHERE kind = ? ORDER BY id")
        .unwrap();
    statement
        .query_map([sys::KIND_DISCOVERY_METHOD], |row| {
            Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?))
        })
        .unwrap()
        .map(Result::unwrap)
        .collect()
}

fn record_map(facts: &Facts) -> BTreeMap<String, String> {
    txt::record(facts, now()).into_iter().collect()
}

// ---------------------------------------------------------------------------------------
// §6.1 — the TXT record
// ---------------------------------------------------------------------------------------

/// `§6.1` — the record carries `v`, `id`, `cl`, `nm`, `apps`, `build`, `pair` and `p`, in
/// that order, and however many apps a node mounts the whole record stays under 1300
/// bytes as DNS encodes it.
#[test]
fn test_spec_6_1_txt_record_carries_the_full_key_set_and_stays_under_1300_bytes() {
    let small = facts("k7m2q9xf", "Study", &["hello", "animals"]);
    let record = txt::record(&small, now());
    let keys: Vec<&str> = record.iter().map(|(k, _)| k.as_str()).collect();
    assert_eq!(keys, txt::KEYS);
    let map = record_map(&small);
    assert_eq!(map["v"], "1");
    assert_eq!(map["id"], "k7m2q9xf");
    assert_eq!(map["cl"], "q4w8rt2n");
    assert_eq!(map["nm"], "Study");
    assert_eq!(map["apps"], "hello,animals");
    assert_eq!(map["build"], "custom");
    assert_eq!(map["pair"], "0");
    assert_eq!(map["p"], "8420");
    assert!(txt::encoded_size(&record) < txt::BUDGET);

    // Three hundred apps of the longest legal slug: the record still fits one packet.
    let slugs: Vec<String> = (0..300)
        .map(|i| format!("app-{i:03}-{}", "x".repeat(20)))
        .collect();
    let mut big = small.clone();
    big.apps = slugs.clone();
    let record = txt::record(&big, now());
    assert!(
        txt::encoded_size(&record) <= txt::BUDGET,
        "{} bytes",
        txt::encoded_size(&record)
    );
    let apps = &record.iter().find(|(k, _)| k == "apps").unwrap().1;
    assert!(apps.ends_with(txt::ELLIPSIS), "{apps}");
    assert!(apps.starts_with(&slugs[0]), "{apps}");
    // Read back: the truncated list ends with the marker and nothing is invented.
    let seen = txt::read(&record_map(&big), vec![], 8420, "", now()).unwrap();
    assert_eq!(seen.apps.last().map(String::as_str), Some("…"));
    assert!(seen.apps.len() < slugs.len());
}

/// `§6.1` — `apps` is cut at a whole slug, from the front, and ends in `,…` whenever
/// anything was dropped; a list that fits is untouched.
#[test]
fn test_spec_6_1_apps_is_truncated_with_an_ellipsis_when_over_budget() {
    let apps: Vec<String> = ["hello", "animals", "sketch", "meds"]
        .iter()
        .map(|s| (*s).to_owned())
        .collect();
    assert_eq!(txt::fit_apps(&apps, 100), "hello,animals,sketch,meds");
    assert_eq!(txt::fit_apps(&apps, 25), "hello,animals,sketch,meds");
    // 23 bytes of room: "hello,animals" (13) + ",…" (4) fits; adding ",sketch" would not.
    assert_eq!(txt::fit_apps(&apps, 24), "hello,animals,sketch,…");
    assert_eq!(txt::fit_apps(&apps, 23), "hello,animals,…");
    assert_eq!(txt::fit_apps(&apps, 10), "hello,…");
    // Not even the first slug fits: the marker alone says the list was cut.
    assert_eq!(txt::fit_apps(&apps, 5), ",…");
    assert_eq!(txt::fit_apps(&[], 5), "");
}

/// `§6.1` — a subtype is advertised for a mounted app with `nav.advertise = true` and a
/// slug of at most fifteen characters; a longer slug is refused a subtype and warned at
/// load; `advertise = false` gets none.
#[test]
fn test_spec_6_1_subtypes_only_for_advertised_slugs_of_15_chars_or_less() {
    let root = tempfile::tempdir().unwrap();
    let mut node = Node::open(root.path()).unwrap();
    let apps = node.paths().apps_dir();
    let lua = &[(
        "app.lua",
        "local pv = require 'privatium'\npv.get('/', function() return pv.html('<h1>x</h1>') end)\n",
    )];
    write_app(&apps, "short", Some(&lua_manifest("short")), lua);
    write_app(
        &apps,
        "quiet",
        Some(&format!(
            "{}\n[nav]\nadvertise = false\n",
            lua_manifest("quiet")
        )),
        lua,
    );
    let long = "sixteen-letters1";
    assert_eq!(long.len(), 16);
    write_app(&apps, long, Some(&lua_manifest(long)), lua);
    let report = node.load_apps(&[AppRoot::local(apps)]).unwrap();
    assert_eq!(report.loaded.len(), 3, "{report:?}");
    assert!(
        report.warnings.iter().any(|w| w.to_string().contains(long)),
        "{:?}",
        report.warnings
    );

    let facts = node.discovery_facts().unwrap();
    assert_eq!(facts.apps, ["quiet", "short", long]);
    assert_eq!(facts.advertised, ["short"]);
    assert_eq!(facts.subtypes(), [format!("_short._sub.{SERVICE_TYPE}")]);
}

/// `§6.1` — the instance name is `sys_node.display_name`, or the Node ID while none is
/// set, and never more than 63 bytes, cut at a character boundary.
#[test]
fn test_spec_6_1_instance_name_is_the_display_name_or_the_node_id() {
    let root = tempfile::tempdir().unwrap();
    let mut node = Node::open(root.path()).unwrap();
    let id = node.id().as_str().to_owned();
    let current = node.discovery_facts().unwrap();
    assert_eq!(current.name, id);
    assert_eq!(current.instance_name(), id);
    assert_eq!(current.id, id);
    assert_eq!(current.cluster, node.identity().cluster_id().as_str());
    assert_eq!(current.build, "custom");
    assert_eq!(current.port, node.config().node.port);

    // The owner sets a display name: an amendment of this node's own row.
    let mut row = sys_row(&node, sys::NODE, &id).unwrap();
    row["display_name"] = Value::String("Gabriel's Study".to_owned());
    node.sys_log_mut().put(sys::NODE, &id, &row).unwrap();
    node.refresh().unwrap();
    let current = node.discovery_facts().unwrap();
    assert_eq!(current.name, "Gabriel's Study");
    assert_eq!(current.instance_name(), "Gabriel's Study");
    assert_eq!(record_map(&current)["nm"], "Gabriel's Study");

    // Sixty-three bytes, at a character boundary: 'é' is two bytes, so 40 of them
    // (80 bytes) cut to 31 (62 bytes), never to a half of one.
    let long = facts("k7m2q9xf", &"é".repeat(40), &[]);
    let name = long.instance_name();
    assert!(name.len() <= INSTANCE_NAME_MAX, "{}", name.len());
    assert_eq!(name.chars().count(), 31);
    // Blank names fall back to the ID.
    assert_eq!(facts("k7m2q9xf", "   ", &[]).instance_name(), "k7m2q9xf");
}

/// `§6.1` — a record off the wire is judged before it is kept: an `id` or a `cl` that is
/// not shaped as an ID makes it not a `pv/1` record; a name is cut to what an instance
/// name may hold, at a character boundary, with control characters dropped; `apps`
/// keeps slugs alone, at most `APPS_MAX` of them, and the marker of a truncated list; a
/// `p` that is not a port falls back to the SRV port. Nothing here panics on any input.
#[test]
fn test_spec_6_1_a_record_off_the_wire_is_validated_and_bounded() {
    let record = |pairs: &[(&str, &str)]| -> BTreeMap<String, String> {
        pairs
            .iter()
            .map(|(k, v)| ((*k).to_owned(), (*v).to_owned()))
            .collect()
    };
    let read = |txt: &BTreeMap<String, String>| {
        txt::read(
            txt,
            vec![IpAddr::V4(Ipv4Addr::new(192, 168, 1, 5))],
            8420,
            "",
            now(),
        )
    };
    let good = record(&[
        ("v", "1"),
        ("id", "k7m2q9xf"),
        ("cl", "q4w8rt2n"),
        ("nm", "Study"),
        ("apps", "hello,sketch"),
        ("p", "8421"),
    ]);
    let seen = read(&good).unwrap();
    assert_eq!(
        (seen.id.as_str(), seen.cluster.as_str(), seen.port),
        ("k7m2q9xf", "q4w8rt2n", 8421)
    );
    // The empty, the malformed and the oversized ID: not a record.
    for bad in [
        "",
        "k7m2q9x",
        "k7m2q9xfz",
        "K7M2Q9XF",
        "../etc/x",
        "k7m2q9xi",
        &"a".repeat(300),
        "<script>",
    ] {
        let mut txt = good.clone();
        txt.insert("id".into(), bad.into());
        assert!(read(&txt).is_none(), "id {bad:?}");
        let mut txt = good.clone();
        txt.insert("cl".into(), bad.into());
        assert_eq!(read(&txt).is_some(), bad.is_empty(), "cl {bad:?}");
    }
    let mut missing = good.clone();
    missing.remove("id");
    assert!(read(&missing).is_none());
    // The name: bounded, at a character boundary, without control characters.
    let mut txt = good.clone();
    txt.insert("nm".into(), format!("  {}\u{7}\n", "é".repeat(200)));
    let seen = read(&txt).unwrap();
    assert!(seen.name.len() <= INSTANCE_NAME_MAX, "{}", seen.name.len());
    assert_eq!(seen.name.chars().count(), 31);
    assert!(seen.name.chars().all(|c| c == 'é'));
    txt.insert("nm".into(), String::new());
    assert_eq!(read(&txt).unwrap().name, "");
    txt.remove("nm");
    assert_eq!(read(&txt).unwrap().name, "");
    // The apps: slugs and the marker only, bounded in number.
    let mut txt = good.clone();
    let many: Vec<String> = (0..200).map(|n| format!("app{n}")).collect();
    txt.insert(
        "apps".into(),
        format!("hello,Bad Slug,../x,,{},…", many.join(",")),
    );
    let seen = read(&txt).unwrap();
    assert_eq!(seen.apps.len(), txt::APPS_MAX);
    assert_eq!(seen.apps[0], "hello");
    assert!(
        seen.apps
            .iter()
            .all(|s| s != "Bad Slug" && s != "../x" && !s.is_empty())
    );
    txt.insert("apps".into(), "hello,…".into());
    assert_eq!(read(&txt).unwrap().apps, ["hello", "…"]);
    txt.remove("apps");
    assert!(read(&txt).unwrap().apps.is_empty());
    // The port and the version.
    let mut txt = good.clone();
    txt.insert("p".into(), "99999".into());
    assert_eq!(read(&txt).unwrap().port, 8420, "the SRV port stands in");
    txt.insert("p".into(), "-1".into());
    assert_eq!(read(&txt).unwrap().port, 8420);
    txt.insert("v".into(), "2".into());
    assert!(read(&txt).is_none());
    txt.remove("v");
    assert!(read(&txt).is_none());
    // The same bound applies to this node's own name before it is advertised.
    assert_eq!(privatium_core::discover::bound_name(" a\u{0}b "), "ab");
}

/// `§6.1`, `§9.2` — `pair` is `1` while a window is open and `0` once it closes, expires
/// or is consumed, from the same facts the manifest reads.
#[test]
fn test_spec_6_1_pair_flag_flips_when_pairing_opens() {
    let root = tempfile::tempdir().unwrap();
    let mut node = node_with_reference_apps(&root);
    let t0 = common::at("2026-09-06T12:00:00Z");
    assert!(!node.discovery_facts().unwrap().pair(t0));
    assert_eq!(api::manifest(&node).unwrap()["pair"], false);

    node.pair_at(Duration::from_secs(120), t0).unwrap();
    let facts = node.discovery_facts().unwrap();
    assert!(facts.pair(t0));
    assert!(facts.pair(t0 + Duration::from_secs(119)));
    // The window's expiry is in the facts, so an answer after it says 0 with no call.
    assert!(!facts.pair(t0 + Duration::from_secs(120)));
    // The record is written for the clock it is asked about: `1` inside the window and
    // `0` after it, whatever the wall clock says while this test runs.
    let at = |when: jiff::Timestamp| -> BTreeMap<String, String> {
        txt::record(&facts, when).into_iter().collect()
    };
    assert_eq!(at(t0)["pair"], "1");
    assert_eq!(at(t0 + Duration::from_secs(120))["pair"], "0");
    // The manifest reads the real clock — the same rule, at whatever instant that is.
    assert_eq!(api::manifest(&node).unwrap()["pair"], facts.pair(now()));

    assert!(node.close_pairing(t0).unwrap());
    let facts = node.discovery_facts().unwrap();
    assert!(!facts.pair(t0));
    assert_eq!(record_map(&facts)["pair"], "0");

    // And the running mechanisms carry the flip: the UDP answer changes at once.
    let discovery = Discovery::start(
        node.discovery_facts().unwrap(),
        Options {
            mdns: Switch::Off,
            udp: Switch::On,
            udp_port: 0,
        },
    );
    let port = discovery.udp_port().unwrap();
    let target = SocketAddr::from((Ipv4Addr::LOCALHOST, port));
    let before = udp::probe_at(target, Duration::from_millis(500)).unwrap();
    assert!(!before[0].pair);
    let mut open = node.discovery_facts().unwrap();
    open.pair_until = Some(now() + Duration::from_secs(60));
    discovery.update(open);
    // One answer per source per second (`§6.4`): the second probe waits its turn.
    std::thread::sleep(Duration::from_millis(1100));
    let after = udp::probe_at(target, Duration::from_millis(500)).unwrap();
    assert!(after[0].pair);
}

/// `§6.1` — two registrations carrying one instance name and different `id`s browse as
/// two entries: nodes are keyed by `id`, never by name (the roadmap's "distinguishable
/// by ID, not name").
#[test]
fn test_spec_6_1_two_nodes_with_one_name_are_distinct_by_id() {
    let discovery = Discovery::start(
        facts("aaaaaaaa", "Me", &[]),
        Options {
            mdns: Switch::Off,
            udp: Switch::Off,
            udp_port: 0,
        },
    );
    let fullname = format!("Study.{SERVICE_TYPE}");
    let seen = |id: &str, port: u16| {
        let mut theirs = facts(id, "Study", &["hello"]);
        theirs.port = port;
        Seen {
            fullname: fullname.clone(),
            addrs: vec![IpAddr::V4(Ipv4Addr::new(192, 168, 1, port as u8))],
            port,
            txt: record_map(&theirs),
        }
    };
    let first = seen("k7m2q9xf", 8420).read(now()).unwrap();
    let second = seen("b3nn8t2q", 8421).read(now()).unwrap();
    assert_eq!(first.name, second.name);
    discovery.absorb(first.clone());
    discovery.absorb(second.clone());
    let found = discovery.discovered();
    let ids: Vec<&str> = found.iter().map(|d| d.id.as_str()).collect();
    assert_eq!(ids, ["b3nn8t2q", "k7m2q9xf"]);
    assert_eq!(found[1].port, 8420);
    assert_eq!(found[1].apps, ["hello"]);
    assert_eq!(found[1].cluster, "q4w8rt2n");

    // The same node seen again replaces its record rather than adding one.
    let again = seen("k7m2q9xf", 8422).read(now()).unwrap();
    discovery.absorb(again);
    assert_eq!(discovery.discovered().len(), 2);
    assert_eq!(discovery.discovered()[1].port, 8422);

    // A record that is not pv/1's is not a node.
    let mut foreign = seen("zzzzzzzz", 1).txt;
    foreign.insert("v".into(), "2".into());
    assert!(txt::read(&foreign, vec![], 1, "", now()).is_none());
    let mut anonymous = seen("zzzzzzzz", 1).txt;
    anonymous.remove("id");
    assert!(txt::read(&anonymous, vec![], 1, "", now()).is_none());

    // The instance going away forgets every node resolved from it.
    discovery.forget_instance(&fullname);
    assert!(discovery.discovered().is_empty());
}

// ---------------------------------------------------------------------------------------
// §6.4 — the UDP fallback
// ---------------------------------------------------------------------------------------

/// `§6.4` — a probe of `PVDISCO1` plus a nonce is answered by unicast with the same
/// prefix, the nonce, and the TXT key set as JSON; anything else is dropped.
#[test]
fn test_spec_6_4_udp_probe_is_answered_with_the_txt_key_set() {
    let discovery = Discovery::start(
        facts("k7m2q9xf", "Study", &["hello", "sketch"]),
        Options {
            mdns: Switch::Off,
            udp: Switch::On,
            udp_port: 0,
        },
    );
    assert_eq!(discovery.status().udp, Outcome::Started);
    assert_eq!(discovery.status().mdns, Outcome::Off);
    let port = discovery.udp_port().unwrap();
    let target = SocketAddr::from((Ipv4Addr::LOCALHOST, port));

    let found = udp::probe_at(target, Duration::from_millis(500)).unwrap();
    assert_eq!(found.len(), 1, "{found:?}");
    let seen = &found[0];
    assert_eq!(seen.id, "k7m2q9xf");
    assert_eq!(seen.cluster, "q4w8rt2n");
    assert_eq!(seen.name, "Study");
    assert_eq!(seen.apps, ["hello", "sketch"]);
    assert_eq!(seen.port, 8420);
    assert_eq!(seen.addrs, [IpAddr::V4(Ipv4Addr::LOCALHOST)]);
    assert!(!seen.pair);

    // The raw datagram: prefix, nonce, then a JSON object of exactly the eight keys.
    // The probe above used this second's answer for loopback (`§6.4`).
    std::thread::sleep(Duration::from_millis(1100));
    let socket = UdpSocket::bind("127.0.0.1:0").unwrap();
    socket
        .set_read_timeout(Some(Duration::from_millis(500)))
        .unwrap();
    let nonce = [9u8, 8, 7, 6];
    let mut probe = udp::MAGIC.to_vec();
    probe.extend_from_slice(&nonce);
    socket.send_to(&probe, target).unwrap();
    let mut buffer = [0u8; 2048];
    let (len, from) = socket.recv_from(&mut buffer).unwrap();
    assert_eq!(from.port(), port);
    assert_eq!(&buffer[..8], udp::MAGIC);
    assert_eq!(buffer[8..12], nonce);
    let object: serde_json::Map<String, Value> = serde_json::from_slice(&buffer[12..len]).unwrap();
    let keys: Vec<&str> = object.keys().map(String::as_str).collect();
    let mut expected = txt::KEYS.to_vec();
    expected.sort_unstable();
    assert_eq!(keys, expected);
    assert!(object.values().all(Value::is_string));
    assert_eq!(
        udp::read_answer(&buffer[..len], nonce).unwrap()["p"],
        "8420"
    );
    assert!(udp::read_answer(&buffer[..len], [0, 0, 0, 0]).is_none());

    // Malformed probes — wrong magic, wrong length — get nothing, and the responder
    // is still there for the next well-formed one.
    for bad in [
        &b"PVDISCO2\x01\x02\x03\x04"[..],
        b"PVDISCO1",
        b"PVDISCO1\x01\x02\x03\x04\x05",
    ] {
        socket.send_to(bad, target).unwrap();
        assert!(
            socket.recv_from(&mut buffer).is_err(),
            "{bad:?} was answered"
        );
    }
    assert!(udp::probe_nonce(b"PVDISCO1\x01\x02\x03\x04").is_some());
    assert!(udp::probe_nonce(b"PVDISCO1\x01\x02\x03").is_none());
    std::thread::sleep(Duration::from_millis(1100));
    assert_eq!(
        udp::probe_at(target, Duration::from_millis(500))
            .unwrap()
            .len(),
        1
    );
}

/// `§6.4` — a probe from outside RFC 1918 / RFC 4193, link-local and loopback space is
/// not answered, and one source gets one answer per second.
#[test]
fn test_spec_6_4_udp_refuses_a_public_source_and_answers_once_a_second() {
    let allowed = [
        "10.0.0.1",
        "172.16.0.1",
        "172.31.255.254",
        "192.168.1.5",
        "169.254.1.1",
        "127.0.0.1",
        "::1",
        "fd12:3456::1",
        "fe80::1",
        "::ffff:192.168.1.5",
    ];
    for ip in allowed {
        assert!(udp::source_allowed(ip.parse().unwrap()), "{ip}");
    }
    let refused = [
        "8.8.8.8",
        "172.32.0.1",
        "192.0.2.10",
        "2001:db8::1",
        "::ffff:8.8.8.8",
        "0.0.0.0",
        "::",
    ];
    for ip in refused {
        assert!(!udp::source_allowed(ip.parse().unwrap()), "{ip}");
    }

    let mut limit = udp::RateLimit::default();
    let t0 = Instant::now();
    let a: IpAddr = Ipv4Addr::new(192, 168, 1, 5).into();
    let b: IpAddr = Ipv6Addr::LOCALHOST.into();
    assert!(limit.allow(a, t0));
    assert!(!limit.allow(a, t0 + Duration::from_millis(999)));
    assert!(
        limit.allow(b, t0 + Duration::from_millis(10)),
        "another source"
    );
    assert!(limit.allow(a, t0 + Duration::from_secs(1)));
    assert!(!limit.allow(a, t0 + Duration::from_millis(1500)));

    // Over the socket: a second probe from the same source inside a second is dropped.
    let discovery = Discovery::start(
        facts("k7m2q9xf", "Study", &[]),
        Options {
            mdns: Switch::Off,
            udp: Switch::On,
            udp_port: 0,
        },
    );
    let target = SocketAddr::from((Ipv4Addr::LOCALHOST, discovery.udp_port().unwrap()));
    let socket = UdpSocket::bind("127.0.0.1:0").unwrap();
    socket
        .set_read_timeout(Some(Duration::from_millis(400)))
        .unwrap();
    let mut buffer = [0u8; 2048];
    for (attempt, expect) in [(1, true), (2, false)] {
        socket.send_to(b"PVDISCO1\x01\x02\x03\x04", target).unwrap();
        let answered = socket.recv_from(&mut buffer).is_ok();
        assert_eq!(answered, expect, "attempt {attempt}");
    }
}

/// `§6.4` — a probe storm from invented private addresses is answered within a budget
/// and then not at all: at most `GLOBAL_PER_SECOND` answers in any second across every
/// source, at most `SOURCES_MAX` sources remembered, an unseen source refused while the
/// table is full of fresh ones, a source already known keeping its one answer a second
/// throughout, and the table forgetting after a minute. Over a fake clock.
#[test]
fn test_spec_6_4_a_probe_storm_is_bounded_by_a_global_budget_and_a_source_cap() {
    let mut limit = udp::RateLimit::default();
    let t0 = Instant::now();
    let at =
        |secs: u64, millis: u64| t0 + Duration::from_secs(secs) + Duration::from_millis(millis);
    let stranger = |n: u32| -> IpAddr { Ipv4Addr::from(0x0A00_0000 + n).into() };
    let household: IpAddr = Ipv4Addr::new(192, 168, 1, 5).into();
    assert!(limit.allow(household, at(0, 0)));
    // One second, two thousand sources: the budget answers the first few and no more.
    let answered = (1..=2000)
        .filter(|n| limit.allow(stranger(*n), at(0, 1)))
        .count();
    assert_eq!(answered, udp::GLOBAL_PER_SECOND as usize - 1);
    assert!(
        !limit.allow(household, at(0, 500)),
        "within its own second, and the budget is spent"
    );
    // The next second: another budget's worth, and the household device is answered first.
    assert!(limit.allow(household, at(1, 0)));
    let answered = (2001..=4000)
        .filter(|n| limit.allow(stranger(*n), at(1, 1)))
        .count();
    assert_eq!(answered, udp::GLOBAL_PER_SECOND as usize - 1);
    // Fill the table a budget a second, inside the minute it remembers a source for;
    // past the cap an unseen source is refused even with budget to spare, while a
    // known source is still answered.
    let mut n = 10_000;
    let mut second = 2;
    let mut in_second = 0;
    loop {
        if in_second == udp::GLOBAL_PER_SECOND {
            second += 1;
            in_second = 0;
        }
        assert!(
            second < 60,
            "the table never filled inside the minute it remembers a source for"
        );
        if !limit.allow(stranger(n), at(second, 0)) {
            break;
        }
        n += 1;
        in_second += 1;
    }
    assert!(
        in_second < udp::GLOBAL_PER_SECOND,
        "refused by the cap, with budget to spare"
    );
    assert!(
        !limit.allow(stranger(n), at(second, 100)),
        "refused again: full of fresh entries"
    );
    assert!(
        limit.allow(household, at(second, 200)),
        "a known source is unaffected"
    );
    assert!(
        limit.allow(stranger(10_000), at(second, 300)),
        "a remembered stranger too"
    );
    // A minute after the first entries were made, they are forgotten and a new source is
    // answered again.
    assert!(limit.allow(stranger(n), at(62, 0)));
    // Empty input: the very first probe of any run is answered.
    assert!(udp::RateLimit::default().allow(stranger(1), t0));
}

// ---------------------------------------------------------------------------------------
// §6.5 — together, never in sequence
// ---------------------------------------------------------------------------------------

/// `§6.5` — both mechanisms are started at once, one being refused by the platform does
/// not stop the other, and dropping the discovery stops both.
#[test]
fn test_spec_6_5_mdns_and_udp_start_together_and_stop_together() {
    // Take a port so the responder's bind fails, and see mDNS unaffected by it.
    let taken = UdpSocket::bind("0.0.0.0:0").unwrap();
    let taken_port = taken.local_addr().unwrap().port();
    let both = Options {
        mdns: Switch::On,
        udp: Switch::On,
        udp_port: taken_port,
    };
    let refused = Discovery::start(facts(&unique_id(), &unique("Study"), &["hello"]), both);
    assert!(
        matches!(refused.status().udp, Outcome::Failed(_)),
        "{:?}",
        refused.status()
    );
    let mdns_outcome = refused.status().mdns.clone();
    assert_ne!(mdns_outcome, Outcome::Off);
    drop(refused);
    drop(taken);

    let running = Discovery::start(
        facts(&unique_id(), &unique("Study"), &["hello"]),
        Options {
            mdns: Switch::On,
            udp: Switch::On,
            udp_port: 0,
        },
    );
    assert_eq!(running.status().udp, Outcome::Started);
    // The platform's answer for mDNS is whatever it was a moment ago — the UDP
    // refusal changed nothing about it.
    assert_eq!(running.status().mdns, mdns_outcome);
    let port = running.udp_port().unwrap();
    let target = SocketAddr::from((Ipv4Addr::LOCALHOST, port));
    assert_eq!(
        udp::probe_at(target, Duration::from_millis(500))
            .unwrap()
            .len(),
        1
    );
    drop(running);
    // Stopped: the port answers nobody, and can be bound again.
    assert!(
        udp::probe_at(target, Duration::from_millis(300))
            .unwrap()
            .is_empty()
    );
    UdpSocket::bind(SocketAddr::from((Ipv4Addr::UNSPECIFIED, port))).unwrap();

    // The switches: refused settings keep a mechanism off with the reason, never a
    // panic and never the other mechanism's business.
    let none = Discovery::start(
        facts("k7m2q9xf", "Study", &[]),
        Options {
            mdns: Switch::Refused("discovery.mdns is not true or false".to_owned()),
            udp: Switch::Off,
            udp_port: 0,
        },
    );
    assert_eq!(
        none.status().mdns,
        Outcome::Failed("discovery.mdns is not true or false".to_owned())
    );
    assert_eq!(none.status().udp, Outcome::Off);
    assert!(!none.status().any_started());
    assert!(none.discovered().is_empty());
}

/// `spec/data-dictionary.md §3.6` — `discovery.mdns` and `discovery.udp` each turn their
/// mechanism off; a value that is not `true` or `false` keeps it off with the reason;
/// `serve_discovery` writes one `discovery.method` row naming what it did, and a second
/// call changes nothing.
#[test]
fn test_discovery_settings_disable_each_mechanism() {
    let root = tempfile::tempdir().unwrap();
    let mut node = node_with_reference_apps(&root);
    set_setting(&mut node, "discovery.mdns", "false");
    set_setting(&mut node, "discovery.udp", "false");
    assert!(node.discovery_status().is_none());
    assert!(node.discovered().is_empty());
    node.serve_discovery().unwrap();
    let status = node.discovery_status().unwrap().clone();
    assert_eq!(status.mdns, Outcome::Off);
    assert_eq!(status.udp, Outcome::Off);
    assert!(!status.any_started());
    node.serve_discovery().unwrap();
    assert_eq!(node.discovery_status().unwrap(), &status);
    node.refresh().unwrap();
    let audits = discovery_audits(&node);
    assert_eq!(audits.len(), 1, "{audits:?}");
    assert_eq!(audits[0].0, "info");
    let detail: Value = serde_json::from_str(&audits[0].1).unwrap();
    assert_eq!(detail["mdns"], "off");
    assert_eq!(detail["udp"], "off");
    assert_eq!(detail["port"], node.config().node.port);
    drop(node);

    // Values that are not booleans keep both off, naming the key.
    let root = tempfile::tempdir().unwrap();
    let mut node = node_with_reference_apps(&root);
    set_setting(&mut node, "discovery.mdns", "\"maybe\"");
    set_setting(&mut node, "discovery.udp", "1");
    node.serve_discovery().unwrap();
    let status = node.discovery_status().unwrap();
    assert_eq!(
        status.mdns,
        Outcome::Failed("discovery.mdns is not true or false".into())
    );
    assert_eq!(
        status.udp,
        Outcome::Failed("discovery.udp is not true or false".into())
    );
    drop(node);

    // The string spellings `"true"` and `"false"` read as the booleans, per §3.6's
    // JSON-encoded scalars; only the UDP responder is asked for here, on the real
    // port, so it is started or refused by the platform and never off.
    let root = tempfile::tempdir().unwrap();
    let mut node = node_with_reference_apps(&root);
    set_setting(&mut node, "discovery.mdns", "\"false\"");
    set_setting(&mut node, "discovery.udp", "\"true\"");
    node.serve_discovery().unwrap();
    let status = node.discovery_status().unwrap();
    assert_eq!(status.mdns, Outcome::Off);
    assert_ne!(status.udp, Outcome::Off, "{status:?}");
}

/// `§6.1` — a real daemon: the node's registration of `_privatium._tcp.local.` is browsed
/// back by the same daemon with its TXT record, keyed by `id`, and a facts update is
/// reflected in what is browsed. It needs multicast, which a CI runner may not have, so
/// it runs only when `PRIVATIUM_TEST_MDNS` is set.
#[test]
fn test_spec_6_1_mdns_registration_is_browsable_and_keyed_by_id() {
    // Tests in this process register on the same host at once, and the stack renames a
    // colliding instance (`§6.1`), so this test's id and name are its own.
    let id = unique_id();
    let study = unique("Study");
    let kitchen = unique("Kitchen");
    let mine = facts(&id, &study, &["hello", "animals"]);
    let discovery = Discovery::start(
        mine.clone(),
        Options {
            mdns: Switch::On,
            udp: Switch::Off,
            udp_port: 0,
        },
    );
    assert_eq!(
        discovery.status().mdns,
        Outcome::Started,
        "no multicast on this machine: {:?}; gate with PRIVATIUM_TEST_MDNS if this is CI",
        discovery.status()
    );
    let seen = wait_for(&discovery, &id, Duration::from_secs(15)).unwrap_or_else(|| {
        panic!(
            "own registration never browsed: {:?}",
            discovery.discovered()
        )
    });
    assert_eq!(seen.name, study);
    assert_eq!(seen.cluster, "q4w8rt2n");
    assert_eq!(seen.port, 8420);
    assert_eq!(seen.apps, ["hello", "animals"]);
    assert!(!seen.pair);
    assert!(!seen.addrs.is_empty());
    assert_eq!(seen.instance, format!("{study}.{SERVICE_TYPE}"));

    // New facts replace the registration; the browser sees the new record.
    let mut renamed = mine;
    renamed.name = kitchen.clone();
    renamed.apps = vec!["sketch".to_owned()];
    discovery.update(renamed);
    let deadline = Instant::now() + Duration::from_secs(15);
    let seen = loop {
        let latest = discovery.discovered().into_iter().find(|d| d.id == id);
        if let Some(latest) = latest.as_ref()
            && latest.name == kitchen
        {
            break latest.clone();
        }
        assert!(
            Instant::now() < deadline,
            "rename never browsed: {latest:?}"
        );
        std::thread::sleep(Duration::from_millis(200));
    };
    assert_eq!(seen.apps, ["sketch"]);
    assert_eq!(seen.instance, format!("{kitchen}.{SERVICE_TYPE}"));
}

/// A name no other test in this process registers: the tag, the process and a counter.
fn unique(tag: &str) -> String {
    format!("{tag}-{}-{}", std::process::id(), next_counter())
}

/// A Node ID no other test in this process registers — shaped as one, since a record
/// whose `id` is not (`§6.1`) is not a `pv/1` record and is never kept: forty bits of the
/// process and a counter as eight Crockford characters.
fn unique_id() -> String {
    const ALPHABET: &[u8] = b"0123456789abcdefghjkmnpqrstvwxyz";
    let bits = (u64::from(std::process::id()) << 20) ^ u64::from(next_counter());
    (0..8)
        .rev()
        .map(|i| char::from(ALPHABET[((bits >> (i * 5)) & 31) as usize]))
        .collect()
}

fn next_counter() -> u32 {
    use std::sync::atomic::{AtomicU32, Ordering};
    static COUNTER: AtomicU32 = AtomicU32::new(0);
    COUNTER.fetch_add(1, Ordering::Relaxed)
}

fn wait_for(discovery: &Discovery, id: &str, timeout: Duration) -> Option<Discovered> {
    let deadline = Instant::now() + timeout;
    loop {
        if let Some(found) = discovery.discovered().into_iter().find(|d| d.id == id) {
            return Some(found);
        }
        if Instant::now() >= deadline {
            return None;
        }
        std::thread::sleep(Duration::from_millis(200));
    }
}

/// `spec/app-contract.md §6` — `serve_discovery` on a node is the whole thing: the
/// facts come from the node, the audit row is written, and `discovered` answers.
#[test]
fn test_spec_app_contract_6_serve_discovery_runs_from_the_nodes_facts() {
    let root = tempfile::tempdir().unwrap();
    let mut node = node_with_reference_apps(&root);
    set_setting(&mut node, "discovery.mdns", "false");
    node.serve_discovery().unwrap();
    let status = node.discovery_status().unwrap().clone();
    assert_eq!(status.mdns, Outcome::Off);
    // Port 52525 may be another test node's; either way the row says which.
    node.refresh().unwrap();
    let audits = discovery_audits(&node);
    assert_eq!(audits.len(), 1, "{audits:?}");
    assert_eq!(audits[0].0, "info");
    let detail: Value = serde_json::from_str(&audits[0].1).unwrap();
    assert_eq!(detail["mdns"], "off");
    assert_eq!(detail["udp"], status.udp.to_string());
    if status.udp == Outcome::Started {
        let target = SocketAddr::from((Ipv4Addr::LOCALHOST, udp::PORT));
        let found = udp::probe_at(target, Duration::from_millis(800)).unwrap();
        let me = found.iter().find(|d| d.id == node.id().as_str()).unwrap();
        assert_eq!(me.apps, ["animals", "hello", "pantry", "sketch"]);
        assert_eq!(me.port, node.config().node.port);
    }
}
