<!--
Project:  Privatium™
File:     docs/plans/phase-5.md
Authors:  Gabriel Mongefranco (@gabrielmongefranco)
Created:  2026-09-05
Modified: 2026-09-05
Summary:  Stub. The Phase 5 plan — reaching home from outside — is written from
          docs/roadmap.md when Phase 4 closes, in the shape of docs/plans/phase-1.md.
          Non-normative.
-->

# Phase 5 Implementation Plan — stub

**Not yet written.** Write it from `docs/roadmap.md` Phase 5 — *reaching home from
outside*, in its three parts 5a, 5b and 5c — when Phase 4 closes, in the shape of
`docs/plans/phase-1.md`: scope, decisions to confirm, spec gaps, layout, dependencies,
milestones with named tests, the `spec/protocol.md §13` items claimed, risks, PR
sequence. Milestone numbers continue from Phase 4's last.

Deliverable, from the roadmap: the app works on cell data with no account, no domain and
no payment, and installs as a PWA for those who want one. 5a is pkarr discovery alone,
useful by itself; 5b the direct peer transport with a relay fallback; 5c the routes
retained — mesh VPN, DuckDNS with a DNS-01 certificate, the PWA, the onion service, and
Cloudflare Tunnel documented and not implemented.

What the earlier plans leave for this phase to pick up:

- The last unclaimed lines of `§13`: the pkarr lines of `§6.2`–`§6.3`, which Phase 3's
  conformance mapping names as this phase's.
- The Phase 2 channel's session layer MAY be skipped on a transport `§8.2` exempts — a
  CA-issued certificate, a mesh VPN, an onion service; this phase decides the auth layer's
  behaviour on an HTTPS origin, where a session cookie rather than a WebSocket may carry
  the device.
- The residual gap of `docs/plans/phase-2.md §2.10` closes on an HTTPS origin; an import
  map with integrity is the browser feature to re-check here.
- `docs/security.md §3b` on what iroh publishes by default — the relay address only — and
  `include_direct_addresses` as the explicit switch the account-free path needs.
- Attachments over cellular: `api.max_blob` may want a lower default per transport
  (`docs/plans/phase-3.md`, R20).
- The PWA client replica of `docs/roadmap.md`'s open questions: the `(dev, lam)`
  watermark plus an outbox, roughly three hundred lines, never a sync library (ADR 0004).

---

Copyright © 2026 Gabriel Mongefranco
