# apps/

Reference applications. These are normative examples of `spec/app-contract.md`, not
demos — the framework's own tests run against them.

| App | Tier | Read it for |
|---|---|---|
| [`hello`](hello) | 1 — Lua | Three routes, one table, two LSP templates. **Start here.** |
| [`animals`](animals) | 1 — Lua | Atomic multi-event writes, recursive SQL, stored session state, `lib/` modules. |
| [`sketch`](sketch) | 2 — Web | Your own HTML and JavaScript, no SQL at all. The framework as a syncing datastore. |

`hello` and `animals` contain no JavaScript and no build step, because Tier 1 renders
server-side. `sketch` is nothing but JavaScript, because Tier 2 renders itself. Both are
normal.

**Tiers differ by language, not by capability.** None has a ceiling. If your app is records
and forms, Tier 1 saves you a front end. If it is a game or a canvas, use Tier 2 — you lose
no storage, sync, auth, or backup by doing so. See `spec/app-contract.md`.

Each app also carries a `SKILL.md` describing its own schema and conventions, so an
assistant extending it has the local context. See `docs/skills.md`.

## Bundled vs installed

Apps in this directory are *bundled* — they ship inside the binary or the package and are
read-only at runtime (a Flatpak install directory is not writable).

Apps you write go in `$XDG_DATA_HOME/privatium/apps/<slug>/`, which is writable and
survives upgrades. The framework loads both and records the origin in `sys_app.source`.

```bash
cp -r apps/hello ~/.local/share/privatium/apps/myapp
```

## The first real application

The medication fill and prior-authorization tracker that motivated this framework will
live in its own repository as an app folder, once `pv/1` is implemented and proven. It is
deliberately not here: if the framework needs to be modified to support it, that is a
finding about the framework, and it should surface as a spec change rather than a special
case in the reference apps.

---

Copyright © 2026 Gabriel Mongefranco
