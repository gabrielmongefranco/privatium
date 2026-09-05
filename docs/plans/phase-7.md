<!--
Project:  Privatium™
File:     docs/plans/phase-7.md
Authors:  Gabriel Mongefranco (@gabrielmongefranco)
Created:  2026-09-05
Modified: 2026-09-05
Summary:  Stub. The Phase 7 plan — the first real app — is written from docs/roadmap.md
          when Phase 6 closes, in the shape of docs/plans/phase-1.md. Non-normative.
-->

# Phase 7 Implementation Plan — stub

**Not yet written.** Write it from `docs/roadmap.md` Phase 7 — *the first real app* —
when Phase 6 closes. The app lives in its own repository as an app folder; what this plan
covers is the framework side: the decisions to confirm, the spec gaps the app exposes,
and the milestones that fix the framework rather than the app. The roadmap's rule
governs it: if the app needs a framework change to work, the framework was wrong and the
change belongs in `pv/1` before the app ships.

Deliverable, from the roadmap: the medication fill and prior-authorization tracker.

What the earlier plans leave for this phase to pick up:

- Attachments (Phase 3, M25) exist for this app: a photo of a prescription, a PDF from the
  pharmacy. Whether the type whitelist, the size default and the scaffold's control fit a
  real form is decided here, against real use.
- Issue #21's second half — framework-provided Lua modules behind the `require` whitelist
  of `spec/lua-api.md §5` — waits for this app to show which module it actually lacks; the
  vendoring convention of `docs/frameworks.md §3` is the answer until then.
- `docs/frameworks.md §7`'s rule: a library earns a row by being used in a real app. This
  is the app.
- The open questions of `docs/roadmap.md` — Tier 1 rendering offline, SQLite in the
  browser — are re-read with this app's screens in front of you, and stay unscheduled
  unless one of them is what the app needs.

---

Copyright © 2026 Gabriel Mongefranco
