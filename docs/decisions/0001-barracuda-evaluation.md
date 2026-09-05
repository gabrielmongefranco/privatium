<!--
Project:  Privatium™
File:     docs/decisions/0001-barracuda-evaluation.md
Authors:  Gabriel Mongefranco (@gabrielmongefranco)
Created:  2026-08-28
Modified: 2026-09-05
Summary:  Decision record. Barracuda App Server / Mako Server evaluated as a
          foundation and declined. Status: DECIDED — Rust.
Copyright © 2026 Gabriel Mongefranco
Privatium™ is a trademark of Gabriel Mongefranco.
Documentation license: GFDL-1.3-or-later, with no Invariant Sections,
                      no Front-Cover Texts, and no Back-Cover Texts.
Software license: GPL-3.0-or-later.
See ../../README.md for the full license notices and project credits.
-->

# ADR 0001 — Barracuda App Server as a foundation

**Status: DECIDED — declined. The core is Rust.**

## Context

Privatium needs to run personal apps, keep their data on the owner's devices, and support
sync and recovery without requiring the owner to manage a server. The question was whether
Barracuda App Server (BAS) and Mako Server should provide the foundation for that work.

The decision rests on how the components fit together and how much of the core can be
shared across platforms.

## Decision

Use `privatium-core`, a Rust library, for the event log, storage, app execution, and the
planned pairing, discovery, and sync features. BAS is not a dependency. Lua 5.4 runs through
`mlua`, and the template engine is part of the core
([Lua API](../../spec/lua-api.md)).

### 1. Share one core across platforms

The same storage and application code needs to serve the command-line program, desktop
and mobile apps, and applications that embed Privatium. Rust provides that shared library
without requiring an HTTP connection between the app and its data.

Adding BAS would introduce another runtime to integrate and maintain. Keeping the core in
Rust lets the planned native shells call it directly, with platform adapters responsible
for their own user interface and operating-system integration. See
[the in-process adapter decision](0003-in-process-adapter.md).

### 2. Integrate peer transport with the Rust runtime

Finding a device's address does not by itself make that device reachable. Privatium's
planned remote access also needs to connect through home routers, handle changing network
addresses, and use an encrypted relay when a direct connection is unavailable.

The selected peer transport is `iroh`, integrated with the Rust runtime. Adding a second
server and socket stack would increase the integration work without supplying Privatium's
sync protocol or append-only storage model. See the
[transport decision](0002-rust-core.md) and
[roadmap](../roadmap.md) for the planned work.

**BAS also owns an event loop.** Its `SoDisp` socket dispatcher controls how network
events are handled. Embedding BAS in a Rust binary would still require deciding which
runtime drives the network:

- **Let BAS drive it.** Integrate Privatium's Rust networking with `SoDisp` across a
  foreign-function interface (FFI), coordinating the dispatcher with the Tokio runtime
  used by `iroh` and the other Rust components.
- **Let Rust drive it.** Run BAS alongside Tokio and coordinate two event loops, their
  threads, and calls between Rust and C. BAS would supply a Lua runtime and template
  engine alongside capabilities already provided by `mlua` and Privatium's templates.

Both approaches add threading, scheduling, and shutdown coordination to the core. Linking
BAS as a library does not remove that integration work. Keeping networking and sync in
the Rust runtime avoids this second event loop. This is an architectural reason for the
decision, independent of the database choice. The original analysis is in
[ADR 0004, BAS embedded in Rust](0004-declined-alternatives.md#5-bas-embedded-in-rust).

### 3. SQLite is the current query engine

The earlier DuckDB comparison is no longer a reason to reject BAS. Privatium now uses
SQLite through `rusqlite`. DuckDB's compilation time, build size, and operational overhead
in this project led to that change. The framework supplies exact-decimal handling and
validates typed values before writing its SQLite cache.

[ADR 0006](0006-sqlite-engine.md) records the engine decision and its trade-offs.
SQLite is a fit for Privatium; the choice of Rust as the shared core remains a separate
decision.

## Consequences

- Privatium maintains its own Lua host and template engine, including escaping, hot reload,
  and the app sandbox.
- The event log remains the source of truth. SQLite is a rebuildable query cache, not the
  file used as the basis for device sync.
- Pairing, discovery, and sync belong in the shared core as their roadmap phases are
  implemented. Platform adapters do not need separate implementations of those features.
- The project remains responsible for testing the integrations and maintaining its chosen
  dependencies.

## Would reopen if

Revisit this choice if the project no longer needs a shared native core or integrated peer
transport, and its scope becomes a desktop-hosted application accessed through a browser.
That would change the requirements behind this decision.

---

Copyright © 2026 Gabriel Mongefranco
