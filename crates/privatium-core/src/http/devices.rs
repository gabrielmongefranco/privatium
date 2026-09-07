// Project:  Privatium™  |  File: crates/privatium-core/src/http/devices.rs
// Authors:  Gabriel Mongefranco (@gabrielmongefranco)
// Created:  2026-09-06  |  Modified: 2026-09-07
// Summary:  The devices page and the code page (spec/protocol.md §7.1, §7.2, §9.2;
//           spec/data-dictionary.md §3.2) — a window for devices or for a node — and the
//           owner's part of the node page: the display-name form, the join form
//           (§2.3.1), this node's standing, and the nodes discovered on the network as
//           peers and strangers (§6.1), by ID. Every label, user agent and display name
//           is a device's or the owner's text and is escaped on the way into the page;
//           the forms appear for the owner alone.
//           See main README.md for full license information.

use std::fmt::Write as _;

use crate::http::pairing::DISCLOSURE;
use crate::http::shell::{Context, code, dl, query};
use crate::icons::{escape, icon};
use crate::identity::NodeId;
use crate::pair::{Pairing, PairingSnapshot, qr};
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
    let for_node = node.pairing().is_some_and(Pairing::is_for_node);
    body.push_str("<div class=\"pv-card\"><h3>");
    body.push_str(&icon("qr-code"));
    body.push_str(" Pair a device</h3>\n");
    if open {
        let _ = writeln!(
            body,
            "<p>Pairing is <strong>open</strong>{}. The code is on the \
             <a href=\"/settings/devices/pairing\">pairing page</a>.</p>",
            if for_node { " for another space" } else { "" }
        );
    } else {
        body.push_str(
            "<p>Pairing is <strong>closed</strong>: nothing can pair until you open it.</p>\n",
        );
    }
    if cx.owner {
        let action = "/settings/devices/pair";
        let admit = "/settings/devices/admit";
        let _ = writeln!(
            body,
            "<form class=\"pv-inline\" method=\"post\" action=\"{action}\">{}\
             <button type=\"submit\" class=\"pv-btn pv-btn-primary\">{} Open pairing</button></form> \
             <form class=\"pv-inline\" method=\"post\" action=\"{admit}\">{}\
             <button type=\"submit\" class=\"pv-btn\">{} Admit a space</button></form> \
             <span class=\"pv-muted\">or run <code>privatium pair</code> or \
             <code>privatium pair --node</code> in a terminal</span>",
            cx.csrf.field(action),
            icon("qr-code"),
            cx.csrf.field(admit),
            icon("hdd-network")
        );
        body.push_str(
            "<p class=\"pv-help\">Pair a device to give a phone or a browser access. Admit a \
             space to add another computer running Privatium to this cluster: it then holds a \
             full copy of your data and the cluster key, and one pairing covers both.</p>\n",
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
                 <button type=\"submit\" class=\"pv-btn pv-btn-danger\">{} Revoke {id}</button></form>{}",
                cx.csrf.field(&action),
                icon("x-circle"),
                if row.kind.as_deref() == Some("node") {
                    " <span class=\"pv-help\">Revoking a space removes it from the cluster \
                     on every space that hears of it; it cannot be re-admitted with the same \
                     key.</span>"
                } else {
                    ""
                }
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
        "<div id=\"pv-pairing\" class=\"pv-card\"{}><h3>{} {}</h3>",
        if polling {
            " hx-get=\"/settings/devices/pairing\" hx-trigger=\"every 5s\" \
             hx-select=\"#pv-pairing\" hx-swap=\"outerHTML\""
        } else {
            ""
        },
        icon(if window.node {
            "hdd-network"
        } else {
            "qr-code"
        }),
        if window.node {
            "Admit a space"
        } else {
            "Pair a device"
        }
    );
    if window.node {
        let _ = writeln!(
            out,
            "<p>On the other space, run <strong><code>privatium pair --join {url}</code></strong> \
             in a terminal — or open its Space settings and use <em>Join a cluster</em> with \
             this address — then type the two words, or the four emoji labels, shown here. \
             Whichever space is new joins the other's cluster.</p>",
            url = escape(&window.url)
        );
    } else {
        let _ = writeln!(
            out,
            "<p>On the other device, open <strong><code>{url}</code></strong> — or scan the code — \
             then tap the four emoji shown here, or type the two words.</p>",
            url = escape(&window.url)
        );
    }
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
                "<p role=\"status\">{} {}: <strong>{}</strong> (<code>{}</code>). It is on the \
                 <a href=\"/settings/devices\">devices page</a>; this window is closed.</p>",
                icon("check-lg"),
                if window.node { "Admitted" } else { "Paired" },
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
/// this node's standing in its cluster (`§2.3.1`, `§2.3.4`), the join form (`§2.3.1`),
/// and the nodes discovered on this network as the cluster's peers and as strangers,
/// keyed by ID (`§6.1`, `Node::peers`, `Node::strangers`).
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
    standing_section(cx, body);
    if cx.owner {
        join_form(cx, body);
    }
    let running = cx.node.discovery_status().is_some();
    body.push_str("<h3>Your other spaces on this network</h3>\n");
    let peers = cx.node.peers();
    if !running {
        body.push_str(
            "<p class=\"pv-muted\">Discovery is not running, so no other space can be seen from \
             here.</p>\n",
        );
    } else if peers.is_empty() {
        body.push_str(
            "<p class=\"pv-muted\">No other space of this cluster has been seen on this network \
             yet. Admit one from the devices page, or join one above.</p>\n",
        );
    } else {
        found_list(body, &peers);
    }
    body.push_str("<h3>Other spaces on this network</h3>\n");
    let strangers = cx.node.strangers();
    if !running {
        body.push_str("<p class=\"pv-muted\">Discovery is not running.</p>\n");
    } else if strangers.is_empty() {
        body.push_str("<p class=\"pv-muted\">No space of another cluster has been seen.</p>\n");
    } else {
        body.push_str(
            "<p class=\"pv-help\">These belong to other clusters — a neighbour's, or a space of \
             yours that has not joined this cluster. They are never contacted.</p>\n",
        );
        found_list(body, &strangers);
    }
}

/// The cluster this node belongs to, and what its certificate or a revocation says.
fn standing_section(cx: &Context<'_>, body: &mut String) {
    let now = jiff::Timestamp::now();
    body.push_str("<h3>Cluster</h3>\n<dl>\n");
    dl(
        body,
        "Cluster ID",
        &code(cx.node.identity().cluster_id().as_str()),
    );
    let expires = escape(&cx.node.identity().certificate().expires_at);
    match cx.node.standing(now) {
        Ok(crate::Standing::Member) => {
            dl(
                body,
                "Certificate",
                &format!("valid until {expires}; renewed on every completed sync"),
            );
        }
        Ok(crate::Standing::Expired) => {
            dl(
                body,
                "Certificate",
                &format!(
                    "<span class=\"pv-badge pv-badge-warn\">{} expired</span> on {expires}. \
                     This space serves you here and answers no other device until it is \
                     re-admitted: run <code>privatium pair --node</code> on another space of \
                     this cluster and join it from here, or open <em>Admit a space</em> here \
                     and run <code>privatium pair --join</code> there.",
                    icon("shield-exclamation")
                ),
            );
        }
        Ok(crate::Standing::Revoked) => {
            dl(
                body,
                "Standing",
                &format!(
                    "<span class=\"pv-badge pv-badge-alert\">{} revoked</span>. This space was \
                     revoked from its cluster; it serves you here and nobody else, and it \
                     cannot be re-admitted with this key. To use it again, stop it, delete \
                     its <code>identity</code> folder, start it, and admit it as a new space.",
                    icon("shield-exclamation")
                ),
            );
        }
        Err(_) => {
            dl(body, "Standing", "could not be read");
        }
    }
    body.push_str("</dl>\n");
}

/// The form that joins another space's cluster (`spec/protocol.md §2.3.1`,
/// `spec/cli.md §8`): the address the other space printed and the code it shows.
fn join_form(cx: &Context<'_>, body: &mut String) {
    let action = "/settings/join";
    let _ = writeln!(
        body,
        "<form method=\"post\" action=\"{action}\" class=\"pv-join-form\">{}\
         <h4>Join a cluster</h4>\
         <p class=\"pv-help\">On the other space, open its devices page and choose \
         <em>Admit a space</em>, or run <code>privatium pair --node</code>. Enter the address \
         it shows and the code — the two words, or the four emoji labels. Whichever space is \
         new joins the other's cluster; this space then holds the same data and the same \
         devices as the other.</p>\
         <label for=\"join-url\">Address of the other space</label>\
         <input id=\"join-url\" name=\"url\" inputmode=\"url\" autocomplete=\"off\" \
         placeholder=\"http://192.0.2.5:8420\" maxlength=\"255\">\
         <label for=\"join-code\">Code shown there</label>\
         <input id=\"join-code\" name=\"code\" autocomplete=\"off\" autocapitalize=\"none\" \
         spellcheck=\"false\" maxlength=\"120\">\
         <button type=\"submit\" class=\"pv-btn pv-btn-primary\">{} Join</button></form>",
        cx.csrf.field(action),
        icon("hdd-network")
    );
}

fn found_list(body: &mut String, found: &[crate::Discovered]) {
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
