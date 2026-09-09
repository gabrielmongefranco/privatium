// Project:  Privatium™  |  File: crates/privatium-core/src/app/examples.rs
// Authors:  Gabriel Mongefranco (@gabrielmongefranco)
// Created:  2026-09-06  |  Modified: 2026-09-06
// Summary:  The example apps of apps/README.md — hello, animals, sketch and pantry — embedded in
//           the binary as the files they are in the repository, so a release download has
//           them to write into <data-dir>/apps/ on a first run and for `privatium new
//           --examples` and `--from` (spec/cli.md §2, §4). A checkout mounts the same
//           folders from disk; nothing here is read at runtime by the loader.
//           See main README.md for full license information.

use include_dir::{Dir, DirEntry, include_dir};

use super::scaffold::File;

/// `apps/hello` — the floor: three routes, one table, two templates.
static HELLO: Dir<'static> = include_dir!("$CARGO_MANIFEST_DIR/../../apps/hello");
/// `apps/animals` — the ceiling: batches, recursive SQL, `lib/`, HTMX beside Alpine.
static ANIMALS: Dir<'static> = include_dir!("$CARGO_MANIFEST_DIR/../../apps/animals");
/// `apps/sketch` — the escape hatch: a Tier 2 app with no SQL at all.
static SKETCH: Dir<'static> = include_dir!("$CARGO_MANIFEST_DIR/../../apps/sketch");
/// `apps/pantry` — the same tier with tables: named views, exact decimals, forms.
static PANTRY: Dir<'static> = include_dir!("$CARGO_MANIFEST_DIR/../../apps/pantry");

/// The example apps by slug, in the order `apps/README.md` lists them.
pub const SLUGS: [&str; 4] = ["hello", "animals", "sketch", "pantry"];

fn dir_of(slug: &str) -> Option<&'static Dir<'static>> {
    match slug {
        "hello" => Some(&HELLO),
        "animals" => Some(&ANIMALS),
        "sketch" => Some(&SKETCH),
        "pantry" => Some(&PANTRY),
        _ => None,
    }
}

/// Every file of the example app `slug`, paths relative to the app folder and sorted, or
/// `None` when no example has that slug.
#[must_use]
pub fn files(slug: &str) -> Option<Vec<File>> {
    let dir = dir_of(slug)?;
    let mut files = Vec::new();
    collect(dir, &mut files);
    files.sort_by(|a, b| a.path.cmp(&b.path));
    Some(files)
}

/// Every example app, as `(slug, files)`, in [`SLUGS`] order.
#[must_use]
pub fn all() -> Vec<(&'static str, Vec<File>)> {
    SLUGS
        .iter()
        .filter_map(|slug| files(slug).map(|files| (*slug, files)))
        .collect()
}

fn collect(dir: &'static Dir<'static>, into: &mut Vec<File>) {
    for entry in dir.entries() {
        match entry {
            DirEntry::Dir(sub) => collect(sub, into),
            DirEntry::File(file) => into.push(File {
                path: file.path().to_string_lossy().replace('\\', "/"),
                contents: file.contents().to_vec(),
            }),
        }
    }
}
