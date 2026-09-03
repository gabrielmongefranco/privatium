<!--
Project:  Privatium™
File:     spec/lua-api.md
Authors:  Gabriel Mongefranco (@gabrielmongefranco)
Created:  2026-08-28
Modified: 2026-09-03
Summary:  NORMATIVE. Tier 1 — the Lua application API and LSP template engine.
-->

# Lua API — Tier 1

Tier 1 apps are written in Lua 5.4 and HTML templates. No build step, no compilation, no
restart. Save a file, refresh the browser, see the change.

---

## 1. Why Lua, and why 5.4

Lua exists to be embedded in a host application with a sandbox. nginx, Redis, Neovim,
World of Warcraft, and Roblox all arrived at it independently. It is a small language a
non-professional can learn in an afternoon, and it is far easier to hold in your head than
JavaScript or Rust.

**Lua 5.4 via `mlua`**, not LuaJIT and not Luau:

- **Not LuaJIT** — iOS forbids JIT compilation (W^X). A LuaJIT core cannot ship in the
  mobile shells, and LuaJIT is frozen at 5.1 semantics.
- **Not Luau** — genuinely attractive (gradual typing, sandboxing by design), but it is a
  dialect. Dialects break the "paste a snippet from the internet" experience that makes Lua
  approachable, and they degrade LLM assistance.
- **5.4** gives integer/float distinction, `goto`, to-be-closed variables, and a generational
  GC, with the largest body of ordinary Lua documentation behind it.

### 1.1 What Privatium does NOT use

The Barracuda App Server (BAS) and Mako Server were evaluated as a foundation and rejected
for two independent reasons:

1. **License.** The BAS library is distributed under a non-commercial license while the
   surrounding Lua is MIT. GPL-3.0 requires granting downstream users rights that a
   non-commercial license withholds. The combination is undistributable.
2. **Sandbox posture.** BAS executes Lua as trusted manufacturer logic with unrestricted
   Lua-to-C binding access. Privatium treats an app folder as semi-trusted code the owner
   downloaded. Opposite threat models.

Lua Server Pages, as a *concept*, is excellent and is reimplemented here (§4). The idea was
worth taking; the library was not.

---

## 2. App layout

```
apps/<slug>/
├── app.toml            manifest (tier = "lua")
├── app.lua             REQUIRED  entry point
├── lib/                your own modules; require'd by name
├── views/              LSP templates (.lsp)
├── static/             css, images, vendored js
├── schema.sql          OPTIONAL  tables, if you want SQL
└── SKILL.md            RECOMMENDED  see docs/skills.md
```

---

## 3. The `pv` module

```lua
local pv = require 'privatium'
```

### 3.1 Routing

```lua
pv.get('/',            function(req) ... end)
pv.get('/fill/:id',    function(req) return req.params.id end)
pv.post('/fill',       function(req) ... end)
pv.route('PUT', '/x',  function(req) ... end)
```

Handlers return one of:

| Return | Result |
|---|---|
| `pv.render('index', ctx)` | Render `views/index.lsp` |
| `pv.redirect('/')` | 303 See Other |
| `pv.json(tbl)` | `application/json` |
| `pv.text(str)` | `text/plain` |
| a string | `text/html` |
| `nil` | 204 No Content |

`req` fields: `method`, `path`, `params`, `query`, `form`, `body`, `headers`, `device`
(the paired device's ID), `is_htmx` (true when `HX-Request` is present).

### 3.2 Reading

```lua
local rows = pv.query('SELECT * FROM fill WHERE drug = ?', {'Example'})
local one  = pv.query1('SELECT count(*) AS n FROM fill')
local row  = pv.get_row('fill', id)
```

Runs on the sandboxed SQLite connection (`spec/app-contract.md §7`). Parameters are bound,
never interpolated; string concatenation into SQL MUST be rejected by the linter. Sums
over `DECIMAL` columns use `decimal_sum()`; date arithmetic uses `date(x, '+30 days')`
(`spec/data-dictionary.md §2`).

`DECIMAL` and `BIGINT` columns arrive as **strings**, for the reason in
`spec/data-dictionary.md §2.1`. `pv.dec(s)` returns a decimal userdata supporting
`+ - * /` and comparison with exact semantics. Never convert money to a Lua number.

### 3.3 Writing

```lua
local id = pv.append('fill', { drug = 'Example', copay_amount = '12.50' })  -- mints a ULID
pv.append('fill', id, { copay_amount = '10.00' })                          -- amend by id
pv.delete('fill', id)                                                      -- tombstone

pv.batch(function(tx)                                -- atomic multi-event
  local a = tx.append('node', { kind = 'a', text = 'wombat' })   -- returns the new ULID
  local b = tx.append('node', { kind = 'a', text = 'penguin' })
  tx.append('node', existing_id, { kind = 'q', yes_id = a, no_id = b })
  tx.delete('cursor', 'cursor')
end)
```

**Signatures.** Both `pv.append` and `tx.append` accept two forms:

| Call | Behaviour |
|---|---|
| `append(tbl, data)` | Mints a ULID and returns it |
| `append(tbl, id, data)` | Appends under the given `id` and returns it |

`id` may be `nil` in the three-argument form, which is equivalent to the two-argument form.
That makes `pv.append('profile', existing and existing.id or nil, data)` a legal
create-or-amend without a branch.

`tx.append` returns the minted ULID **before** the batch is written, so later events in the
same batch may reference it. A `pv.batch` either appends every event or none; the framework
assigns contiguous `seq` values to the batch.

### 3.4 Other

```lua
pv.ulid()                     -- fresh ULID
pv.now()                      -- RFC 3339 UTC string
pv.device()                   -- this device's node ID
pv.node()                     -- { id, name, solo, peers, restore_tier }
pv.setting('key', default)    -- read a sys_setting
pv.log('info', 'message')     -- structured log; never write to stdout directly
pv.on('append', function(ev) ... end)   -- server-side reaction to any event
```

`pv.on('append', ...)` fires for events arriving via sync from other devices too, which is
how you write "when a fill lands, recompute the refill window."

---

## 4. LSP templates

`views/*.lsp` is HTML with embedded Lua. Compiled once to a Lua chunk, cached, invalidated
on file mtime. In development that means **save, refresh, done** — no build, no restart.

```html
<!-- views/index.lsp -->
<h1>Upcoming refills</h1>

<? if #fills == 0 then ?>
  <p class="empty">Nothing due. <?= icon('check-circle') ?></p>
<? else ?>
  <ul>
  <? for _, f in ipairs(fills) do ?>
    <li>
      <?= f.drug ?> — due <?= fmt.date(f.due_on) ?>
      <button hx-delete="/fill/<?= f.id ?>" hx-target="closest li">
        <?= icon('trash', 'Delete this fill') ?>
      </button>
    </li>
  <? end ?>
  </ul>
<? end ?>
```

| Tag | Meaning |
|---|---|
| `<? ... ?>` | Execute Lua, emit nothing |
| `<?= expr ?>` | Emit `expr`, **HTML-escaped** |
| `<?raw expr ?>` | Emit unescaped. Flagged by the linter every time it appears. |
| `<?-- ... --?>` | Comment, stripped |

**Escaping is the default and is not optional.** `<?= ?>` always escapes. There is no
configuration flag to change this. `<?raw ?>` exists because escaping cannot be universal,
and its presence in a diff is a review trigger.

### 4.0 Where helpers are available

`url`, `icon`, `fmt.*`, and `t` are available **both in templates and in handler code**, as
globals in the app's sandbox. `render`, `layout`, and `csrf` are template-only.

```lua
pv.post('/name', function(req)          -- url() in a handler
  return pv.redirect(url('/'))
end)
```

`pv.url(path)` is available as an alias of the global `url(path)`, identical in behaviour.
Both exist because `PV301` and `spec/protocol.md §9.1` refer to the qualified form, while
sandbox code more naturally reaches for the bare one. Implementations MUST provide both.

Hardcoding `/a/<slug>/...` anywhere — handler or template — breaks the app in solo mode and
is rejected by `privatium lint` rule `PV301`.

### 4.1 Helpers available in every template

| Helper | Purpose |
|---|---|
| `render('partial', ctx)` | Include another template |
| `layout('base')` | Wrap this template in `views/base.lsp` |
| `icon(name[, label])` | Inline a Bootstrap Icon (`docs/icons.md`) |
| `url('/path')` | Mount-aware URL (host mode prefixes `/a/<slug>`, solo mode does not) |
| `fmt.date`, `fmt.money`, `fmt.rel` | Locale-aware formatting from `sys_setting` |
| `csrf()` | Hidden CSRF field. Required in every non-GET form. |
| `t('key')` | Translation lookup, if `locales/` exists |

`url()` is not cosmetic — hardcoding `/a/myapp/...` breaks the app in solo mode. The linter
flags literal mount paths.

### 4.2 Why LSP and not Jinja

The handler is Lua; the template should be Lua. Two syntaxes for one app is a tax on the
author and a source of LLM confusion. LSP's `<? ?>` is also isomorphic to PHP, ASP, EJS,
and ERB, which is an enormous body of training data — an LLM writes it correctly on the
first attempt far more often than it writes a bespoke DSL.

---

## 5. Sandbox

The Lua state MUST be created with these removed or replaced:

| Removed | Reason |
|---|---|
| `io` | Filesystem access. Use `pv.*`. |
| `os.execute`, `os.exit`, `os.getenv`, `os.remove`, `os.rename`, `os.tmpname` | Process and filesystem control |
| `package.loadlib`, `package.cpath` | Loading native code |
| `debug` | Escapes every other restriction |
| `load`, `loadstring`, `dofile`, `loadfile` | Arbitrary code from data |

`require` MUST be replaced with a loader confined to the app's own `lib/` directory plus
the framework's whitelisted modules.

Retained: `os.time`, `os.date`, `os.clock`, `string`, `table`, `math`, `coroutine`, `utf8`.

**Resource limits, all REQUIRED:**

| Limit | Default | Setting |
|---|---|---|
| Instruction count per request | 50,000,000 | `lua.max_instructions` |
| Memory per VM | 64 MB | `lua.max_memory_mb` |
| Wall clock per request | 5 s | `lua.max_seconds` |
| VM pool size | CPU count | `lua.pool_size` |

Instruction limits are enforced with a debug hook installed by the host before app code
runs; memory via a custom allocator. Exceeding a limit aborts the request, returns 500, and
writes a `lua.limit_exceeded` audit event. It MUST NOT take down the node.

Lua states are not thread-safe. The host maintains a pool; one request holds one VM. Global
state in an app is therefore **per-VM and not shared** — apps needing shared state MUST use
the event log. This is documented as a footgun and checked by the linter.

---

## 6. Client-side Lua (optional)

For sharing pure-logic modules between node and browser:

| Option | Size | Notes |
|---|---|---|
| **wasmoon** | ~250 KB wasm | Lua 5.4, matches the server exactly. Requires `permissions.wasm = true`. |
| **Fengari** | ~200 KB js | Lua 5.3, no WASM permission needed, slower |

Share **logic only** — validation, date math, scoring — from `lib/shared/`. Do not attempt
to share handler or template code; `pv.*` does not exist in the browser.

This is optional and off by default. Most apps never need it.

---

## 7. Development loop

```
privatium dev --app myapp
```

- Watches `app.lua`, `lib/`, `views/`, `static/`, `schema.sql`
- Lua and templates: reloaded in place, next request picks them up. **No restart.**
- `schema.sql`: triggers rematerialization from the logs, which is safe at any time
- Errors render in the browser with the Lua traceback and the offending template line
- `--open` prints a QR code so a phone on the LAN follows along live

The tight loop is the point. If a change requires a restart, that is a bug in the host.

---

Copyright © 2026 Gabriel Mongefranco
