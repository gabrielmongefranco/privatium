---
name: project-preferences
description: Privatium's file header format, header-check exemptions, and repository-specific style rules. Apply when creating or editing any file that carries a header, and when writing Rust, Lua, or SQL in this repository.
---

<!--
This file is part of Privatium
skills/project-preferences/SKILL.md
Author(s): Gabriel Mongefranco
Created: 2026-09-09
Last Modified: 2026-09-09
Summary: Repository-specific header format and style rules for Privatium.
Copyright © 2026 Gabriel Mongefranco
Licensed under the GNU Free Documentation License v1.3 or later.
See <https://www.gnu.org/licenses/fdl-1.3.html>. See README for full license information.
-->

# Privatium

## Project preferences

Use this skill when planning, implementing, or reviewing changes in this repository. It
supplements [AGENTS.md](../../AGENTS.md), especially section 3, and cannot weaken its
security, privacy, accessibility, licensing, testing, or authorization rules.

### The header block

Six fields — project, the file's own path, authors, created, modified, summary — in a
comment at the top of the file. Two renderings are in use and both are correct: the
spread-out form used throughout `spec/` and `docs/`, and the compact form used in
`apps/`, which pairs `Project:` with `File:` and `Created:` with `Modified:`.

```rust
// Project:  Privatium™  |  File: crates/privatium-core/src/lib.rs
// Authors:  Gabriel Mongefranco (@gabrielmongefranco)
// Created:  2026-08-31  |  Modified: 2026-09-05
// Summary:  What this file is for, in a sentence or three.
//           See main README.md for full license information.
```

The summary tells a reader what they will find in the file and cites the documents it
implements — never a plan, a milestone, a round of work or a conversation — and it ends
with the sentence `See main README.md for full license information.`, kept whole on one
line, which is how every file points at the licence notices without repeating them.

`.lsp` templates carry a reduced form: project, path, and summary. Authorship on every
partial of an app nobody reads separately is noise.

`cargo xtask header-check` enforces this over `.rs`, `.lua`, `.sql`, `.js`, `.css`,
`.lsp`, and `.md` under `spec/` and `docs/`. Markdown elsewhere — this file, `README.md`,
every `apps/**` README and `SKILL.md`, everything under `skills/` — is prose rather than
source and is exempt by design. So is anything vendored, which is marked by a `VENDOR.md`
beside it or above it and carries its own provenance.

The dates are checked for shape, not for accuracy. A mechanical `Modified:` check would
either be wrong or fight every commit that touches the file.

### Verification

`cargo xtask header-check` is the check of record for headers. Run it after adding or
editing any file it covers, and fix what it reports rather than exempting the file.

### Additional resources

- [Project instructions](../../AGENTS.md)
- [Response style skill](../response-style/SKILL.md)
- [Skills index](../../SKILLS.md)
