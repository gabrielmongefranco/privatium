# Privatium™

***The private element of personal software.***

## Description

Privatium™ is a framework for building small, personal applications that run entirely on your hardware, that can be reached from any of your devices, and that sync your data across all your devices without depending on cloud services.

A Privatium node is a single binary. It finds your devices on the LAN with mDNS and
anywhere else by publishing signed records to the BitTorrent mainline DHT — your key is the
address, with no registrar, no dynamic-DNS account, and nothing to pay. It stores everything
as append-only JSONL event logs that any file-sync tool can replicate without conflicts.
Backup is copying a folder. Restore is copying it back.

**What you build on top is up to you.** Write it in Lua with server-rendered templates that
hot reload as you type. Or ship your own HTML, JavaScript, canvas, or WASM and use the
framework purely as a syncing database with authentication solved. Or link the core crate
into your own binary and keep your own `main()`. Three tiers by language, none with a
ceiling, mixable on one node.

One node can host one app or twenty unrelated ones, and in solo mode it is
indistinguishable from a purpose-built application.

Privatium is for people who want an app for exactly one purpose, want it private by
construction, and do not want to run a server, buy a domain, trust a cloud, or learn a
framework to get it.

## Status

**Phase 1 in progress.** `docs/roadmap.md` Phase 1 — *a node that works on one machine*
— is implemented through milestone M12 of [docs/plans/phase-1.md](docs/plans/phase-1.md):
the event log, materialization into SQLite, snapshots and the three-tier restore, the app
loader, the Lua host and LSP templates, the data API and `pv.js`, the CLI, and the linter.
Pairing, discovery and sync are later phases, so the binary calls itself
`pv/1 (partial: phase 1)` and listens on loopback only. The documents below are the
contract the code satisfies; where they disagree, the specification wins and the code is
wrong.

| Document | Purpose |
|---|---|
| [docs/architecture.md](docs/architecture.md) | How the system is put together, and why |
| [spec/protocol.md](spec/protocol.md) | **Normative.** Wire formats, events, discovery, pairing, sync |
| [spec/app-contract.md](spec/app-contract.md) | **Normative.** The three app tiers and three deployment modes |
| [spec/lua-api.md](spec/lua-api.md) | **Normative.** Tier 1 — the Lua API and LSP templates |
| [spec/data-api.md](spec/data-api.md) | **Normative.** The data API custom front ends build against |
| [spec/data-dictionary.md](spec/data-dictionary.md) | System tables, app index, field definitions |
| [spec/cli.md](spec/cli.md) | **Normative.** Command line, and the lint rules the skills enforce |
| [docs/security.md](docs/security.md) | Threat model and what is actually protected |
| [docs/connectivity.md](docs/connectivity.md) | Bootstrap and reachability per client type |
| [docs/deployment.md](docs/deployment.md) | Topologies, the always-on machine, per-OS firewall behaviour |
| [docs/backup-and-restore.md](docs/backup-and-restore.md) | The restore drill, for non-technical users |
| [docs/roadmap.md](docs/roadmap.md) | Build phases and acceptance criteria |
| [docs/frameworks.md](docs/frameworks.md) | Which libraries, frameworks and game engines fit, and which do not |
| [docs/skills.md](docs/skills.md) | How LLM-authored apps get correct, accessible, secure code |
| [docs/icons.md](docs/icons.md) | Icon system: Bootstrap Icons, inlined server-side |
| [docs/decisions/](docs/decisions/) | Decision records: Barracuda declined (0001); Rust core, pkarr, peer transport (0002); one core interface behind three transports (0003); Gun, RxDB, libp2p, SharkTrustX and BAS-in-Rust declined (0004); what a phone is in the cluster (0005); SQLite as the query engine (0006) |
| [docs/plans/phase-1.md](docs/plans/phase-1.md) | The Phase 1 work breakdown, the decisions it made, and the spec defects it found and fixed |
| [docs/naming.md](docs/naming.md) | Name, taglines, and the rename checklist |

## Quick Start Guide

From a checkout, with a Rust toolchain (`rust-toolchain.toml` pins it):

1. `cargo build --release` — one binary, `target/release/privatium`, with SQLite and Lua
   compiled in.
2. Run it: `privatium`. It prints `http://127.0.0.1:8420/`. Phase 1 listens on loopback
   only; a LAN URL and a pairing code arrive with Phase 2.
3. Open the URL. The launcher lists the bundled apps — `hello`, `animals`, `sketch`.
4. Drop an app folder into `~/.local/share/privatium/apps/` (`%LOCALAPPDATA%\privatium\apps\`
   on Windows), or start one with `privatium new <slug>` and run `privatium dev --app <slug>`.
5. To back up, copy the `data/` folder anywhere — Syncthing, a USB stick, Dropbox.
   `privatium restore --from <the copy>` brings it back.

`privatium --help` lists the rest: `lint`, `snapshot`, `skill`. Binaries for each
platform arrive with M13.

## Example Applications

Example apps ship with the framework and serve as the normative templates:

- **[apps/hello](apps/hello)** — Tier 1. Three routes, one table, two templates, no
  JavaScript. Read this first to see how little a simple app needs.
- **[apps/animals](apps/animals)** — Tier 1 at its interesting end. The guess-the-animal game
  from the console era; recursive SQL, atomic multi-event writes, stored session state.
- **[apps/sketch](apps/sketch)** — Tier 2. A canvas drawing app with its own JavaScript and
  no SQL whatsoever. The framework used purely as a syncing datastore.

`skills/` holds instruction sets you can hand to any AI assistant so it writes apps that
actually conform. See [docs/skills.md](docs/skills.md).

The first real application, a medication fill and prior-authorization tracker, will be
built as a separate repository once this framework is proven.

## About the Author

Privatium is built by [Gabriel Mongefranco](https://gabriel.mongefranco.com), a database and
software architect who has spent two decades building data platforms in healthcare and
research — enterprise data warehouses, BI systems, knowledge bases, and the first architecture for mobile and
wearable research data at a large research university.

Learn more at: https://gabriel.mongefranco.com


## Contact

Questions, bug reports, enhancement ideas and requests are welcome as GitHub issues. Feel free to send pull requests as well!


## Credits

#### This work is based in part on the following projects and libraries:

- [SQLite](https://sqlite.org/) — the in-process SQL engine the event log is materialized
  into, via [rusqlite](https://github.com/rusqlite/rusqlite); public domain, on every
  platform the framework targets.
- [iroh](https://github.com/n0-computer/iroh) — QUIC-based peer-to-peer transport with
  hole punching, used for direct node-to-node sync.
- [pkarr](https://github.com/pubky/pkarr) — signed DNS records on the BitTorrent mainline
  DHT, which is how a public key becomes an address with no registrar.
- [Arti](https://gitlab.torproject.org/tpo/core/arti) — the Tor Project's Rust
  implementation of Tor; provides in-process onion service hosting with no external daemon.
- [Noble cryptography](https://github.com/paulmillr/noble-curves) — audited, dependency-free
  JavaScript implementations of X25519, Ed25519 and ChaCha20-Poly1305, required because
  `crypto.subtle` is unavailable on plain-HTTP origins.
- [Tauri](https://github.com/tauri-apps/tauri) — desktop and mobile application shells.
- [HTMX](https://github.com/bigskysoftware/htmx) and [Alpine.js](https://github.com/alpinejs/alpine)
  — server-rendered interactivity and local reactivity, both without a build step.
- [Lua](https://www.lua.org/) and [mlua](https://github.com/mlua-rs/mlua) — the Tier 1
  application language and its Rust bindings.
- [Mako Server / Barracuda App Server](https://github.com/RealTimeLogic/BAS) — Real Time
  Logic's Lua Server Pages inspired this project's Tier 1 template engine and much of its
  developer-experience goal. No code from those projects was used. See [docs/decisions/0001](docs/decisions/0001-barracuda-evaluation.md).
- [Bootstrap Icons](https://github.com/twbs/icons) — MIT-licensed SVG icon set, vendored
  and inlined server-side; the only icon source used anywhere in the project.
- [SPAKE2 (RFC 9382)](https://www.rfc-editor.org/rfc/rfc9382.html) and CPace — the
  password-authenticated key exchange protocols used for device pairing.
- [Magic Wormhole](https://github.com/magic-wormhole/magic-wormhole) — inspiration for the
  short human-readable pairing code experience. No code from that project was used.
- [DataLaVista](https://github.com/DepressionCenter/datalavista) — its normalization
  pipeline informed the list of date, time and timestamp spellings the framework accepts
  on write (`spec/lua-api.md §3.3`): ISO, Excel-style `M/D/YYYY`, Oracle-style
  `DD-MMM-YY`, long month names, and epochs. No code from that project was used; the
  parser is the framework's own.
- [Animal](https://github.com/coding-horror/basic-computer-games/tree/main/03_Animal) —
  the `apps/animals` example follows the classic "Animal" guessing game from
  David H. Ahl's *BASIC Computer Games* (1973), preserved and ported to many
  languages by the basic-computer-games project under the Unlicense. No code is
  copied: that project's Lua port is a console program with an in-memory tree,
  while this one stores the tree as an append-only event log so it can sync
  across devices.

## License

### Copyright Notice

Copyright © 2026 Gabriel Mongefranco

### Trademark Notice

Privatium™ is a trademark of Gabriel Mongefranco.

### Software and Library License Notice

This program is free software: you can redistribute it and/or modify it under the terms
of the GNU General Public License as published by the Free Software Foundation, either
version 3 of the License, or (at your option) any later version.

This program is distributed in the hope that it will be useful, but WITHOUT ANY WARRANTY;
without even the implied warranty of MERCHANTABILITY or FITNESS FOR A PARTICULAR PURPOSE.
See the GNU General Public License for more details.

You should have received a copy of the GNU General Public License along with this program.
If not, see <https://www.gnu.org/licenses/gpl-3.0-standalone.html>.

### Documentation License Notice

Permission is granted to copy, distribute and/or modify the documentation in this
repository under the terms of the GNU Free Documentation License, Version 1.3 or any later
version published by the Free Software Foundation; with no Invariant Sections, no
Front-Cover Texts, and no Back-Cover Texts. See
<https://www.gnu.org/licenses/fdl-1.3-standalone.html>

## Citation

If you find this repository or its specifications useful, please cite it.

> *Mongefranco, Gabriel (2026). Privatium™. Software. <https://github.com/gabrielmongefranco/privatium>*

---

Copyright © 2026 Gabriel Mongefranco
