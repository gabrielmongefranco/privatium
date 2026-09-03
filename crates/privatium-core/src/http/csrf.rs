// Project:  Privatium™  |  File: crates/privatium-core/src/http/csrf.rs
// Authors:  Gabriel Mongefranco (@gabrielmongefranco)
// Created:  2026-09-03  |  Modified: 2026-09-03
// Summary:  The csrf() of docs/plans/phase-1.md §2.2: HMAC-SHA256 over node_id ‖ nonce ‖ path
//           under a key HKDF-derived from the node key by Identity::csrf_key. Nothing is
//           written to disk; the nonce lives for the process, so a restart invalidates
//           every outstanding form, which on one machine is correct and unremarkable.

use hmac::{Hmac, KeyInit as _, Mac as _};
use sha2::Sha256;

use crate::identity::{DerivedKey, Identity};

type HmacSha256 = Hmac<Sha256>;

/// The form field the token travels in.
pub const FIELD: &str = "_csrf";

/// The token issuer and verifier for one process.
pub struct Csrf {
    key: DerivedKey,
    node_id: String,
    /// Fresh per process, held in memory only.
    nonce: [u8; 16],
}

impl std::fmt::Debug for Csrf {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Csrf")
            .field("node_id", &self.node_id)
            .finish_non_exhaustive()
    }
}

impl Csrf {
    /// Derive the key from `identity` and mint the session nonce.
    #[must_use]
    pub fn new(identity: &Identity) -> Self {
        let mut nonce = [0u8; 16];
        rand::fill(&mut nonce);
        Self {
            key: identity.csrf_key(),
            node_id: identity.id().as_str().to_owned(),
            nonce,
        }
    }

    /// The token for a form that will POST to `path`: lowercase hex of
    /// `HMAC-SHA256(csrf_key, node_id ‖ nonce ‖ path)`.
    ///
    /// The three parts have fixed widths but the last — eight characters, sixteen bytes,
    /// then the path — so the concatenation is unambiguous without length prefixes.
    #[must_use]
    pub fn token(&self, path: &str) -> String {
        let mut mac = match HmacSha256::new_from_slice(self.key.as_bytes()) {
            Ok(mac) => mac,
            // `new_from_slice` accepts any key length for HMAC; the branch is unreachable
            // and a token that verifies nothing is the safe way to express that.
            Err(_) => return String::new(),
        };
        mac.update(self.node_id.as_bytes());
        mac.update(&self.nonce);
        mac.update(path.as_bytes());
        hex(&mac.finalize().into_bytes())
    }

    /// Whether `token` is the token for `path`, in constant time over the MAC.
    #[must_use]
    pub fn verify(&self, path: &str, token: &str) -> bool {
        let Some(presented) = unhex(token) else {
            return false;
        };
        let Ok(mut mac) = HmacSha256::new_from_slice(self.key.as_bytes()) else {
            return false;
        };
        mac.update(self.node_id.as_bytes());
        mac.update(&self.nonce);
        mac.update(path.as_bytes());
        mac.verify_slice(&presented).is_ok()
    }

    /// The hidden input a form carries — the `csrf()` template helper's output.
    #[must_use]
    pub fn field(&self, path: &str) -> String {
        format!(
            r#"<input type="hidden" name="{FIELD}" value="{}">"#,
            self.token(path)
        )
    }
}

fn hex(bytes: &[u8]) -> String {
    use std::fmt::Write as _;
    let mut out = String::with_capacity(bytes.len() * 2);
    for byte in bytes {
        let _ = write!(out, "{byte:02x}");
    }
    out
}

fn unhex(text: &str) -> Option<Vec<u8>> {
    if !text.len().is_multiple_of(2) {
        return None;
    }
    (0..text.len())
        .step_by(2)
        .map(|i| u8::from_str_radix(text.get(i..i + 2)?, 16).ok())
        .collect()
}

// AGENTS.md, Style: unwrap() is permitted in tests. The crate-level deny reaches unit
// tests inside src/, so each one opts out where it is declared.
#[allow(clippy::unwrap_used, clippy::expect_used)]
#[cfg(test)]
mod tests {
    use super::*;

    fn identity() -> Identity {
        let dir = tempfile::tempdir().unwrap();
        Identity::load_or_create(dir.path()).unwrap()
    }

    #[test]
    fn a_token_is_bound_to_its_path_and_its_process() {
        let identity = identity();
        let a = Csrf::new(&identity);
        let token = a.token("/settings/apps/hello/seed");
        assert_eq!(token.len(), 64);
        assert!(a.verify("/settings/apps/hello/seed", &token));
        assert!(!a.verify("/settings/apps/other/seed", &token));
        assert!(!a.verify("/settings/apps/hello/seed", ""));
        assert!(!a.verify("/settings/apps/hello/seed", "zz"));

        // Same key, fresh nonce: another process would not accept it.
        let b = Csrf::new(&identity);
        assert!(!b.verify("/settings/apps/hello/seed", &token));

        // Another node's key: never.
        let other = Csrf::new(&self::identity());
        assert!(!other.verify("/settings/apps/hello/seed", &token));
    }

    #[test]
    fn the_field_is_a_hidden_input() {
        let csrf = Csrf::new(&identity());
        let field = csrf.field("/x");
        assert!(field.starts_with(r#"<input type="hidden" name="_csrf" value=""#));
        assert!(field.contains(&csrf.token("/x")));
    }

    /// `docs/plans/phase-1.md §2.2`: the key is derived, so two identities from the same
    /// key file agree on it and nothing about it ever touches the disk.
    #[test]
    fn the_key_is_derived_not_stored() {
        let dir = tempfile::tempdir().unwrap();
        let first = Identity::load_or_create(dir.path()).unwrap();
        let again = Identity::load_or_create(dir.path()).unwrap();
        assert_eq!(first.csrf_key().as_bytes(), again.csrf_key().as_bytes());
        assert_ne!(first.csrf_key().as_bytes(), &[0u8; 32]);
        let mut files: Vec<String> = std::fs::read_dir(dir.path())
            .unwrap()
            .map(|e| e.unwrap().file_name().to_string_lossy().into_owned())
            .collect();
        // `read_dir` order is the filesystem's — macOS returned `node.pub` first.
        files.sort();
        assert_eq!(files, ["node.key", "node.pub"]);
    }
}
