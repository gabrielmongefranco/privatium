<!--
This file is part of Privatium
docs/plans/phase-6.md
Author(s): Gabriel Mongefranco
Created: 2026-09-05
Last Modified: 2026-09-05
Summary: Stub. The Phase 6 plan — packaging — is written from docs/roadmap.md when Phase 5 closes, in
         the shape of docs/plans/phase-1.md. Non-normative.
Notes: See README file for documentation and full license information.

Copyright © 2026 Gabriel Mongefranco

Permission is granted to copy, distribute and/or modify this document
under the terms of the GNU Free Documentation License, Version 1.3 or
any later version published by the Free Software Foundation; with no
Invariant Sections, no Front-Cover Texts, and no Back-Cover Texts.
See <https://www.gnu.org/licenses/fdl-1.3.html>.
-->

# Phase 6 Implementation Plan — stub

**Not yet written.** Write it from `docs/roadmap.md` Phase 6 — *packaging* — when Phase 5
closes, in the shape of `docs/plans/phase-1.md`: scope, decisions to confirm, spec gaps,
layout, dependencies, milestones with named tests, the acceptance bullets claimed, risks,
PR sequence. Milestone numbers continue from Phase 5's last.

Deliverable, from the roadmap: install it the way your distribution expects — `.deb`,
`.rpm`, AppImage, Flatpak, MSI, a notarized `.app` — with the per-OS firewall guidance of
`docs/deployment.md §4`.

What the earlier plans leave for this phase to pick up:

- `privatium firewall [--apply]` (`spec/cli.md §9`) parses and refuses naming this phase
  since M11; it becomes real here, never elevating silently, and the Windows *Public*
  profile trap of `docs/deployment.md §4.1` is what it explains first.
- UDP 5353 for mDNS and UDP 52525 for the broadcast responder (Phase 2, M18) are two rules,
  not one; the roadmap's "not forgotten" bullet names the first and the plan adds the
  second.
- Bundled reference apps: `spec/data-dictionary.md §3.4` gives "the package's folder at
  install" to packaging, which M13 declined to embed in the binary.
- Flatpak with no `--filesystem=host`, the data root through the file-chooser portal,
  autostart through the Background portal, and mDNS inside the sandbox — all
  `AGENTS.md` invariant 7's, and the reason ports are ≥ 1024 (invariant 8).
- Release automation: the `v*` tag job of `.github/workflows/ci.yml` attaches three raw
  binaries today and grows the installers here.

---

Copyright © 2026 Gabriel Mongefranco
