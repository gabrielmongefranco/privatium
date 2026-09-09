// Project:  Privatium™  |  File: crates/privatium/tests/cluster.rs
// Authors:  Gabriel Mongefranco (@gabrielmongefranco)
// Created:  2026-09-07  |  Modified: 2026-09-07
// Summary:  Nodes of one cluster over real sockets: admission as two child binaries
//           (spec/cli.md §8, spec/protocol.md §2.3.1) — `pair --node` on one and
//           `pair --join` on the other, in both directions, a wrong code and the usage
//           error — and synchronization between in-process nodes (spec/protocol.md §10),
//           where a test needs to reach inside each node as well as across the wire.
//           See main README.md for full license information.

#![allow(clippy::unwrap_used, clippy::expect_used)]

mod common;

/// `spec/protocol.md §10.2`, `§10.4`: a node with nobody looking at it still applies
/// what arrives, and a write on one machine reaches the other without anyone asking.
/// The system log crosses before any app, so a peer is known before its rows are.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn test_sync_wakes_an_idle_drain_and_debounces_local_appends() {
    let a = common::Fixture::new().await;
    let b = common::Fixture::new().await;
    b.join(&a).await;
    {
        let mut node = a.handler.node().lock().unwrap();
        node.sys_log_mut()
            .batch(|batch| {
                for _ in 0..1000 {
                    batch.put(
                        "sys_audit",
                        &privatium_core::new_ulid(),
                        &serde_json::json!({
                            "at": privatium_core::log::now(),
                            "kind": "sync.measurement",
                            "severity": "info",
                            "detail": "{}"
                        }),
                    )?;
                }
                Ok(())
            })
            .unwrap();
        node.refresh().unwrap();
    }
    a.append("initial", "after system audit batch");
    let mut wake = {
        let mut node = b.handler.node().lock().unwrap();
        node.start_sync().unwrap();
        node.sync_events().unwrap()
    };
    let handler = b.handler.clone();
    let drain = tokio::spawn(async move {
        while wake.changed().await.is_ok() {
            handler.drain_sync().await.unwrap();
        }
    });
    let started = Instant::now();
    tokio::time::timeout(Duration::from_secs(15), async {
        while b.rows().is_empty() {
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
    })
    .await
    .unwrap();
    eprintln!(
        "first app after 1000 system audit rows: {:?}",
        started.elapsed()
    );
    b.append("automatic", "local debounce");
    tokio::time::timeout(Duration::from_secs(15), async {
        while a.rows().len() != 2 {
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
    })
    .await
    .unwrap();
    b.pass().await;
    b.pass().await;
    {
        let node = b.handler.node().lock().unwrap();
        let (seen, unsynced): (i64, i64) = node
            .store()
            .conn()
            .query_row(
                "SELECT (SELECT count(*) FROM sys_audit
                         WHERE kind = 'sync.peer_seen' AND subject = ?),
                        unsynced_peers
                   FROM v_health WHERE app_id = '_sys'",
                [a.id()],
                |row| Ok((row.get(0)?, row.get(1)?)),
            )
            .unwrap();
        assert_eq!(seen, 1);
        assert_eq!(unsynced, 0);
    }
    drain.abort();
}

/// `spec/protocol.md §10.3`: a machine that was off is not a special case. What was
/// written while it was away reaches it from whichever node has it, and it comes back
/// holding the same bytes as everyone else — the ordinary catch-up path.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn test_spec_10_3_power_cut_desktop_catches_up_through_the_laptop() {
    let desktop = common::Fixture::new().await;
    let laptop = common::Fixture::new().await;
    let third = common::Fixture::new().await;
    let browser = laptop.pair_browser().await;
    // The established laptop admits the other two; its availability confers no role.
    desktop.join(&laptop).await;
    third.join(&laptop).await;
    assert!(desktop.pass().await.peers.iter().any(|p| p.completed));
    let root = desktop.stop().await;
    let mut phone = laptop.browser_connection(&browser).await;
    let body = br#"{"events":[{"op":"put","tbl":"item","d":{"value":"offline desktop"}}]}"#;
    assert_eq!(
        phone
            .request_type("POST", "/a/sync-test/api/events", body, "application/json")
            .await
            .0,
        200
    );
    drop(phone);
    third.pass().await;
    let desktop = common::Fixture::open(root).await;
    let started = Instant::now();
    let report = desktop.pass().await;
    assert!(report.peers.iter().any(|p| p.completed), "{report:?}");
    eprintln!("desktop catch-up and rebuild: {:?}", started.elapsed());
    assert_eq!(desktop.rows(), laptop.rows());
    assert_eq!(desktop.rows(), third.rows());
    assert_eq!(desktop.bytes(&laptop.id()), laptop.bytes(&laptop.id()));
}

/// `spec/protocol.md §10.3`: no node is the authoritative copy. Whichever order the
/// three of them start passes in, all three end with the same bytes in every log.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn test_spec_10_3_no_node_is_primary() {
    let a = common::Fixture::new().await;
    let b = common::Fixture::new().await;
    let c = common::Fixture::new().await;
    b.join(&a).await;
    c.join(&a).await;
    a.append("a", "one");
    b.append("b", "two");
    c.append("c", "three");
    // Teach every node both addresses through the existing discovery facts.
    for source in [&a, &b, &c] {
        let mut node = source.handler.node().lock().unwrap();
        for key in ["discovery.mdns", "discovery.udp"] {
            node.sys_log_mut()
                .put("sys_setting", key, &serde_json::json!({"value":"false"}))
                .unwrap();
        }
        node.refresh().unwrap();
        node.serve_discovery().unwrap();
    }
    for source in [&a, &b, &c] {
        for target in [&a, &b, &c] {
            if source.port == target.port {
                continue;
            }
            let id = target.id();
            let mut node = source.handler.node().lock().unwrap();
            let cluster = node.identity().cluster_id().to_string();
            assert!(node.absorb_discovered(privatium_core::Discovered {
                id,
                cluster,
                name: "Synthetic peer".into(),
                addrs: vec!["127.0.0.1".parse().unwrap()],
                port: target.port,
                apps: vec![],
                pair: false,
                seen_at: jiff::Timestamp::now(),
                instance: String::new()
            }));
            node.refresh().unwrap();
        }
    }
    for source in [&c, &b, &a, &b, &c, &a] {
        source.pass().await;
    }
    for source in [&a, &b, &c] {
        assert_eq!(source.rows().len(), 3);
        for origin in [&a, &b, &c] {
            assert_eq!(source.bytes(&origin.id()), origin.bytes(&origin.id()));
        }
    }
}

/// `spec/protocol.md §10.4`: when the address that was working dies mid-pass, another
/// one is tried quickly rather than hung on, and the owner is told once. What crossed
/// the dead path was ciphertext, so losing it exposes nothing.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn test_spec_10_4_killing_the_active_endpoint_fails_over_in_under_five_seconds() {
    let a = common::Fixture::new().await;
    let b = common::Fixture::new().await;
    let proxy = common::Proxy::new(a.port).await;
    b.join_at(&a, proxy.port).await;
    {
        let mut node = b.handler.node().lock().unwrap();
        for key in ["discovery.mdns", "discovery.udp"] {
            node.sys_log_mut()
                .put("sys_setting", key, &serde_json::json!({"value":"false"}))
                .unwrap();
        }
        node.refresh().unwrap();
        node.serve_discovery().unwrap();
        node.start_sync().unwrap();
    }
    assert!(b.pass().await.peers[0].completed);
    let cluster = a
        .handler
        .node()
        .lock()
        .unwrap()
        .identity()
        .cluster_id()
        .to_string();
    {
        let node = b.handler.node().lock().unwrap();
        assert!(node.absorb_discovered(privatium_core::Discovered {
            id: a.id(),
            cluster,
            name: "Synthetic peer".into(),
            addrs: vec!["127.0.0.1".parse().unwrap()],
            port: a.port,
            apps: vec![],
            pair: false,
            seen_at: jiff::Timestamp::now(),
            instance: "synthetic-mdns".into()
        }));
    }
    a.append("secret-row", "synthetic-secret-marker-71ef");
    while tokio::time::timeout(Duration::from_millis(1), proxy.activity.notified())
        .await
        .is_ok()
    {}
    let handler = b.handler.clone();
    let running =
        tokio::task::spawn_blocking(move || handler.node().lock().unwrap().sync_now().unwrap());
    tokio::time::timeout(Duration::from_secs(3), proxy.activity.notified())
        .await
        .unwrap();
    let started = Instant::now();
    proxy.task.abort();
    let report = tokio::time::timeout(Duration::from_secs(5), running)
        .await
        .unwrap()
        .unwrap();
    assert!(report.peers[0].completed, "{report:?}");
    assert!(started.elapsed() < Duration::from_secs(5));
    assert_eq!(report.peers[0].url, a.origin());
    assert_eq!(a.rows(), b.rows());
    let node = b.handler.node().lock().unwrap();
    let detail: String = node
        .store()
        .conn()
        .query_row(
            "SELECT detail FROM sys_audit
              WHERE kind = 'endpoint.failover' ORDER BY \"at\" DESC LIMIT 1",
            [],
            |row| row.get(0),
        )
        .unwrap();
    assert!(detail.contains("from_kind") && detail.contains("to_kind"));
    assert!(!detail.contains("http"));
    assert!(
        !proxy
            .captured
            .lock()
            .unwrap()
            .windows(b"synthetic-secret-marker-71ef".len())
            .any(|bytes| bytes == b"synthetic-secret-marker-71ef")
    );
}

/// `spec/protocol.md §10.1`, `§10.3`: both machines were written to while neither could
/// reach the other. Sync is a union, so both sets of writes survive — nothing is
/// resolved away, and no line arrives twice.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn test_spec_10_3_offline_edits_on_both_nodes_converge() {
    let a = common::Fixture::new().await;
    let b = common::Fixture::new().await;
    b.join(&a).await;
    a.append("one", "from-a");
    b.append("two", "from-b");
    let report = b.pass().await;
    assert!(report.peers.iter().any(|peer| peer.completed), "{report:?}");
    assert_eq!(a.rows(), b.rows());
    assert_eq!(a.rows().len(), 2);
    assert_eq!(a.bytes(&a.id()), b.bytes(&a.id()));
    assert_eq!(a.bytes(&b.id()), b.bytes(&b.id()));
}

/// `spec/protocol.md §9.2`, `§8.4`: the sync routes hand over whole logs, so only
/// another node of this cluster may ask. A paired browser and the owner's own standing
/// are refused; and a node, in return, may reach nothing but sync, health and manifest.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn test_spec_9_2_sync_routes_answer_a_node_session_alone() {
    let a = common::Fixture::new().await;
    let browser = a.pair_browser().await;
    let b = common::Fixture::new().await;
    b.join(&a).await;
    let mut node = a.node_connection(&b).await;
    let mut device = a.browser_connection(&browser).await;
    for path in [
        "/api/v1/sync/heads",
        "/api/v1/sync/pull",
        "/api/v1/sync/push",
    ] {
        let method = if path.ends_with("push") {
            "POST"
        } else {
            "GET"
        };
        assert_eq!(device.request(method, path, b"").await.0, 403);
        let owner = a
            .handler
            .handle(
                axum::http::Request::builder()
                    .method(method)
                    .uri(path)
                    .body(axum::body::Body::empty())
                    .unwrap(),
            )
            .await;
        assert_eq!(owner.status(), 403);
    }
    assert_eq!(node.request("GET", "/api/v1/sync/heads", b"").await.0, 200);
    for path in [
        "/",
        "/settings",
        "/skills/privatium-lua.md",
        "/a/hello/",
        "/api/v1/pair",
    ] {
        assert_eq!(node.request("GET", path, b"").await.0, 403, "{path}");
    }
}

/// `spec/protocol.md §10.2`, `§2.3.1`: a log whose range a peer's bounded body cannot
/// hold is refused on its own. The apps after it still sync, and the pass is not
/// completed, so nothing renews on it.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn test_spec_10_2_a_log_that_cannot_be_offered_does_not_stop_the_apps_after_it() {
    let a = common::Fixture::new().await;
    let b = common::Fixture::new().await;
    b.join(&a).await;
    for fixture in [&a, &b] {
        common::set_body_bound(fixture, 8 * 1024);
    }
    b.append("one", "synthetic");

    // A batch no page can offer, as a log written before the bound existed holds one.
    // Its slug sorts before the mounted app's, so the pass meets it first.
    let dev = b.id();
    let dir = b.root.path().join("data/blocked/log");
    std::fs::create_dir_all(&dir).unwrap();
    let ts = privatium_core::log::now();
    let lines: Vec<String> = (1..=10)
        .map(|seq| {
            let batch = if seq == 1 { ",\"batch\":10" } else { "" };
            let value = "x".repeat(1024);
            format!(
                "{{\"seq\":{seq},\"lam\":{seq},\"ts\":\"{ts}\",\"dev\":\"{dev}\",\"app\":\"blocked\"{batch},\"op\":\"put\",\"tbl\":\"item\",\"id\":\"wide-{seq}\",\"d\":{{\"value\":\"{value}\"}}}}"
            )
        })
        .collect();
    std::fs::write(
        dir.join(format!("{dev}.jsonl")),
        lines.join(
            "
",
        ) + "
",
    )
    .unwrap();

    let report = b.pass().await;
    let peer = &report.peers[0];
    assert!(!peer.completed, "{report:?}");
    assert!(
        peer.refusals
            .iter()
            .any(|reason| reason == "push range exceeds the peer request body bound"),
        "{report:?}"
    );
    // The app after the refused one crossed anyway.
    assert_eq!(a.rows(), vec![("one".to_owned(), "synthetic".to_owned())]);
    assert!(!a.root.path().join("data/blocked/log").exists());
}

/// `spec/protocol.md §2.3.2`: a phone pairs with the cluster, not with one machine.
/// The second node has never met it and refuses it until the first pass carries the
/// device's row across; after that the phone simply works, with no second pairing.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn test_spec_2_3_2_a_device_pinned_to_the_cluster_reaches_an_unmet_node() {
    let a = common::Fixture::new().await;
    let browser = a.pair_browser().await;
    let b = common::Fixture::new().await;
    b.join(&a).await;
    b.refused(&browser.device).await;
    assert!(b.pass().await.peers[0].completed);
    let mut device = b.browser_connection(&browser).await;
    assert_eq!(device.request("GET", "/a/sync-test/", b"").await.0, 200);
}

/// `spec/protocol.md §8.3`, `§3`: right after admission neither node has the other's
/// row yet, so the address remembered at joining is what lets either side open the
/// first channel. Once a revocation arrives it outranks that memory, and a node nobody
/// admitted is refused whichever way it dials.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn test_spec_8_3_either_side_can_start_the_first_pass_after_admission() {
    let a = common::Fixture::new().await;
    let b = common::Fixture::new().await;
    b.join(&a).await;
    let mut inbound = b.node_connection(&a).await;
    assert_eq!(
        inbound.request("GET", "/api/v1/sync/heads", b"").await.0,
        200
    );
    let mut outbound = a.node_connection(&b).await;
    assert_eq!(
        outbound.request("GET", "/api/v1/sync/heads", b"").await.0,
        200
    );
    assert!(b.pass().await.peers[0].completed);
    b.refused("zzzzzzzz").await;
    let id = a.id();
    {
        let mut node = b.handler.node().lock().unwrap();
        node.sys_log_mut()
            .put(
                "sys_node_revocation",
                &id,
                &serde_json::json!({"revoked_at":privatium_core::log::now()}),
            )
            .unwrap();
        node.refresh().unwrap();
    }
    b.refused(&id).await;
    assert!(b.pass().await.peers.is_empty());
}

/// `spec/protocol.md §10.1`: the whole of sync, across several apps at once — each side
/// says how far it has each device's log, sends what the other lacks and asks for what
/// it lacks. Both end with the same bytes and the same tables.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn test_spec_10_1_heads_pull_and_push_are_a_set_union() {
    let a = common::Fixture::new().await;
    let b = common::Fixture::new().await;
    b.join(&a).await;
    for fixture in [&a, &b] {
        let mut node = fixture.handler.node().lock().unwrap();
        let id = node.id().to_string();
        for slug in ["first-app", "second-app"] {
            node.open_app(slug, common::DDL).unwrap();
            node.append(
                slug,
                privatium_core::Event::put("item", &id, serde_json::json!({"value":"synthetic"})),
            )
            .unwrap();
        }
    }
    a.append("a", "one");
    b.append("b", "two");
    assert!(b.pass().await.peers[0].completed);
    for slug in [common::APP, "first-app", "second-app"] {
        for dev in [a.id(), b.id()] {
            let relative = format!("data/{slug}/log/{dev}.jsonl");
            assert_eq!(
                fs::read(a.root.path().join(&relative)).unwrap(),
                fs::read(b.root.path().join(&relative)).unwrap()
            );
        }
    }
}

/// `spec/protocol.md §4.1`, `§10.2`: a page ends before a batch it cannot fit, never
/// inside one. A receiver that met half a batch would take it for a batch a crash left
/// short and skip it, so the two nodes would show different rows from the same bytes.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn test_spec_10_2_pull_keeps_batches_whole_and_defers_only_trailing_filler() {
    let a = common::Fixture::new().await;
    let b = common::Fixture::new().await;
    b.join(&a).await;
    a.append("before", "single");
    {
        let mut node = a.handler.node().lock().unwrap();
        node.append_batch(
            common::APP,
            (0..3)
                .map(|n| {
                    privatium_core::Event::put(
                        "item",
                        format!("batch-{n}"),
                        serde_json::json!({"value":"batch"}),
                    )
                })
                .collect(),
        )
        .unwrap();
    }
    let dev = a.id();
    let bytes = a.bytes(&dev);
    let pieces = bytes.split_inclusive(|b| *b == b'\n').collect::<Vec<_>>();
    let batch_len = pieces[1..].iter().map(|line| line.len()).sum::<usize>();
    {
        let mut node = a.handler.node().lock().unwrap();
        node.sys_log_mut()
            .put(
                "sys_setting",
                "api.max_body",
                &serde_json::json!({"value":batch_len.to_string()}),
            )
            .unwrap();
        node.refresh().unwrap();
    }
    let mut client = a.node_connection(&b).await;
    let path = format!("/api/v1/sync/pull?app={}&dev={dev}&after=0", common::APP);
    let (status, headers, first) = client.request("GET", &path, b"").await;
    assert_eq!(status, 200);
    assert_eq!(first, pieces[0]);
    assert_eq!(headers["pv-next"], "1");
    let (_, headers, second) = client
        .request("GET", &path.replace("after=0", "after=1"), b"")
        .await;
    assert_eq!(second, pieces[1..].concat());
    assert_eq!(headers["pv-next"], "4");
    let file = a
        .root
        .path()
        .join(format!("data/{}/log/{dev}.jsonl", common::APP));
    fs::write(&file, [bytes.clone(), b"not json\n".to_vec()].concat()).unwrap();
    let (_, _, tail) = client
        .request("GET", &path.replace("after=0", "after=4"), b"")
        .await;
    assert!(tail.is_empty());
    a.append("after", "next envelope");
    let (_, headers, tail) = client
        .request("GET", &path.replace("after=0", "after=4"), b"")
        .await;
    assert!(tail.starts_with(b"not json\n{"));
    assert_eq!(headers["pv-next"], "5");
}

/// `spec/protocol.md §10.2`: a range that does not continue the log is refused whole
/// and writes nothing — not even the lines before the bad one. The answer says where
/// the log actually stands, so the sender can ask again from there.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn test_spec_10_2_push_refuses_invalid_ranges_atomically_over_the_channel() {
    let a = common::Fixture::new().await;
    let b = common::Fixture::new().await;
    b.join(&a).await;
    b.append("one", "synthetic");
    b.append("two", "synthetic");
    let bytes = b.bytes(&b.id());
    let mut client = a.node_connection(&b).await;
    let path = format!("/api/v1/sync/push?app={}&dev={}", common::APP, b.id());
    let wrong = String::from_utf8(bytes.clone())
        .unwrap()
        .replace("\"seq\":2", "\"seq\":3");
    let (status, _, body) = client.request("POST", &path, wrong.as_bytes()).await;
    assert_eq!(status, 409);
    let error: serde_json::Value = serde_json::from_slice(&body).unwrap();
    assert_eq!(error["seq"], 3);
    assert_eq!(error["head"], 0);
    assert!(
        !a.root
            .path()
            .join(format!("data/{}/log/{}.jsonl", common::APP, b.id()))
            .exists()
    );
    assert_eq!(client.request("POST", &path, &bytes).await.0, 200);
    assert_eq!(a.bytes(&b.id()), bytes);
    assert_eq!(
        client
            .request_type("POST", &path, b"", "application/json")
            .await
            .0,
        415
    );
    for path in [
        "/api/v1/sync/pull?app=sync-test&dev=../x&after=0",
        "/api/v1/sync/pull?app=_lint&dev=aaaaaaaa&after=0",
        "/api/v1/sync/pull?app=sync-test&dev=aaaaaaaa",
        "/api/v1/sync/pull?app=sync-test&dev=aaaaaaaa&after=-1",
        "/api/v1/sync/heads?app=hello&app=other",
    ] {
        assert_eq!(client.request("GET", path, b"").await.0, 400, "{path}");
    }
    assert_eq!(
        client
            .request("GET", "/api/v1/sync/heads?app=absent", b"")
            .await
            .2,
        b"{}"
    );
    let own = format!("/api/v1/sync/push?app={}&dev={}", common::APP, a.id());
    assert_eq!(client.request("POST", &own, b"").await.0, 403);
    assert_eq!(
        client
            .request("POST", "/api/v1/sync/push?app=_lint&dev=aaaaaaaa", b"")
            .await
            .0,
        400
    );
    {
        let mut node = a.handler.node().lock().unwrap();
        node.sys_log_mut()
            .put(
                "sys_setting",
                "api.max_body",
                &serde_json::json!({"value":"512"}),
            )
            .unwrap();
        node.refresh().unwrap();
    }
    assert_eq!(client.request("POST", &path, &[b' '; 513]).await.0, 413);
}

/// `spec/protocol.md §4.1`, `§10.2`: every line of a batch a crash left short carries a
/// sequence and counts in what a node advertises, so those lines cross at once. A line
/// that is not an envelope has no sequence to travel on and waits for the next one.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn test_spec_10_2_short_tail_heads_and_filler_converge_by_sequence() {
    let a = common::Fixture::new().await;
    let b = common::Fixture::new().await;
    b.join(&a).await;
    a.append("one", "synthetic");
    let id = a.id();
    let file = a
        .root
        .path()
        .join(format!("data/{}/log/{id}.jsonl", common::APP));
    let mut bytes = a.bytes(&id);
    let ts = privatium_core::log::now();
    for seq in [2, 3] {
        let mut line = serde_json::json!({
            "seq": seq, "lam": seq, "dev": id, "app": common::APP, "ts": ts,
            "op": "put", "tbl": "item", "id": format!("short-{seq}"),
            "d": {"value": "short"}
        });
        if seq == 2 {
            line["batch"] = serde_json::json!(3);
        }
        bytes.extend_from_slice(format!("{line}\n").as_bytes());
    }
    fs::write(&file, &bytes).unwrap();
    a.handler
        .node()
        .lock()
        .unwrap()
        .refresh_app(common::APP)
        .unwrap();
    assert!(b.pass().await.peers[0].completed);
    assert_eq!(a.bytes(&id), b.bytes(&id));
    assert_eq!(a.rows(), b.rows());
    assert_eq!(a.rows().len(), 1);
    let mut client = a.node_connection(&b).await;
    let (_, _, body) = client
        .request("GET", "/api/v1/sync/heads?app=sync-test", b"")
        .await;
    assert_eq!(
        serde_json::from_slice::<serde_json::Value>(&body).unwrap()[&id],
        3
    );
    bytes.extend_from_slice(b"not json\n");
    fs::write(&file, &bytes).unwrap();
    assert!(b.pass().await.peers[0].completed);
    assert!(!b.bytes(&id).ends_with(b"not json\n"));
    assert_eq!(a.rows(), b.rows());
    a.append("next", "after corruption");
    assert!(b.pass().await.peers[0].completed);
    assert_eq!(a.bytes(&id), b.bytes(&id));
    assert_eq!(a.rows(), b.rows());
}

use std::fs;
use std::io::{BufRead as _, BufReader, Read as _, Write as _};
use std::net::TcpStream;
use std::path::Path;
use std::process::{Child, Command, Stdio};
use std::time::{Duration, Instant};

const BIN: &str = env!("CARGO_BIN_EXE_privatium");

/// A node run in the background with its output captured, killed on drop.
struct Running {
    child: Child,
    port: u16,
    dir: String,
}

impl Drop for Running {
    fn drop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}

impl Running {
    /// Start a node over `root` on a port `config.toml` names, so `privatium pair` and
    /// the node agree on it, with discovery off.
    fn start(root: &tempfile::TempDir, name: &str) -> Self {
        let port = free_port();
        let dir = root.path().join(name).to_string_lossy().into_owned();
        fs::create_dir_all(&dir).unwrap();
        fs::write(
            Path::new(&dir).join("config.toml"),
            format!("[node]\nport = {port}\n"),
        )
        .unwrap();
        let mut child = Command::new(BIN)
            .args(["--data-dir", &dir, "--no-discovery"])
            .env("PRIVATIUM_TEST_NO_CHECKOUT", "1")
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .unwrap();
        let mut reader = BufReader::new(child.stdout.take().unwrap());
        let started = Instant::now();
        loop {
            let mut line = String::new();
            let read = reader.read_line(&mut line).unwrap();
            assert_ne!(read, 0, "the node exited before announcing");
            if line.contains("privatium: listening on http://") {
                break;
            }
            assert!(
                started.elapsed() < Duration::from_secs(30),
                "no announce line"
            );
        }
        std::thread::spawn(move || {
            let mut sink = String::new();
            let _ = reader.read_to_string(&mut sink);
        });
        let stderr = child.stderr.take().unwrap();
        std::thread::spawn(move || {
            let mut sink = String::new();
            let _ = BufReader::new(stderr).read_to_string(&mut sink);
        });
        std::thread::sleep(Duration::from_millis(200));
        Self { child, port, dir }
    }

    fn id(&self) -> String {
        let (status, body) = http(self.port, "GET", "/api/v1/health", "");
        assert_eq!(status, 200, "{body}");
        let health: serde_json::Value = serde_json::from_str(&body).unwrap();
        health["id"].as_str().unwrap().to_owned()
    }

    fn cluster_pub(&self) -> Vec<u8> {
        fs::read(Path::new(&self.dir).join("identity/cluster.pub")).unwrap()
    }

    fn identity_files(&self) -> Vec<(String, Vec<u8>)> {
        let mut files: Vec<_> = fs::read_dir(Path::new(&self.dir).join("identity"))
            .unwrap()
            .map(|entry| {
                let path = entry.unwrap().path();
                (
                    path.file_name().unwrap().to_string_lossy().into_owned(),
                    fs::read(&path).unwrap(),
                )
            })
            .collect();
        files.sort();
        files
    }

    /// The URL another node dials.
    fn url(&self) -> String {
        format!("http://127.0.0.1:{}", self.port)
    }

    /// Run `pair --node` against this node in the background, and the words of the
    /// window it opened once it is open.
    fn open_node_window(&self) -> (Child, Vec<String>) {
        let pair = Command::new(BIN)
            .args(["--data-dir", &self.dir, "pair", "--node", "--timeout", "60"])
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .unwrap();
        let started = Instant::now();
        let words = loop {
            let (status, body) = http(self.port, "GET", "/api/v1/pair", "");
            assert_eq!(status, 200);
            // A window a browser consumed earlier is still reported until it is
            // retired; the one `pair --node` opened is for a node and not consumed.
            let window: serde_json::Value = serde_json::from_str(&body).unwrap();
            if window["node"] == true && window.get("consumed_by").is_none() {
                break window["words"]
                    .as_array()
                    .unwrap()
                    .iter()
                    .map(|w| w.as_str().unwrap().to_owned())
                    .collect::<Vec<_>>();
            }
            assert!(
                started.elapsed() < Duration::from_secs(20),
                "pair --node never opened a window"
            );
            std::thread::sleep(Duration::from_millis(100));
        };
        (pair, words)
    }

    /// Run `pair --join <url>` on this node with `code` on standard input; exit code,
    /// stdout, stderr.
    fn join(&self, url: &str, code: &str) -> (Option<i32>, String, String) {
        let mut join = Command::new(BIN)
            .args(["--data-dir", &self.dir, "pair", "--join", url])
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .unwrap();
        join.stdin
            .take()
            .unwrap()
            .write_all(format!("{code}\n").as_bytes())
            .unwrap();
        let output = join.wait_with_output().unwrap();
        (
            output.status.code(),
            String::from_utf8_lossy(&output.stdout).into_owned(),
            String::from_utf8_lossy(&output.stderr).into_owned(),
        )
    }
}

fn free_port() -> u16 {
    std::net::TcpListener::bind("127.0.0.1:0")
        .unwrap()
        .local_addr()
        .unwrap()
        .port()
}

/// One HTTP/1.1 exchange with a node on loopback, by hand.
fn http(port: u16, method: &str, path: &str, body: &str) -> (u16, String) {
    let mut stream = TcpStream::connect(("127.0.0.1", port)).unwrap();
    stream
        .set_read_timeout(Some(Duration::from_secs(10)))
        .unwrap();
    let request = format!(
        "{method} {path} HTTP/1.1\r\nHost: 127.0.0.1:{port}\r\nConnection: close\r\n\
         Content-Type: application/json\r\nAccept: text/html\r\nContent-Length: {}\r\n\r\n{body}",
        body.len()
    );
    stream.write_all(request.as_bytes()).unwrap();
    let mut raw = Vec::new();
    stream.read_to_end(&mut raw).unwrap();
    let split = raw.windows(4).position(|w| w == b"\r\n\r\n").unwrap();
    let head = String::from_utf8_lossy(&raw[..split]).into_owned();
    let status: u16 = head
        .lines()
        .next()
        .unwrap()
        .split(' ')
        .nth(1)
        .unwrap()
        .parse()
        .unwrap();
    (
        status,
        String::from_utf8_lossy(&raw[split + 4..]).into_owned(),
    )
}

type Ws =
    tokio_tungstenite::WebSocketStream<tokio_tungstenite::MaybeTlsStream<tokio::net::TcpStream>>;

async fn next(socket: &mut Ws) -> tokio_tungstenite::tungstenite::Message {
    use futures_util::StreamExt as _;
    tokio::time::timeout(Duration::from_secs(10), socket.next())
        .await
        .unwrap()
        .unwrap()
        .unwrap()
}

/// Pair a synthetic browser with the node on `port` over a real `/ws/pair` socket, so
/// the node has paired something and is no longer disposable.
fn pair_a_browser(port: u16) -> String {
    use futures_util::SinkExt as _;
    use privatium_core::pair::{Code, handshake::Client};
    use tokio_tungstenite::tungstenite::Message;
    let (status, body) = http(port, "POST", "/api/v1/pair", "{\"ttl\":60}");
    assert_eq!(status, 200, "{body}");
    let window: serde_json::Value = serde_json::from_str(&body).unwrap();
    let words: Vec<&str> = window["words"]
        .as_array()
        .unwrap()
        .iter()
        .map(|w| w.as_str().unwrap())
        .collect();
    let code = Code::parse(&words.join(" ")).unwrap();
    tokio::runtime::Runtime::new().unwrap().block_on(async {
        let (mut socket, _) =
            tokio_tungstenite::connect_async(format!("ws://127.0.0.1:{port}/ws/pair"))
                .await
                .unwrap();
        let hello = next(&mut socket).await.into_text().unwrap();
        let (mut client, start) = Client::start(&hello, code, "browser").unwrap();
        socket.send(Message::Text(start.into())).await.unwrap();
        let reply = next(&mut socket).await.into_text().unwrap();
        let confirm = client.reply(&reply).unwrap();
        socket.send(Message::Text(confirm.into())).await.unwrap();
        let sealed = next(&mut socket).await.into_data();
        let (paired, finish) = client
            .finish(
                &sealed,
                Some("Synthetic phone"),
                None,
                jiff::Timestamp::now(),
            )
            .unwrap();
        socket.send(Message::Binary(finish.into())).await.unwrap();
        let _ = next(&mut socket).await;
        paired.device
    })
}

/// `spec/cli.md §8`, `spec/protocol.md §2.3.1` — `pair --node` on the established node,
/// the code on the other's standard input: the fresh node joins, both commands exit 0
/// naming the cluster and the peer, the fresh node's `identity/` carries the cluster,
/// and the devices page on the established node lists it.
#[test]
fn test_spec_cli_8_pair_join_admits_this_node() {
    let root = tempfile::tempdir().unwrap();
    let a = Running::start(&root, "a");
    let b = Running::start(&root, "b");
    pair_a_browser(a.port);
    assert_ne!(a.cluster_pub(), b.cluster_pub());
    let (pair, words) = a.open_node_window();
    let (code, out, err) = b.join(&a.url(), &words.join(" "));
    assert_eq!(code, Some(0), "{out}\n{err}");
    assert!(
        out.contains("this space joined cluster") && out.contains(&a.id()),
        "{out}"
    );
    let output = pair.wait_with_output().unwrap();
    let pair_out = String::from_utf8_lossy(&output.stdout);
    assert_eq!(output.status.code(), Some(0), "{pair_out}");
    assert!(pair_out.contains("privatium pair --join"), "{pair_out}");
    assert!(pair_out.contains(&b.id()), "{pair_out}");
    assert_eq!(b.cluster_pub(), a.cluster_pub());
    let (status, page) = http(a.port, "GET", "/settings/devices", "");
    assert_eq!(status, 200);
    assert!(page.contains(&b.id()), "{page}");
    let (_, page) = http(b.port, "GET", "/settings", "");
    assert!(page.contains("Cluster ID"), "{page}");
}

/// `§2.3.1`, Phase 3b's direction — the fresh node opens the window and the established
/// node dials it: the dialed node joins the dialer's cluster.
#[test]
fn test_spec_cli_8_pair_join_from_the_established_node_admits_the_dialed_one() {
    let root = tempfile::tempdir().unwrap();
    let established = Running::start(&root, "home");
    let fresh = Running::start(&root, "vps");
    pair_a_browser(established.port);
    let before = established.cluster_pub();
    let (pair, words) = fresh.open_node_window();
    let (code, out, err) = established.join(&fresh.url(), &words.join(" "));
    assert_eq!(code, Some(0), "{out}\n{err}");
    assert!(
        out.contains("admitted space") && out.contains(&fresh.id()),
        "{out}"
    );
    let output = pair.wait_with_output().unwrap();
    assert_eq!(output.status.code(), Some(0));
    assert_eq!(
        established.cluster_pub(),
        before,
        "the established node kept its cluster"
    );
    assert_eq!(fresh.cluster_pub(), before, "the dialed node joined it");
    let (_, page) = http(established.port, "GET", "/settings/devices", "");
    assert!(page.contains(&fresh.id()), "{page}");
}

/// `§8` — a wrong code exits 1 by name and leaves this node's identity untouched; the
/// window on the other node stays open for another try.
#[test]
fn test_spec_cli_8_pair_join_with_a_wrong_code_exits_1_without_writing_identity() {
    let root = tempfile::tempdir().unwrap();
    let a = Running::start(&root, "a");
    let b = Running::start(&root, "b");
    let (pair, words) = a.open_node_window();
    let before = b.identity_files();
    let wrong: Vec<String> = words
        .iter()
        .map(|word| {
            let at = privatium_core::pair::WORDS
                .iter()
                .position(|w| w == word)
                .unwrap();
            privatium_core::pair::WORDS[(at + 1) % privatium_core::pair::WORDS.len()].to_owned()
        })
        .collect();
    let (code, out, err) = b.join(&a.url(), &wrong.join(" "));
    assert_eq!(code, Some(1), "{out}\n{err}");
    assert!(
        err.contains("did not match") || err.contains("refused"),
        "{err}"
    );
    assert_eq!(b.identity_files(), before);
    assert_ne!(a.cluster_pub(), b.cluster_pub());
    let (_, body) = http(a.port, "GET", "/api/v1/pair", "");
    assert_ne!(body.trim(), "null", "the window is still open");
    // Something that is not a code at all is refused before anything is dialed.
    let (code, _, err) = b.join(&a.url(), "not a code");
    assert_eq!(code, Some(1));
    assert!(!err.contains("refused"), "{err}");
    drop(pair);
}

/// `§8` — `--join` with `--node` or `--open` is a usage error, exit 2.
#[test]
fn test_spec_cli_8_pair_node_and_join_together_are_a_usage_error() {
    for args in [
        ["pair", "--node", "--join", "http://127.0.0.1:8420"],
        ["pair", "--open", "--join", "http://127.0.0.1:8420"],
    ] {
        let output = Command::new(BIN).args(args).output().unwrap();
        assert_eq!(output.status.code(), Some(2), "{args:?}");
        let err = String::from_utf8_lossy(&output.stderr);
        assert!(err.contains("--join"), "{err}");
    }
}
