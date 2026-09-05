<!--
Project:  Privatium™
File:     docs/plans/phase-4.md
Authors:  Gabriel Mongefranco (@gabrielmongefranco)
Created:  2026-09-05
Modified: 2026-09-05
Summary:  Stub. The Phase 4 plan — native shells — is written from docs/roadmap.md when
          Phase 3 closes, in the shape of docs/plans/phase-1.md. Non-normative.
-->

# Phase 4 Implementation Plan — stub

**Not yet written.** Write it from `docs/roadmap.md` Phase 4 — *native shells* — when
Phase 3 closes, in the shape of `docs/plans/phase-1.md`: scope in and out with the
one-sentence test, decisions to confirm before the first milestone, spec gaps found and
where each is fixed, the workspace layout it adds, dependencies with versions and
licences, milestones with named tests, the `spec/protocol.md §13` items it can claim,
risks, and the PR sequence. Milestone numbers continue from Phase 3's last.

Deliverable, from the roadmap: an installable desktop app, and Android and iOS apps.
Scope: Tauri v2 desktop, Tauri mobile, `uniffi` bindings, `privatium-ffi` (the C ABI),
offline read plus a write outbox, and the `lantern` reference app (Tier 3, LÖVE) with its
paired Tier 1 app.

What the earlier plans leave for this phase to pick up:

- The shells are adapters over `core::handle` (ADR 0003) and add no routing; the
  custom-scheme *streaming* spike, and the long-poll fallback of `spec/data-api.md §3` if
  WKWebView needs it, are this phase's open risk.
- The residual program-authenticity gap of `docs/plans/phase-2.md §2.10` — a Tier 2
  module's own `import` graph on a plain-HTTP origin — closes in the native shell, where
  the core is in-process and nothing crosses a wire.
- Endpoint re-attempt on network change (`spec/protocol.md §10.4`, the `§13` half Phase 3
  could not claim) is a native-client behaviour and lands here, with the multi-endpoint
  list browsers cannot hold.
- `sys_device.replica` and reachability reported separately for a phone (ADR 0005);
  mobile resolves discovery and does not publish by default.
- Issue #22's question — other languages — is answered here by `privatium-ffi`: a Python
  or Node example beside `lantern` is a quarter of a milestone, and no new tier.

---

Copyright © 2026 Gabriel Mongefranco
