// Project:  Privatium™  |  File: crates/privatium-core/src/sync/source.rs
// Authors:  Gabriel Mongefranco (@gabrielmongefranco)
// Created:  2026-09-07  |  Modified: 2026-09-07
// Summary:  Length-frozen log reads, sequence heads, and raw bounded pull pages for
//           spec/protocol.md §10.1 and §10.2.
//           See main README.md for full license information.

use std::collections::BTreeMap;
use std::fs;
use std::io::{BufRead, BufReader, Read};
use std::path::PathBuf;

use crate::log::foreign::{envelope_seq, inspect_tail, validate_destination};
use crate::{Error, NodeId, Paths, Result, io_at, log::Reader};

pub(crate) type Heads = BTreeMap<String, BTreeMap<String, u64>>;

pub(crate) fn heads(paths: &Paths, limit: usize) -> Result<Heads> {
    let mut heads = Heads::new();
    for entry in fs::read_dir(paths.data_dir()).map_err(io_at(&paths.data_dir()))? {
        let entry = entry.map_err(io_at(&paths.data_dir()))?;
        let slug = entry.file_name().to_string_lossy().into_owned();
        if validate_destination(&slug, "00000000").is_err()
            || !entry.file_type().map_err(io_at(&entry.path()))?.is_dir()
        {
            continue;
        }
        heads.insert(slug.clone(), app_heads(paths, &slug, limit)?);
    }
    Ok(heads)
}

pub(crate) fn app_heads(paths: &Paths, app: &str, limit: usize) -> Result<BTreeMap<String, u64>> {
    validate_destination(app, "00000000")?;
    let mut heads = BTreeMap::new();
    for segment in Reader::open(&paths.app_log_dir(app))?.segments() {
        if !NodeId::is_valid(segment.dev()) {
            continue;
        }
        let head = heads.entry(segment.dev().into()).or_insert(0u64);
        if let Some(seq) = inspect_tail(segment.path(), limit)?.seq {
            *head = (*head).max(seq);
        }
    }
    Ok(heads)
}

/// A source snapshot holds lengths, never an app lock or a writer.
pub(crate) struct Source {
    segments: Vec<(PathBuf, u64)>,
    app: String,
    dev: String,
    head: u64,
    limit: usize,
}

pub(crate) struct Page {
    pub(crate) lines: Vec<Vec<u8>>,
    pub(crate) next: u64,
    pub(crate) head: u64,
}

impl Source {
    pub(crate) fn open(paths: &Paths, app: &str, dev: &str, limit: usize) -> Result<Self> {
        validate_destination(app, dev)?;
        let mut segments = Vec::new();
        let mut head = 0;
        for segment in Reader::open(&paths.app_log_dir(app))?.segments_for(dev) {
            let tail = inspect_tail(segment.path(), limit)?;
            head = head.max(tail.seq.unwrap_or(0));
            segments.push((segment.path().to_path_buf(), tail.complete));
        }
        Ok(Self {
            segments,
            app: app.into(),
            dev: dev.into(),
            head,
            limit,
        })
    }

    pub(crate) fn page(&self, after: u64) -> Result<Page> {
        let mut page = Page {
            lines: Vec::new(),
            next: after.min(self.head),
            head: self.head,
        };
        // Lines are added a group at a time so a page carries a batch whole or not at
        // all: a reader that met one split across two pages would take it for a batch a
        // crash left short and skip it (`spec/protocol.md §4.1`, `§10.2`).
        let mut pending = Vec::new();
        let mut group = Group::default();
        let mut batch: Option<(String, u64, u64)> = None;
        let mut seen = 0;
        let bound = self.limit.checked_mul(2).ok_or(Error::ForeignLine {
            problem: "page bound overflow",
        })?;
        for (path, len) in &self.segments {
            let file = fs::File::open(path).map_err(io_at(path))?;
            let mut reader = BufReader::new(file.take(*len));
            loop {
                let mut line = Vec::new();
                let read = (&mut reader)
                    .take(self.limit.saturating_add(1) as u64)
                    .read_until(b'\n', &mut line)
                    .map_err(io_at(path))?;
                if read == 0 {
                    break;
                }
                if read > self.limit {
                    return Err(Error::ForeignLineTooLong { limit: self.limit });
                }
                if line.last() != Some(&b'\n') {
                    break;
                }
                let seq = envelope_seq(&line, &self.app, &self.dev)?;
                if let Some(seq) = seq {
                    seen = seq;
                }
                if seen < after || seq.is_some_and(|seq| seq <= after) {
                    continue;
                }
                let pending_bytes: usize = pending.iter().map(Vec::len).sum();
                if pending_bytes
                    .saturating_add(group.size)
                    .saturating_add(line.len())
                    > bound
                {
                    return Err(Error::ForeignLine {
                        problem: "filler and its next envelope exceed the bounded pull page",
                    });
                }
                let Some(seq) = seq else {
                    pending.push(line);
                    continue;
                };
                let meta: serde_json::Value =
                    serde_json::from_slice(&line).map_err(|_| Error::ForeignLine {
                        problem: "invalid envelope",
                    })?;
                let ts = meta
                    .get("ts")
                    .and_then(serde_json::Value::as_str)
                    .unwrap_or_default();
                let count = meta
                    .get("batch")
                    .and_then(serde_json::Value::as_u64)
                    .filter(|n| *n >= 2);
                let continuation = batch.as_ref().is_some_and(|(at, expected, _)| {
                    at == ts && *expected == seq && count.is_none()
                });
                if !continuation && !group.lines.is_empty() {
                    if !page.add(std::mem::take(&mut group), self.limit) {
                        return Ok(page);
                    }
                    batch = None;
                }
                pending.push(line);
                group.size += pending.iter().map(Vec::len).sum::<usize>();
                group.lines.append(&mut pending);
                group.next = seq;
                if continuation {
                    if let Some((_, expected, remaining)) = &mut batch {
                        *expected = expected.saturating_add(1);
                        *remaining -= 1;
                        if *remaining == 0 {
                            batch = None;
                        }
                    }
                } else {
                    batch = count.map(|n| (ts.to_owned(), seq.saturating_add(1), n - 1));
                }
                if batch.is_none() {
                    if !page.add(std::mem::take(&mut group), self.limit) {
                        return Ok(page);
                    }
                    if page.lines.iter().map(Vec::len).sum::<usize>() >= self.limit {
                        return Ok(page);
                    }
                }
            }
            // Rotation does not join a batch across segments; every replay reader applies
            // the batch rule per segment (spec/protocol.md §3.2 and §4.1).
            if !group.lines.is_empty() && !page.add(std::mem::take(&mut group), self.limit) {
                return Ok(page);
            }
            batch = None;
        }
        // Trailing lines without a seq travel only with the next envelope (§10.2).
        Ok(page)
    }
}

#[derive(Default)]
struct Group {
    lines: Vec<Vec<u8>>,
    size: usize,
    next: u64,
}

impl Page {
    fn add(&mut self, mut group: Group, limit: usize) -> bool {
        let size: usize = self.lines.iter().map(Vec::len).sum();
        if !self.lines.is_empty() && size.saturating_add(group.size) > limit {
            return false;
        }
        self.lines.append(&mut group.lines);
        self.next = group.next;
        true
    }
}
