// This file is part of Privatium
// crates/privatium-core/src/http/api.rs
// Author(s): Gabriel Mongefranco
// Created: 2026-09-03
// Last Modified: 2026-09-06
// Summary: The two unauthenticated API routes of spec/protocol.md §9.2. Health is the protocol major
//          and the Node ID and nothing else; the manifest is what discovery needs — ID,
//          display name, the mounted apps by slug and title, the pair flag — and never a row
//          count, a timestamp, or any app content.
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

use serde_json::{Value, json};

use crate::{Node, Result, StoreError, sys};

/// `GET /api/v1/health` — `{"v":1,"id":"..."}` only, spelled in that order: `serde_json`
/// would sort the keys (`preserve_order` is off, `docs/plans/phase-1.md §5`), and the
/// liveness line is the one a person reads with `curl`.
#[must_use]
pub fn health(node: &Node) -> String {
    format!(
        "{{\"v\":1,\"id\":{}}}",
        Value::String(node.id().as_str().to_owned())
    )
}

/// `GET /api/v1/manifest`.
///
/// `name` is `sys_node.display_name`, or the Node ID while the owner has set none, so a
/// client always has something to show. `apps` is every app with a mount — the ones a
/// device could open — with slug, title and icon; `pair` is whether the node is accepting a
/// pairing right now, read from the same facts the TXT record and the UDP answer carry
/// (`spec/protocol.md §6.1`) so the three never disagree.
pub fn manifest(node: &Node) -> Result<Value> {
    let facts = node.discovery_facts()?;
    let id = node.id().as_str().to_owned();
    let name = display_name(node)?.unwrap_or_else(|| id.clone());
    let apps: Vec<Value> = node
        .mounts()
        .map(|(_, app)| {
            json!({
                "slug": app.slug(),
                "title": app.manifest().app.title,
                "icon": app.manifest().app.icon,
            })
        })
        .collect();
    Ok(json!({
        "v": 1,
        "id": id,
        "name": name,
        "apps": apps,
        "pair": facts.pair(jiff::Timestamp::now()),
    }))
}

/// `sys_node.display_name`, if set.
pub fn display_name(node: &Node) -> Result<Option<String>> {
    let sql = format!("SELECT display_name FROM {} WHERE id = ?", sys::NODE);
    match node
        .store()
        .conn()
        .query_row(&sql, rusqlite::params![node.id().as_str()], |row| {
            row.get::<_, Option<String>>(0)
        }) {
        Ok(name) => Ok(name.filter(|n| !n.trim().is_empty())),
        Err(rusqlite::Error::QueryReturnedNoRows) => Ok(None),
        Err(error) => Err(crate::Error::Store(Box::new(StoreError::Sql(error)))),
    }
}
