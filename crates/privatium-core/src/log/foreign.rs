// This file is part of Privatium
// crates/privatium-core/src/log/foreign.rs
// Author(s): Gabriel Mongefranco
// Created: 2026-09-07
// Last Modified: 2026-09-07
// Summary: Byte-preserving reception of another device's log, with contiguous sequence validation and
//          append-only torn-line completion (spec/protocol.md §10.2).
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

use std::fs::{self, File};
use std::io::{Read, Seek, SeekFrom, Write};
use std::path::{Path, PathBuf};

use serde_json::Value;

use super::{Durability, Reader};
use crate::{Error, NodeId, Paths, Result, io_at};

/// The sole append path for another device's bytes (`spec/protocol.md §10.2`).
/// The owner must hold the data-root lock and serialize receivers for the same log.
#[derive(Debug)]
pub struct Receiver {
    path: PathBuf,
    app: String,
    dev: String,
    head: u64,
    torn: Vec<u8>,
    offset: u64,
    durability: Durability,
    line_limit: usize,
    poisoned: bool,
}

impl Receiver {
    /// Inspect the foreign stream under `paths` without creating anything. Refuses this
    /// node's `own` ID, unsafe names, an unreadable tail, or a line beyond `line_limit`.
    /// `durability` governs the flush after each accepted range.
    pub fn open(
        paths: &Paths,
        app: &str,
        dev: &str,
        own: &NodeId,
        durability: Durability,
        line_limit: usize,
    ) -> Result<Self> {
        validate_destination(app, dev)?;
        if dev == own.as_str() {
            return Err(invalid("cannot receive this node's own log"));
        }
        if line_limit == 0 {
            return Err(invalid("line bound must be positive"));
        }
        let reader = Reader::open(&paths.app_log_dir(app))?;
        let segments: Vec<_> = reader
            .segments()
            .iter()
            .filter(|s| s.dev() == dev)
            .collect();
        let path = segments.last().map_or_else(
            || paths.app_log_dir(app).join(format!("{dev}.jsonl")),
            |segment| segment.path().to_path_buf(),
        );
        let mut head = 0;
        let mut torn = Vec::new();
        let mut offset = 0;
        for segment in segments.iter().rev() {
            let tail = inspect_tail(segment.path(), line_limit)?;
            if segment.path() == path {
                torn = tail.torn;
                offset = tail.complete;
            } else if !tail.torn.is_empty() {
                return Err(invalid("an earlier segment has an incomplete line"));
            }
            if let Some(seq) = tail.seq {
                head = seq;
                break;
            }
        }
        Ok(Self {
            path,
            app: app.into(),
            dev: dev.into(),
            head,
            torn,
            offset,
            durability,
            line_limit,
            poisoned: false,
        })
    }

    /// Highest complete envelope sequence in the origin's stream.
    #[must_use]
    pub fn head(&self) -> u64 {
        self.head
    }

    /// Unfinished bytes at the end of the last segment, never including a newline.
    #[must_use]
    pub fn torn_tail(&self) -> &[u8] {
        &self.torn
    }

    /// Validate the entire newline-terminated range, then append and flush it once.
    /// Empty input is a no-op. A rejected range changes neither bytes nor head. An I/O
    /// failure poisons this receiver; reopen it to inspect whatever prefix landed.
    pub fn append(&mut self, bytes: &[u8]) -> Result<u64> {
        if bytes.is_empty() {
            return Ok(self.head);
        }
        if !self.torn.is_empty() {
            return Err(invalid("complete the foreign line before appending"));
        }
        let head = self.validate(bytes)?;
        self.commit(bytes)?;
        self.head = head;
        self.offset += bytes.len() as u64;
        Ok(head)
    }

    /// Complete a torn tail from the origin's full line and any following lines. The
    /// stored bytes must be a strict prefix; only its missing suffix is written. A
    /// mismatch or invalid range leaves the existing prefix untouched.
    pub fn complete_torn(&mut self, bytes: &[u8]) -> Result<u64> {
        if self.torn.is_empty() || bytes.len() <= self.torn.len() || !bytes.starts_with(&self.torn)
        {
            return Err(Error::ForeignDiverged {
                path: self.path.clone(),
                offset: self.offset,
            });
        }
        let head = self.validate(bytes)?;
        self.commit(&bytes[self.torn.len()..])?;
        self.offset += bytes.len() as u64;
        self.head = head;
        self.torn.clear();
        Ok(head)
    }

    fn validate(&self, bytes: &[u8]) -> Result<u64> {
        if bytes.last() != Some(&b'\n') {
            return Err(invalid("range ends inside a line"));
        }
        let mut head = self.head;
        for line in bytes.split_inclusive(|b| *b == b'\n') {
            if line.len() > self.line_limit {
                return Err(Error::ForeignLineTooLong {
                    limit: self.line_limit,
                });
            }
            if let Some(seq) = envelope_seq(line, &self.app, &self.dev)? {
                let expected = head
                    .checked_add(1)
                    .ok_or_else(|| invalid("sequence exhausted"))?;
                if seq != expected {
                    return Err(Error::ForeignSeq {
                        app: self.app.clone(),
                        dev: self.dev.clone(),
                        expected,
                        found: seq,
                    });
                }
                head = seq;
            }
        }
        Ok(head)
    }

    fn commit(&mut self, bytes: &[u8]) -> Result<()> {
        if self.poisoned {
            return Err(invalid("reopen the receiver after its failed write"));
        }
        let parent = self
            .path
            .parent()
            .ok_or_else(|| invalid("missing log directory"))?;
        fs::create_dir_all(parent).map_err(io_at(parent))?;
        let existed = self.path.exists();
        let mut file = fs::OpenOptions::new()
            .create(true)
            .append(true)
            .open(&self.path)
            .map_err(io_at(&self.path))?;
        // A failed flush may leave a prefix; no further write is safe before a fresh scan.
        self.poisoned = true;
        file.write_all(bytes).map_err(io_at(&self.path))?;
        if self.durability == Durability::Sync {
            file.sync_all().map_err(io_at(&self.path))?;
            if !existed {
                crate::durable::sync_dir(parent).map_err(io_at(parent))?;
            }
        }
        self.poisoned = false;
        Ok(())
    }
}

pub(crate) fn validate_destination(app: &str, dev: &str) -> Result<()> {
    if !(app == "_sys"
        || (crate::app::manifest::is_valid_slug(app) && !crate::app::manifest::is_reserved(app)))
    {
        return Err(invalid("invalid app slug"));
    }
    if !NodeId::is_valid(dev) {
        return Err(invalid("invalid device ID"));
    }
    Ok(())
}

fn invalid(problem: &'static str) -> Error {
    Error::ForeignLine { problem }
}

pub(crate) fn envelope_seq(line: &[u8], app: &str, dev: &str) -> Result<Option<u64>> {
    let Ok(value) = serde_json::from_slice::<Value>(line) else {
        return Ok(None);
    };
    let Some(object) = value
        .as_object()
        .filter(|object| object.contains_key("seq"))
    else {
        return Ok(None);
    };
    let seq = object
        .get("seq")
        .and_then(Value::as_u64)
        .filter(|n| *n > 0)
        .ok_or_else(|| invalid("invalid envelope sequence"))?;
    if value.get("dev").and_then(Value::as_str) != Some(dev) {
        return Err(Error::ForeignEnvelope {
            seq,
            problem: "envelope device differs from destination",
        });
    }
    if value.get("app").and_then(Value::as_str) != Some(app) {
        return Err(Error::ForeignEnvelope {
            seq,
            problem: "envelope app differs from destination",
        });
    }
    if value
        .get("lam")
        .and_then(Value::as_u64)
        .filter(|n| *n > 0)
        .is_none()
        || value
            .get("ts")
            .and_then(Value::as_str)
            .and_then(|s| s.parse::<jiff::Timestamp>().ok())
            .is_none()
        || !["op", "tbl", "id"].iter().all(|key| {
            value
                .get(key)
                .and_then(Value::as_str)
                .is_some_and(|s| !s.is_empty())
        })
        || (value.get("op").and_then(Value::as_str) == Some("put")
            && !value.get("d").is_some_and(Value::is_object))
    {
        return Err(Error::ForeignEnvelope {
            seq,
            problem: "invalid envelope shape",
        });
    }
    Ok(Some(seq))
}

pub(crate) struct Tail {
    pub(crate) seq: Option<u64>,
    pub(crate) torn: Vec<u8>,
    pub(crate) complete: u64,
}

// Backwards block reads keep head discovery proportional to the last envelope, even
// when the file is larger than memory. Malformed and blank lines retain their bytes.
pub(crate) fn inspect_tail(path: &Path, limit: usize) -> Result<Tail> {
    let mut file = File::open(path).map_err(io_at(path))?;
    let mut pos = file.metadata().map_err(io_at(path))?.len();
    let mut complete = pos;
    let mut reversed = Vec::new();
    let mut torn = None;
    let mut block = [0u8; 4096];
    while pos > 0 {
        let size = usize::try_from(pos.min(block.len() as u64))
            .map_err(|_| invalid("invalid file length"))?;
        pos -= size as u64;
        file.seek(SeekFrom::Start(pos)).map_err(io_at(path))?;
        file.read_exact(&mut block[..size]).map_err(io_at(path))?;
        for (index, byte) in block[..size].iter().enumerate().rev() {
            if *byte == b'\n' {
                reversed.reverse();
                if torn.is_none() {
                    complete = pos + index as u64 + 1;
                    torn = Some(std::mem::take(&mut reversed));
                } else if let Some(seq) = tail_seq(&reversed) {
                    return Ok(Tail {
                        seq: Some(seq),
                        torn: torn.unwrap_or_default(),
                        complete,
                    });
                }
                reversed.clear();
            } else {
                reversed.push(*byte);
                if reversed.len() >= limit {
                    return Err(Error::ForeignLineTooLong { limit });
                }
            }
        }
    }
    reversed.reverse();
    match torn {
        Some(torn) => Ok(Tail {
            seq: tail_seq(&reversed),
            torn,
            complete,
        }),
        // No newline anywhere: the whole file is one unfinished line, so it is the torn
        // tail and no sequence is complete. A head taken from it would be counted twice
        // when the origin sends that line whole.
        None => Ok(Tail {
            seq: None,
            torn: reversed,
            complete: 0,
        }),
    }
}

fn tail_seq(line: &[u8]) -> Option<u64> {
    let value: Value = serde_json::from_slice(line).ok()?;
    value.get("seq").and_then(Value::as_u64)
}
