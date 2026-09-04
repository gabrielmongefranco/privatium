// Project:  Privatium™  |  File: crates/privatium/src/lint.rs
// Authors:  Gabriel Mongefranco (@gabrielmongefranco)
// Created:  2026-09-05  |  Modified: 2026-09-05
// Summary:  `privatium lint` (spec/cli.md §5): the paths named, or every installed app plus
//           the node configuration; one line per finding, or one JSON object (§5.2);
//           `--severity` as the floor; `--fix` for the mechanical corrections of §5.3, then
//           a second pass so what is printed is what remains; exit 3 when anything is
//           reported (§1). The rules themselves are the core's (privatium_core::lint).

use std::path::PathBuf;

use anyhow::{Context as _, Result};
use privatium_core::config::Config;
use privatium_core::lint::{self, Depth, Options, Report};

use crate::cli::{Format, Global, Severity};
use crate::node;

pub fn lint(
    global: &Global,
    paths: &[PathBuf],
    format: Format,
    severity: Severity,
    fix: bool,
) -> Result<u8> {
    // "plus the node configuration": the mode decides PV502 and PV506, and a config that
    // does not load would stop the node too.
    let node_paths = node::paths(global)?;
    let config =
        Config::load(node_paths.config_file()).context("reading the node configuration")?;
    let options = Options::from_config(&config);

    let run = || -> Report {
        if paths.is_empty() {
            let mut report = Report::default();
            let mut roots = vec![node_paths.apps_dir()];
            if let Some(checkout) = node::checkout_apps() {
                roots.push(checkout);
            }
            for root in roots {
                for app in lint::discover(&root, Depth::Installed) {
                    let display = app.to_string_lossy().replace('\\', "/");
                    report.apps.push(display.clone());
                    report
                        .findings
                        .extend(lint::lint_app(&app, &display, &options));
                }
            }
            report
        } else {
            lint::lint_paths(paths, &options)
        }
    };

    let mut report = run();
    if fix {
        let written = lint::apply(&report.findings).context("applying fixes")?;
        if !written.is_empty() {
            eprintln!(
                "privatium lint: fixed {} file(s): {}",
                written.len(),
                written
                    .iter()
                    .map(|p| p.to_string_lossy().replace('\\', "/"))
                    .collect::<Vec<_>>()
                    .join(", ")
            );
        }
        report = run();
    }

    let floor = match severity {
        Severity::Error => lint::Severity::Error,
        Severity::Warn => lint::Severity::Warn,
        Severity::Info => lint::Severity::Info,
    };
    let shown: Vec<&lint::Finding> = report.at_or_above(floor).collect();
    for finding in &shown {
        match format {
            Format::Text => println!("{}", finding.text()),
            Format::Json => println!("{}", finding.json()),
        }
    }
    let errors = shown
        .iter()
        .filter(|f| f.severity == lint::Severity::Error)
        .count();
    let warnings = shown
        .iter()
        .filter(|f| f.severity == lint::Severity::Warn)
        .count();
    eprintln!(
        "privatium lint: {} finding(s) — {errors} error(s), {warnings} warning(s) — in {} app(s)",
        shown.len(),
        report.apps.len()
    );
    Ok(if shown.is_empty() { 0 } else { 3 })
}
