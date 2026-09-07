// Project:  Privatium™  |  File: crates/privatium-core/src/http/devices.rs
// Authors:  Gabriel Mongefranco (@gabrielmongefranco)
// Created:  2026-09-06  |  Modified: 2026-09-06
// Summary:  The devices page and the code page (spec/protocol.md §7.1, §7.2, §9.2;
//           spec/data-dictionary.md §3.2), and the owner's part of the node page — the
//           display-name form and the nodes discovered on the network, by ID. Every label,
//           user agent and display name is a device's or the owner's text and is escaped on
//           the way into the page; the forms appear for the owner alone.
//           See main README.md for full license information.

use std::fmt::Write as _;

use crate::http::pairing::DISCLOSURE;
use crate::http::shell::{Context, code, dl, query};
use crate::icons::{escape, icon};
use crate::identity::NodeId;
use crate::pair::{PairingSnapshot, qr};
use crate::{Result, sys};

/// One active device as the page lists it.
struct DeviceRow {
    id: String,
    kind: Option<String>,
    replica: Option<bool>,
    label: Option<String>,
    paired_at: Option<String>,
    last_seen_at: Option<String>,
    user_agent: Option<String>,
}

fn active_devices(cx: &Context<'_>) -> Result<Vec<DeviceRow>> {
    query(
        cx.node,
        "SELECT id, kind, replica, label, paired_at, last_seen_at, user_agent \
         FROM v_device_active ORDER BY id",
        |row| {
            Ok(DeviceRow {
                id: row.get(0)?,
                kind: row.get(1)?,
                replica: row.get(2)?,
                label: row.get(3)?,
                paired_at: row.get(4)?,
                last_seen_at: row.get(5)?,
                user_agent: row.get(6)?,
            })
        },
    )
}

/// The user-facing word for a device kind: the shell calls a node a space.
fn kind_word(kind: Option<&str>) -> &str {
    match kind {
        Some("node") => "space",
        other => other.unwrap_or(""),
    }
}

/// `/settings/devices`: the pairing state with the button that opens it, then every
/// active device with its label form and its revoke button. A paired session sees the
/// list and none of the forms.
pub fn page(cx: &Context<'_>, body: &mut String) -> Result<()> {
    let node = cx.node;
    let now = jiff::Timestamp::now();
    let open = node.pairing_open(now);
    body.push_str("<div class=\"pv-card\"><h3>");
    body.push_str(&icon("qr-code"));
    body.push_str(" Pair a device</h3>\n");
    if open {
        body.push_str(
            "<p>Pairing is <strong>open</strong>. The code is on the \
             <a href=\"/settings/devices/pairing\">pairing page</a>.</p>\n",
        );
    } else {
        body.push_str(
            "<p>Pairing is <strong>closed</strong>: nothing can pair until you open it.</p>\n",
        );
    }
    if cx.owner {
        let action = "/settings/devices/pair";
        let _ = writeln!(
            body,
            "<form class=\"pv-inline\" method=\"post\" action=\"{action}\">{}\
             <button type=\"submit\" class=\"pv-btn pv-btn-primary\">{} Open pairing</button></form> \
             <span class=\"pv-muted\">or run <code>privatium pair</code> in a terminal</span>",
            cx.csrf.field(action),
            icon("qr-code")
        );
    } else {
        body.push_str(
            "<p class=\"pv-muted\">Open pairing, label a device or revoke one from the space \
             itself: only the owner at its keyboard can.</p>\n",
        );
    }
    body.push_str("</div>\n");

    let rows = active_devices(cx)?;
    body.push_str(
        "<table class=\"pv-records\" role=\"table\" aria-label=\"Devices\"><thead role=\"rowgroup\"><tr role=\"row\">\
         <th scope=\"col\">Device</th><th scope=\"col\">Kind</th><th scope=\"col\">Replica</th>\
         <th scope=\"col\">Label</th><th scope=\"col\">Browser</th><th scope=\"col\">Connected</th>\
         <th scope=\"col\">Last seen</th><th scope=\"col\">Actions</th></tr></thead><tbody role=\"rowgroup\">\n",
    );
    for row in rows {
        let this_node = row.id == node.id().as_str();
        let id = escape(&row.id);
        // The row came from the log, which anything may have appended to. A form is
        // offered only for an ID shaped as one — the router answers nothing else — and
        // the ID goes into an attribute escaped even then.
        let actionable = cx.owner && !this_node && NodeId::is_valid(&row.id);
        let label_cell = if actionable {
            let action = format!("/settings/devices/{id}/label");
            format!(
                "<form class=\"pv-inline pv-label-form\" method=\"post\" action=\"{action}\">{}\
                 <label for=\"label-{id}\" class=\"pv-visually-hidden\">Label for {id}</label>\
                 <input id=\"label-{id}\" name=\"label\" maxlength=\"{}\" value=\"{}\"> \
                 <button type=\"submit\" class=\"pv-btn\">Save</button></form>",
                cx.csrf.field(&action),
                crate::registry::LABEL_MAX,
                escape(row.label.as_deref().unwrap_or(""))
            )
        } else {
            escape(row.label.as_deref().unwrap_or(""))
        };
        let actions = if actionable {
            let action = format!("/settings/devices/{id}/revoke");
            format!(
                "<form class=\"pv-inline\" method=\"post\" action=\"{action}\">{}\
                 <button type=\"submit\" class=\"pv-btn pv-btn-danger\">{} Revoke {id}</button></form>",
                cx.csrf.field(&action),
                icon("x-circle")
            )
        } else {
            String::new()
        };
        let _ = writeln!(
            body,
            "<tr role=\"row\"><td role=\"cell\"><span class=\"pv-cell-label\" aria-hidden=\"true\">Device</span>{} <code>{id}</code>{}</td>\
             <td role=\"cell\"><span class=\"pv-cell-label\" aria-hidden=\"true\">Kind</span>{}</td>\
             <td role=\"cell\"><span class=\"pv-cell-label\" aria-hidden=\"true\">Replica</span>{}</td>\
             <td role=\"cell\"><span class=\"pv-cell-label\" aria-hidden=\"true\">Label</span>{label_cell}</td>\
             <td role=\"cell\"><span class=\"pv-cell-label\" aria-hidden=\"true\">Browser</span>{}</td>\
             <td role=\"cell\"><span class=\"pv-cell-label\" aria-hidden=\"true\">Connected</span>{}</td>\
             <td role=\"cell\"><span class=\"pv-cell-label\" aria-hidden=\"true\">Last seen</span>{}</td>\
             <td role=\"cell\"><span class=\"pv-cell-label\" aria-hidden=\"true\">Actions</span>{actions}</td></tr>",
            if row.kind.as_deref() == Some("node") {
                icon("hdd-network")
            } else {
                icon("phone")
            },
            if this_node {
                " <span class=\"pv-badge pv-badge-ok\">this space</span>"
            } else {
                ""
            },
            escape(kind_word(row.kind.as_deref())),
            match row.replica {
                Some(true) => "yes",
                Some(false) => "no",
                None => "",
            },
            escape(row.user_agent.as_deref().unwrap_or("")),
            escape(row.paired_at.as_deref().unwrap_or("—")),
            escape(row.last_seen_at.as_deref().unwrap_or("—")),
        );
    }
    body.push_str("</tbody></table>\n");
    Ok(())
}

/// The card of `/settings/devices/pairing`, always with the id `pv-pairing` so a poll
/// can replace it in place: the four glyphs with their labels beneath, the two words,
/// the QR code as an image with the URL as text beside it, the seconds remaining, the
/// close button, and the sentence `spec/protocol.md §7.7` asks for on a plain-HTTP page.
/// While the window is open the card asks htmx to fetch the page again every five
/// seconds and swap the card, so the seconds and a replaced code appear without a
/// reload; once the window is consumed or closed the poll stops.
pub fn pairing_card(
    cx: &Context<'_>,
    window: Option<&PairingSnapshot>,
    now: jiff::Timestamp,
) -> Result<String> {
    let mut out = String::new();
    let Some(window) = window else {
        out.push_str("<div id=\"pv-pairing\" class=\"pv-card\"><h3>");
        out.push_str(&icon("qr-code"));
        out.push_str(
            " Pairing is closed</h3>\n<p role=\"status\">No pairing window is open. Open one to \
             show a code, or run <code>privatium pair</code> in a terminal.</p>\n",
        );
        let action = "/settings/devices/pair";
        let _ = writeln!(
            out,
            "<form class=\"pv-inline\" method=\"post\" action=\"{action}\">{}\
             <button type=\"submit\" class=\"pv-btn pv-btn-primary\">{} Open pairing</button></form>",
            cx.csrf.field(action),
            icon("qr-code")
        );
        out.push_str("</div>\n");
        return Ok(out);
    };
    let consumed = window.consumed_by.as_deref();
    let expires: Option<jiff::Timestamp> = window.expires_at.parse().ok();
    let remaining = expires.map_or(0, |at| at.duration_since(now).as_secs().max(0));
    let polling = consumed.is_none() && remaining > 0;
    let _ = writeln!(
        out,
        "<div id=\"pv-pairing\" class=\"pv-card\"{}><h3>{} Pair a device</h3>",
        if polling {
            " hx-get=\"/settings/devices/pairing\" hx-trigger=\"every 5s\" \
             hx-select=\"#pv-pairing\" hx-swap=\"outerHTML\""
        } else {
            ""
        },
        icon("qr-code")
    );
    let _ = writeln!(
        out,
        "<p>On the other device, open <strong><code>{url}</code></strong> — or scan the code — \
         then tap the four emoji shown here, or type the two words.</p>",
        url = escape(&window.url)
    );
    out.push_str("<div class=\"pv-pair-code\">\n");
    match qr::svg(&window.url, &format!("QR code that opens {}", window.url)) {
        Some(svg) => {
            let _ = writeln!(
                out,
                "<figure class=\"pv-qr\">{svg}<figcaption><code>{}</code></figcaption></figure>",
                escape(&window.url)
            );
        }
        None => {
            let _ = writeln!(
                out,
                "<p>Type this address on the other device: <code>{}</code></p>",
                escape(&window.url)
            );
        }
    }
    out.push_str("<div class=\"pv-code\"><h4>Tap these four emoji</h4>\n<ol class=\"pv-glyphs\">");
    for (glyph, label) in window.emoji.iter().zip(window.labels.iter()) {
        let _ = write!(
            out,
            "<li><span class=\"pv-glyph\" aria-hidden=\"true\">{glyph}</span>\
             <span class=\"pv-glyph-label\">{label}</span></li>"
        );
    }
    let _ = writeln!(
        out,
        "</ol>\n<h4>Or type these two words</h4>\n<p class=\"pv-words\">{} {}</p></div>\n</div>",
        window.words[0], window.words[1]
    );
    match consumed {
        Some(device) => {
            let name = device_label(cx, device)?;
            let _ = writeln!(
                out,
                "<p role=\"status\">{} Paired: <strong>{}</strong> (<code>{}</code>). It is on the \
                 <a href=\"/settings/devices\">devices page</a>; this window is closed.</p>",
                icon("check-lg"),
                escape(name.as_deref().unwrap_or(device)),
                escape(device)
            );
        }
        None if remaining == 0 => {
            out.push_str(
                "<p role=\"status\">This pairing window has closed. Open pairing again for a new \
                 code.</p>\n",
            );
        }
        None => {
            let _ = writeln!(
                out,
                "<p role=\"status\">Waiting for a device. About {remaining} second{} remain; a \
                 code allows five attempts before a new one is shown here.</p>",
                if remaining == 1 { "" } else { "s" }
            );
        }
    }
    let _ = writeln!(out, "<p class=\"pv-muted pv-disclosure\">{DISCLOSURE}</p>");
    if consumed.is_none() && remaining > 0 {
        let action = "/settings/devices/pairing/close";
        let _ = writeln!(
            out,
            "<form class=\"pv-inline\" method=\"post\" action=\"{action}\">{}\
             <button type=\"submit\" class=\"pv-btn\">{} Close pairing</button></form>",
            cx.csrf.field(action),
            icon("x-lg")
        );
    } else {
        let action = "/settings/devices/pair";
        let _ = writeln!(
            out,
            "<form class=\"pv-inline\" method=\"post\" action=\"{action}\">{}\
             <button type=\"submit\" class=\"pv-btn pv-btn-primary\">{} Open pairing again</button></form>",
            cx.csrf.field(action),
            icon("qr-code")
        );
    }
    out.push_str("</div>\n");
    Ok(out)
}

/// A device's label, if it has one.
fn device_label(cx: &Context<'_>, device: &str) -> Result<Option<String>> {
    let sql = format!("SELECT label FROM {} WHERE id = ?1", sys::DEVICE);
    let conn = cx.node.store().conn();
    match conn.query_row(&sql, rusqlite::params![device], |row| {
        row.get::<_, Option<String>>(0)
    }) {
        Ok(label) => Ok(label),
        Err(rusqlite::Error::QueryReturnedNoRows) => Ok(None),
        Err(error) => Err(crate::Error::Store(Box::new(crate::StoreError::Sql(error)))),
    }
}

/// The owner's part of the node page: the display-name form (`spec/protocol.md §6.1`),
/// and the nodes discovered on this network, keyed by ID (`Node::discovered`).
pub fn node_section(cx: &Context<'_>, display_name: Option<&str>, body: &mut String) {
    if cx.owner {
        let action = "/settings/name";
        let _ = writeln!(
            body,
            "<form method=\"post\" action=\"{action}\">{}\
             <label for=\"display-name\">Display name</label>\
             <input id=\"display-name\" name=\"display_name\" maxlength=\"{}\" value=\"{}\" \
             autocomplete=\"off\">\
             <button type=\"submit\" class=\"pv-btn pv-btn-primary\">Save name</button>\
             <p class=\"pv-help\">Shown to your other devices when they look for this space. Leave \
             it empty to show the Space ID instead.</p></form>",
            cx.csrf.field(action),
            crate::registry::DISPLAY_NAME_MAX,
            escape(display_name.unwrap_or(""))
        );
    }
    body.push_str("<h3>Spaces on this network</h3>\n");
    let found = cx.node.discovered();
    if cx.node.discovery_status().is_none() {
        body.push_str(
            "<p class=\"pv-muted\">Discovery is not running, so no other space can be seen from \
             here.</p>\n",
        );
    } else if found.is_empty() {
        body.push_str(
            "<p class=\"pv-muted\">No other space has been seen on this network yet.</p>\n",
        );
    } else {
        body.push_str("<ul class=\"pv-found\">\n");
        for seen in found {
            let address = seen
                .addrs
                .first()
                .map(|ip| format!("http://{}", std::net::SocketAddr::new(*ip, seen.port)));
            let _ = writeln!(
                body,
                "<li>{} <code>{}</code> {} — {} — pairing {}</li>",
                icon("hdd-network"),
                escape(&seen.id),
                escape(&seen.name),
                address.map_or_else(|| "no address yet".to_owned(), |a| code(&a)),
                if seen.pair { "open" } else { "closed" }
            );
        }
        body.push_str("</ul>\n");
    }
}

/// The two rows of the node page's card that this module knows: where the node listens.
pub fn listening_rows(cx: &Context<'_>, body: &mut String) {
    dl(
        body,
        "On this network",
        &format!(
            "{} — the address a phone on the same network opens",
            code(&cx.node.listen_url())
        ),
    );
    dl(
        body,
        "On this computer",
        &code(&format!("http://127.0.0.1:{}/", cx.node.config().node.port)),
    );
}
