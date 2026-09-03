// Project:  Privatium™  |  File: crates/privatium-core/src/http/mod.rs
// Authors:  Gabriel Mongefranco (@gabrielmongefranco)
// Created:  2026-09-03  |  Modified: 2026-09-03
// Summary:  What sits behind core::handle (docs/plans/phase-1.md §4): the §9.3 headers, the
//           auth layer and csrf() of §2.2, the shell's pages, the two API routes of §9.2,
//           the skills routes of spec/cli.md §6, the shell's embedded assets, and the Tier 2
//           file server. Every one of them is reached through wire::Handler and nothing else.

use std::collections::BTreeMap;

pub mod api;
pub mod apps;
pub mod assets;
pub mod auth;
pub mod csrf;
pub mod headers;
pub mod shell;
pub mod skills;

pub use auth::{AuthLayer, AuthService, Device, Peer};
pub use csrf::Csrf;

/// The most a form POST may carry. The shell's forms carry a token and nothing else.
pub const FORM_LIMIT: usize = 64 * 1024;

/// Parse an `application/x-www-form-urlencoded` body. Later keys win.
#[must_use]
pub fn parse_form(body: &[u8]) -> BTreeMap<String, String> {
    let mut out = BTreeMap::new();
    for pair in body.split(|b| *b == b'&') {
        if pair.is_empty() {
            continue;
        }
        let (key, value) = match pair.iter().position(|b| *b == b'=') {
            Some(at) => (&pair[..at], &pair[at + 1..]),
            None => (pair, &[][..]),
        };
        out.insert(percent_decode(key), percent_decode(value));
    }
    out
}

/// `+` is a space, `%XX` is a byte, and what is not UTF-8 afterwards is replaced. The Lua
/// host decodes route parameters with it too.
pub(crate) fn percent_decode(text: &[u8]) -> String {
    let mut bytes = Vec::with_capacity(text.len());
    let mut i = 0;
    while i < text.len() {
        match text[i] {
            b'+' => bytes.push(b' '),
            b'%' if i + 2 < text.len() => {
                let hex = &text[i + 1..i + 3];
                match std::str::from_utf8(hex)
                    .ok()
                    .and_then(|h| u8::from_str_radix(h, 16).ok())
                {
                    Some(byte) => {
                        bytes.push(byte);
                        i += 2;
                    }
                    None => bytes.push(b'%'),
                }
            }
            other => bytes.push(other),
        }
        i += 1;
    }
    String::from_utf8_lossy(&bytes).into_owned()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn forms_decode() {
        let form = parse_form(b"_csrf=ab%2Fcd&x=a+b&empty=&flag&bad=%zz");
        assert_eq!(form["_csrf"], "ab/cd");
        assert_eq!(form["x"], "a b");
        assert_eq!(form["empty"], "");
        assert_eq!(form["flag"], "");
        assert_eq!(form["bad"], "%zz");
        assert!(parse_form(b"").is_empty());
    }
}
