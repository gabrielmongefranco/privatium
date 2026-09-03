// Project:  Privatium™  |  File: crates/privatium-core/src/http/shell.rs
// Authors:  Gabriel Mongefranco (@gabrielmongefranco)
// Created:  2026-09-03  |  Modified: 2026-09-03
// Summary:  The framework's own pages — launcher, settings, errors — as server-rendered HTML
//           with HTMX and inlined Bootstrap Icons (docs/architecture.md §2.5, docs/icons.md).
//           No client framework, no bundler, no inline script or style: every page renders
//           under the default CSP of spec/protocol.md §9.3 exactly as written.

use std::fmt::Write as _;

use axum::http::StatusCode;

use crate::app::{LoadReport, Warning};
use crate::config::Mode;
use crate::http::csrf::Csrf;
use crate::icons::{escape, icon};
use crate::store::Tier;
use crate::wire::router::{SettingsPage, url};
use crate::{Node, Result, StoreError, sys};

/// What a page needs to render.
pub struct Context<'a> {
    /// The node, under the handler's lock.
    pub node: &'a Node,
    /// What `load_apps` reported at startup.
    pub report: &'a LoadReport,
    /// The token issuer for the page's forms.
    pub csrf: &'a Csrf,
}

/// A one-line message shown at the top of a settings page.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Notice {
    /// `ok`, `warn`, or `alert`, which is also the icon: status never by colour alone.
    pub kind: NoticeKind,
    /// The text.
    pub text: String,
}

/// The three tones a notice takes.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum NoticeKind {
    /// Something worked.
    Ok,
    /// Something needs the owner's attention.
    Warn,
    /// `§3.10`'s `alert`: MUST surface in the UI.
    Alert,
}

impl NoticeKind {
    fn class(self) -> &'static str {
        match self {
            Self::Ok => "pv-notice-info",
            Self::Warn => "pv-notice-warn",
            Self::Alert => "pv-notice-alert",
        }
    }

    fn icon(self) -> String {
        match self {
            Self::Ok => icon("check-lg"),
            Self::Warn => icon("exclamation-triangle"),
            Self::Alert => icon("shield-exclamation"),
        }
    }
}

/// Which header link is the current page.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Active {
    Launcher,
    Settings,
    None,
}

/// The page frame. `solo` drops the launcher link: there is no launcher to link to.
fn layout(title: &str, active: Active, solo: bool, body: &str) -> String {
    let mut out = String::with_capacity(body.len() + 2048);
    out.push_str("<!doctype html>\n<html lang=\"en\">\n<head>\n<meta charset=\"utf-8\">\n");
    out.push_str("<meta name=\"viewport\" content=\"width=device-width, initial-scale=1\">\n");
    // htmx never evaluates anything: the shell has no `hx-on` and no `js:` values, and the
    // config says so, which keeps the default CSP's `script-src 'self'` honest (AGENTS.md).
    out.push_str(
        "<meta name=\"htmx-config\" content='{\"allowEval\":false,\"allowScriptTags\":false,\
         \"selfRequestsOnly\":true}'>\n",
    );
    let _ = writeln!(out, "<title>{} — Privatium</title>", escape(title));
    out.push_str("<link rel=\"stylesheet\" href=\"/static/shell.css\">\n");
    out.push_str("<script src=\"/static/htmx.min.js\" defer></script>\n");
    out.push_str("</head>\n<body>\n<a class=\"pv-skip\" href=\"#main\">Skip to content</a>\n");
    out.push_str("<header class=\"pv-header\">\n<h1><a href=\"/\">");
    out.push_str(&icon("grid-3x3-gap"));
    out.push_str(" Privatium</a></h1>\n<nav aria-label=\"Framework\">\n");
    if !solo {
        let _ = writeln!(
            out,
            "<a href=\"/\"{}>{} Apps</a>",
            current(active == Active::Launcher),
            icon("grid-3x3-gap")
        );
    }
    let _ = writeln!(
        out,
        "<a href=\"/settings\"{}>{} Settings</a>",
        current(active == Active::Settings),
        icon("gear")
    );
    out.push_str("</nav>\n</header>\n<main id=\"main\">\n");
    out.push_str(body);
    out.push_str(
        "\n</main>\n<footer>Privatium — <code>pv/1</code>, Phase 1: this node listens on \
                  loopback only; LAN access arrives with pairing.</footer>\n</body>\n</html>\n",
    );
    out
}

fn current(active: bool) -> &'static str {
    if active { " aria-current=\"page\"" } else { "" }
}

/// `/` in host mode: every enabled app in the index, mounted ones as links and the rest as
/// unavailable with the reason — an app whose folder is missing is shown, not hidden
/// (`spec/data-dictionary.md §3.4`).
pub fn launcher(cx: &Context<'_>) -> Result<String> {
    let mut body = String::from("<h2>Apps</h2>\n");
    let rows = query(
        cx.node,
        "SELECT id, title, icon, last_error FROM v_app_nav",
        |row| {
            Ok((
                row.get::<_, String>(0)?,
                row.get::<_, Option<String>>(1)?,
                row.get::<_, Option<String>>(2)?,
                row.get::<_, Option<String>>(3)?,
            ))
        },
    )?;
    if rows.is_empty() {
        body.push_str(
            "<p class=\"pv-muted\">No apps yet. Copy an app folder into <code>apps/</code> under \
             the data directory (see <a href=\"/settings/data\">Data and backup</a>) and restart.</p>",
        );
        return Ok(layout("Apps", Active::Launcher, false, &body));
    }
    body.push_str("<ul class=\"pv-launcher\">\n");
    for (slug, title, glyph, last_error) in rows {
        let title = title.unwrap_or_else(|| slug.clone());
        let glyph = glyph.as_deref().unwrap_or(crate::icons::FALLBACK);
        match cx.node.app(&slug).and_then(|app| app.mount()) {
            Some(mount) => {
                let _ = writeln!(
                    body,
                    "<li><a href=\"{}\">{}<span>{}<small>{}</small></span></a></li>",
                    escape(&url(mount, "")),
                    icon(glyph),
                    escape(&title),
                    escape(&slug)
                );
            }
            None => {
                let reason = last_error.unwrap_or_else(|| "not loaded".to_owned());
                let _ = writeln!(
                    body,
                    "<li><div class=\"pv-unavailable\">{}<span>{}<small>{} — unavailable: {}</small>\
                     </span></div></li>",
                    icon(glyph),
                    escape(&title),
                    escape(&slug),
                    escape(&reason)
                );
            }
        }
    }
    body.push_str("</ul>\n");
    Ok(layout("Apps", Active::Launcher, false, &body))
}

/// One of the four settings pages.
pub fn settings(cx: &Context<'_>, page: SettingsPage, notice: Option<&Notice>) -> Result<String> {
    let solo = cx.node.config().node.mode == Mode::Solo;
    let mut body = String::from("<h2>Settings</h2>\n<ul class=\"pv-subnav\">\n");
    for item in SettingsPage::ALL {
        let _ = writeln!(
            body,
            "<li><a href=\"{}\"{}>{}</a></li>",
            item.path(),
            current(item == page),
            item.title()
        );
    }
    body.push_str("</ul>\n");
    if let Some(notice) = notice {
        let _ = writeln!(
            body,
            "<div class=\"pv-notice {}\" role=\"status\">{}<div>{}</div></div>",
            notice.kind.class(),
            notice.kind.icon(),
            escape(&notice.text)
        );
    }
    match page {
        SettingsPage::Node => node_page(cx, &mut body)?,
        SettingsPage::Apps => apps_page(cx, &mut body)?,
        SettingsPage::Data => data_page(cx, &mut body),
        SettingsPage::Devices => devices_page(cx, &mut body)?,
    }
    Ok(layout(page.title(), Active::Settings, solo, &body))
}

fn node_page(cx: &Context<'_>, body: &mut String) -> Result<()> {
    let node = cx.node;
    let config = &node.config().node;
    let row = query(
        node,
        &format!(
            "SELECT display_name, pubkey, created_at, protocol, build FROM {}",
            sys::NODE
        ),
        |row| {
            Ok((
                row.get::<_, Option<String>>(0)?,
                row.get::<_, Option<String>>(1)?,
                row.get::<_, Option<String>>(2)?,
                row.get::<_, Option<String>>(3)?,
                row.get::<_, Option<String>>(4)?,
            ))
        },
    )?;
    let (display_name, pubkey, created_at, protocol, build) =
        row.into_iter().next().unwrap_or_default();

    body.push_str("<div class=\"pv-card\"><h3>");
    body.push_str(&icon("info-circle"));
    body.push_str(" This node</h3>\n<dl>\n");
    dl(body, "Node ID", &code(node.id().as_str()));
    dl(
        body,
        "Display name",
        &display_name.map_or_else(
            || "<span class=\"pv-muted\">not set — the Node ID stands in for it</span>".to_owned(),
            |name| escape(&name),
        ),
    );
    dl(body, "Public key", &code(pubkey.as_deref().unwrap_or("")));
    dl(
        body,
        "Protocol",
        &code(protocol.as_deref().unwrap_or(crate::PROTOCOL)),
    );
    dl(body, "Build", &escape(build.as_deref().unwrap_or("")));
    dl(
        body,
        "Created",
        &escape(created_at.as_deref().unwrap_or("")),
    );
    dl(
        body,
        "Mode",
        &match (config.mode, config.app.as_deref()) {
            (Mode::Solo, Some(app)) => {
                format!("solo — <code>{}</code> at <code>/</code>", escape(app))
            }
            (Mode::Solo, None) => "solo".to_owned(),
            (Mode::Host, _) => {
                "host — apps at <code>/a/&lt;slug&gt;/</code>, launcher at <code>/</code>"
                    .to_owned()
            }
        },
    );
    dl(
        body,
        "Listening",
        &format!(
            "<code>http://127.0.0.1:{}/</code> — loopback only; LAN access arrives with pairing \
             (Phase 2)",
            config.port
        ),
    );
    body.push_str("</dl></div>\n");

    // §3.10: alerts MUST surface in the UI, not only in the log.
    let alerts = query(
        node,
        "SELECT \"at\", kind, subject, detail FROM v_audit_recent WHERE severity = 'alert'",
        |row| {
            Ok((
                row.get::<_, Option<String>>(0)?,
                row.get::<_, Option<String>>(1)?,
                row.get::<_, Option<String>>(2)?,
                row.get::<_, Option<String>>(3)?,
            ))
        },
    )?;
    body.push_str("<h3>Alerts</h3>\n");
    if alerts.is_empty() {
        body.push_str("<p class=\"pv-muted\">No alerts in the last 200 audit rows.</p>\n");
    } else {
        body.push_str("<table><thead><tr><th>When</th><th>Kind</th><th>Subject</th><th>Detail</th></tr></thead><tbody>\n");
        for (at, kind, subject, detail) in alerts {
            let _ = writeln!(
                body,
                "<tr><td>{}</td><td><span class=\"pv-badge pv-badge-alert\">{} {}</span></td><td>{}</td><td><code>{}</code></td></tr>",
                escape(at.as_deref().unwrap_or("")),
                icon("shield-exclamation"),
                escape(kind.as_deref().unwrap_or("")),
                escape(subject.as_deref().unwrap_or("")),
                escape(detail.as_deref().unwrap_or(""))
            );
        }
        body.push_str("</tbody></table>\n");
    }
    Ok(())
}

/// One `sys_app` row as the apps page shows it.
struct AppIndexRow {
    slug: String,
    title: Option<String>,
    version: Option<String>,
    tier: Option<String>,
    source: Option<String>,
    enabled: bool,
    icon: Option<String>,
    installed_at: Option<String>,
    last_error: Option<String>,
}

fn apps_page(cx: &Context<'_>, body: &mut String) -> Result<()> {
    let node = cx.node;
    let rows = query(
        node,
        &format!(
            "SELECT id, title, version, tier, source, enabled, icon, installed_at, last_error \
             FROM {} ORDER BY nav_order NULLS LAST, title, id",
            sys::APP
        ),
        |row| {
            Ok(AppIndexRow {
                slug: row.get(0)?,
                title: row.get(1)?,
                version: row.get(2)?,
                tier: row.get(3)?,
                source: row.get(4)?,
                enabled: row.get::<_, Option<bool>>(5)?.unwrap_or(true),
                icon: row.get(6)?,
                installed_at: row.get(7)?,
                last_error: row.get(8)?,
            })
        },
    )?;

    if rows.is_empty() {
        body.push_str("<p class=\"pv-muted\">No app folders were found.</p>\n");
    }
    for row in rows {
        let loaded = node.app(&row.slug);
        let glyph = row.icon.as_deref().unwrap_or(crate::icons::FALLBACK);
        let _ = writeln!(
            body,
            "<section class=\"pv-card\" id=\"app-{slug}\" aria-labelledby=\"app-{slug}-title\">\n\
             <h3 id=\"app-{slug}-title\">{} {} <span class=\"pv-muted\">{slug}</span> {}</h3>\n<dl>",
            icon(glyph),
            escape(row.title.as_deref().unwrap_or(&row.slug)),
            status_badge(&row, loaded.is_some()),
            slug = escape(&row.slug),
        );
        dl(
            body,
            "Version",
            &escape(row.version.as_deref().unwrap_or("")),
        );
        dl(body, "Tier", &escape(row.tier.as_deref().unwrap_or("")));
        dl(body, "Source", &escape(row.source.as_deref().unwrap_or("")));
        dl(
            body,
            "Installed",
            &escape(
                row.installed_at
                    .as_deref()
                    .unwrap_or("never loaded cleanly"),
            ),
        );
        if let Some(error) = &row.last_error {
            dl(
                body,
                "Last error",
                &format!(
                    "<span class=\"pv-badge pv-badge-warn\">{} {}</span>",
                    icon("exclamation-triangle"),
                    escape(error)
                ),
            );
        }
        if let Some(app) = loaded {
            dl(
                body,
                "Mounted at",
                &app.mount().map_or_else(
                    || "<span class=\"pv-muted\">not mounted in this mode</span>".to_owned(),
                    |mount| {
                        format!(
                            "<a href=\"{0}\"><code>{0}</code></a>",
                            escape(&url(mount, ""))
                        )
                    },
                ),
            );
            dl(
                body,
                "Cache built by",
                &node
                    .restore_tier(&row.slug)
                    .map_or_else(|| "—".to_owned(), |tier| tier_text(tier).to_owned()),
            );
            let mut tables = String::new();
            let tier2 = app.manifest().app.tier == crate::app::Tier::Web;
            for table in &app.store().schema().tables {
                let count = table_count(app, &table.name);
                let _ = write!(
                    tables,
                    "<code>{}</code>: {} row{} ",
                    escape(&table.name),
                    count,
                    if count == 1 { "" } else { "s" }
                );
            }
            if tables.is_empty() {
                tables.push_str(if tier2 {
                    "<span class=\"pv-muted\">no schema.sql — the event log is the store</span>"
                } else {
                    "<span class=\"pv-muted\">no tables declared</span>"
                });
            }
            dl(body, "Tables", &tables);
            dl(body, "Events (this node)", &app.log().seq().to_string());
        }
        body.push_str("</dl>\n");

        let warnings: Vec<&Warning> = cx
            .report
            .warnings
            .iter()
            .filter(|w| w.slug() == row.slug)
            .collect();
        if !warnings.is_empty() {
            body.push_str("<div class=\"pv-notice pv-notice-warn\">");
            body.push_str(&icon("exclamation-triangle"));
            body.push_str("<div>Load warnings<ul>");
            for warning in warnings {
                let _ = write!(body, "<li>{}</li>", escape(&warning.to_string()));
            }
            body.push_str("</ul></div></div>\n");
        }

        // The seed offer (spec/app-contract.md §9): shown only for a loaded app whose log is
        // empty and whose folder ships a seed; loading is a POST, never a GET, never automatic.
        if let Some(app) = loaded
            && app.seed_path().is_some()
            && app.log().seq() == 0
            && app.log().heads().is_empty()
        {
            let action = format!("/settings/apps/{}/seed", row.slug);
            let _ = writeln!(
                body,
                "<form class=\"pv-inline\" method=\"post\" action=\"{action}\" hx-post=\"{action}\" \
                 hx-target=\"body\" hx-push-url=\"true\">{}<button type=\"submit\" class=\"pv-btn\">\
                 {} Load sample data</button> <span class=\"pv-muted\">synthetic events from \
                 <code>sample/seed.jsonl</code>, appended as this node's; only offered while the \
                 app has no events</span></form>",
                cx.csrf.field(&action),
                icon("plus-lg"),
                action = escape(&action)
            );
        }
        body.push_str("</section>\n");
    }

    if !cx.report.failed.is_empty() || !cx.report.missing.is_empty() {
        body.push_str("<h3>Not loaded at startup</h3>\n<ul>\n");
        for failure in &cx.report.failed {
            let _ = writeln!(
                body,
                "<li>{} <code>{}</code> ({}, at <em>{}</em>): {}</li>",
                icon("exclamation-triangle"),
                escape(&failure.folder),
                failure.source,
                failure.stage,
                escape(&failure.reason)
            );
        }
        for slug in &cx.report.missing {
            let _ = writeln!(
                body,
                "<li>{} <code>{}</code>: folder missing — the index row and the data are kept</li>",
                icon("exclamation-triangle"),
                escape(slug)
            );
        }
        body.push_str("</ul>\n");
    }
    Ok(())
}

fn status_badge(row: &AppIndexRow, loaded: bool) -> String {
    if !row.enabled {
        format!(
            "<span class=\"pv-badge pv-badge-muted\">{} disabled</span>",
            icon("x-lg")
        )
    } else if loaded {
        format!(
            "<span class=\"pv-badge pv-badge-ok\">{} loaded</span>",
            icon("check-lg")
        )
    } else {
        format!(
            "<span class=\"pv-badge pv-badge-warn\">{} unavailable</span>",
            icon("exclamation-triangle")
        )
    }
}

fn tier_text(tier: Tier) -> &'static str {
    match tier {
        Tier::Sqlite => "tier 1 — SQLite snapshot plus log tail",
        Tier::Csv => "tier 2 — CSV snapshot plus log tail",
        Tier::Replay => "tier 3 — full replay of the log",
    }
}

/// Rows in one of an app's tables, through the sandboxed connection — the same one the data
/// API will use, taken and dropped inside the handler's lock so it never outlives a
/// privileged window (M5: never hold an `app_conn()` across `refresh_app`).
fn table_count(app: &crate::App, table: &str) -> i64 {
    let Ok(conn) = app.store().app_conn() else {
        return 0;
    };
    conn.query_row(
        &format!(
            "SELECT count(*) FROM {}",
            crate::store::materialize::quote_ident(table)
        ),
        [],
        |row| row.get(0),
    )
    .unwrap_or(0)
}

fn data_page(cx: &Context<'_>, body: &mut String) {
    let paths = cx.node.paths();
    body.push_str("<div class=\"pv-card\"><h3>");
    body.push_str(&icon("archive"));
    body.push_str(" Where your data is</h3>\n<dl>\n");
    dl(
        body,
        "Data directory",
        &code(&paths.root().display().to_string()),
    );
    dl(
        body,
        "Your information",
        &format!(
            "{} — every event, as plain text you can open in any editor",
            code(&paths.data_dir().display().to_string())
        ),
    );
    dl(
        body,
        "Your node's key",
        &format!(
            "{} — back up separately and privately; leaking it is worse than losing it",
            code(&paths.identity_dir().display().to_string())
        ),
    );
    dl(
        body,
        "App folders",
        &format!(
            "{} — optional; re-downloadable",
            code(&paths.apps_dir().display().to_string())
        ),
    );
    dl(
        body,
        "Disposable",
        &format!(
            "{} and {} — rebuilt on demand; never back these up",
            code(&paths.cache_dir().display().to_string()),
            code(&paths.local_dir().display().to_string())
        ),
    );
    body.push_str("</dl></div>\n");
    body.push_str(
        "<h3>Backup</h3>\n\
         <p><strong>Copy the <code>data</code> folder. That is the backup.</strong> \
         <strong>Copy it back. That is the restore.</strong></p>\n\
         <p>Point Syncthing, Dropbox, OneDrive, or a monthly USB stick at the folder above. No \
         Privatium configuration is needed for any of them: two devices never write the same \
         file, so a file syncer can never produce a conflict.</p>\n\
         <p>Snapshots under <code>data/&lt;app&gt;/snap/</code> and the SQLite files under \
         <code>cache/</code> are caches. Deleting every one of them loses no data.</p>\n\
         <p>Backups are plain text by design. Encrypt the destination if the destination needs \
         it; the filesystem is where at-rest encryption belongs.</p>\n\
         <p class=\"pv-muted\">The full procedure is <code>docs/backup-and-restore.md</code>.</p>\n",
    );
}

fn devices_page(cx: &Context<'_>, body: &mut String) -> Result<()> {
    let node = cx.node;
    let rows = query(
        node,
        "SELECT id, kind, replica, label, paired_at, last_seen_at FROM v_device_active ORDER BY id",
        |row| {
            Ok((
                row.get::<_, String>(0)?,
                row.get::<_, Option<String>>(1)?,
                row.get::<_, Option<bool>>(2)?,
                row.get::<_, Option<String>>(3)?,
                row.get::<_, Option<String>>(4)?,
                row.get::<_, Option<String>>(5)?,
            ))
        },
    )?;
    body.push_str("<div class=\"pv-notice pv-notice-info\">");
    body.push_str(&icon("qr-code"));
    body.push_str(
        "<div>Pairing arrives in Phase 2. Until then this node is the only device: it listens on \
         loopback, and every request is its own.</div></div>\n",
    );
    body.push_str("<table><thead><tr><th>Device</th><th>Kind</th><th>Replica</th><th>Label</th><th>Paired</th><th>Last seen</th></tr></thead><tbody>\n");
    for (id, kind, replica, label, paired_at, last_seen) in rows {
        let this_node = id == node.id().as_str();
        let _ = writeln!(
            body,
            "<tr><td>{} <code>{}</code>{}</td><td>{}</td><td>{}</td><td>{}</td><td>{}</td><td>{}</td></tr>",
            if kind.as_deref() == Some("node") {
                icon("hdd-network")
            } else {
                icon("phone")
            },
            escape(&id),
            if this_node {
                " <span class=\"pv-badge pv-badge-ok\">this node</span>"
            } else {
                ""
            },
            escape(kind.as_deref().unwrap_or("")),
            match replica {
                Some(true) => "yes",
                Some(false) => "no",
                None => "",
            },
            escape(label.as_deref().unwrap_or("")),
            escape(paired_at.as_deref().unwrap_or("—")),
            escape(last_seen.as_deref().unwrap_or("—")),
        );
    }
    body.push_str("</tbody></table>\n");
    Ok(())
}

/// The 404 page.
#[must_use]
pub fn not_found(path: &str, solo: bool) -> String {
    let body = format!(
        "<h2>Not found</h2>\n<p class=\"pv-error\">{} Nothing is served at <code>{}</code>.</p>\n",
        icon("exclamation-triangle"),
        escape(path)
    );
    layout("Not found", Active::None, solo, &body)
}

/// A Tier 1 route in a build without the Lua host — a clear 503, not a 404 that looks like
/// a routing bug.
#[must_use]
pub fn no_handler(slug: &str, solo: bool) -> String {
    let body = format!(
        "<h2>No handler in this build</h2>\n<p class=\"pv-error\">{} <code>{}</code> is a Tier 1 \
         (Lua) app and is mounted, but this build has no Lua host yet; its routes answer once it \
         does.</p>\n",
        icon("info-circle"),
        escape(slug)
    );
    layout("No handler in this build", Active::None, solo, &body)
}

/// A handler answered `pv.render` in a build without the template engine (M8): the app
/// and its routes work, the view does not yet. A clear 503, not a routing bug.
#[must_use]
pub fn view_not_rendered(slug: &str, view: &str, solo: bool) -> String {
    let body = format!(
        "<h2>Templates are not in this build</h2>\n<p class=\"pv-error\">{} <code>{}</code> \
         answered with <code>pv.render('{}')</code>. <code>views/{}.lsp</code> exists, but this \
         build has no LSP compiler yet; the view renders once it does.</p>\n",
        icon("info-circle"),
        escape(slug),
        escape(view),
        escape(view)
    );
    layout("Templates are not in this build", Active::None, solo, &body)
}

/// A failure page. `detail` is the error's own text; the owner is the only reader.
#[must_use]
pub fn error(status: StatusCode, detail: &str, solo: bool) -> String {
    let body = format!(
        "<h2>{}</h2>\n<p class=\"pv-error\">{} {}</p>\n",
        escape(status.canonical_reason().unwrap_or("Error")),
        icon("exclamation-triangle"),
        escape(detail)
    );
    layout(
        status.canonical_reason().unwrap_or("Error"),
        Active::None,
        solo,
        &body,
    )
}

fn dl(out: &mut String, term: &str, definition: &str) {
    let _ = writeln!(out, "<dt>{}</dt><dd>{definition}</dd>", escape(term));
}

fn code(text: &str) -> String {
    format!("<code>{}</code>", escape(text))
}

/// Run a read on the `_sys` store's privileged connection.
fn query<T>(
    node: &Node,
    sql: &str,
    map: impl FnMut(&rusqlite::Row<'_>) -> rusqlite::Result<T>,
) -> Result<Vec<T>> {
    let duck = |error| crate::Error::Store(Box::new(StoreError::Sql(error)));
    let conn = node.store().conn();
    let mut statement = conn.prepare(sql).map_err(duck)?;
    let rows = statement.query_map([], map).map_err(duck)?;
    let mut out = Vec::new();
    for row in rows {
        out.push(row.map_err(duck)?);
    }
    Ok(out)
}
