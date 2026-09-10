// This file is part of Privatium
// crates/privatium-core/src/sync/receive.rs
// Author(s): Gabriel Mongefranco
// Created: 2026-09-07
// Last Modified: 2026-09-07
// Summary: Foreign log validation, cache rebuilds, and notification delivery for spec/protocol.md
//          §10.2, §4.3 and spec/lua-api.md §3.4.
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

use std::collections::{BTreeMap, BTreeSet};

use axum::body::Bytes;

use crate::log::foreign::Receiver;
use crate::store::events;
use crate::wire::ApiSettings;
use crate::{
    Appended, Durability, Event, Node, Result, StreamEvent, audit_recovery, boxed, note_health,
    store, sys,
};

/// Durable reception outcome. Counts describe newly accepted events, not cache winners.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ReceiveReport {
    /// Highest complete sequence in the origin's log, including rejected events.
    pub head: u64,
    /// Complete physical lines received, including malformed and short-batch lines.
    pub lines: usize,
}

impl Node {
    /// The one way another device's lines enter this node, so every check a peer's
    /// bytes must pass is here rather than in each caller.
    ///
    /// Validates and durably copies `bytes` into `app`'s log owned by `dev`. The range
    /// must be contiguous and within `api.max_body` per line. A torn foreign tail must
    /// match the prefix of the first supplied line. Refuses this node's own device.
    /// Rebuilds loaded caches, folds causal counters, and queues origin-aware callbacks.
    /// Validation errors leave the file unchanged; an I/O failure may leave a prefix.
    pub fn receive(&mut self, app: &str, dev: &str, bytes: &[u8]) -> Result<ReceiveReport> {
        let reports =
            self.receive_ranges(app, &BTreeMap::from([(dev.to_owned(), bytes.to_vec())]))?;
        reports.into_values().next().ok_or(crate::Error::Sync {
            problem: "receive result missing",
        })?
    }

    pub(crate) fn receive_ranges(
        &mut self,
        app: &str,
        ranges: &BTreeMap<String, Vec<u8>>,
    ) -> Result<BTreeMap<String, Result<ReceiveReport>>> {
        for dev in ranges.keys() {
            crate::log::foreign::validate_destination(app, dev)?;
        }
        let cutoff = store::cutoff_now();
        let loaded = self.apps.contains_key(app);
        // What the log already held, so that after the copy the events new to this node
        // can be told apart and announced once. A range may repeat lines a peer sent
        // before, and a subscriber must not see those twice (`spec/data-api.md §3`).
        let before: BTreeSet<(String, u64)> = if loaded {
            events::read_log(&self.paths.app_log_dir(app), app, &cutoff)
                .map_err(boxed)?
                .into_iter()
                .map(|event| (event.dev, event.seq))
                .collect()
        } else {
            BTreeSet::new()
        };
        let mut reports = BTreeMap::new();
        let mut touched = false;
        for (dev, bytes) in ranges {
            let result = (|| {
                let mut receiver = Receiver::open(
                    &self.paths,
                    app,
                    dev,
                    self.id(),
                    Durability::Sync,
                    ApiSettings::read(self).max_body,
                )?;
                let head = if bytes.is_empty() {
                    receiver.head()
                } else if receiver.torn_tail().is_empty() {
                    receiver.append(bytes)?
                } else {
                    receiver.complete_torn(bytes)?
                };
                Ok(ReceiveReport {
                    head,
                    lines: bytes.iter().filter(|b| **b == b'\n').count(),
                })
            })();
            if let Err(crate::Error::ForeignDiverged { path, offset }) = &result {
                eprintln!(
                    "privatium: cannot complete foreign log {} at byte {offset}: origin prefix differs; compare the origin and this copy before retrying",
                    path.display()
                );
            }
            touched |= !bytes.is_empty() && result.is_ok();
            reports.insert(dev.clone(), result);
        }
        if !touched {
            return Ok(reports);
        }
        if app == sys::SLUG {
            let recovered = self.sys.rescan()?;
            audit_recovery(&mut self.sys, app, &recovered)?;
            self.store.restore(&cutoff).map_err(boxed)?;
            self.audit_standing(jiff::Timestamp::now())?;
            self.store.refresh(&cutoff).map_err(boxed)?;
        } else if let Some(loaded) = self.apps.get_mut(app) {
            let recovered = loaded.log.rescan()?;
            audit_recovery(&mut self.sys, app, &recovered)?;
            // Foreign causal ranks can be below cached winners; blind incremental apply
            // would overwrite them (store/materialize.rs, spec/protocol.md §4.5).
            loaded.store.restore(&cutoff).map_err(boxed)?;
            note_health(&self.store, &loaded.store, app)?;
            let accepted: BTreeMap<(String, u64), events::Event> =
                events::read_log(loaded.log.log_dir(), app, &cutoff)
                    .map_err(boxed)?
                    .into_iter()
                    .filter(|event| {
                        ranges.contains_key(&event.dev)
                            && !before.contains(&(event.dev.clone(), event.seq))
                    })
                    .map(|event| ((event.dev.clone(), event.seq), event))
                    .collect();
            let mut batch: Option<Appended> = None;
            for segment in loaded
                .log
                .reader()?
                .segments()
                .iter()
                .filter(|segment| ranges.contains_key(segment.dev()))
            {
                let dev = segment.dev();
                for line in segment.lines()? {
                    let line = line?;
                    let Some(seq) = crate::log::foreign::envelope_seq(line.raw(), app, dev)? else {
                        continue;
                    };
                    let Some(event) = accepted.get(&(dev.to_owned(), seq)) else {
                        continue;
                    };
                    let raw = line.raw().to_vec();
                    let _ = loaded.stream.send(StreamEvent::Append {
                        lam: event.lam,
                        line: Bytes::from(raw.clone()),
                    });
                    // Embedded and web apps have no Lua callback consumer. Retaining
                    // their batches would turn routine drains into an unbounded queue.
                    if loaded.lua_host().is_none() {
                        continue;
                    }
                    let ts = event.ts.clone().unwrap_or_default();
                    let starts = batch.as_ref().is_none_or(|b| {
                        b.dev != dev || b.ts != ts || b.seq + b.events.len() as u64 != seq
                    });
                    if starts {
                        if let Some(previous) = batch.take() {
                            let origin = previous.dev.clone();
                            loaded.unfired.push((previous, origin));
                        }
                        batch = Some(Appended {
                            ts,
                            seq,
                            lam: event.lam,
                            dev: dev.into(),
                            app: app.into(),
                            events: Vec::new(),
                            lines: Vec::new(),
                        });
                    }
                    if let Some(batch) = &mut batch {
                        batch.events.push(Event {
                            tbl: event.tbl.clone(),
                            id: event.id.clone(),
                            d: event.d.as_deref().map(serde_json::from_str).transpose()?,
                        });
                        batch.lines.push(raw);
                    }
                }
            }
            if let Some(batch) = batch {
                let origin = batch.dev.clone();
                loaded.unfired.push((batch, origin));
            }
            self.store.refresh(&cutoff).map_err(boxed)?;
        } else {
            // Unmounted data participates in the same recovery and audit rules, without
            // creating an app cache or opening a local writer (spec/protocol.md §10.2).
            let reader = crate::log::Reader::open(&self.paths.app_log_dir(app))?;
            let known = self.state.get(app).cloned().unwrap_or_default();
            let recovered = crate::log::recover(
                &reader,
                self.id(),
                known.lam,
                &known.heads,
                jiff::Timestamp::now(),
            )?;
            audit_recovery(&mut self.sys, app, &recovered)?;
            self.state.set(app, recovered.lam.get(), recovered.heads);
            self.store.refresh(&cutoff).map_err(boxed)?;
        }
        self.flush()?;
        self.publish_peers()?;
        Ok(reports)
    }

    /// Take durable received batches awaiting `pv.on('append')` for `slug`. Callbacks
    /// run after the node lock is released. Unknown apps have no pending callbacks.
    pub fn take_unfired(&mut self, slug: &str) -> Vec<(Appended, String)> {
        self.apps
            .get_mut(slug)
            .map(|app| std::mem::take(&mut app.unfired))
            .unwrap_or_default()
    }
}
