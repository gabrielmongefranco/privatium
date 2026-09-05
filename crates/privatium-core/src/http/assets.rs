// Project:  Privatium™  |  File: crates/privatium-core/src/http/assets.rs
// Authors:  Gabriel Mongefranco (@gabrielmongefranco)
// Created:  2026-09-03  |  Modified: 2026-09-05
// Summary:  /static/* (spec/protocol.md §9.1): the shell's own assets, embedded from
//           assets/shell/ — stylesheet, htmx, pv.js, and browser session modules.

use include_dir::{Dir, include_dir};

/// Shell scripts, stylesheets and the Noble import closure. Provenance is not served.
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
/// are served; nested paths are confined to the vendored Noble module directory.
#[must_use]
pub fn get(rest: &str) -> Option<Asset> {
    if rest.contains('\\')
        || rest.split('/').any(|part| {
            part.is_empty()
                || part.starts_with('.')
                || !part
                    .bytes()
                    .all(|b| b.is_ascii_alphanumeric() || b"_.-".contains(&b))
        })
        || (rest.contains('/') && !rest.starts_with("vendor/noble/"))
    {
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
    fn the_shell_ships_its_stylesheet_htmx_and_pv_js_and_nothing_else() {
        assert_eq!(
            get("shell.css").unwrap().content_type,
            "text/css; charset=utf-8"
        );
        let htmx = get("htmx.min.js").unwrap();
        assert_eq!(htmx.content_type, "text/javascript; charset=utf-8");
        assert!(htmx.bytes.starts_with(b"var htmx="));
        let pv = get("pv.js").unwrap();
        assert_eq!(pv.content_type, "text/javascript; charset=utf-8");
        assert!(
            std::str::from_utf8(pv.bytes)
                .unwrap()
                .contains("export const pv")
        );
        assert!(
            pv.bytes.len() < 12 * 1024,
            "{} bytes: spec/data-api.md §5 says under 12 KB, unminified, no build",
            pv.bytes.len()
        );
        assert!(get("VENDOR.md").is_none());
        assert!(get("../icons/LICENSE").is_none());
    }

    #[test]
    fn test_spec_8_browser_crypto_modules_are_served_without_path_traversal() {
        for path in [
            "session.js",
            "pair.js",
            "vendor/noble/curves/ed25519.js",
            "vendor/noble/hashes/sha2.js",
            "vendor/noble/ciphers/chacha.js",
            "vendor/noble/curves/abstract/edwards.js",
        ] {
            assert_eq!(
                get(path).unwrap().content_type,
                "text/javascript; charset=utf-8"
            );
        }
        for path in [
            "vendor/noble/../session.js",
            "vendor/noble//curves/ed25519.js",
            "vendor/noble/curves/./ed25519.js",
            "vendor/noble/curves\\ed25519.js",
            "vendor/noble/curves/LICENSE",
            "vendor/noble/VENDOR.md",
            "vendor/noble/curves/%2e%2e/ed25519.js",
            "other/pv.js",
            "/session.js",
        ] {
            assert!(get(path).is_none(), "{path}");
        }
    }
}
