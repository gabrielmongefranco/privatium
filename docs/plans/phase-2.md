<!--
Project:  Privatium™
File:     docs/plans/phase-2.md
Authors:  Gabriel Mongefranco (@gabrielmongefranco)
Created:  2026-09-05
Modified: 2026-09-05
Summary:  Implementation plan for Phase 2 — other devices on the LAN: cluster identity,
          session cryptography, pairing, the encrypted browser channel, discovery, and
          the device registry. Non-normative. Where this plan and spec/ disagree, spec/
          wins and this file is wrong.
-->

# Phase 2 Implementation Plan

Target: `docs/roadmap.md` Phase 2 — *other devices on the LAN*. Deliverable: open the app
on your phone by scanning a QR code.

## 0. How to use this

Read `AGENTS.md` in full first, then `spec/protocol.md §2, §6, §7, §8, §9`,
`spec/data-dictionary.md §3.1–§3.3, §3.6, §3.10`, `spec/data-api.md §2.1, §5`,
`spec/cli.md §2, §8`, `docs/security.md`, and `docs/decisions/0003-in-process-adapter.md`.
This plan is a work breakdown, not a substitute for the contract. `docs/plans/phase-1.md`
is the shape it follows and the record of what it builds on; that plan's §2.1 (loopback
only, no flag) and §2.2 (the node is the device) are the two decisions this phase retires.

One milestone per branch, one PR per milestone, in order — M14 to M19, continuing Phase 1's
numbering. A milestone is done when its checklist passes and its named tests are green on
all three platforms, not when it compiles. Write the named tests first; the milestone's
shape is in them. Do not start M(n+1) before M(n) merges.

Section 2 lists the decisions this plan makes that the spec did not. **All eleven are
decided**, by the owner, and the spec carries every one; each section ends with where.
Section 3 is the record of the spec gaps found while writing this plan — all fixed, as
Phase 1's §3 was, so an implementer is not handed a specification they have been told is
wrong. A milestone edits those sections only where the code proves them wrong, in the PR
that finds it, with `skills/` regenerated in the same change (`docs/skills.md §7`).

The Phase 1 rule stands: do not invent CLI flags, `sys_*` column values, routes, or config
keys. Every one of those surfaces is specified. Where Phase 2 needs a new one, §3 names the
spec edit and the milestone makes it — never quietly.

---

## 1. Scope

### In

Cluster identity founded on this node; the X25519 static key; the session layer of
`protocol.md §8` in Rust and in browser JavaScript; pairing — the PAKE, the emoji and word
renderings of the code, `/ws/pair`, the device registry, revocation; the encrypted
application channel at `/ws`; the LAN bind and the auth policy that goes with it; mDNS
advertisement and browsing, the UDP responder; the settings pages for devices and pairing;
`privatium pair`, a real `--no-discovery`, and the QR code behind `--open`; the
documentation and skills that describe all of it.

### Out — do not implement, do not stub, do not leave TODOs referencing

Admitting a second **node** (a pairing that declares `kind = "node"` is refused naming
Phase 3), sync, node certificate renewal on sync, revocation of a node, pkarr, DNS
discovery, relays, iroh, onion services, native shells, `uniffi`, packaging, `privatium
firewall`, HTTPS and certificates, the PWA, attachments.

`start_sync` and `sync_now` stay `Error::Unimplemented`, now naming Phase 3 alone.

### The one-sentence test

If a Phase 2 change lets a second *node* hold this node's data, it is Phase 3's.

---

## 2. Decisions this plan makes — confirm before M14

Eleven, all decided. §2.1, §2.2, §2.3, §2.4, §2.5 and §2.8 change the wire or the
posture; §2.6, §2.7, §2.9 and §2.11 are shape; §2.10 follows from §2.1. Each ends with
the sections that now carry it.

### 2.1 The encrypted channel is an adapter over `core::handle`, and it carries everything a browser does on the LAN except the bootstrap set — DECIDED

`protocol.md §8.2` forbids skipping the session layer on plain HTTP; `§9.2` names `/ws`
"the encrypted application channel"; the roadmap's acceptance bullet is "Wireshark on the
LAN shows no plaintext application data". Nothing in the spec says how a server-rendered
HTMX page gets from the node to a browser through that channel. This is the decision that
shapes the phase.

**The design.** `/ws` is a WebSocket. After the `§8` handshake, every frame from the browser
is an encrypted, serialized HTTP request — method, path, headers, body — and every frame
back is an encrypted piece of the response: a head, then body chunks, then an end marker.
The node decodes a frame into the `Request` of ADR 0003, attaches the session's `Device`,
calls `Handler::handle`, and encrypts the `Response` as it streams. **The channel is a
second adapter in the same process**, beside the socket adapter, and adds no route of its
own — which is exactly what ADR 0003 exists for. Requests on one channel run concurrently
and answers interleave by id, so an SSE stream and a page navigation share a connection.

**What a browser on a plain-HTTP LAN origin fetches in the clear** is the *bootstrap set*
and nothing else: `GET /` and every other page path, which answer the same bootstrap page
— no app data, the client script, a `<noscript>` explanation, and the path that was asked
for; `/static/*`, the framework's own embedded assets; a mounted app's `static/` and `web/`
files; `/api/v1/health` and `/api/v1/manifest`; and the two WebSocket routes. Everything
else — every page, every fragment, every form post, the whole data API and its stream —
is refused on plain HTTP from a non-loopback peer with 403, and is reachable only through
the channel. A Tier 2 app's own `fetch('/a/x/api/...')` is therefore refused on that
origin; `pv.js` routes through the channel and is the way, which §3 row 7 writes down.

**The browser side** is `client.js`, an ES module served from `/static/`, with
`@noble/curves`, `@noble/ciphers` and `@noble/hashes` vendored beside it as ES modules
(`AGENTS.md`, browser crypto). It holds the device keys, runs the handshake, fetches the
real page through the channel and puts it in place of the bootstrap page, and then keeps
htmx on the channel with an extension in the shape of htmx's own `ws` extension. Plain
links and forms that htmx does not own are intercepted at the document and sent the same
way, with `history.pushState`. `pv.js` uses the channel when one exists and plain `fetch`
otherwise — on loopback, in a native shell, on an HTTPS origin — and apps see no
difference, as `spec/data-api.md §5` promises.

**Scripts and stylesheets stay plaintext, and are pinned.** A page delivered through the
channel names its scripts with `<script src>`; the browser fetches those over plain HTTP,
which is where an active attacker after pairing could substitute code. The client closes
that with Subresource Integrity: for every script and stylesheet element it re-creates, it
fetches the file through the channel first, hashes it, and sets `integrity` on the plain
element, so the browser refuses bytes that differ from what the authenticated channel
delivered. No CSP change, no `blob:` scripts. What SRI cannot reach is a Tier 2 module's
own `import` graph; §2.10 says what is claimed and what is not.

The rejected alternatives, so they are not re-litigated: a service worker (unavailable on
a LAN IP, ADR 0003); encrypting bodies over plain HTTP requests without a WebSocket (the
same channel with worse framing, and `§9.2` already names `/ws`); loading app scripts from
`blob:` URLs decrypted in the page (needs `script-src blob:`, a one-way CSP widening).

*Decided. `protocol.md §8.3` is the channel's normative form — the handshake, the frame,
the request and response kinds, the integrity rule — and `§8.4` is the bootstrap set;
`§7.7`, `§9.1` and `§13` follow it, and `data-api.md` says where a plain `fetch` works.
`docs/security.md §3–§4` and `docs/architecture.md §2.6` describe it. M15 and M17 hold
the code to those sections and edit them only where implementation proves them wrong.*

### 2.2 The node binds every interface, and adds no flag to say so — DECIDED

Phase 1 bound loopback because it had no session layer; Phase 2 has one, so the bind is
`0.0.0.0` and `[::]` on `[node] port`, with the IPv6 listener skipped where the platform
refuses it. No `--bind` flag, for the reason Phase 1's §2.1 gave: `spec/cli.md §10` keeps
the surface narrow and a bind address is a property of the phase. Startup prints the LAN
URL — the address of the interface the default route uses, found by connecting a UDP
socket to a public address without sending anything — and every other interface under
`--verbose`.

**A loopback request keeps Phase 1's meaning.** The owner at the keyboard is this node's
own device: no pairing, no session, every route, exactly as `docs/plans/phase-1.md §2.2`
made it, with the `Host` check against DNS rebinding unchanged. A native shell and an
embedder's in-process call keep the same standing. Only a non-loopback peer meets the
policy of §2.1.

The first non-loopback bind is where Windows Defender prompts (`docs/deployment.md §4`);
the helper that opens the port is Phase 6, and the prompt is documented rather than
worked around.

*Decided: `cli.md §2` and `protocol.md §8.4`.*

### 2.3 The cluster is founded now, and devices pin the cluster key from the first pairing — DECIDED

`protocol.md §2.3.2` has devices pin the *cluster* public key, and `docs/roadmap.md` Phase
3's first bullet — a second node admitted with one pairing, the phone reaching it
without re-pairing — only holds if the phone pinned the cluster key in Phase 2. So M14
founds the cluster on the first Phase 2 start: `identity/cluster.key` (`0600`),
`identity/cluster.pub`, this node's `identity/node.cert` signed by the cluster key, the
`sys_cluster` row, and `cluster_id`, `cert` and `cert_expires_at` on `sys_node`. A root
Phase 1 created has none of these and is founded on its next start; that is the ordinary
first Phase 2 run, not a migration.

The certificate's signed bytes are the canonical form of §3 row 4. The founding node
renews its own certificate whenever fewer than ninety days remain, at start; renewal on
sync (`§2.3.1`) is Phase 3's, since a sync is.

A node that founded a cluster alone can still be admitted to another one later: while it
has paired nothing and admitted nobody its cluster is empty and disposable, and joining
discards it and tombstones its `sys_cluster` row, so one row remains. That is what keeps
"found at first start" from making every node the first node.

*Decided: `protocol.md §2.3` and `§2.3.1`, `data-dictionary.md §3.1b`.*

### 2.4 The PAKE is SPAKE2 as RFC 9382 specifies it, over edwards25519, written on both sides from vetted primitives — DECIDED

`§7.4` allows CPace or SPAKE2 over X25519/Ristretto255. The browser side has to be
JavaScript either way, and no audited JavaScript PAKE exists; the Rust `spake2` crate
follows the CFRG draft's own constants rather than the RFC's and has no JavaScript
counterpart; the `cpace` crate is a 2020 snapshot of a draft that has moved since. So
both sides are written to **RFC 9382 §3**, with the RFC's `M` and `N` for edwards25519,
its transcript `TT` and its confirmation MACs — in Rust over `curve25519-dalek`, already
in the graph beneath `ed25519-dalek`, and in JavaScript over `@noble/curves`. Neither side
hand-rolls a primitive: the group arithmetic, the hashes, the KDF and the MAC are the
libraries'. What is written is the protocol, and it is held together by a vector file the
Rust side generates and both sides' tests read (`tests/fixtures/pake-vectors.json`),
because the RFC's own vectors cover P-256 only.

The identities bind both static keys, which `§7.4` step 4 requires: `A` is
`"pv/1 device " ‖ device Ed25519 public key (base64)`, `B` is `"pv/1 node " ‖ node Ed25519
public key (base64)`. The password `w` is `HKDF-SHA256(ikm = the two code bytes, big
endian; salt = ""; info = "pv/1 pake w")`, 64 bytes reduced modulo the group order.

This is a security decision, and the one in this list most worth a second opinion.

*Decided. `protocol.md §7.4` names SPAKE2 alone and strikes CPace; `§7.4.1` fixes the
ciphersuite, the identities, `w`, the transcript encodings and the key schedule; `§7.4.2`
is the message sequence M16 implements. R9 stands: the vector file is what holds the two
implementations to one another.*

### 2.5 The word list is 256 words from the EFF short wordlist, checked in as normative — DECIDED

`§7.2` named "the 256-word list" and no list existed anywhere. The list is wire meaning —
`amber otter` must decode to the same sixteen bits on every implementation — so it is a
spec artefact, `spec/pairing-words.txt`, one word per line, index order normative, and
changing it is a breaking protocol change exactly as `§7.3` says of the glyphs. The words
come from the EFF Short Wordlist 2.0 (1,296 words, every one distinct in its first three
letters and at edit distance three from every other, which is what lets a screen-reader
user abbreviate and lets a typo be caught rather than mis-decoded): the words of four to
six letters, in alphabetical order, the first 256, with three words unsuited to saying
aloud skipped on review. The first draft of the rule said four or five letters and yields
only 193, which is why the rule says six.

*Decided, and the file is written — `spec/pairing-words.txt`, `abyss` first.
`protocol.md §7.2` names it and `NOTICE` attributes it (CC BY 3.0 US).*

### 2.6 The node's X25519 static key is derived from the node key, as the CSRF key is — DECIDED

`§7.4` and `§8` use an X25519 static key; `§3`'s layout has no file for one. Rather than
add a file the spec does not show, M14 derives it: `HKDF-SHA256(ikm = node private key,
info = "privatium/x25519/v1")`, the shape `docs/plans/phase-1.md §2.2` chose for the CSRF
key, with a new `info` string because one purpose is one string. Deterministic, never
stored, wiped on drop. A browser device generates a real X25519 keypair beside its
Ed25519 one, since it has storage to keep both in.

*Decided: `protocol.md §8`.*

### 2.7 Pairing state lives in memory, and there is no Argon2 — DECIDED

`data-dictionary.md §3.3` gives `sys_pairing` a `code_hash` "so that a crash dump or stray
log does not contain a live code". The node has to hold the PAKE secret `w` for the whole
window to answer the handshake at all, so hashing the code beside it protects nothing.
M16 keeps one pairing at a time in memory — `w`, `created_at`, `expires_at`, `attempts`,
`consumed_by` — drops the code bytes the moment `w` is derived, and writes nothing to
disk: `local/` keeps its two files (`§3`), and a code that never touched a file needs no
hash. §3 row 6 amends `§3.3` to say so. No `argon2` crate.

*Decided: `data-dictionary.md §3.3`.*

### 2.8 `privatium pair` asks the running node over loopback, and `--open` on a node with no paired device opens pairing once — DECIDED

A data root is one process's (`§3.1`), so `privatium pair` cannot open the node the
daemon holds; `spec/cli.md §1` already lists it among the commands that take no lock. It
therefore talks to the running node: `POST /api/v1/pair` opens a window and returns the
code, `GET /api/v1/pair` reports it, both answered for this node's own device only —
loopback, in Phase 2 — and both spec edits (§3 row 5). Without a running node, `pair` is a
runtime error saying to start one. The settings page's button posts the same act as a
form.

`§7.1` allows pairing to open on "a button press, a CLI flag, or first-run". This plan
reads *first-run* narrowly: `privatium --open` on a node whose `sys_device` holds no row
but its own opens one pairing window as it starts, prints the code beside the QR, and
never does so again once any device is paired. That is what makes the quick start "run
it, scan it, tap four emoji" true with one command, and it is a posture decision to
confirm. `--open` on a node that has a paired device prints the QR and opens the browser
and opens nothing else.

The QR code encodes the node's LAN URL and nothing more — never the code. The code is on
the screen for the person standing there, which is `§7.1`'s authorization.

*Decided: `protocol.md §7.1` and `§9.2`, `cli.md §2` and `§8`.*

### 2.9 Browser keys live in `localStorage` under the origin, and a browser without JavaScript cannot pair — DECIDED

`§2.2` allows `localStorage` or IndexedDB. `pv.js` already keeps the outbox in
`localStorage`; the device keys, the pinned cluster public key and the node's identity go
beside it under one key, `pv:device`, and loss means re-pairing with no other path
(`§7.6`). The bootstrap page carries a `<noscript>` block saying that pairing needs
JavaScript and that the node at the keyboard needs none — every Phase 1 no-JavaScript
path holds on loopback exactly as before, since loopback never sees the channel.

*Decided: `protocol.md §7.6` and `§8.4`.*

### 2.10 What is claimed about program authenticity after pairing

`§7.7` and `docs/security.md §1` say an active attacker arriving after pairing is
*detected*. With §2.1 that is true of every page, fragment, API call and stream (they
never leave the channel), and of every script and stylesheet the page names (SRI, hashed
over the channel). It is not true of a module a Tier 2 app's own script imports, because
a static `import` carries no integrity: on a plain-HTTP origin those files can still be
substituted. M17 states this in `docs/security.md §4` as the residual gap, closed by the
native shell (Phase 4) and by an HTTPS origin (Phase 5); an import map with integrity is
noted there as the browser feature that would close it in place, once every target
browser has it. `docs/roadmap.md` Phase 4's stub carries the pointer.

*Follows from §2.1 and is written: `protocol.md §7.7` and `docs/security.md §4` say what
is pinned after pairing and what is not.*

### 2.11 Two small shapes: `peers`, and what `--version` claims — DECIDED

`spec/lua-api.md §3.4` calls `pv.node().peers` "the number of paired peers" and
`spec/data-api.md §4` calls `/api/node`'s the "sync peer count". Both mean **paired
nodes** — active `sys_device` rows with `kind = 'node'` other than this one — so both stay
`0` through Phase 2 while browsers appear on the devices page (§3 row 10).

`privatium --version` prints `pv/1 (partial: phase 2)` from M19: the `§13` items Phase 2
cannot claim are all sync's and the remote transports' (§7).

*Decided: `lua-api.md §3.4` and `data-api.md §4`; the version string is `cli.md §1`'s
rule applied.*

---

## 3. Spec gaps found — all fixed, none deferred

Every row is fixed, and the milestone named is the one that holds the code to it. As in
Phase 1, this is the record of what changed and why, not a to-do list;
`cargo xtask gen-skill-reference` ran with the edits.

| # | Was | Proposed | Files | Milestone |
|---|---|---|---|---|
| 1 | `§7.4` gives the handshake's six steps and no message shapes, encodings or close codes | The messages of M16 spelled out: the node's hello, the client's `pA` with its identity, the node's `pB` and `cB`, the client's `cA`, the two key exchanges over `K_pair`, and the WebSocket close codes for *closed*, *wrong code* and *exhausted* | `protocol.md §7.4.1, §7.4.2` | **Fixed**; M16 |
| 2 | `§8` gives the key schedule and says nothing about how a session starts on `/ws`, what a frame is, or what a request or response looks like inside one | The handshake messages, the frame — `nonce = direction ‖ counter`, one AEAD ciphertext per WebSocket binary message, no associated data — the request and response frames of §2.1 with their `id` and `kind`, the confirm frame, and the rule that a side closes at 2³² frames rather than rekeying | `protocol.md §8.3` | **Fixed**; M15, M17 |
| 3 | `§7.2` says "the 256-word list" and no list exists | `spec/pairing-words.txt`, index order normative, produced by the rule in §2.5; `§7.2` names it and says a change is a breaking protocol change | `protocol.md §7.2`, `spec/pairing-words.txt`, `NOTICE` | **Fixed**; M16 |
| 4 | `§2.3.1` signs "the other fields" of the certificate and never says which bytes | The signed message is the JSON object `{"node_id","node_pub","cluster_id","issued_at","expires_at"}` in that key order, no whitespace, UTF-8; `sig` is base64 of the Ed25519 signature; the certificate is that object plus `sig`, and `sys_node.cert` holds it base64-encoded | `protocol.md §2.3, §2.3.1`, `data-dictionary.md §3.1b` | **Fixed**; M14 |
| 5 | `§9.2` has no route that opens pairing; `cli.md §8` does not say how `pair` reaches a running node, or what happens without one | `POST /api/v1/pair` (`{"ttl": seconds}`, answers the code, the URL and `expires_at`) and `GET /api/v1/pair` (the open window or `null`), this node's own device only; `pair` uses them and is a runtime error with no node running; `--open`'s first-run window per §2.8 | `protocol.md §9.2`, `cli.md §2, §8` | **Fixed**; M16, M19 |
| 6 | `§3.3` `sys_pairing` holds an Argon2id `code_hash` and a `salt` | The row is held in memory by the node for the window and never written; it holds the PAKE secret rather than the code; the hash and salt columns go | `data-dictionary.md §3.3` | **Fixed**; M16 |
| 7 | `data-api.md` says "Cookies carry [the session]" and `§5` says "`fetch` works fine" | On a plain-HTTP origin from a non-loopback address the session is the channel and nothing else: no cookie, and a plain `fetch` of the API is refused, naming `pv.js`; on loopback, in a native shell and on an origin `§8.2` exempts, `fetch` works as written | `data-api.md` preamble, `§5` | **Fixed**; M17 |
| 8 | `§7.7` says "any later substitution is refused (§8.1)"; `§8.1` is about keys, and a script on plain HTTP is not a key | `§7.7` says what is pinned after pairing — the session, and every file the page names, by integrity — and what is not, per §2.10; `docs/security.md §4` carries the same | `protocol.md §7.7`, `docs/security.md §4` | **Fixed**; M17 |
| 9 | `§9.1` reserves five prefixes; `/ws` and `/ws/pair` are routes of `§9.2` and `ws` a reserved slug of `§1.1`, but `/ws` is in no prefix table and the router does not know it | `/ws` joins the table as the framework's; the reserved slug already covers the mount | `protocol.md §9.1` | **Fixed**; M17 |
| 17 | `§13` had no line for what plain HTTP may serve, or for the integrity rule | Two items, `§8.4` and `§8.3` | `protocol.md §13` | **Fixed**; M17 |
| 18 | `§7.4` step 5 had the client pin "the node's key" while `§2.3.2` and `§7.6` pin the cluster's | Step 5 says the cluster public key and the node's certificate | `protocol.md §7.4` | **Fixed**; M16 |
| 10 | `lua-api.md §3.4` `peers` "paired peers"; `data-api.md §4` "sync peer count" | Both are the paired nodes, per §2.11 | `lua-api.md §3.4`, `data-api.md §4` | **Fixed**; M19 |
| 11 | `§6.4` refuses a probe "from outside RFC 1918 / RFC 4193 space"; loopback is neither, and the responder's test has nowhere else to probe from | Loopback and link-local are accepted too | `protocol.md §6.4` | **Fixed**; M18 |
| 12 | `§7.1` allows pairing to open on "first-run" and says nothing about what that is | §2.8's definition | `protocol.md §7.1`, `cli.md §2` | **Fixed**; M19 |
| 13 | `app-contract.md §6` lists `pair` and the Phase 1 signature is `pair(&mut self) -> Result<()>`, which cannot hand back a code | `pair(&mut self, ttl: Duration) -> Result<Pairing>`; `serve_discovery(&mut self) -> Result<()>` stays | `app-contract.md §6` | **Fixed**; M16 |
| 14 | `cli.md §2` "prints the LAN URL" — a machine has several | The default route's, and the rest under `--verbose` (§2.2) | `cli.md §2` | **Fixed**; M17 |
| 15 | `§3.2` `sys_device.replica` for a browser is defined; `last_seen_at` "at most hourly" names no writer | The channel handshake writes it, and a session older than an hour writes it again on its next request | `data-dictionary.md §3.2` | **Fixed**; M19 |
| 16 | `§6.1` instance name is "the owner-set display name" and no surface sets one | The node settings page sets `sys_node.display_name`; while unset the Node ID stands in, as `§9.2` already says | `protocol.md §6.1` | **Fixed**; M19 |

Two are additions rather than corrections and deserve to be called out: **`/api/v1/pair`**
widens `§9.2`, and **`spec/pairing-words.txt`** is a new normative file. Both were the
owner's call (§2.8, §2.5).

**The rule from Phase 1 stands:** when implementation reveals a further gap, fix the spec
in the PR that found it. Do not accumulate a list and do not code around it.

---

## 4. Workspace layout — what Phase 2 adds

```
crates/privatium-core/
├── src/
│   ├── identity.rs           + cluster keypair, node certificate, the X25519 static (M14)
│   ├── session/
│   │   ├── mod.rs            key schedule, frames, the nonce discipline (M15)
│   │   └── handshake.rs      the /ws handshake, both roles (M15, M17)
│   ├── pair/
│   │   ├── mod.rs            the pairing window: state, TTL, attempts, audits (M16)
│   │   ├── code.rs           the 16-bit code, glyphs, words, parsing (M16)
│   │   └── spake2.rs         RFC 9382 over edwards25519 (M16)
│   ├── discover/
│   │   ├── mod.rs            serve_discovery: the two mechanisms started together (M18)
│   │   ├── txt.rs            the TXT record and its budget (M18)
│   │   ├── mdns.rs           advertise and browse (M18)
│   │   └── udp.rs            the 52525 responder and probe (M18)
│   ├── wire/channel.rs       /ws and /ws/pair: frames in, handle, frames out (M17)
│   ├── http/auth.rs          the Phase 2 policy (M17)
│   ├── http/devices.rs       the devices page's actions (M19)
│   └── http/pairing.rs       the code page, the QR, the bootstrap page (M16, M19)
├── assets/shell/
│   ├── client.js             keys, handshake, channel, the htmx extension, pairing UI (M17, M19)
│   ├── pair.css              the pad and the code page (M19)
│   └── vendor/noble/         @noble/curves, ciphers, hashes as ES modules, with VENDOR.md (M15)
└── tests/
    ├── identity.rs           + the cluster tests (M14)
    ├── session.rs            (M15)
    ├── pair.rs               (M16)
    ├── channel.rs            through handle, with a faked peer (M17)
    ├── discover.rs           (M18)
    ├── fixtures/session-vectors.json, pake-vectors.json   generated by Rust, read by both (M15, M16)
    └── js/session.test.mjs, pake.test.mjs, client.test.mjs   under node --test (M15–M17)
crates/privatium/
├── src/lib.rs                the bind of §2.2 (M17)
├── src/pair.rs               privatium pair (M19)
└── tests/channel.rs, pairing.rs   real sockets, a peer-faking service, a capturing proxy (M17, M19)
spec/pairing-words.txt        (M16)
```

`Node` is `Send` and not `Sync` (`wire/mod.rs`), and stays so. Discovery threads get a
`watch` channel of the facts the TXT record needs, which the node updates on every change
of its own — pairing opened or closed, apps loaded — so `serve_discovery(&mut self)` keeps
its signature. Nothing in Phase 2 needs to reach *into* the node from another thread; the
channel decoder calls `Handler::handle`, which already shares the node the way every
request does.

---

## 5. Dependencies

| Need | Crate / package | Version, licence | Note |
|---|---|---|---|
| X25519 | `x25519-dalek` | 3.0.0, BSD-3-Clause | `§8`; `static_secrets` feature for the derived static |
| AEAD | `chacha20poly1305` | 0.11.0, Apache-2.0 OR MIT | `§8` requires it over AES-GCM |
| Group arithmetic for SPAKE2 | `curve25519-dalek` | 5.0.0, BSD-3-Clause — already in `Cargo.lock` beneath `ed25519-dalek` | Taken directly for `EdwardsPoint` and `Scalar`; no new compile unit |
| KDF, MAC, hash | `hkdf`, `hmac`, `sha2` | already here | The salt of `§8`, the confirmation MACs of RFC 9382 |
| WebSocket in the core | `axum` feature `ws` | pulls `tokio-tungstenite` 0.29.0, MIT, plus `sha1` and `base64` | `/ws` and `/ws/pair` are routes of the core (`§9.2`); the upgrade is answered where `handle` is |
| WebSocket client, tests | `tokio-tungstenite` | 0.29.0 | A dev-dependency of `crates/privatium` for the socket tests only |
| mDNS | `mdns-sd` | 0.21.1, Apache-2.0 OR MIT | Registers with TXT and sub types, browses, runs its own thread — no runtime dependency, so an embedder without tokio can call `serve_discovery` |
| QR | `qrcode` | 0.14.1, MIT OR Apache-2.0 | Renders to text for the terminal and SVG for the page; last released 2024-07 — check `cargo deny` and its issue tracker at M19 |
| Browser crypto | `@noble/curves`, `@noble/ciphers`, `@noble/hashes` | 2.4.0, MIT | ES modules vendored at `assets/shell/vendor/noble/`, the files `client.js` imports and their imports, unmodified, with `VENDOR.md` and a `NOTICE` entry; loaded as `<script type="module">` under `script-src 'self'` — no bundle exists upstream and none is built here |

**Not taken, and why:** `spake2` (one group, the draft's constants, no JavaScript
counterpart — §2.4); `cpace` (0.1.0 from 2020); `argon2` (§2.7); `notify` (Phase 3
decides it does not need one either); a JavaScript QR library (the QR is rendered by the
node); `local-ip-address` or similar (the UDP-connect trick needs no crate).

Raw event lines stay `String`/`&[u8]` end to end. A frame carries a request's body as
bytes it never parses; `§4.2` is unchanged by the channel.

---

## 6. Milestones

### M14 — Cluster identity, the certificate, the X25519 static

- `Identity` gains the cluster: `load_or_create` founds one when `identity/cluster.key`
  is absent — keypair generated, `cluster.key` written `0600` with the same `create_new`
  discipline as `node.key`, `cluster.pub` beside it, the Cluster ID derived as a Node ID
  is (`§2.3`), and `node.cert` issued and written. A root with a cluster loads it.
- The certificate (`§2.3.1`, §3 row 4): `Certificate { node_id, node_pub, cluster_id,
  issued_at, expires_at, sig }`, `issued_at + 180 days`, canonical JSON for the signed
  bytes, base64 in `sys_node.cert`. `Certificate::verify(&cluster_pub, now)` refuses a
  bad signature and an expired certificate as two distinct errors.
- Renewal at start when fewer than ninety days remain; `cert.renewed` audit (info).
- `sys_cluster` row (`§3.1b`) on founding, keyed by the Cluster ID: `pubkey`,
  `pkarr_name` (z-base32 of the public key — a thirty-line encoder with a test vector,
  not a crate for one function), `created_at`, `created_by`. `sys_node` amended with
  `cluster_id`, `cert`, `cert_expires_at`. One batch, then `cluster.created` (info).
- `Identity::x25519_static() -> x25519_dalek::StaticSecret`, derived per §2.6, and
  `x25519_public_base64()`; this node's own `sys_device` row amended with `ed25519_pub`
  and `x25519_pub` — a `put` under the same key, blessed by `§4.6`.
- `Identity::cluster_public()`, `cluster_id()`, `certificate()`; the cluster private key
  is reachable only through `Identity::sign_certificate(&self, node_pub, now)`, so nothing
  outside the module can copy it.
- The `Debug` impl stays hand-written: no key material prints.

**Produces:** `identity::{Certificate, ClusterId}`, `Identity::{cluster_id, cluster_public,
certificate, sign_certificate, x25519_static, x25519_public_base64}`; `sys::ClusterRow`;
`sys::{KIND_CLUSTER_CREATED, KIND_CERT_RENEWED}`.

**Tests** (`tests/identity.rs`): `test_spec_2_3_first_start_founds_a_cluster`,
`test_spec_2_3_a_phase_1_root_founds_a_cluster_on_its_next_start`,
`test_spec_2_3_1_certificate_verifies_against_the_cluster_key_and_expires_at_180_days`,
`test_spec_2_3_1_certificate_signed_bytes_are_canonical` (a checked-in key, a checked-in
certificate), `test_spec_2_3_1_certificate_renews_under_ninety_days`,
`test_spec_2_3_3_cluster_private_key_is_absent_from_every_event_snapshot_and_backup` (the
raw bytes and their base64 grepped out of `data/`, every `snap/`, and a `backup::Plan`
copy), `test_spec_2_1_x25519_static_is_derived_and_stable`,
`test_spec_3_1b_pkarr_name_is_zbase32_of_the_cluster_public_key`,
`test_identity_second_run_keeps_the_cluster_and_the_node_id`. Unix only:
`test_spec_2_3_cluster_key_mode_0600`.

**Documentation:** `protocol.md §2.3, §2.3.1` and `data-dictionary.md §3.1b` are written
(row 4); `docs/backup-and-restore.md §1` names `cluster.key` beside `node.key`.

---

### M15 — The session layer, in Rust and in JavaScript

- `session::Keys::derive(role, my_static, their_static, my_eph, their_eph, node_id,
  device_id)` — `§8` verbatim: `ss`, `salt = SHA-256(sorted(node_id, device_id) ‖ "pv/1
  session")`, `prk = HKDF-Extract(salt, ss ‖ ee)`, `k_c2s`, `k_s2c`.
- `session::Frame`: `seal(&mut self, plaintext) -> Vec<u8>` and `open(&mut self,
  ciphertext) -> Result<Vec<u8>>` per direction, `nonce = direction tag (4 bytes, big
  endian: 1 for c2s, 2 for s2c) ‖ counter (8 bytes, big endian)`, counter from 0, no
  associated data, one ciphertext per WebSocket binary message. A counter that reaches
  2³² closes the session (§3 row 2). A counter never repeats because the type owns it and
  it is not `Clone`.
- `session::handshake` — the `/ws` messages of §3 row 2, as plain data with no I/O so
  the same code is driven by a test, by the channel and, in Phase 3, by the sync client:
  `ClientHello { v, dev, e }`, `NodeHello { v, id, e, cert }`, then `Confirm { transcript
  }` as the first sealed c2s frame, transcript = `SHA-256(client hello bytes ‖ node hello
  bytes)`. `Handshake::node(identity, lookup: impl Fn(&str) -> Option<DevicePins>)`
  answers a hello and, on the confirm, yields `Session { device, keys }`; the lookup is
  `sys_device` — active, not revoked, with an `x25519_pub`.
- Vendor `@noble/*` (§5) with `VENDOR.md` (versions, SHA-256 per file, licence) and the
  `NOTICE` entry. `assets/shell/session.js`: the same schedule and frame over
  `x25519`, `hkdf`, `sha256` and `chacha20poly1305` from the vendored modules; the
  client role of the handshake.
- `tests/fixtures/session-vectors.json`: fixed statics and ephemerals, the derived keys,
  ten sealed frames in each direction. Generated by a Rust test with a `--` flag that
  writes it (kept out of `cargo test`'s default run), read by the Rust tests and by
  `session.test.mjs`. The Rust side seals, the JavaScript side opens, and back.
- `.github/workflows/ci.yml`'s `node --test` step runs the whole
  `crates/privatium-core/tests/js/` directory from here on, so every later `.test.mjs` is
  in the gate without another edit.

**Produces:** `session::{Keys, Frame, Direction, Session, handshake::{ClientHello,
NodeHello, Confirm, Handshake}}`; `assets/shell/session.js` exporting `derive`,
`Frame`, `clientHandshake`.

**Tests** (`tests/session.rs`): `test_spec_8_key_schedule_matches_the_checked_in_vectors`,
`test_spec_8_frames_round_trip_and_the_counter_never_repeats`,
`test_spec_8_a_tampered_frame_is_refused`,
`test_spec_8_handshake_derives_the_same_keys_on_both_sides`,
`test_spec_8_1_a_static_key_that_is_not_the_pinned_one_fails_the_confirm`,
`test_spec_8_a_session_closes_at_the_counter_limit` (the limit lowered by a constructor
the test alone uses). Under `node --test`: `session.test.mjs` — the vectors, the
cross-language frames, the client handshake against the fixture's node hello.

**Documentation:** `protocol.md §8.3` is written (row 2); this milestone edits it only
where the code proves it wrong.

---

### M16 — Pairing

- `pair::code`: `Code(u16)` from the CSPRNG; `glyphs() -> [Glyph; 4]` from the normative
  table of `§7.3` stored as byte strings with their labels — index 8 and 9 keep U+FE0F
  and a test grep proves no normalization touched them; `words() -> [&'static str; 2]`
  from `spec/pairing-words.txt`, included at compile time and checked by a test for 256
  distinct lowercase words with unique three-letter prefixes; `Code::parse(&str)` accepts
  four glyphs (labels accepted too, so a screen reader's user can type "fox pizza
  lightning die") or two words, case-insensitive, ignoring spaces, hyphens and
  punctuation, and refuses anything else naming what it expected.
- `pair::spake2`: RFC 9382 §3 over edwards25519 with the RFC's `M` and `N` (§2.4). `Side
  A` and `Side B`, `start(w, identity) -> (State, Message)`, `finish(state, their_message)
  -> (Ke, Ka, confirm_to_send, confirm_expected)`; the transcript `TT` with eight-byte
  little-endian lengths as the RFC spells it. `w` from the code per §2.4. The vector file
  `tests/fixtures/pake-vectors.json` — code, both scalars, both messages, `TT`, `Ke`,
  both MACs — generated as M15's is, and read by `pake.test.mjs`.
- `pair::Pairing` — the window: `open(ttl, now) -> Pairing` with `code`, `expires_at`,
  `attempts`, `consumed_by`; `attempt(source_ip, now)` enforces one per two seconds per
  source and five per code; on the fifth failure the code is destroyed and a fresh one
  issued for the remaining window (`§7.5`), announced to the settings page and the CLI as
  a new code. Every attempt, success or failure, and every expiry is a `sys_audit` row —
  `pair.opened`, `pair.attempt`, `pair.success`, `pair.failed`, `pair.expired` — with the
  source's address in `detail` and never the code. The first success closes the window.
- `Node::pair(&mut self, ttl: Duration) -> Result<Pairing>` (§3 row 13) and
  `Node::pairing(&self) -> Option<&Pairing>`; `Node::close_pairing(&mut self)`.
- The `/ws/pair` handshake, as data in `pair::handshake` and as I/O in `wire::channel`
  (M17 wires the socket; this milestone drives the messages from a test):

  ```json
  → {"v":1,"id":"k7m2q9xf","pub":"<node ed25519 b64>","open":true}
  ← {"v":1,"dev":"b3nn8t2q","pub":"<device ed25519 b64>","kind":"browser","pA":"<b64>"}
  → {"pB":"<b64>","cB":"<b64>"}
  ← {"cA":"<b64>"}
  → sealed(K_pair, s2c) {"x25519":"<b64>","cert":"<b64>","cluster_id":"q4w8rt2n","cluster_pub":"<b64>"}
  ← sealed(K_pair, c2s) {"x25519":"<b64>","label":"Pixel 9","ua":"Mozilla/5.0 …"}
  ```

  `open: false` closes with 4404; a wrong `cA` closes with 4401 after the audit and the
  attempt count; an exhausted code with 4429. `K_pair` is `Ke`; the two sealed messages
  use M15's frame with keys `HKDF-Expand(HKDF-Extract("pv/1 pair", Ke), "pv/1 c2s" |
  "pv/1 s2c", 32)`. On the client's sealed message the node writes the `sys_device` row —
  `kind`, `replica = false` for a browser, `ed25519_pub`, `x25519_pub`, `paired_at`,
  `paired_via = 'lan'`, `user_agent`, `label` — marks the code consumed, audits
  `pair.success`, closes the window. `kind = "node"` is refused with 4403 naming Phase 3.
  The device ID the client sends is checked against its public key by the same
  derivation `NodeId::derive` uses.
- `assets/shell/pair.js`: the client role — key generation (`ed25519`, `x25519` from the
  vendored modules), the device ID derivation, the PAKE, the two sealed messages, and
  storage under `pv:device` (§2.9). No UI yet; M19's page drives it.

**Produces:** `pair::{Code, Glyph, GLYPHS, WORDS, Pairing, PairingSnapshot, handshake::{...}}`;
`Node::{pair, pairing, close_pairing}`; `sys::KIND_PAIR_*`; `spec/pairing-words.txt`;
`assets/shell/pair.js` exporting `pair(socket, code, options)`.

**Tests** (`tests/pair.rs`): `test_spec_7_2_code_is_16_bits_rendered_as_four_glyphs_and_two_words`,
`test_spec_7_2_word_input_is_case_and_punctuation_insensitive`,
`test_spec_7_2_glyph_labels_are_accepted_as_input`,
`test_spec_7_3_glyph_table_is_normative_and_keeps_variation_selectors`,
`test_spec_7_2_word_list_has_256_distinct_words_with_unique_three_letter_prefixes`,
`test_spec_7_4_spake2_matches_the_checked_in_vectors`,
`test_spec_7_4_a_wrong_code_derives_nothing` (the MACs differ; no key comes out),
`test_spec_7_0_the_code_never_crosses_the_wire` (neither the code bytes, their glyphs,
their words nor `w` appear in any message of a full run),
`test_spec_7_4_pairing_completes_and_writes_the_device_row` (a Rust client through the
message types; the row's every column checked; `replica` false),
`test_spec_7_1_pairing_is_closed_until_opened_and_closes_on_first_success`,
`test_spec_7_5_code_expires_at_120s_and_five_attempts_issue_a_new_one` (a fake clock),
`test_spec_7_5_attempts_are_rate_limited_per_source`,
`test_spec_7_5_every_attempt_writes_an_audit_row_without_the_code`,
`test_spec_7_4_a_node_kind_is_refused_naming_phase_3`,
`test_spec_app_contract_6_pair_opens_a_window_and_returns_the_code`. Under `node --test`:
`pake.test.mjs` (the vectors; a full exchange against the Rust vectors' `B` side),
`client.test.mjs` gains the device ID derivation against a Rust vector.

**Documentation:** `protocol.md §7.2, §7.4.1, §7.4.2`, `data-dictionary.md §3.3`,
`app-contract.md §6`, `spec/pairing-words.txt` and `NOTICE` are written (rows 1, 3, 6, 13,
18) and are edited only where the code proves them wrong.

---

### M17 — The LAN bind, the auth policy, and the channel

- `crates/privatium/src/lib.rs`: `bind(port)` opens `0.0.0.0:<port>` and `[::]:<port>`
  (the second best-effort), `announce` prints the default route's URL and the rest under
  `--verbose` (§2.2, §3 row 14), and the adapter still inserts `Peer` and nothing else.
- `http::auth`: the policy of §2.1. `AuthLayer` learns three things a request may carry:
  a `Peer` (as now), a `Session` (inserted by the channel decoder — the device, its kind,
  when it last wrote `last_seen_at`), and the route class from `Router`. Loopback with a
  loopback `Host`: this node, every route. Non-loopback without a session: the bootstrap
  set; the answer for a page path is the bootstrap page, for anything else 403 with a
  sentence naming pairing. A session: `Device(session.device)`, every route, with the
  grant resolution of `§3.5` still landing on `*+*`. The embedder's layer refuses a
  peerless request as before.
- `wire::channel`: `/ws` — the upgrade answered from `handle` with axum's
  `WebSocketUpgrade` (the `OnUpgrade` extension hyper attaches; an in-process caller
  without one gets 426), M15's handshake on the socket, then the frame loop: a `req` frame
  becomes a `Request` with `Peer`, `Session` and `Host` from the upgrade request, is given
  to `Handler::handle` on its own task, and its `Response` goes back as `res`, `chunk`s
  and `end` — the body streamed as it is produced, which is what carries SSE. `cancel`
  from the client drops the task. A revoked device's session is closed by the handler that
  revoked it (M19) and is refused at the next handshake. `/ws/pair`: M16's messages on the
  socket, with the pairing window read and written under the node lock.
- Frame plaintext: `[u32 BE json_len][json][payload]`; `json` is `{"id","kind"}` plus, for
  `req`, `method`, `path`, `headers` and, for `res`, `status`, `headers`. A `req` carries
  its whole body in one frame, bounded by `api.max_body`; `chunk` for a request is
  reserved for Phase 3's uploads and refused here.
- The bootstrap page (`http::pairing::bootstrap`): a document with the node's title, the
  requested path in a `data-path` attribute, `<script type="module"
  src="/static/client.js">` with `integrity`, the `<noscript>` of §2.9, and no app data —
  `test_spec_9_2_unauthenticated_leaks_nothing` extends to it.
- `assets/shell/client.js`: on load, read `pv:device`; with none, the pairing screen
  (M19's UI, this milestone's plumbing); with one, open `/ws`, run M15's client handshake,
  verify the node's certificate against the pinned cluster key — a mismatch is `§8.1`'s
  full-screen refusal with the two fingerprints and no way past it but *forget this node
  and pair again*, which wipes `pv:device` — then fetch the requested path through the
  channel and put it in place of the bootstrap document. Scripts and stylesheets are
  re-created with `integrity` computed from the channel's copy (§2.1). Then the htmx
  extension: `htmx:beforeRequest` cancelled and the request sent through the channel,
  the response swapped with the internal `api.swap` exactly as htmx's `sse` extension
  does; and a document-level `click`/`submit` handler for what htmx does not own, with
  `pushState`. `pv.js` learns `window.__pv_channel`: `call` and the stream go through it
  when present, the stream parsed from `chunk` frames by the same SSE line parser.
- `Route::Ws`, `Route::WsPair` in `wire::router` with `/ws` in `FRAMEWORK_PREFIXES` (§3
  row 9); the auth layer's route class comes from there.
- The page frame carries `integrity` on the framework's own script and stylesheet tags
  (`shell.rs`, `assets.rs` computing SHA-256 once at startup) so that a page fetched over
  the channel is pinned end to end without the client's help for the framework's files.

**Produces:** `http::auth::Session`; `wire::channel::{serve_ws, serve_ws_pair, Frame,
Kind}`; `http::pairing::bootstrap`; `assets/shell/client.js` exporting `channel()`,
`fetchThroughChannel()`; `pv.js` unchanged in surface.

**Tests** (`tests/channel.rs`, through `handle` with an inserted `Peer(192.0.2.10:4000)` —
TEST-NET, never loopback): `test_spec_8_4_plain_http_on_the_lan_serves_only_the_bootstrap_set`
(every path of `HOST_ROUTES` from `tests/wire.rs`, the answer classified),
`test_spec_9_2_bootstrap_page_carries_no_app_data`,
`test_channel_requests_reach_every_route_of_handle` (the decoder fed frames, each route of
`§9.1` answered), `test_channel_streams_a_response_body_frame_by_frame` (`/api/stream`
through the decoder: an append arrives as a `chunk` before the response ends),
`test_channel_requests_carry_the_session_device` (`req.device`, `/api/node`'s `dev`),
`test_spec_8_1_a_revoked_device_is_refused_at_the_handshake`,
`test_loopback_keeps_phase_1_semantics` (every Phase 1 wire test still passes with a
loopback peer), `test_spec_8_3_page_frame_scripts_carry_integrity`,
`test_channel_refuses_a_request_chunk_naming_phase_3`. In `crates/privatium/tests/`:
`test_adapter_binds_every_interface` (replaces `test_binds_loopback_only`),
`test_spec_8_2_lan_socket_carries_no_plaintext_app_data` — a listener served through a
service that inserts a TEST-NET `Peer` before `handle`, a capturing TCP proxy in front of
it, a Rust client that pairs and then reads `/a/hello/` and `/a/hello/api/schema` through
the channel, and an assertion that the proxy's bytes contain neither the page's `<h1>`
text nor a column name — the roadmap's Wireshark bullet, automated;
`test_spec_10_4_browser_client_holds_exactly_one_endpoint` (`client.test.mjs`: every URL
the client builds is on its own origin). `client.test.mjs` also drives the extension
against the `pv.test.mjs` harness with a fake channel.

**Documentation:** `protocol.md §7.7, §8, §8.3, §8.4, §9.1, §13`, `cli.md §2`,
`data-api.md`, `docs/security.md §3, §4`, `docs/architecture.md §2.6` and the security and
Tier 2 skills are written (rows 2, 7, 8, 9, 14, 17) and are edited only where the code
proves them wrong; `docs/deployment.md §4` says the Windows prompt now happens;
`apps/sketch/README.md`.

---

### M18 — Discovery: mDNS and UDP, together

- `discover::txt`: the record of `§6.1` — `v`, `id`, `cl`, `nm`, `apps`, `build`, `pair`,
  `p` — built from a `Facts` struct, `apps` truncated with `,…` to keep the whole under
  1300 bytes; instance name `sys_node.display_name` or the Node ID, ≤ 63 bytes.
- `discover::mdns`: register `_privatium._tcp.local.` with the TXT and one sub type per
  mounted app with `nav.advertise = true` and a slug of ≤ 15 characters — the M5 warning
  for longer slugs stays the surfacing `§6.1` requires; re-register when the facts change
  (a `watch::Receiver<Facts>` the node updates); browse the same type and keep
  `Discovered { id, cl, name, addrs, port, apps, pair, seen_at }` keyed by `id`, never by
  name.
- `discover::udp`: a socket on `0.0.0.0:52525`; a probe of `PVDISCO1` plus four nonce
  bytes from a private, link-local or loopback source (§3 row 11) is answered by unicast
  with the same prefix, the nonce and the TXT keys as a JSON object; one answer per source
  per second; anything else dropped. `probe(timeout) -> Vec<Discovered>` for the node's
  own use in Phase 3.
- `Node::serve_discovery(&mut self) -> Result<()>` starts both, per `discovery.mdns` and
  `discovery.udp` from `sys_setting`, at once and never in sequence (`§6.5`); a mechanism
  the platform refuses — no multicast interface — is a line on standard error, not a
  failure of the other. Both stop when the node drops. `discovery.method` audit (info)
  names what started.
- `--no-discovery` starts neither and says so; `run.rs` calls `serve_discovery` after
  `load_apps` unless it is set. `Node::discovered() -> Vec<Discovered>` for the settings
  page and `--verbose`.
- The `pair` key flips through the facts channel the moment a window opens or closes, and
  `/api/v1/manifest` reports the same flag from the same source.

**Produces:** `discover::{Facts, Discovered, txt::record, mdns::{Advertiser, Browser},
udp::{Responder, probe}}`; `Node::{serve_discovery, discovered, discovery_facts}`.

**Tests** (`tests/discover.rs`): `test_spec_6_1_txt_record_carries_the_full_key_set_and_stays_under_1300_bytes`,
`test_spec_6_1_apps_is_truncated_with_an_ellipsis_when_over_budget`,
`test_spec_6_1_subtypes_only_for_advertised_slugs_of_15_chars_or_less`,
`test_spec_6_1_instance_name_is_the_display_name_or_the_node_id`,
`test_spec_6_4_udp_probe_is_answered_with_the_txt_key_set` (over loopback),
`test_spec_6_4_udp_refuses_a_public_source_and_answers_once_a_second` (the source check
as a pure function over addresses, the rate limit over a fake clock),
`test_spec_6_5_mdns_and_udp_start_together_and_stop_together`,
`test_spec_6_1_pair_flag_flips_when_pairing_opens`,
`test_spec_6_1_two_nodes_with_one_name_are_distinct_by_id` (two registrations carrying
the same instance name and different `id`s browse as two entries — the roadmap's
"distinguishable by ID, not name"),
`test_discovery_settings_disable_each_mechanism`,
`test_spec_6_1_mdns_registration_is_browsable_and_keyed_by_id` — a real daemon browsing
its own registration; runs on every platform, and if a CI runner proves to have no
multicast, it is gated by `PRIVATIUM_TEST_MDNS=1` in the same PR and stays in the manual
pass, with the CI log as the evidence (risk R10). In `crates/privatium/tests/cli.rs`:
`test_cli_no_discovery_starts_nothing`.

**Documentation:** `protocol.md §6.4` is written (row 11); `docs/deployment.md §4.1` (UDP
5353 and 52525 both named); `docs/connectivity.md §1` (the browser row's "owner types the
IP" becomes "scans the QR").

---

### M19 — The devices page, the pairing UI, `privatium pair`, and the documents

- Settings, node page: a display-name form (§3 row 16) → `sys_node.display_name`;
  "Listening" shows the LAN URL; "Nodes on this network" lists `Node::discovered()` by ID.
- Settings, devices page: every active device with kind, replica, label, paired, last
  seen; a label form and a **Revoke** button per device (`POST
  /settings/devices/<id>/revoke`, `csrf()`, a `put` with `revoked_at` and
  `revoked_reason`, never a `del` — `§3.2`; `device.revoked` audit; the device's open
  channel closed at once); **Open pairing** → `POST /settings/devices/pair` → the code
  page: the four glyphs with their labels beneath, the two words, the QR as inline SVG
  with the URL as text beside it, the seconds remaining, a *Close pairing* button, and
  the sentence `§7.7` asks for when the page is served over plain HTTP. `last_seen_at`
  written at the handshake and then at most hourly (§3 row 15).
- `POST /api/v1/pair` and `GET /api/v1/pair` (§3 row 5), this node's own device only.
- `privatium pair [--open] [--timeout 120]` (`crates/privatium/src/pair.rs`): POST to the
  running node on loopback, print the glyphs with labels, the words, the QR in Unicode
  blocks and the URL as text, poll `GET /api/v1/pair` every two seconds, exit 0 on success
  naming the device, 1 on expiry; `--open` opens the devices page. No node → a runtime
  error naming `privatium`. `--open` on a bare run per §2.8: on a node with no paired
  device the QR is printed with a code beneath it.
- The pairing screen in `client.js`: the emoji pad — sixteen buttons, glyph as text with
  its label beneath, `aria-label` the label, 44-pixel targets — and the word field, both
  visible at once, the node's name and ID above, the plain-HTTP sentence, and three
  outcomes said in a `role="status"` region: paired (then the requested path loads), wrong
  code with attempts left, closed or expired with what to do. A `<noscript>` never reaches
  here. Completable without reading (tap four glyphs) and without seeing (type two words,
  every control labelled), which `AGENTS.md` makes a requirement rather than a choice.
- `§8.1`'s refusal screen: full-page, both fingerprints, the node's name, no dismiss, one
  action — forget and pair again.
- `req.device`, `pv.device()` and `/api/node`'s `dev` are the session's device;
  `pv.node().peers` and `/api/node`'s `peers` count paired nodes (§2.11).
- `--version` claims `pv/1 (partial: phase 2)`; `protocol_claim()` in `main.rs` and the
  two CLI tests that read it.
- `xtask gen-skill-reference`: the sentence about the four methods now says two remain.

**Tests:** `test_settings_devices_lists_paired_devices_and_revokes_one`,
`test_spec_3_2_revocation_is_a_put_never_a_del`,
`test_spec_3_2_last_seen_at_is_written_at_most_hourly`,
`test_settings_node_display_name_is_set_by_the_owner_and_reaches_the_manifest`,
`test_spec_9_2_manifest_pair_flag_is_true_while_open`,
`test_spec_7_7_plain_http_pairing_page_discloses_the_gap`,
`test_spec_cli_5_pv4xx_pairing_and_devices_pages` (`tests/common/a11y.rs` over the code
page, the bootstrap page and the pairing screen's markup),
`test_spec_lua_3_4_device_and_peers_come_from_the_session`; in
`crates/privatium/tests/`: `test_spec_cli_8_pair_prints_the_code_and_exits_on_success`,
`test_spec_cli_8_pair_without_a_node_is_a_runtime_error`,
`test_spec_cli_2_open_prints_a_qr_and_the_lan_url`,
`test_spec_cli_2_open_on_an_unpaired_node_opens_one_window`,
`test_spec_8_1_a_reinitialized_node_is_refused_by_a_paired_client` (the Rust channel
client pairs, the node's `identity/` is deleted and the node restarted — a new node key
and a new cluster — and the client's handshake fails the certificate check with no path
past it; the roadmap's "changing the node key" bullet),
`test_spec_cli_1_version_qualifies_protocol` (now `phase 2`). Under `node --test`:
`client.test.mjs` — both renderings accepted, the pad and the field produce the same
sixteen bits, the refusal screen has no dismiss.

**Manual pass, recorded in the PR:** a phone on the LAN, from scan to first page under
twenty seconds; the word path with VoiceOver or TalkBack and images disabled; keyboard-only
through the devices page and the code page; 200 % zoom; Wireshark beside the automated
proxy test.

**Documentation:** `protocol.md §6.1, §7.1, §9.2`, `cli.md §2, §8`, `data-dictionary.md
§3.2`, `lua-api.md §3.4` and `data-api.md §4` are written (rows 5, 10, 12, 15, 16);
`lua-api.md §3.1`'s sentence on `device` drops its Phase 1 clause; `README.md` quick start
step 2 and the status paragraph; `docs/security.md §2.2`;
`skills/privatium-overview`, `-tier1-lua`, `-tier2-web`, `-tier3-rust`, `-security`,
`-accessibility` (the pairing screen's two paths, now real); `apps/*/README.md` where
they name Phase 2; `spec/protocol.md`'s status line.

---

### Hardening after M19

Phase 1 needed four rounds after its last milestone; expect at least one here. The review
reads every path a non-loopback peer can take through `auth.rs` and `channel.rs` against
OWASP ASVS 5.0 V2 and V9, the pairing state machine against `§7.5`'s three limits under
concurrency, and the client script against the CSP with the browser console open on each
of the three platforms' browsers plus a phone. Fix the spec in the same PR, as always.

---

## 7. Conformance mapping

Phase 2 can satisfy these lines of `protocol.md §13`, and `.github/scripts/conformance.sh`
gains a `run` per binary naming the tests:

| Checklist item (§13 wording) | Milestone | Test |
|---|---|---|
| Advertises `_privatium._tcp` with the full TXT key set (§6.1) | M18 | `test_spec_6_1_txt_record_carries_the_full_key_set_and_stays_under_1300_bytes`, `…_mdns_registration_is_browsable_and_keyed_by_id` |
| Runs all configured discovery mechanisms concurrently, not chained (§6.5) | M18 | `test_spec_6_5_mdns_and_udp_start_together_and_stop_together` |
| UDP fallback refuses non-private source addresses (§6.2) | M18 | `test_spec_6_4_udp_refuses_a_public_source_and_answers_once_a_second` |
| Pairing requires explicit owner action (§7.1) | M16 | `test_spec_7_1_pairing_is_closed_until_opened_and_closes_on_first_success` |
| Pairing code is node-generated, 16-bit, 120s TTL, 5 attempts (§7.2, §7.5) | M16 | `test_spec_7_2_code_is_16_bits_…`, `test_spec_7_5_code_expires_at_120s_…` |
| Both emoji and word encodings accepted (§7.2) | M16 | `test_spec_7_2_word_input_is_case_and_punctuation_insensitive`, `…_glyph_labels_are_accepted_as_input` |
| Variation selectors preserved on glyphs 8 and 9 (§7.3) | M16 | `test_spec_7_3_glyph_table_is_normative_and_keeps_variation_selectors` |
| The pairing code is never transmitted as a bearer credential (§7.0) | M16 | `test_spec_7_0_the_code_never_crosses_the_wire` |
| Pairing state persists per origin; loss requires re-pairing with no bypass (§7.6) | M17 | `client.test.mjs` |
| Plain-HTTP pairing screens disclose the property-1 gap (§7.7) | M19 | `test_spec_7_7_plain_http_pairing_page_discloses_the_gap` |
| Pinned key mismatch has no override path (§8.1) | M17, M19 | `test_spec_8_1_a_static_key_that_is_not_the_pinned_one_fails_the_confirm`, `test_spec_8_1_a_reinitialized_node_is_refused_by_a_paired_client`, `client.test.mjs` |
| Session layer never skipped on plain HTTP (§8.2) | M17 | `test_spec_8_2_lan_socket_carries_no_plaintext_app_data` |
| Plain HTTP from a non-loopback peer serves only the bootstrap set (§8.4) | M17 | `test_spec_8_4_plain_http_on_the_lan_serves_only_the_bootstrap_set` |
| Framework-named scripts and stylesheets carry integrity through the channel (§8.3) | M17 | `test_spec_8_3_page_frame_scripts_carry_integrity` |
| Cluster private key never leaves nodes; devices receive the public key only (§2.3.3) | M14, M16 | `test_spec_2_3_3_cluster_private_key_is_absent_…`, `test_spec_7_4_pairing_completes_and_writes_the_device_row` |
| Node certificates expire at 180 days (§2.3.1) — the expiry half | M14 | `test_spec_2_3_1_certificate_verifies_…_expires_at_180_days` |
| Cluster-key mismatch has no override path (§2.3.2, §8.1) | M19 | `client.test.mjs` |
| Browser clients hold exactly one endpoint (§10.4, §10.8) | M17 | `test_spec_10_4_browser_client_holds_exactly_one_endpoint` |
| `sys_device.replica` declared accurately (§10.7) — browsers | M16 | `test_spec_7_4_pairing_completes_and_writes_the_device_row` |
| No SAS confirmation step exists (§7.8) | M19 | Review item: the pairing screen and `pair.js` carry no comparison step; `AGENTS.md` already forbids adding one |

Phase 2 **cannot** claim: renewal on sync (§2.3.1), an unmet node trusted by a pinned
device (§2.3.2), discovery filtered by `cl` once paired (§6.1 — a browser cannot browse,
and the node's own filter needs a second node), and everything of §10 and §6.2–§6.3. Those
are Phase 3 and Phase 5.

---

## 8. Risks

**R9 — The PAKE.** Two implementations of RFC 9382 written here, in two languages, with no
third to check against. Mitigations: the vector file both sides read; a test that a wrong
code yields no key on either side; the RFC's security considerations read against the
transcript before M16 opens; and §2.4 flagged for a second pair of eyes. If a JavaScript
SPAKE2 or CPace with an audit appears before M16 starts, prefer it and re-decide.

**R10 — Multicast on CI.** GitHub's runners may drop mDNS. M18's real-daemon test runs
everywhere first; the CI log decides whether it is gated, and the gate is an environment
variable named in the test, not a silent skip.

**R11 — htmx's internal API.** The extension leans on `api.swap` and its neighbours, which
htmx documents for extension authors but does not promise as stable. Pin htmx at 2.0.9 as
`VENDOR.md` does; the extension is a hundred lines, and a change in htmx is a day, not a
redesign.

**R12 — Twenty seconds on a phone.** Scan, load a page with a hundred kilobytes of crypto
modules, tap four glyphs. The budget is real on a slow phone on a busy network. Measure
in M19's manual pass; if it fails, the first lever is caching `/static/*` with a long
`max-age` and `immutable` (the assets carry integrity and are versioned by build), not a
minifier.

**R13 — Windows network profiles.** A home Wi-Fi classified *Public* blocks the node after
the owner clicked Allow (`docs/deployment.md §4.1`). Phase 2 documents it in the quick
start and on the node page; the helper is Phase 6.

**R14 — Browser storage eviction.** iOS Safari clears storage a site has not used for a
week (`§10.7`). The device re-pairs; the plan says so on the pairing screen rather than
pretending otherwise.

**R15 — Rate limits and the lock.** `§7.5`'s per-source limit and the attempt counter are
touched from WebSocket tasks; they live in the node behind its mutex, taken for
microseconds, never across an await — the discipline `wire/mod.rs` already states.

**R16 — The Wireshark bullet is a manual claim.** The proxy test proves the socket carries
no known plaintext; a person with Wireshark still looks, in M19's manual pass, because a
test only finds the strings it was told to look for.

---

## 9. PR sequence

| # | Branch | Depends on | Spec edits |
|---|---|---|---|
| 16 | `m14-cluster-identity` | Phase 1 | as found — §3 is written |
| 17 | `m15-session` | M14 | as found |
| 18 | `m16-pairing` | M15 | as found |
| 19 | `m17-channel-lan` | M16 | as found |
| 20 | `m18-discovery` | M17 | as found |
| 21 | `m19-devices-shell` | M18 | as found; roadmap: tick Phase 2 |
| 22 | `phase2-hardening` | M19 | as found |

---

Copyright © 2026 Gabriel Mongefranco
