---
name: privatium-tier1-lua
description: Write Tier 1 Privatium apps in Lua 5.4 with LSP templates. Covers routing, the pv module, reading and writing events, LSP syntax, the sandbox, and the specific mistakes LLMs make in this framework. Load for any app that is records, lists, forms, reports, or trackers.
---

# Privatium Tier 1 — Lua

A Tier 1 app is `app.toml`, `app.lua`, and `views/*.lsp`. No build step. Save a file,
refresh the browser, see the change.

## Skeleton

```lua
-- app.lua
local pv = require 'privatium'

pv.get('/', function(req)
  return pv.render('index', {
    fills = pv.query('SELECT * FROM fill ORDER BY filled_on DESC LIMIT 50')
  })
end)

pv.post('/fill', function(req)
  pv.append('fill', {
    drug         = req.form.drug,
    filled_on    = req.form.filled_on,
    copay_amount = req.form.copay_amount,   -- string, stays a string
  })
  return pv.redirect(url('/'))
end)
```

```html
<!-- views/index.lsp -->
<h1>Fills</h1>
<? for _, f in ipairs(fills) do ?>
  <li><?= f.drug ?> — <?= fmt.money(f.copay_amount) ?></li>
<? end ?>
```

## LSP tags

| Tag | Meaning |
|---|---|
| `<? ... ?>` | Execute, emit nothing |
| `<?= expr ?>` | Emit, **HTML-escaped** |
| `<?raw expr ?>` | Emit unescaped — every use is a review trigger |
| `<?-- ... --?>` | Comment |

Helpers in every template: `render`, `layout`, `icon`, `url`, `fmt.date`, `fmt.money`,
`fmt.rel`, `csrf`, `t`.

## MUST

- Bind SQL parameters: `pv.query('... WHERE drug = ?', {name})`
- Treat `DECIMAL`/`BIGINT` as strings; use `pv.dec()` for arithmetic. A declared column
  arrives typed by its declaration (`BOOLEAN` a boolean, `JSON` a table); an expression
  such as `count(*) AS n` arrives by storage class, so `n` is a number
- Divide with `a:div(b, scale)` — `pv.dec` has no `/`, because a quotient is not exact
- Wrap every internal link in `url()`
- Put `<?= csrf() ?>` in every non-GET form
- Give every icon-only control a label: `icon('trash', 'Delete this fill')`
- Use `pv.batch()` when more than one event must land together

## MUST NOT

- Concatenate values into SQL
- Call `io`, `os.execute`, `os.getenv`, `os.setlocale`, `debug`, `load`, `dofile` — all removed
- Write `INSERT`/`UPDATE`/`DELETE` — reads are SQL, writes are `pv.append`/`pv.delete`
- Store state in a Lua global expecting it to persist — VMs are pooled, globals are per-VM
- Set `seq`, `lam`, `ts`, `dev`, or `app` on an event
- Call `pv.append` inside `pv.batch` (use `tx.append`), or `pv.query`/`pv.append` while
  `app.lua` loads — reads and writes run inside a handler
- Catch a limit error with `pcall` and carry on — the request fails regardless

## Anti-patterns

```lua
-- WRONG: SQL injection, and it will not even be reached — the linter rejects it
pv.query("SELECT * FROM fill WHERE drug = '" .. req.form.drug .. "'")
-- RIGHT
pv.query('SELECT * FROM fill WHERE drug = ?', {req.form.drug})

-- WRONG: money as a float. 0.1 + 0.2 ~= 0.3
local total = tonumber(a.copay_amount) + tonumber(b.copay_amount)
-- RIGHT
local total = pv.dec(a.copay_amount) + pv.dec(b.copay_amount)

-- WRONG: breaks in solo mode
return pv.redirect('/a/medtracker/')
-- RIGHT
return pv.redirect(url('/'))

-- WRONG: two events, one can land without the other
pv.append('node', a, {...}); pv.append('node', b, {...})
-- RIGHT
pv.batch(function(tx) tx.append('node', a, {...}); tx.append('node', b, {...}) end)

-- WRONG: mutation
pv.query('UPDATE fill SET copay_amount = ? WHERE id = ?', {x, id})
-- RIGHT
pv.append('fill', id, { copay_amount = x })

-- WRONG: XSS
<?raw req.form.note ?>
-- RIGHT
<?= req.form.note ?>
```

## Schema

`schema.sql` is optional. Include it when you want typed tables and SQL queries; omit it to
use the log as a document store. Every table needs `id VARCHAR PRIMARY KEY`, holding a
ULID — unless the table is a singleton you key by a constant, the way `apps/animals` keys
its `cursor` row `'cursor'`. Use `DECIMAL(18,2)` for money and `DATE` for dates — never
text.

Changing `schema.sql` rematerializes from the logs. This is safe at any time and loses
nothing; new columns are simply NULL for old events.

## Escalate

If the app needs canvas, WebGL, animation, or a custom interaction model, stop and load
`privatium-tier2-web`. Do not fight LSP into being a game engine.

## Verify

```bash
privatium dev --app <slug>      # hot reload, prints a LAN QR code
privatium lint apps/<slug>
```

Full API: `spec/lua-api.md`. CLI and lint rules: `spec/cli.md`.
