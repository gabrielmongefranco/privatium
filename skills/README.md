# Privatium Skills

Instruction sets for AI assistants building Privatium apps. Drop these into Claude, Cursor,
Copilot, or anything else that accepts context files.

## Install

| Assistant | Where |
|---|---|
| Claude Code / Cowork | Copy the folder into `.claude/skills/` |
| Cursor | Copy `SKILL.md` contents into `.cursorrules` or `.cursor/rules/` |
| Copilot | Copy into `.github/copilot-instructions.md` |
| Anything else | Paste the relevant `SKILL.md` into context |

Or fetch the set matching your running node:

```bash
curl -sL http://your-node:8420/skills/bundle.zip | bsdtar -xf-
```

## Which one

| You are | Load |
|---|---|
| Starting out, unsure of tier | `privatium-overview` |
| Writing a Lua app | `privatium-tier1-lua` + `privatium-accessibility` |
| Writing a custom front end | `privatium-tier2-web` + `privatium-accessibility` |
| Writing Rust against the core | `privatium-tier3-rust` |
| Building a game | `privatium-games` |
| Reviewing anything before install | `privatium-security` |

`privatium-accessibility` and `privatium-security` apply to every tier. Load them alongside,
not instead of.

## The rule that matters

Every skill ends with `privatium lint apps/<slug>`. **Run it.** Advice you cannot verify is
advice you should not trust — including this advice.

---

Copyright © 2026 Gabriel Mongefranco
