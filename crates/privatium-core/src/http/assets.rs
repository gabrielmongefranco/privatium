// Project:  Privatium™  |  File: crates/privatium-core/src/http/assets.rs
// Authors:  Gabriel Mongefranco (@gabrielmongefranco)
// Created:  2026-09-03  |  Modified: 2026-09-03
// Summary:  /static/* (spec/protocol.md §9.1): the shell's own assets, embedded from
//           assets/shell/ — its stylesheet and the vendored htmx. pv.js is M9's and is not
//           here; until then the path is a 404 like any other file the shell does not ship.

use include_dir::{Dir, include_dir};

/// `assets/shell/`: `shell.css`, `htmx.min.js`, and the `VENDOR.md` that is not served.
static SHELL: Dir<'static> = include_dir!("$CARGO_MANIFEST_DIR/assets/shell");

/// One embedded asset.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Asset {
    /// The bytes, for the life of the process.
    pub bytes: &'static [u8],
    /// The `Content-Type` to send it with.
    pub content_type: &'static str,
}

/// The asset at `/static/<rest>`, if the shell ships one. Only stylesheets and scripts
/// are served; provenance files and anything with a path separator are not.
#[must_use]
pub fn get(rest: &str) -> Option<Asset> {
    if rest.contains(['/', '\\']) || rest.starts_with('.') {
        return None;
    }
    let content_type = match rest.rsplit_once('.').map(|(_, ext)| ext)? {
        "css" => "text/css; charset=utf-8",
        "js" => "text/javascript; charset=utf-8",
        _ => return None,
    };
    let file = SHELL.get_file(rest)?;
    Some(Asset {
        bytes: file.contents(),
        content_type,
    })
}

// AGENTS.md, Style: unwrap() is permitted in tests. The crate-level deny reaches unit
// tests inside src/, so each one opts out where it is declared.
#[allow(clippy::unwrap_used, clippy::expect_used)]
#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_shell_ships_its_stylesheet_and_htmx_and_nothing_else() {
        assert_eq!(
            get("shell.css").unwrap().content_type,
            "text/css; charset=utf-8"
        );
        let htmx = get("htmx.min.js").unwrap();
        assert_eq!(htmx.content_type, "text/javascript; charset=utf-8");
        assert!(htmx.bytes.starts_with(b"var htmx="));
        assert!(get("VENDOR.md").is_none());
        assert!(get("../icons/LICENSE").is_none());
        assert!(get("pv.js").is_none(), "pv.js is M9's");
    }
}
