// This file is part of Privatium
// crates/privatium-core/examples/embedded.rs
// Author(s): Gabriel Mongefranco
// Created: 2026-09-05
// Last Modified: 2026-09-06
// Summary: Embedded mode in thirty lines (spec/app-contract.md §2.3, §6): your main(), your axum
//          router, privatium-core as the log, the store and the auth layer. `cargo run
//          --example embedded -- <data-dir>`; CI runs it and curls it.
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
