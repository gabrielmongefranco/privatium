// Project:  Privatium™  |  File: crates/privatium/src/new.rs
// Authors:  Gabriel Mongefranco (@gabrielmongefranco)
// Created:  2026-09-04  |  Modified: 2026-09-06
// Summary:  `privatium new` (spec/cli.md §4): decide what to write — an empty app for the
//           tier, a rewritten copy of an existing one, the CRUD screens for a table, or
//           with --examples every example app the binary carries — from the generator in
//           the core, then write it under <data-dir>/apps/ without overwriting a single
//           file. The first-run copy of §2 uses the same writer. No node is opened;
//           nothing here has a runtime presence.

use std::collections::BTreeMap;
use std::fs;
use std::path::{Path, PathBuf};

use anyhow::{Context as _, Result, bail};
use privatium_core::Schema;
use privatium_core::app::examples;
use privatium_core::app::manifest::{self, Tier};
use privatium_core::app::scaffold::{self, File};

use crate::cli::{Global, HELP};
use crate::node;

pub fn new(
    global: &Global,
    slug: &str,
    tier: Option<Tier>,
    from: Option<&str>,
    scaffold_table: Option<&str>,
) -> Result<u8> {
    if !manifest::is_valid_slug(slug) {
        return usage(format!(
            "new: {slug:?} is not a slug — ^[a-z][a-z0-9-]{{1,30}}$ (spec/app-contract.md §3)"
        ));
    }
    if manifest::is_reserved(slug) {
        return usage(format!(
            "new: {slug:?} is a reserved slug (spec/protocol.md §1.1)"
        ));
    }

    let paths = node::paths(global)?;
    let apps_dir = paths.apps_dir();
    let target = apps_dir.join(slug);
    let exists = target.is_dir();
    if exists && scaffold_table.is_none() {
        bail!(
            "{}: already exists; `--scaffold <table>` adds screens to an existing app, \
             anything else needs a fresh folder",
            target.display()
        );
    }
    if exists && from.is_some() {
        bail!(
            "{}: already exists; --from copies into a new folder",
            target.display()
        );
    }

    // What to write, by path. A later source replaces an earlier one within this one
    // invocation — `--from hello --scaffold profile` copies hello and then the scaffold's
    // app.lua and views take the place of hello's — but nothing on disk is ever replaced.
    let title = scaffold::title_from_slug(slug);
    let mut files: BTreeMap<String, Vec<u8>> = BTreeMap::new();

    if let Some(source) = from {
        let (copied, origin) = match resolve_source(&apps_dir, source)? {
            Source::Folder(dir) => (
                scaffold::copy(&dir, slug, &title)
                    .with_context(|| format!("copying {}", dir.display()))?,
                dir.display().to_string(),
            ),
            Source::Example(example) => (
                scaffold::copy_files(examples::files(example).unwrap_or_default(), slug, &title),
                format!("the example app {example}"),
            ),
        };
        let copied_tier = copied
            .iter()
            .find(|f| f.path == "app.toml")
            .and_then(|f| std::str::from_utf8(&f.contents).ok())
            .and_then(|text| manifest::Manifest::parse(text).ok())
            .map(|m| m.app.tier);
        if let (Some(asked), Some(found)) = (tier, copied_tier)
            && asked != found
        {
            return usage(format!(
                "new: --tier {} but {source} is a {} app; drop --tier to copy it",
                asked.as_str(),
                found.as_str()
            ));
        }
        eprintln!("privatium new: copying {origin} as {slug}");
        add(&mut files, copied);
    } else if !exists {
        add(
            &mut files,
            scaffold::fresh(slug, &title, tier.unwrap_or(Tier::Lua)),
        );
    }

    if let Some(table) = scaffold_table {
        // The schema is the target's own: what the copy brought, or what is already there.
        let ddl = match files.get("schema.sql") {
            Some(bytes) => String::from_utf8_lossy(bytes).into_owned(),
            None => {
                let path = target.join("schema.sql");
                fs::read_to_string(&path).with_context(|| {
                    format!(
                        "{}: --scaffold reads the app's schema.sql, and there is none — write \
                         `CREATE TABLE {table} (id VARCHAR PRIMARY KEY, …)` there first \
                         (spec/cli.md §4)",
                        path.display()
                    )
                })?
            }
        };
        let schema = Schema::parse(&ddl).context("parsing schema.sql")?;
        let generated = scaffold::crud(&schema, table)?;
        add(&mut files, generated);
    }

    // Refuse before writing anything, so a half-written folder never exists.
    for path in files.keys() {
        let on_disk = target.join(path);
        if on_disk.exists() {
            bail!(
                "{}: exists; `privatium new` never overwrites — delete it or choose another slug",
                on_disk.display()
            );
        }
    }
    fs::create_dir_all(&target).with_context(|| format!("creating {}", target.display()))?;
    for (path, contents) in &files {
        let on_disk = target.join(path);
        if let Some(parent) = on_disk.parent() {
            fs::create_dir_all(parent).with_context(|| format!("creating {}", parent.display()))?;
        }
        fs::write(&on_disk, contents).with_context(|| format!("writing {}", on_disk.display()))?;
        println!("{}", relative(&on_disk, &apps_dir));
    }
    eprintln!(
        "privatium new: {slug} written to {} — `privatium dev --app {slug}` runs it",
        target.display()
    );
    Ok(0)
}

/// `privatium new --examples` (`spec/cli.md §4`): every example app the binary carries,
/// each under its own slug in `<data-dir>/apps/`, exactly as the repository holds it.
pub fn examples(global: &Global) -> Result<u8> {
    let paths = node::paths(global)?;
    let apps_dir = paths.apps_dir();
    let written = write_examples(&apps_dir)?;
    for path in &written {
        println!("{}", relative(path, &apps_dir));
    }
    eprintln!(
        "privatium new: {} written to {} — run `privatium` and pick one in the launcher",
        examples::SLUGS.join(", "),
        apps_dir.display()
    );
    Ok(0)
}

/// Write every example app under `apps_dir`, refusing before anything is written if any
/// of their folders already exists — `new` never overwrites (`spec/cli.md §4`). Returns
/// the files written, in order.
pub fn write_examples(apps_dir: &Path) -> Result<Vec<PathBuf>> {
    let all = examples::all();
    for (slug, _) in &all {
        let target = apps_dir.join(slug);
        if target.exists() {
            bail!(
                "{}: exists; `privatium new` never overwrites — delete that folder for a \
                 fresh copy of the example, or leave it and keep your changes",
                target.display()
            );
        }
    }
    let mut written = Vec::new();
    for (slug, files) in all {
        let target = apps_dir.join(slug);
        for file in files {
            let on_disk = target.join(&file.path);
            if let Some(parent) = on_disk.parent() {
                fs::create_dir_all(parent)
                    .with_context(|| format!("creating {}", parent.display()))?;
            }
            fs::write(&on_disk, &file.contents)
                .with_context(|| format!("writing {}", on_disk.display()))?;
            written.push(on_disk);
        }
    }
    Ok(written)
}

fn add(into: &mut BTreeMap<String, Vec<u8>>, files: Vec<File>) {
    for file in files {
        into.insert(file.path, file.contents);
    }
}

/// What `--from <existing-app>` names: a folder on disk, or an example app the binary
/// carries.
enum Source {
    Folder(PathBuf),
    Example(&'static str),
}

/// `--from <existing-app>`: an installed app by slug, an example app in a checkout, a
/// folder by path, or — on a binary with no checkout beside it — the embedded example.
fn resolve_source(apps_dir: &Path, source: &str) -> Result<Source> {
    let candidates = [
        Some(apps_dir.join(source)),
        node::checkout_apps().map(|apps| apps.join(source)),
        Some(PathBuf::from(source)),
    ];
    for candidate in candidates.into_iter().flatten() {
        if candidate.join("app.toml").is_file() {
            return Ok(Source::Folder(candidate));
        }
    }
    if let Some(slug) = examples::SLUGS.iter().find(|slug| **slug == source) {
        return Ok(Source::Example(slug));
    }
    bail!(
        "--from {source}: no app of that slug under {}, no example app of that name ({}), \
         and no folder of that name holds an app.toml",
        apps_dir.display(),
        examples::SLUGS.join(", ")
    )
}

fn relative(path: &Path, base: &Path) -> String {
    path.strip_prefix(base)
        .unwrap_or(path)
        .to_string_lossy()
        .replace('\\', "/")
}

/// A usage error found past the parser (`spec/cli.md §1`, exit 2).
fn usage(message: String) -> Result<u8> {
    eprintln!("privatium: {message}\n\n{HELP}");
    Ok(2)
}
