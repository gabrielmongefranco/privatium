<!--
Project:  Privatium™
File:     spec/cli.md
Authors:  Gabriel Mongefranco (@gabrielmongefranco)
Created:  2026-08-30
Modified: 2026-08-30
Summary:  NORMATIVE. The command-line interface, including the linter that makes
          the skills system enforceable rather than advisory.
-->

# Command-Line Interface — `pv/1`

One binary, `privatium`. Running it with no arguments starts a node; every other function is
a subcommand.

Referenced throughout `skills/` and `docs/skills.md`. This document is what those references
point at.

---

## 1. Global behaviour

```
privatium [--data-dir <path>] [--config <file>] [--verbose] [--version] [<command> [args]]
```

| Flag | Default |
|---|---|
| `--data-dir` | `$XDG_DATA_HOME/privatium`, or the platform equivalent |
| `--config` | `<data-dir>/config.toml` |
| `--verbose` | off |
| `--version` | — |

`--version` prints the build version and the protocol version it implements. An
implementation that does not satisfy every item in `spec/protocol.md §13` MUST qualify the
protocol string rather than print a bare `pv/1` — for example `pv/1 (partial: phase 1)`.

**No subcommand requires elevated privileges.** The one exception is `privatium firewall
--apply`, which prints what it would run and requires explicit confirmation
(`docs/deployment.md §4.2`). Implementations MUST NOT elevate silently.

Exit codes: `0` success, `1` runtime error, `2` usage error, `3` lint findings present.

---

## 2. `privatium` — run a node

```
privatium [--port 8420] [--solo <slug>] [--no-discovery] [--open]
```

Starts the node, mounts every enabled app, begins discovery (`spec/protocol.md §6`), and
prints the LAN URL. `--open` additionally prints a QR code for pairing.

`--solo <slug>` overrides `[node] mode` from the config file for this run.

---

## 3. `privatium dev` — the development loop

```
privatium dev [--app <slug>] [--open]
```

Runs a node with file watching enabled. On change:

| Changed | Effect |
|---|---|
| `views/*.lsp` | Template chunk cache invalidated; next request recompiles |
| `app.lua`, `lib/*.lua` | App reloaded in place, routes re-registered |
| `static/*` | Served fresh; no action needed |
| `schema.sql` | Rematerialization from the logs (`spec/app-contract.md §4.5`) |
| `app.toml` | Routes and manifest re-read; data untouched |

**No restart, ever.** If a change requires restarting the node, that is a defect in the host
(`docs/architecture.md §2.4`).

Errors render in the browser with the Lua traceback and the offending template line, and are
also written to the terminal.

---

## 4. `privatium new` — scaffold an app

```
privatium new <slug> [--tier lua|web|rust] [--from <existing-app>] [--scaffold <table>]
```

Creates `<data-dir>/apps/<slug>/` populated for the chosen tier. Defaults to `--tier lua`.

- `--from hello` copies a reference app and rewrites its slug and title.
- `--scaffold <table>` reads `schema.sql` and emits `app.lua` plus `views/*.lsp` giving list,
  detail, create, and edit screens for that table.

**The generator has no runtime presence.** It writes ordinary source files you then edit,
delete, or rewrite. Implementations MUST NOT introduce a config format that describes a UI
(`spec/app-contract.md §1`).

---

## 5. `privatium lint` — the enforcement mechanism

```
privatium lint [<path>...] [--format text|json] [--severity error|warn|info] [--fix]
```

With no path, lints every installed app plus the node configuration.

Advice an assistant can ignore is worth little. The linter is what makes `skills/`
enforceable, which is why it ships in Phase 1 rather than later (`docs/roadmap.md`).

### 5.1 Rule classes

Rule IDs are stable. Removing or renumbering one is a breaking change to the skills.

**Contract — `PV1xx`**

| ID | Rule |
|---|---|
| `PV101` | `app.toml` parses and carries `slug`, `title`, `version`, `api`, `tier` |
| `PV102` | Slug matches `^[a-z][a-z0-9-]{1,30}$` and is not reserved (`spec/protocol.md §1.1`) |
| `PV103` | `api` does not exceed the framework's supported version |
| `PV104` | Slug directory name matches `app.slug` |
| `PV105` | Tier-required files present — `app.lua` for `lua`, `web/index.html` for `web` |
| `PV106` | Every table in `schema.sql` has `id VARCHAR PRIMARY KEY` |
| `PV107` | `schema.sql` contains only `CREATE TABLE`, `CREATE VIEW`, `CREATE MACRO`, `COMMENT ON` |

**Security — `PV2xx`**

| ID | Rule | Severity |
|---|---|---|
| `PV201` | No string-concatenated SQL — parameters must be bound | error |
| `PV202` | Every `<?raw ?>` use is reported for review | warn |
| `PV203` | No banned Lua global — `io`, `os.execute`, `os.getenv`, `debug`, `load`, `dofile`, `package.loadlib` | error |
| `PV204` | Every non-GET form contains `csrf()` | error |
| `PV205` | Declared `[permissions]` beyond the defaults carry a justifying comment | warn |
| `PV206` | No `innerHTML` with non-literal data in Tier 2 JavaScript | error |
| `PV207` | No external origin referenced without a matching `permissions.remote` entry | error |
| `PV208` | No apparent secret in `schema.sql`, `app.toml`, or sample data | error |

**Correctness — `PV3xx`**

| ID | Rule | Severity |
|---|---|---|
| `PV301` | No literal `/a/<slug>/` path — use `url()` or `pv.url()` (breaks solo mode) | error |
| `PV302` | No `tonumber()` or JavaScript `Number()` applied to a `DECIMAL` or `BIGINT` column | error |
| `PV303` | No `INSERT`, `UPDATE`, or `DELETE` in app SQL — writes are appends | error |
| `PV304` | Client code does not set `seq`, `lam`, `ts`, `dev`, or `app` on an event | error |
| `PV305` | No outbox dedupe table, transaction ID, or acknowledgement protocol | warn |
| `PV306` | Multi-event writes that must land together use `pv.batch` | warn |
| `PV307` | No Lua global assigned at module scope expecting persistence (VMs are pooled) | warn |

**Accessibility — `PV4xx`**

| ID | Rule | Severity |
|---|---|---|
| `PV401` | No icon-only control without a label argument or `aria-label` | error |
| `PV402` | Every form input has an associated `<label for>` | error |
| `PV403` | Radio and checkbox groups are wrapped in `fieldset`/`legend` | warn |
| `PV404` | Heading levels do not skip; exactly one `<h1>` per view | warn |
| `PV405` | No status conveyed by colour alone | warn |
| `PV406` | Declared colour tokens meet 4.5:1 body / 3:1 large and UI | warn |
| `PV407` | Tabular data uses `<table>` with `<th scope>`, not a grid of divs | warn |

**Portability — `PV5xx`**

| ID | Rule | Severity |
|---|---|---|
| `PV501` | Slug ≤ 15 characters when `nav.advertise = true` (DNS-SD label limit) | error |
| `PV502` | `permissions.cross_origin_isolated` only in solo mode (`docs/frameworks.md §5.4`) | error |
| `PV503` | Icon names exist in the vendored Bootstrap Icons set | warn |
| `PV504` | No CDN reference — libraries are vendored under `web/vendor/` | error |
| `PV505` | No absolute filesystem path, and nothing written beside the binary | error |
| `PV506` | No app route matching a framework prefix — shadowed in solo mode (`spec/protocol.md §9.1`) | warn |

### 5.2 Output

`--format json` emits one object per finding so an assistant can read its own failures and
iterate:

```json
{"id":"PV301","severity":"error","file":"apps/meds/app.lua","line":42,
 "message":"Literal mount path '/a/meds/' breaks solo mode",
 "fix":"Use url('/') instead","spec":"spec/app-contract.md §2.2"}
```

Every finding MUST carry a `spec` reference. A rule that cannot cite the document it
enforces does not belong in the linter.

### 5.3 `--fix`

Applies only unambiguous mechanical corrections — literal mount paths to `url()`, missing
`focusable="false"` on inline icons. It MUST NOT touch SQL, Lua control flow, or anything
where the intent is inferred. Everything else is reported for a human.

### 5.4 In CI

`privatium lint` runs against every app in this repository. The reference apps are the
linter's test corpus, and a rule without a passing and a failing case in `apps/` is not
considered implemented.

---

## 6. `privatium skill` — skills for an assistant

```
privatium skill list
privatium skill export [<name>...] [--out <dir>]
```

Writes the skill folders matching **the running version** to disk, so an owner on v1.2 hands
their assistant v1.2's contract rather than whatever a search engine returned
(`docs/skills.md §6`).

A running node also serves them at `/skills/<name>.md` — `skills/<name>/SKILL.md` — and
`/skills/bundle.zip`, which holds every file under `skills/` at its repository-relative
path: `README.md`, each `<name>/SKILL.md`, and each skill's `reference/`. Extracting it in
place reproduces the `skills/` tree the running version shipped, which is what the `curl`
line in `skills/README.md` relies on. The entries are stored, not compressed: the bundle is
a few hundred kilobytes of Markdown and every extractor reads a stored zip.

---

## 7. `privatium snapshot` and `privatium restore`

```
privatium snapshot [--app <slug>] [--verify]
privatium restore --from <path> [--app <slug>] [--dry-run]
```

`snapshot` writes a Parquet + CSV + `schema.sql` set (`spec/protocol.md §5`). `--verify`
recomputes checksums against `MANIFEST.json` and exits non-zero on mismatch.

`restore` reports which of the three tiers it used and exits non-zero if it fell through to
tier 3 unexpectedly. `--dry-run` reports without writing.

Neither is required for normal operation — snapshots are written automatically and restore
is ordinarily "copy the folder back" (`docs/backup-and-restore.md`).

---

## 8. `privatium pair`

```
privatium pair [--open] [--timeout 120]
```

Opens pairing mode and prints the code as four emoji, two words, and a QR code. Closes
after the timeout or the first success (`spec/protocol.md §7`).

Pairing mode MUST NOT open without this command or its equivalent in the settings UI.

---

## 9. `privatium firewall`

```
privatium firewall [--apply]
```

Detects the platform and prints the exact command that would open the node's TCP port and
UDP 5353 (`docs/deployment.md §4.2`). `--apply` runs it after showing it and asking for
confirmation.

Never runs implicitly, never elevates silently, and the node runs correctly without it on
any network that does not block inbound connections.

---

## 10. What is deliberately absent

| Not doing | Why |
|---|---|
| `doctor` / `diagnose` | Detect and explain failures where they occur, in context, rather than in a separate command nobody runs |
| `serve` | Running a node is the default; a subcommand for it is ceremony |
| `migrate` | Schema changes rematerialize automatically (`spec/app-contract.md §4.5`) |
| `install <app>` from a registry | There is no registry (`spec/app-contract.md §9`) |
| `login` / `account` | There are no accounts anywhere in this system |

---

Copyright © 2026 Gabriel Mongefranco
