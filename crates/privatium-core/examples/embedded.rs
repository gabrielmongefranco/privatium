// Project:  Privatium™  |  File: crates/privatium-core/examples/embedded.rs
// Authors:  Gabriel Mongefranco (@gabrielmongefranco)
// Created:  2026-09-05  |  Modified: 2026-09-05
// Summary:  Embedded mode in thirty lines (spec/app-contract.md §2.3, §6): your main(),
//           your axum router, privatium-core as the log, the store and the auth layer.
//           `cargo run --example embedded -- <data-dir>`; CI runs it and curls it (M13).

use std::net::SocketAddr;

use axum::{Router, routing::get};
use privatium_core::{Event, Node, new_ulid};
use serde_json::json;

const SCHEMA: &str = "CREATE TABLE score (id VARCHAR PRIMARY KEY, player VARCHAR, points BIGINT);";

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let data_dir = std::env::args().nth(1).unwrap_or("embedded-data".into());
    let mut node = Node::open(&data_dir)?; // the identity, the _sys log, the root's lock
    node.open_app("scores", SCHEMA)?; // this program's own app: no folder, schema inline

    let event = Event::put("score", new_ulid(), json!({"player": "ada", "points": 42}));
    node.append("scores", event)?; // seq, lam, ts and dev are the node's to stamp

    let sql = "SELECT player, points FROM score ORDER BY points DESC";
    let rows = node.query("scores", sql, &[])?; // sandboxed, typed: points is "42"
    println!("{} score(s): {}", rows.len(), serde_json::to_string(&rows)?);

    let router = Router::new()
        .route("/", get(|| async { "an embedder's own route\n" }))
        .layer(node.auth_layer()); // loopback only in this phase; 403 for anyone else
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await?;
    println!("listening on http://{}/", listener.local_addr()?);
    let service = router.into_make_service_with_connect_info::<SocketAddr>();
    axum::serve(listener, service)
        .with_graceful_shutdown(async { tokio::signal::ctrl_c().await.unwrap_or_default() })
        .await?;
    Ok(node.close()?) // writes local/state.jsonl and releases the lock
}
