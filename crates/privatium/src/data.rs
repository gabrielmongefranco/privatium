// Project:  Privatium™  |  File: crates/privatium/src/data.rs
// Authors:  Gabriel Mongefranco (@gabrielmongefranco)
// Created:  2026-09-04  |  Modified: 2026-09-04
// Summary:  `privatium snapshot` and `privatium restore` (spec/cli.md §7). Snapshot writes
//           the SQLite + CSV + schema.sql set of spec/protocol.md §5 for one app or all of
//           them, or with --verify recomputes every existing snapshot's checksums and
//           writes nothing. Restore brings a backed-up data/ folder in (core::backup), then
//           rebuilds each app's cache by the three tiers and says which one it used;
//           --dry-run prints the plan and the prediction instead.

use std::path::Path;

use anyhow::{Context as _, Result, bail};
use privatium_core::backup::Plan;
use privatium_core::store::snapshot;
use privatium_core::{Node, Restored, Tier, sys};

use crate::cli::Global;
use crate::node;

/// The apps a `--app`-less command works on: `_sys` and every loaded app.
fn targets(
    node: &Node,
    app: Option<&str>,
    report: &privatium_core::LoadReport,
) -> Result<Vec<String>> {
    match app {
        Some(slug) if slug == sys::SLUG || node.app(slug).is_some() => Ok(vec![slug.to_owned()]),
        Some(slug) => {
            let why = node::failure_of(report, slug)
                .unwrap_or_else(|| "no such app under apps/".to_owned());
            bail!("--app {slug}: not loaded — {why}")
        }
        None => Ok(std::iter::once(sys::SLUG.to_owned())
            .chain(report.loaded.iter().cloned())
            .collect()),
    }
}

pub fn snapshot(global: &Global, app: Option<&str>, verify: bool) -> Result<u8> {
    let (mut node, report) = node::open_loaded(global)?;
    let apps = targets(&node, app, &report)?;
    let mut mismatches = 0usize;

    for slug in &apps {
        if verify {
            let dir = node.paths().app_snap_dir(slug);
            let ids = snapshot::list(&dir).with_context(|| format!("listing {}", dir.display()))?;
            if ids.is_empty() {
                println!("{slug}: no snapshots");
                continue;
            }
            for id in ids {
                let verification = node
                    .verify_snapshot(slug, &id)
                    .with_context(|| format!("{slug}: verifying {id}"))?;
                for table in &verification.tables {
                    let problem = match (table.sqlite_ok, table.csv_ok) {
                        (true, true) => continue,
                        (false, true) => "sqlite mismatch",
                        (true, false) => "csv mismatch",
                        (false, false) => "sqlite and csv mismatch",
                    };
                    mismatches += 1;
                    println!("{slug}: {id}: {}: {problem}", table.name);
                }
                println!(
                    "{slug}: {id}: {}",
                    if verification.ok() { "ok" } else { "MISMATCH" }
                );
            }
        } else {
            let written = node
                .snapshot(slug)
                .with_context(|| format!("{slug}: writing a snapshot"))?;
            println!(
                "{slug}: {} — {} table(s), {} bytes, {}",
                written.id,
                written.manifest.tables.len(),
                written.bytes,
                written.dir.display()
            );
        }
    }
    node.flush()?;

    if mismatches > 0 {
        eprintln!(
            "privatium: {mismatches} file(s) do not match MANIFEST.json; the three-tier read \
             will skip them (spec/protocol.md §5.3)"
        );
        return Ok(1);
    }
    Ok(0)
}

pub fn restore(global: &Global, from: &Path, app: Option<&str>, dry_run: bool) -> Result<u8> {
    let paths = node::paths(global)?;
    let plan =
        Plan::build(from, &paths, app).with_context(|| format!("reading {}", from.display()))?;
    println!("restore from {}:", plan.from.display());
    for copy in &plan.copies {
        println!(
            "  copy  {}/{} ({})",
            copy.slug,
            copy.what,
            copy.reason.as_str()
        );
    }
    for skip in &plan.skipped {
        println!("  keep  {}/{} ({})", skip.slug, skip.what, skip.reason);
    }
    for conflict in &plan.conflicts {
        println!("  DIVERGED  {}/{}", conflict.slug, conflict.what);
    }
    if plan.copies.is_empty() && plan.skipped.is_empty() && plan.conflicts.is_empty() {
        println!("  nothing for {}", app.unwrap_or("any app"));
    }

    if !plan.is_applicable() {
        // The error names the count and the first file; nothing was written.
        plan.apply()?;
    }

    if dry_run {
        println!("dry run: nothing copied. The tiers as the node stands now:");
    } else {
        plan.apply().context("copying the backup in")?;
    }

    // The rebuild. The node opens after the copy, so the logs it scans are the merged ones.
    let (mut node, report) = node::open_loaded(global)?;
    let mut unexpected = false;
    let mut slugs = plan.slugs();
    if let Some(only) = app {
        slugs.retain(|slug| slug == only);
    }
    for slug in &slugs {
        let loaded = slug == sys::SLUG || node.app(slug).is_some();
        if !loaded {
            println!("{slug}: data in place; no app folder loaded, so its cache is not built");
            continue;
        }
        let restored = if dry_run {
            node.restore_dry_run(slug)?
        } else {
            node.restore(slug)?
        };
        report_tier(slug, &restored, dry_run);
        unexpected |= restored.unexpected();
    }
    node.flush()?;
    let _ = report;

    if unexpected {
        eprintln!(
            "privatium: a snapshot that applied could not be read and the full replay was used \
             (spec/cli.md §7, spec/protocol.md §5.3)"
        );
        return Ok(1);
    }
    Ok(0)
}

fn report_tier(slug: &str, restored: &Restored, predicted: bool) {
    let verb = if predicted { "would use" } else { "used" };
    let source = match (&restored.snapshot, restored.tier) {
        (Some(id), Tier::Sqlite | Tier::Csv) => format!(" from {id}"),
        _ => String::new(),
    };
    println!(
        "{slug}: {verb} tier {} ({}){source}",
        restored.tier.as_u8(),
        restored.tier.name()
    );
    for skipped in &restored.skipped {
        println!(
            "  tier {} skipped: {}",
            skipped.tier.as_u8(),
            skipped.reason
        );
    }
}
