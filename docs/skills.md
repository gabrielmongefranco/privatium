<!--
Project:  Privatium™
File:     docs/skills.md
Authors:  Gabriel Mongefranco (@gabrielmongefranco)
Created:  2026-08-28
Modified: 2026-08-28
Summary:  The skills system: how LLM-authored apps get correct, accessible, secure
          code without relying on the model's training data.
-->

# Skills

## 1. The problem

Owners will build their apps with an LLM. That is the expected workflow, not an edge case.

Left to its training data, a model will produce Privatium apps that are subtly wrong:
`INSERT` statements against a read-only view, `os.execute` in sandboxed Lua, hardcoded
`/a/<slug>/` paths that break in solo mode, `DECIMAL` coerced to a float, icon-only buttons
with no accessible label, Phaser 2 idioms in a Phaser 3 app. Every one of those is a
plausible guess from a model that has never seen this framework.

**Skills are the fix.** Each `skills/<name>/SKILL.md` is a self-contained instruction set
an owner drops into Claude, Cursor, Copilot, or any assistant. It carries the pinned API
reference, the invariants, the anti-patterns, and a verification command — so the model
works from the current contract rather than from memory.

This also decouples library choice from LLM familiarity. A pinned reference in a skill
beats training-data volume, which is why we can ship a smaller, better library instead of
whichever one has the most Stack Overflow answers.

## 2. Layout

```
skills/
├── README.md                      how to install these into your assistant
├── privatium-overview/            START HERE. Which tier? Which mode?
│   ├── SKILL.md
│   └── reference/
│       ├── decision-tree.md
│       └── glossary.md
├── privatium-tier1-lua/           Lua apps: routing, LSP, pv.*, sandbox
│   ├── SKILL.md
│   ├── reference/                 GENERATED at build time; not committed
│   │   ├── pv-api.md              full API, generated from the crate
│   │   ├── lsp-syntax.md
│   │   └── anti-patterns.md
│   └── examples/
│       ├── minimal/
│       └── crud/
├── privatium-tier2-web/           web/ apps, pv.js, CSP, offline
├── privatium-tier3-rust/          privatium-core as a library
├── privatium-games/               Phaser/KAPLAY/Three + save sync + isolation
├── privatium-accessibility/       WCAG 2.2 AA, applies to every tier
└── privatium-security/            applies to every tier
```

Each app template also carries its own `SKILL.md` describing *that app's* schema and
conventions, so an assistant extending an existing app has the local context too.

## 3. What a SKILL.md must contain

Normative. A skill missing any of these is incomplete.

| Section | Contents |
|---|---|
| Frontmatter | `name`, `description` written so an assistant knows when to load it |
| **Invariants** | The rules that make output wrong if broken. Stated as MUST/MUST NOT. |
| **Anti-patterns** | Wrong code beside right code. Models learn far better from contrast than from prose. |
| **Pinned reference** | Exact API surface for the pinned version. No "consult the docs." |
| **Verification** | The command that proves the output is correct |
| **Escalation** | When this tier is the wrong tool and which skill to load instead |

## 4. Verification is the load-bearing part

Advice a model can ignore is worth little. Every skill ends with a command:

```bash
privatium lint apps/myapp
```

The linter is part of the framework, not a doc. **Its rules are specified in
`spec/cli.md §5`**, with stable IDs so a skill can cite `PV301` and mean something durable.
Rule classes:

| Class | Examples |
|---|---|
| **Contract** | `app.toml` valid, slug legal, `api` supported, referenced views exist |
| **Security** | String-concatenated SQL, `<?raw ?>` usage, banned Lua globals, missing `csrf()`, permissions declared but unexplained |
| **Correctness** | Hardcoded `/a/<slug>/` paths, `DECIMAL` arithmetic on Lua numbers, writes against views, `seq`/`lam`/`ts` set client-side |
| **Accessibility** | Icon-only controls without labels, missing form labels, colour-only status, heading order, contrast on declared tokens |
| **Portability** | Slug > 15 chars with `advertise = true`, `cross_origin_isolated` in host mode, external network references without the `remote` permission |

Output is machine-readable (`--format json`) and every finding carries a `spec` reference, so
an assistant can read its own failures, look up the rule, and iterate. That loop — generate,
lint, fix — is what makes LLM-authored apps trustworthy.

A rule that cannot cite the document it enforces does not belong in the linter
(`spec/cli.md §5.2`).

`privatium lint` runs in CI for every app in this repository. The reference apps are the
linter's test corpus.

## 5. Accessibility and security are cross-cutting

They are separate skills rather than sections inside each tier, because they apply
identically to Lua, to `web/`, and to Rust, and because a model asked to "add a delete
button" should be able to load one small skill rather than re-read a tier guide.

The accessibility skill targets **WCAG 2.2 AA** and encodes the two Privatium-specific
requirements from `AGENTS.md`: pairing must be completable without reading text (the emoji
pad) **and** without seeing images (the word code with a screen reader). Both paths, not
either.

## 6. Distribution

Skills ship three ways:

1. **In the repository**, so they version with the code they describe.
2. **In the release archive**, so an owner who downloaded a binary has them.
3. **From the running node**, at `/skills/<name>.md` and `/skills/bundle.zip`, or via
   `privatium skill export` (`spec/cli.md §6`), so an owner gets the skills matching the
   version they are actually running.

Point 3 matters most. An owner running v1.2 should not be handed v2.0's API.

## 7. Maintenance rule

**A change to `spec/` that is not reflected in `skills/` is an incomplete change.** The
reference sections in `privatium-tier1-lua` and `privatium-tier2-web` are generated from
the crate and the spec, and CI fails when they drift.

---

Copyright © 2026 Gabriel Mongefranco
