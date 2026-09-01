# AGENTS.md — Privatium™

Guidance for AI coding agents working in this repository.

## What this repository is

A specification-first project. `spec/` is normative; `docs/` is explanatory; `apps/` holds
example applications that double as the reference templates. As of this writing there is
no implementation.

**If you are asked to write code, read `spec/protocol.md` and `spec/app-contract.md`
first, in full.** They are the contract. Deviating from them silently is the single worst
thing you can do in this repository. If the spec is wrong, say so and propose an edit to
the spec in the same change — do not implement around it.

## Non-negotiable invariants

Violating any of these is a bug, regardless of how well the code works:

1. **Append-only single-writer logs are the source of truth.** DuckDB files, Parquet
   snapshots, and CSV exports are all caches. Deleting every one of them must lose zero data.
   **Plain-text JSONL is a strong default, not a law** — sealed historical segments may be
   compressed or stored as Parquet. **The live tail is always plain JSONL**, uncompressed,
   appendable by `echo`. That property is what the Phase 1 acceptance test protects; do not
   erode it into "we compress everything".
2. **One writer per log file, forever.** A device appends only to its own
   `log/<device-id>.jsonl`. Never write another device's file, not even during a merge.
3. **Append-only.** No line in a log file is ever modified or removed. Corrections are new
   events. Deletions are tombstones.
4. **Unknown fields are preserved.** A node that reads an event with fields it does not
   understand MUST retain them verbatim on replay and re-emission. This is how forward
   compatibility works.
5. **No secret ever enters a log file.** Keys, pairing codes, and tokens live in the OS
   keyring or `identity/`, never in `data/`.
6. **App SQL runs sandboxed.** The app-facing DuckDB connection has
   `enable_external_access=false` and `lock_configuration=true`. Only the framework's
   privileged connection touches the filesystem.
7. **XDG paths only.** Never write beside the binary, never assume a writable install
   directory, never require `--filesystem=host`. Flatpak compatibility is a hard
   requirement from day one, not a later port.
8. **Ports ≥ 1024 only.** No `CAP_NET_BIND_SERVICE`. ACME is DNS-01 only. The node runs as an
   ordinary user; elevation is only ever an optional firewall helper the owner can decline.
9. **No node is primary.** Every node is a peer; an always-on node is a peer that happens to
   be reachable. If you find yourself writing a "server" role, an election, or a
   authoritative-copy check, stop.
10. **The cluster private key never leaves a node.** Phones, tablets, and browsers receive
   the public key only.
11. **No outbox dedupe table.** ULIDs make replay idempotent. Adding transaction IDs or an
   acknowledgement protocol means the merge rule was misread.

## Language and stack

- **Core:** Rust. One workspace, one core crate (`privatium-core`) usable from the server,
  the Tauri shells, and via `uniffi` from Swift/Kotlin.
- **Tier 1 apps:** Lua 5.4 via `mlua`. Not LuaJIT (iOS forbids JIT), not Luau (a dialect
  fragments both documentation and LLM assistance). Templates are LSP (`<? ?>`), compiled
  to cached Lua chunks and invalidated on mtime.
- **Query engine:** DuckDB (bundled, extensions statically linked, autoload disabled).
- **Transport:** `iroh` for node-to-node, `axum` (or equivalent `hyper` stack) for HTTP.
- **Onion:** `arti-client` with the `onion-service-service` and `rustls` features. Do not
  enable the `static` feature; it pulls in native-tls.
- **Browser crypto:** `@noble/curves`, `@noble/ciphers`, `@noble/hashes`. `crypto.subtle`
  is unavailable on plain-HTTP origins — do not reach for it.
- **Framework UI:** server-rendered HTML + HTMX. **No React, Vue, Svelte, Angular, or any
  other client-side framework in the framework itself.** No bundler, no transpiler, no
  `node_modules` in the runtime path. This constraint applies to `privatium-core`, the
  shell, and Tier 1 LSP templates — **not** to what an app author puts in their own `web/`
  directory, which is entirely their choice.
- **Icons:** Bootstrap Icons only, vendored as raw SVGs and inlined server-side. Never the
  web font, never a CDN, never a second icon set. See `docs/icons.md`.

## Style

- Every source file carries the standard header block (see below).
- Errors: `thiserror` for library crates, `anyhow` at binary boundaries. Never `unwrap()`
  outside tests or `main()` startup.
- Tests: every normative MUST in `spec/protocol.md` should map to a named test. Use the
  spec's section number in the test name (e.g. `test_spec_4_3_lamport_monotonic`).
- No `unsafe` without a comment naming the invariant it upholds.

### The header block

Six fields — project, the file's own path, authors, created, modified, summary — in a
comment at the top of the file. Two renderings are in use and both are correct: the
spread-out form used throughout `spec/` and `docs/`, and the compact form used in
`apps/`, which pairs `Project:` with `File:` and `Created:` with `Modified:`.

```rust
// Project:  Privatium™  |  File: crates/privatium-core/src/lib.rs
// Authors:  Gabriel Mongefranco (@gabrielmongefranco)
// Created:  2026-08-31  |  Modified: 2026-08-31
// Summary:  What this file is for, in a sentence or three.
```

`.lsp` templates carry a reduced form: project, path, and summary. Authorship on every
partial of an app nobody reads separately is noise.

`cargo xtask header-check` enforces this over `.rs`, `.lua`, `.sql`, `.js`, `.css`,
`.lsp`, and `.md` under `spec/` and `docs/`. Markdown elsewhere — this file, `README.md`,
every `apps/**` README and `SKILL.md`, everything under `skills/` — is prose rather than
source and is exempt by design. So is anything vendored, which is marked by a `VENDOR.md`
beside it or above it and carries its own provenance.

The dates are checked for shape, not for accuracy. A mechanical `Modified:` check would
either be wrong or fight every commit that touches the file.

## Things agents get wrong here

- **Do not add a session/SAS confirmation screen to pairing.** The PAKE authenticates.
  A short authentication string is redundant and was deliberately removed.
- **Do not make the browser client try LAN and remote endpoints in sequence.** An HTTPS
  origin cannot fetch `http://192.168.x.x` — mixed content blocks it. One origin per browser
  client. Multi-endpoint failover is a native-client capability.
- **Do not push app logic into SQL views** on the grounds that views are portable to a mobile
  replica. That is true and it is not a reason to contort an app. Authors put logic wherever
  suits them.
- **Do not add a `doctor` or diagnostics subcommand** unless asked. Detect and explain
  failures where they occur instead.
- **Do not reintroduce a declarative app format.** `forms.toml` was removed deliberately: it
  had an expressiveness ceiling and imposed an application model. The scaffold generator
  emits Lua source you edit; it has no runtime presence. If you find yourself adding a
  config key that describes a UI, stop.
- **Do not make `schema.sql` mandatory.** Tiers 1 and 2 both work with the event log as a
  document store.
- **Do not vendor Barracuda/BAS** without reading `docs/decisions/0001` and `0004`. It is
  GPLv2-only (incompatible with this project's GPLv3), its GPL clarification extends to web
  content hosted by the server, and — independently of licensing — it owns the event loop,
  so there is no configuration where it coexists with iroh, tokio, and DuckDB cheaply.
- **Do not add `'unsafe-eval'` or `'unsafe-inline'` to the default CSP**, and do not set
  `eval`/`inline_script` in a reference app's `app.toml` to make a library's shorter syntax
  work. Apps share the framework's origin and session, so CSP is *not* an inter-app boundary
  today — the honest justifications are defense in depth around `<?raw ?>`, no-CDN
  discipline, `remote = []`, and keeping the door open for the per-app origins that
  `docs/security.md §7` eventually needs. Relaxing it is one-way: once app authors and the
  models reading `skills/` write inline expressions, the permission can never be withdrawn.
  Use `@alpinejs/csp` rather than the `eval` permission; `apps/animals` is the worked example.
  Inline event handler attributes (`onclick`, `onsubmit`) are script too, and fail silently.
- **Do not adopt a JavaScript sync core.** Gun, RxDB, and their relatives are fine *above*
  the data API and disqualifying *below* it: a Rust core reaches LÖVE, Godot, Unity, Bevy,
  Swift, and Kotlin through a C ABI with no server at all. See `docs/decisions/0004 §6`.
- **Do not add routes in an adapter.** `core::handle(Request) -> Response` is the single
  entry point (ADR 0003). If a platform needs behaviour the others lack, it goes in the core
  behind a capability flag. Adapters do not rewrite paths either — `pv.url()` is the only
  URL construction point.
- **Do not model request or response bodies as `Vec<u8>`.** Both directions stream. SSE
  needs it on the way out; uploads need it on the way in.
- **Do not treat the browser's offline limits as a rendering problem.** They are a secure
  context problem: a LAN IP cannot register a service worker at all. See
  `docs/architecture.md §2.5` and ADR 0003.
- **Do not make a phone a discovery target by default.** Mobile resolves; it does not
  publish (ADR 0005). Foreground-only reachability plus multi-hour record lifetimes means a
  publishing phone advertises a stale address.
- **Do not impose the framework's UI decisions on apps.** The framework ships no client
  framework and uses HTMX; a Tier 2 app may vendor React, Three.js, or anything else in its
  own `web/` directory. The no-framework rule governs `privatium-core` and the shell, not
  app folders.
- **Do not put Tier 1 application logic in the browser.** Tier 1 renders server-side. This
  says nothing about Tier 2, which owns its browser code entirely.
- **Do not weaken LSP escaping.** `<?= ?>` escapes, always, with no configuration flag.
  `<?raw ?>` is the documented exception and every use is linted.
- **Do not add a second icon set** to the *framework*, or hand-draw an SVG because Bootstrap
  Icons lacks the perfect glyph. Apps may ship their own graphics; the shell may not.
- **Do not introduce a server-side mutable database as truth.** If you find yourself
  writing an `UPDATE`, stop; the answer is an append.
- **Do not sync `local/`.** Pairing state, sync cursors, and cached peer addresses are
  node-local by design.

## Skills

`skills/` contains instruction sets for AI assistants building apps on this framework.
**A change to `spec/` that is not reflected in `skills/` is an incomplete change.** The
reference sections are generated from the crate and the spec, and CI fails on drift.

Every skill ends with `privatium lint`. The linter is part of the framework, not advice —
rules are specified with stable IDs in `spec/cli.md §5`, and `docs/skills.md §4` explains why
it exists. A rule that cannot cite the spec section it enforces does not belong in it.

## Security expectations

- Do not commit secrets, real pairing codes, key material, or personal data exports.
- Use synthetic examples in documentation and tests.
- Do not claim regulatory compliance from code behaviour alone. Privatium stores data in
  plain text by design; anyone applying it to regulated data owns that analysis.
- Always verify agent output and test it before opening a pull request.

## Accessibility target

- WCAG 2.1 AA or WCAG 2.2 AA.
- Keyboard-operable controls; visible focus states; labels on all form fields.
- Status updates must not rely on color alone.
- The pairing flow must be completable without reading text (emoji pad) **and** without
  seeing images (word code + screen reader). Both paths are required, not alternatives.

---

Copyright © 2026 Gabriel Mongefranco