<!--
Project:  Privatium™
File:     docs/decisions/0001-barracuda-evaluation.md
Authors:  Gabriel Mongefranco (@gabrielmongefranco)
Created:  2026-08-28
Modified: 2026-08-31
Summary:  Decision record. Barracuda App Server / Mako Server evaluated as a
          foundation and declined. Status: DECIDED — Rust.
-->

# ADR 0001 — Barracuda App Server as a foundation

**Status: DECIDED — declined. The core is Rust.**

Licensing turned out not to be the deciding factor. The record below is kept in full,
including a correction to an error in an earlier draft, because the architectural findings
remain relevant and because someone will ask this question again.

## Context

Mako Server and the Barracuda App Server (BAS) motivated the Lua direction in the first
place. Lua Server Pages is the developer experience this project wants: edit a file, hit
refresh, see the change. BAS also ships SQLite, TLS, WebSockets, and a mature LSP
implementation in one C library.

The question is whether to depend on it or reimplement the part we want.

## Established

Real Time Logic's own repository and license page state:

- **BAS and BWS are offered under three options:** GPLv2, a free commercial license for
  small companies, and a standard royalty-free commercial license.
- The GitHub repository is labelled **GPL-2.0**, not GPLv2-or-later.
- **The free startup license requires company revenue under $1 million USD.** An individual
  publishing a personal project plausibly qualifies, and noncommercial use needs no
  application at all.
- **No application is required for noncommercial use or evaluation.**
- A GPLv2 clarification in the repository states that the license applies to all BAS and
  BWS APIs including the Lua APIs, and that **web content hosted by a BAS or BWS derivative
  product is considered part of the application as a whole.**
- SharkSSL, included, is classified **ECCN 5D002.C.1**, distributed under the ENC/TSU
  exception.
- BAS documentation states Lua runs as trusted application logic and is **not sandboxed by
  default**, with full Lua-to-C binding access.

An earlier draft of this project's documentation claimed BAS was non-commercial-only. That
was **wrong** — it came from `nzinfo/MakoServer`, a 2015 third-party fork carrying
superseded terms. The error is recorded here so it is not repeated.

## Disputed / to verify

| Claim | Status |
|---|---|
| An educational-institution carve-out exists in the commercial license | **Not found** on the published license page. Needs written confirmation from RTL. |
| Mako Server is now MIT licensed | **Not confirmed.** Mako's own Lua code may be MIT, but the binary links BAS. Needs clarification on the compiled artefact. |
| Which Lua version BAS currently embeds | Unverified. This project targets 5.4. |

## Blockers, if GPLv2 is the chosen lane

**1. GPLv2-only is incompatible with GPLv3.** This project is GPL-3.0-or-later. Depending on
BAS requires relicensing the whole project to GPLv2, which forfeits GPLv3's patent grant and
anti-tivoization clause, and makes Apache-2.0-only dependencies incompatible. Most Rust
crates are MIT-OR-Apache, so the MIT branch survives, but not all.

**2. The web-content clause is the decisive one.** Under it, every app folder anyone
publishes becomes GPLv2 **including its HTML, CSS, and JavaScript**. That contradicts the
core commitment recorded in `spec/app-contract.md`: the framework does not impose an
application model, and a Tier 2 author picks their own libraries. Vendoring an
Apache-2.0-only JavaScript library would become impossible.

Note that GPL obligations attach at distribution. A private, undistributed personal app
never triggers any of this. The conflict is specific to publishing a framework for others.

## Architectural objections, independent of license

These would stand at zero license cost:

- **Duplicated concerns.** BAS brings its own HTTP server, TLS stack, socket layer, VFS,
  session model, Lua VM, and SQLite binding. Privatium needs axum, rustls, iroh QUIC, an
  event-log-backed VFS, a PAKE session model, mlua, and DuckDB. Every one overlaps. This is
  not adding LSP; it is arbitrating two application servers across an FFI boundary.
- **SQLite, not DuckDB.** BAS gives SQLite. DuckDB was chosen for real `DECIMAL`, `DATE`,
  `INTERVAL`, and `TIMESTAMPTZ`, and for reading JSONL in place. Adopting BAS reintroduces
  the type problems that started this design — see §3 below for the precise version.
- **Sandbox posture.** BAS treats Lua as trusted manufacturer logic. Privatium treats an app
  folder as a semi-trusted download. The sandbox would have to be built regardless.
- **Export control.** SharkSSL is classified ECCN 5D002.C.1 and distributed under the
  ENC/TSU exception. Redistributing classified crypto carries notification obligations that
  `rustls` does not.
- **LSP is the smallest piece.** A template engine that scans `<? ?>`, compiles to a cached
  Lua chunk, and invalidates on mtime is roughly 300 lines — and gives us hot reload and
  escaping-by-default, neither of which the original has.

## Decision

**The core is Rust. LSP is reimplemented (`spec/lua-api.md §1.1`, §4). BAS is not a
dependency.**

Licensing was not decisive. The published terms — GPLv2, a free license for companies under
$1M revenue, and no application required for noncommercial use — are workable for a personal
project. The earlier claim of non-commercial-only terms was **wrong** and is corrected above.

What decided it was capability, in three findings that emerged from working the problem
through rather than from reading licenses.

### 1. BAS is server-client by construction

It targets desktop HLOS and RTOS. It does not target iOS or Android. That is not a gap to
be worked around; it is what the product is for.

The consequence is not merely "no native mobile app" — a Swift or Kotlin client could still
speak HTTP to a BAS node. The consequence is **no shared core**. Under Rust, one
`privatium-core` crate serves the desktop, both mobile shells (via `uniffi`), and any
embedded use, with one implementation of the event log, replay, sync, discovery, and peer
transport. Under BAS every one of those would be reimplemented in Swift and again in Kotlin,
or omitted — which makes every phone a thin client permanently, on both platforms.

### 2. Hole punching is not writable in Lua

pkarr discovery is (roughly 1000–1500 lines: bencode, a Kademlia routing table, BEP44 over
non-blocking UDP), and that alone yields account-free remote access **for anyone willing to
forward a port**. That is a better outcome than an earlier draft of this record credited.

But magicsock-class traversal — STUN, simultaneous open, path discovery, endpoint migration,
QUIC — is months of work whose failure mode is not "broken" but "40% success instead of 90%,"
discovered slowly and in the field. Removing the router-configuration step is precisely what
hole punching exists for, and for a non-technical audience that step is the difference
between working and not.

### 3. SQLite reintroduces the original problem

BAS supplies SQLite. DuckDB was chosen for native `DECIMAL`, `DATE`, `INTERVAL`, and
`TIMESTAMPTZ`, and for querying JSONL in place.

**A correction to an earlier draft of this document.** It previously said adopting SQLite
would mean "returning to integer cents". That overstated the case and is withdrawn. SQLite
accepts a `DECIMAL(10,2)` column declaration and it works; nobody has to encode cents as
integers. What SQLite has is not a decimal *type* but **type affinity** — a column declared
`DECIMAL(10,2)` gets NUMERIC affinity, and a value like `12.34` is stored as an 8-byte
IEEE-754 float. So it is ergonomically fine and quietly binary: sums across many rows
accumulate the usual binary-fraction error, which is invisible on a single pharmacy price
and produces an unexplainable penny on a year-end total. There is a `decimal` extension in
SQLite's `ext/misc` that does arbitrary-precision decimal over text, but it is not compiled
in by default.

**Dates carry the argument.** SQLite has no date or time type at all — dates are TEXT,
REAL, or INTEGER by convention, and every comparison, interval, and timezone conversion is
the application's problem. The first application is a medication and insurance-cost tracker
whose central questions are all temporal: fill intervals, prior-authorisation expiry, days
of supply remaining. DuckDB answers those in SQL. That is the reason, and decimals are a
supporting note rather than the case.

### On SharkTrustX

Evaluated and does not change the answer. Its zones take the form
`product-name.company.com`, so a domain is registered either way. What it does is move
registration from each owner to the project, which turns the project into a DNS operator
with uptime obligations and visibility into every node's address. That is a legitimate model
for an OEM; it is the opposite of what this project is for.

pkarr on the mainline DHT provides what SharkTrustX cannot at any configuration: nobody
registers anything and nobody operates anything.

## What was kept from BAS

The evaluation was not wasted. Three things were adopted:

- **Lua Server Pages, as a concept.** `spec/lua-api.md §4` reimplements it, with escaping by
  default and mtime-based hot reload — neither of which the original has.
- **Lua as the Tier 1 language**, and the reasoning for it.
- **The developer-experience standard**: edit a file, refresh, see the change. No build step,
  no restart. Recorded as a decision in `docs/architecture.md §2.4` rather than an aspiration.

Real Time Logic's AI-assisted development work — an MCP server exposing a controlled Lua/LSP
lab, `AGENTS.md`, and per-topic skills folders — also directly informed `docs/skills.md`.

## Still recommended

**Prototype the medication tracker in Mako Server Developer Edition.** Under the
noncommercial lane there is no licensing question, and it puts LSP in the owner's hands in
hours rather than after a template engine is specified. Evidence about the app model before
it is frozen is worth more than the time it costs.

Declining BAS as a dependency is not a reason to decline it as a teacher.

## Recommended regardless of outcome

**Use Mako Server Developer Edition as a prototype vehicle.** Building the medication
tracker in Mako now — under the noncommercial lane, with no license question — would put
LSP in the owner's hands in hours rather than after a template engine is specified, and
would produce evidence about the app model before it is frozen.

Real Time Logic has also already built the pattern this project calls for in
`docs/skills.md`: LSP-Claw is an MCP server giving an AI agent a controlled Lua/LSP lab with
runtime trace access, FuguHub exposes development skills over MCP, and the BAS repository
carries `AGENTS.md` and per-topic `skills/` folders. Worth studying before finalising ours.

## The question underneath

If the sync, pairing, and plain-text backup layer is not the point, Mako Server is a better
answer than Privatium and this project should stop. Mako gives no JSONL-as-truth (its SQLite
file is precisely the binary format that naive file sync corrupts — the founding
constraint), no device pairing, no mDNS or iroh sync, and no three-tier restore.

Privatium is not an app server that happens to sync. It is a sync-and-backup layer that
happens to serve apps. If that holds, LSP is a template syntax worth 300 lines, not a C
application server worth a relicense.

## Resolved questions

- **Lua version:** BAS embeds Lua 5.4. Confirmed by the owner. Matches this project's target,
  and was not a differentiator.
- **Licensing:** workable. Not the deciding factor.

## Would reopen if

The project's scope narrowed to desktop-and-browser only, permanently — no native mobile
clients, no shared core, no peer transport. At that point BAS would be the stronger choice
and this decision should be revisited rather than defended.

---

Copyright © 2026 Gabriel Mongefranco
