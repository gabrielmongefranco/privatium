// Project:  Privatium™  |  File: crates/privatium-core/src/app/csp.rs
// Authors:  Gabriel Mongefranco (@gabrielmongefranco)
// Created:  2026-09-02  |  Modified: 2026-09-02
// Summary:  One Content-Security-Policy per app, computed at load from [permissions]
//           (spec/app-contract.md §5.4) over the default of spec/protocol.md §9.3. The
//           default is never relaxed to make anything work (AGENTS.md); each permission
//           widens exactly one directive and each widening is surfaced to the owner.

use crate::app::manifest::Permissions;

/// `spec/protocol.md §9.3`, verbatim, as the directives every app starts from.
///
/// `script-src` is rendered separately because it is the one directive a permission
/// touches; `img-src` and `connect-src` appear only when `remote` names an origin, since
/// until then `default-src 'self'` already says everything they would.
const DEFAULT_PREFIX: &str = "default-src 'self'; script-src ";
const DEFAULT_SUFFIX: &str =
    "; object-src 'none'; base-uri 'none'; form-action 'self'; frame-ancestors 'none'";

/// Where the framework's own scripts live (`spec/protocol.md §9.1`): `pv.js`, HTMX.
const FRAMEWORK_STATIC: &str = "/static/";

/// An app's policy.
///
/// **Two renderings, and why.** `§5.4` says the default is `script-src 'self'` scoped to
/// the app's own path. CSP cannot spell a path without a host — a `host-source` needs one
/// and there is no origin-relative form — and the loader runs before any request exists to
/// take an origin from. So the policy holds the *shape*, [`header`](Self::header) renders
/// it with `'self'` (the whole origin, which is what Phase 1's shared-origin apps share
/// anyway; `AGENTS.md` says CSP is not an inter-app boundary today), and
/// [`header_for`](Self::header_for) renders the path-scoped form once M6 has the request's
/// origin. Both carry the same widenings.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Csp {
    /// The app's mount path (`/a/<slug>/`, or `/` in solo mode), if mounted.
    mount: Option<String>,
    /// `script-src` keywords the permissions added, in header order.
    script_keywords: Vec<&'static str>,
    /// `remote` origins, added to `script-src`, `img-src` and `connect-src`.
    remote: Vec<String>,
    /// [`header`](Self::header), computed at load.
    header: String,
}

impl Csp {
    /// The policy for an app mounted at `mount` with `permissions`.
    ///
    /// Each permission widens exactly one thing: `inline_script` → `'unsafe-inline'`,
    /// `wasm` → `'wasm-unsafe-eval'`, `eval` → `'unsafe-eval'`, `remote` → its origins on
    /// the three source directives. `sql` and `cross_origin_isolated` are not CSP and are
    /// not here.
    #[must_use]
    pub fn for_app(mount: Option<&str>, permissions: &Permissions) -> Self {
        let mut script_keywords = Vec::new();
        if permissions.inline_script {
            script_keywords.push("'unsafe-inline'");
        }
        if permissions.wasm {
            script_keywords.push("'wasm-unsafe-eval'");
        }
        if permissions.eval {
            script_keywords.push("'unsafe-eval'");
        }
        let mut csp = Self {
            mount: mount.map(str::to_owned),
            script_keywords,
            remote: permissions.remote.clone(),
            header: String::new(),
        };
        csp.header = csp.render(&["'self'".to_owned()]);
        csp
    }

    /// The header value with `'self'` as the app's script source.
    #[must_use]
    pub fn header(&self) -> &str {
        &self.header
    }

    /// The header value with `script-src` scoped to the app's path under `origin` —
    /// `<origin>/a/<slug>/` plus the framework's `/static/` — instead of `'self'`.
    ///
    /// In solo mode the app owns `/`, so the app path *is* the origin and this is
    /// [`header`](Self::header). An unmounted app renders the same way; nothing serves it.
    #[must_use]
    pub fn header_for(&self, origin: &str) -> String {
        let origin = origin.trim_end_matches('/');
        match self.mount.as_deref() {
            Some(mount) if mount != "/" => self.render(&[
                format!("{origin}{mount}"),
                format!("{origin}{FRAMEWORK_STATIC}"),
            ]),
            _ => self.header.clone(),
        }
    }

    /// Whether any permission widened the default.
    #[must_use]
    pub fn is_default(&self) -> bool {
        self.script_keywords.is_empty() && self.remote.is_empty()
    }

    fn render(&self, script_self: &[String]) -> String {
        let mut script: Vec<String> = script_self.to_vec();
        script.extend(self.script_keywords.iter().map(|k| (*k).to_owned()));
        script.extend(self.remote.iter().cloned());

        let mut out = String::from(DEFAULT_PREFIX);
        out.push_str(&script.join(" "));
        out.push_str(DEFAULT_SUFFIX);
        if !self.remote.is_empty() {
            let remote = self.remote.join(" ");
            out.push_str(&format!(
                "; img-src 'self' {remote}; connect-src 'self' {remote}"
            ));
        }
        out
    }
}

// AGENTS.md, Style: unwrap() is permitted in tests. The crate-level deny reaches unit
// tests inside src/, so each one opts out where it is declared.
#[allow(clippy::unwrap_used, clippy::expect_used)]
#[cfg(test)]
mod tests {
    use super::*;

    const DEFAULT: &str = "default-src 'self'; script-src 'self'; object-src 'none'; base-uri 'none'; form-action 'self'; frame-ancestors 'none'";

    #[test]
    fn the_default_is_section_9_3_verbatim() {
        let csp = Csp::for_app(Some("/a/hello/"), &Permissions::default());
        assert_eq!(csp.header(), DEFAULT);
        assert!(csp.is_default());
    }

    #[test]
    fn each_permission_widens_exactly_one_token() {
        let one = |p: Permissions| Csp::for_app(Some("/a/x/"), &p).header().to_owned();
        assert_eq!(
            one(Permissions {
                inline_script: true,
                ..Permissions::default()
            }),
            DEFAULT.replace("script-src 'self'", "script-src 'self' 'unsafe-inline'")
        );
        assert_eq!(
            one(Permissions {
                wasm: true,
                ..Permissions::default()
            }),
            DEFAULT.replace("script-src 'self'", "script-src 'self' 'wasm-unsafe-eval'")
        );
        assert_eq!(
            one(Permissions {
                eval: true,
                ..Permissions::default()
            }),
            DEFAULT.replace("script-src 'self'", "script-src 'self' 'unsafe-eval'")
        );
        assert_eq!(
            one(Permissions {
                remote: vec!["https://x".into()],
                ..Permissions::default()
            }),
            format!(
                "{}; img-src 'self' https://x; connect-src 'self' https://x",
                DEFAULT.replace("script-src 'self'", "script-src 'self' https://x")
            )
        );
        // sql is not CSP.
        assert_eq!(
            one(Permissions {
                sql: true,
                ..Permissions::default()
            }),
            DEFAULT
        );
    }

    #[test]
    fn path_scoping_replaces_self_on_script_src_only() {
        let csp = Csp::for_app(Some("/a/sketch/"), &Permissions::default());
        assert_eq!(
            csp.header_for("http://127.0.0.1:8420/"),
            DEFAULT.replace(
                "script-src 'self'",
                "script-src http://127.0.0.1:8420/a/sketch/ http://127.0.0.1:8420/static/"
            )
        );
        let solo = Csp::for_app(Some("/"), &Permissions::default());
        assert_eq!(solo.header_for("http://127.0.0.1:8420"), DEFAULT);
        let unmounted = Csp::for_app(None, &Permissions::default());
        assert_eq!(unmounted.header_for("http://127.0.0.1:8420"), DEFAULT);
    }
}
