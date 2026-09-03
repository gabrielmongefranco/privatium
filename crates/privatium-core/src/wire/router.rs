// Project:  Privatium™  |  File: crates/privatium-core/src/wire/router.rs
// Authors:  Gabriel Mongefranco (@gabrielmongefranco)
// Created:  2026-09-03  |  Modified: 2026-09-03
// Summary:  The route namespaces of spec/protocol.md §9.1 as one function from a path to a
//           Route. Framework prefixes win in both modes; everything else belongs to
//           whichever app is mounted there, and the mount table is Node::mounts(). This is
//           the only place host mode and solo mode differ about where an app lives — that,
//           and url(), which is the only place a URL is built.

use std::collections::BTreeMap;

use crate::config::Mode;

/// The prefixes the framework reserves in every mode (`spec/protocol.md §9.1`), without
/// trailing slashes. `/` itself is the launcher in host mode and the app's root in solo.
///
/// `/api` rather than `/api/v1`: beneath an app's mount only `api/` is reserved, for the
/// data API, and in solo mode the mount is `/`, so the whole of `/api/` is the framework's
/// there too. `/a` is reserved in host mode only — in solo mode there is no prefix.
pub const FRAMEWORK_PREFIXES: [&str; 4] = ["/settings", "/api", "/skills", "/static"];

/// The host-mode app prefix.
const APP_PREFIX: &str = "/a";

/// The settings pages the shell renders under `/settings`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SettingsPage {
    /// `/settings` — node identity, alerts.
    Node,
    /// `/settings/apps` — installed apps, warnings, the seed offer.
    Apps,
    /// `/settings/data` — the data directory and backup instructions.
    Data,
    /// `/settings/devices` — this node's own row; pairing arrives in Phase 2.
    Devices,
}

impl SettingsPage {
    /// Every page, in navigation order.
    pub const ALL: [Self; 4] = [Self::Node, Self::Apps, Self::Data, Self::Devices];

    /// The page's path.
    #[must_use]
    pub fn path(self) -> &'static str {
        match self {
            Self::Node => "/settings",
            Self::Apps => "/settings/apps",
            Self::Data => "/settings/data",
            Self::Devices => "/settings/devices",
        }
    }

    /// The label the settings navigation shows.
    #[must_use]
    pub fn title(self) -> &'static str {
        match self {
            Self::Node => "Node",
            Self::Apps => "Apps",
            Self::Data => "Data and backup",
            Self::Devices => "Devices",
        }
    }
}

/// Where a path leads.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Route {
    /// `/` in host mode: the app launcher.
    Launcher,
    /// One of the settings pages.
    Settings(SettingsPage),
    /// `/settings/apps/<slug>/seed` — the seed offer of `spec/app-contract.md §9`, a POST.
    Seed {
        /// The app.
        slug: String,
    },
    /// `GET /api/v1/health`.
    Health,
    /// `GET /api/v1/manifest`.
    Manifest,
    /// `/skills/<name>.md`.
    Skill {
        /// The skill's folder name.
        name: String,
    },
    /// `/skills/bundle.zip`.
    SkillBundle,
    /// `/static/<rest>` — the shell's own assets.
    Static {
        /// The path under `/static/`, not decoded.
        rest: String,
    },
    /// A path beneath an app's mount.
    App {
        /// The app.
        slug: String,
        /// The mount, `/a/<slug>/` or `/`.
        mount: String,
        /// The path beneath it, beginning with `/` and not decoded. `/` is the mount point.
        rest: String,
    },
    /// `/a/<slug>` without its trailing slash: relative links inside `index.html` need it.
    Redirect {
        /// Where to.
        to: String,
    },
    /// A framework prefix with nothing behind it, an unmounted slug, or — in host mode — a
    /// path that is nobody's.
    NotFound,
}

/// The mount table, built from [`Node::mounts`](crate::Node::mounts).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Router {
    mode: Mode,
    /// `mount → slug`. In host mode every key is `/a/<slug>/`; in solo mode the one key
    /// is `/`.
    mounts: BTreeMap<String, String>,
}

impl Router {
    /// From the `(mount, slug)` pairs the node serves.
    pub fn new<'a>(mode: Mode, mounts: impl IntoIterator<Item = (&'a str, &'a str)>) -> Self {
        Self {
            mode,
            mounts: mounts
                .into_iter()
                .map(|(mount, slug)| (mount.to_owned(), slug.to_owned()))
                .collect(),
        }
    }

    /// The mode this table was built for.
    #[must_use]
    pub fn mode(&self) -> Mode {
        self.mode
    }

    /// Resolve a request path. Framework prefixes are tested first in both modes
    /// (`§9.1`: "Framework prefixes take precedence in both modes").
    #[must_use]
    pub fn resolve(&self, path: &str) -> Route {
        if let Some(rest) = strip_prefix(path, "/settings") {
            return match rest {
                "" | "/" => Route::Settings(SettingsPage::Node),
                "/apps" | "/apps/" => Route::Settings(SettingsPage::Apps),
                "/data" | "/data/" => Route::Settings(SettingsPage::Data),
                "/devices" | "/devices/" => Route::Settings(SettingsPage::Devices),
                _ => match rest
                    .strip_prefix("/apps/")
                    .and_then(|r| r.strip_suffix("/seed"))
                {
                    Some(slug) if crate::app::manifest::is_valid_slug(slug) => Route::Seed {
                        slug: slug.to_owned(),
                    },
                    _ => Route::NotFound,
                },
            };
        }
        if let Some(rest) = strip_prefix(path, "/api") {
            return match rest {
                "/v1/health" => Route::Health,
                "/v1/manifest" => Route::Manifest,
                // In solo mode the mount is `/`, so `/api/…` is also the solo app's data
                // API (`spec/data-api.md`); `§9.2`'s `/api/v1/*` stays the framework's.
                _ if self.mode == Mode::Solo && rest.len() > 1 && !rest.starts_with("/v1") => {
                    match self.mounts.get("/") {
                        Some(slug) => Route::App {
                            slug: slug.clone(),
                            mount: "/".to_owned(),
                            rest: path.to_owned(),
                        },
                        None => Route::NotFound,
                    }
                }
                _ => Route::NotFound,
            };
        }
        if let Some(rest) = strip_prefix(path, "/skills") {
            return match rest {
                "/bundle.zip" => Route::SkillBundle,
                _ => match rest.strip_prefix('/').and_then(|r| r.strip_suffix(".md")) {
                    Some(name) if is_skill_name(name) => Route::Skill {
                        name: name.to_owned(),
                    },
                    _ => Route::NotFound,
                },
            };
        }
        if let Some(rest) = strip_prefix(path, "/static") {
            return match rest.strip_prefix('/') {
                Some(rest) if !rest.is_empty() => Route::Static {
                    rest: rest.to_owned(),
                },
                _ => Route::NotFound,
            };
        }

        match self.mode {
            Mode::Host => self.resolve_host(path),
            Mode::Solo => self.resolve_solo(path),
        }
    }

    fn resolve_host(&self, path: &str) -> Route {
        if path == "/" || path.is_empty() {
            return Route::Launcher;
        }
        let Some(rest) = strip_prefix(path, APP_PREFIX) else {
            return Route::NotFound;
        };
        let Some(after) = rest.strip_prefix('/') else {
            return Route::NotFound;
        };
        let (slug, tail) = match after.find('/') {
            Some(at) => (&after[..at], &after[at..]),
            None => (after, ""),
        };
        if !crate::app::manifest::is_valid_slug(slug) {
            return Route::NotFound;
        }
        let mount = format!("{APP_PREFIX}/{slug}/");
        if !self.mounts.contains_key(&mount) {
            return Route::NotFound;
        }
        if tail.is_empty() {
            return Route::Redirect { to: mount };
        }
        Route::App {
            slug: slug.to_owned(),
            mount,
            rest: tail.to_owned(),
        }
    }

    fn resolve_solo(&self, path: &str) -> Route {
        match self.mounts.get("/") {
            Some(slug) => Route::App {
                slug: slug.clone(),
                mount: "/".to_owned(),
                rest: if path.is_empty() {
                    "/".to_owned()
                } else {
                    path.to_owned()
                },
            },
            // Solo mode with no solo app loaded: `Warning::SoloAppNotLoaded` said so at
            // load, and there is nothing at `/` to serve.
            None => Route::NotFound,
        }
    }
}

/// `path` is `prefix` itself or starts with `prefix/`; the remainder begins with `/` or is
/// empty. `/settingsx` is not `/settings`.
fn strip_prefix<'a>(path: &'a str, prefix: &str) -> Option<&'a str> {
    let rest = path.strip_prefix(prefix)?;
    (rest.is_empty() || rest.starts_with('/')).then_some(rest)
}

/// A skill folder name: lowercase, digits and hyphens, nothing that could name a path.
fn is_skill_name(name: &str) -> bool {
    !name.is_empty()
        && name
            .bytes()
            .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'-')
}

/// The one URL construction point (`spec/protocol.md §9.1`, ADR 0003): a path inside an
/// app, under its mount. Host mode and solo mode differ here and nowhere else.
///
/// `url("/a/hello/", "/edit")` is `/a/hello/edit`; `url("/", "/edit")` is `/edit`; an empty
/// `path` is the mount point itself. `pv.url()` (M7) and `pv.js`'s (M9) are this function.
#[must_use]
pub fn url(mount: &str, path: &str) -> String {
    let mount = mount.trim_end_matches('/');
    let path = path.trim_start_matches('/');
    format!("{mount}/{path}")
}

/// The framework prefix a solo app's top-level route or file is shadowed by
/// (`spec/protocol.md §9.1`): `/settings`, `/api`, `/skills` or `/static`. `None` when the
/// route is the app's own.
#[must_use]
pub fn shadowing_prefix(route: &str) -> Option<&'static str> {
    FRAMEWORK_PREFIXES
        .into_iter()
        .find(|prefix| strip_prefix(route, prefix).is_some())
}

// AGENTS.md, Style: unwrap() is permitted in tests. The crate-level deny reaches unit
// tests inside src/, so each one opts out where it is declared.
#[allow(clippy::unwrap_used, clippy::expect_used)]
#[cfg(test)]
mod tests {
    use super::*;

    fn host() -> Router {
        Router::new(
            Mode::Host,
            [("/a/hello/", "hello"), ("/a/sketch/", "sketch")],
        )
    }

    fn solo() -> Router {
        Router::new(Mode::Solo, [("/", "sketch")])
    }

    fn app(slug: &str, mount: &str, rest: &str) -> Route {
        Route::App {
            slug: slug.into(),
            mount: mount.into(),
            rest: rest.into(),
        }
    }

    /// Beneath the solo mount `/`, `api/` is the app's data API (`spec/protocol.md §9.1`):
    /// `/api/q/x` reaches the app, `/api/v1/*` stays the framework's, and in host mode
    /// nothing else under `/api` exists.
    #[test]
    fn solo_api_is_the_apps_data_api_and_v1_stays_the_frameworks() {
        assert_eq!(
            solo().resolve("/api/q/v_x"),
            app("sketch", "/", "/api/q/v_x")
        );
        assert_eq!(
            solo().resolve("/api/stream"),
            app("sketch", "/", "/api/stream")
        );
        assert_eq!(solo().resolve("/api/v1/health"), Route::Health);
        assert_eq!(solo().resolve("/api/v1/nope"), Route::NotFound);
        assert_eq!(host().resolve("/api/q/v_x"), Route::NotFound);
        assert_eq!(
            Router::new(Mode::Solo, []).resolve("/api/q/v_x"),
            Route::NotFound
        );
    }

    #[test]
    fn framework_prefixes_resolve_the_same_in_both_modes() {
        for router in [host(), solo()] {
            assert_eq!(
                router.resolve("/settings"),
                Route::Settings(SettingsPage::Node)
            );
            assert_eq!(
                router.resolve("/settings/apps"),
                Route::Settings(SettingsPage::Apps)
            );
            assert_eq!(
                router.resolve("/settings/apps/hello/seed"),
                Route::Seed {
                    slug: "hello".into()
                }
            );
            assert_eq!(router.resolve("/settings/apps/Bad/seed"), Route::NotFound);
            assert_eq!(router.resolve("/api/v1/health"), Route::Health);
            assert_eq!(router.resolve("/api/v1/manifest"), Route::Manifest);
            assert_eq!(router.resolve("/api/v1/nope"), Route::NotFound);
            assert_eq!(router.resolve("/api"), Route::NotFound);
            assert_eq!(router.resolve("/api/"), Route::NotFound);
            assert_eq!(
                router.resolve("/skills/privatium-overview.md"),
                Route::Skill {
                    name: "privatium-overview".into()
                }
            );
            assert_eq!(router.resolve("/skills/bundle.zip"), Route::SkillBundle);
            assert_eq!(router.resolve("/skills/../x.md"), Route::NotFound);
            assert_eq!(
                router.resolve("/static/shell.css"),
                Route::Static {
                    rest: "shell.css".into()
                }
            );
            assert_eq!(router.resolve("/static/"), Route::NotFound);
            assert_eq!(router.resolve("/static"), Route::NotFound);
        }
    }

    #[test]
    fn host_mode_mounts_under_a_slug_and_the_launcher_at_root() {
        let router = host();
        assert_eq!(router.resolve("/"), Route::Launcher);
        assert_eq!(router.resolve("/a/hello/"), app("hello", "/a/hello/", "/"));
        assert_eq!(
            router.resolve("/a/sketch/style.css"),
            app("sketch", "/a/sketch/", "/style.css")
        );
        assert_eq!(
            router.resolve("/a/hello"),
            Route::Redirect {
                to: "/a/hello/".into()
            }
        );
        assert_eq!(router.resolve("/a/absent/"), Route::NotFound);
        assert_eq!(router.resolve("/a/"), Route::NotFound);
        assert_eq!(router.resolve("/a"), Route::NotFound);
        assert_eq!(router.resolve("/settingsx"), Route::NotFound);
        assert_eq!(router.resolve("/anything"), Route::NotFound);
    }

    #[test]
    fn solo_mode_hands_root_to_the_app_and_has_no_prefix() {
        let router = solo();
        assert_eq!(router.resolve("/"), app("sketch", "/", "/"));
        assert_eq!(
            router.resolve("/style.css"),
            app("sketch", "/", "/style.css")
        );
        // No `/a/` prefix exists: this is a path inside the solo app's `web/`.
        assert_eq!(
            router.resolve("/a/sketch/"),
            app("sketch", "/", "/a/sketch/")
        );
        assert_eq!(
            router.resolve("/settingsx"),
            app("sketch", "/", "/settingsx")
        );
        assert_eq!(Router::new(Mode::Solo, []).resolve("/"), Route::NotFound);
    }

    #[test]
    fn url_is_the_only_place_the_modes_differ() {
        assert_eq!(url("/a/hello/", "/edit"), "/a/hello/edit");
        assert_eq!(url("/a/hello/", "edit"), "/a/hello/edit");
        assert_eq!(url("/a/hello/", ""), "/a/hello/");
        assert_eq!(url("/", "/edit"), "/edit");
        assert_eq!(url("/", ""), "/");
    }

    #[test]
    fn shadowing_names_the_prefix() {
        assert_eq!(shadowing_prefix("/settings"), Some("/settings"));
        assert_eq!(shadowing_prefix("/static/x.js"), Some("/static"));
        assert_eq!(shadowing_prefix("/api/q/view"), Some("/api"));
        assert_eq!(shadowing_prefix("/settingsx"), None);
        assert_eq!(shadowing_prefix("/play"), None);
    }
}
