<!--
Project:  Privatium™
File:     docs/decisions/0004-declined-alternatives.md
Authors:  Gabriel Mongefranco (@gabrielmongefranco)
Created:  2026-08-31
Modified: 2026-08-31
Summary:  Decision record. Sync and p2p stacks evaluated and declined —
          Gun, RxDB, libp2p, SharkTrustX, and BAS embedded in Rust — with
          the reasoning kept so the questions are not re-litigated.
          Status: DECIDED.
-->

# ADR 0004 — Declined: Gun, RxDB, libp2p, SharkTrustX, BAS-in-Rust

**Status: DECIDED.**

Companion to ADR 0001. That one records why Barracuda was not adopted as the *host*; this
one records why five widely-recommended sync and peer-to-peer stacks were not adopted as
the *core*. Each keeps a "would reopen if", because none of these is a bad project.

## The test every one of them failed

Privatium's expensive problem is **reachability with no operator**: finding a peer and
connecting to it with no account, no domain, no registrar, and nobody's server on the
critical path. Its cheap problem is **merge**, because one writer per log file makes
merging a set union (`AGENTS.md` invariants 1–2).

Most of these tools solve the cheap problem well and the expensive one not at all.

---

## 1. Gun

**Declined.**

- **No NAT traversal, no hole punching, no DHT.** Peers find each other because both were
  initialised with the same relay URL. Two peers pointed at different relays never sync. So
  adopting Gun does not remove pkarr or hole punching — it *adds* a relay someone has to
  operate, which is the exact account-and-infrastructure dependency this project exists to
  avoid.
- **What it actually supplies is a CRDT graph with timestamp-based conflict resolution.**
  That replaces the set-union merge — a few hundred lines and the cheapest component here.
  Its ordering is wall-clock driven rather than Lamport, which is strictly weaker across
  devices whose clocks drift.
- **JavaScript core.** See §6 below.
- Default relay posture accepts arbitrary data from any application pointed at it, and
  upstream releases have been paused since 2020 with an accumulating dependency surface.

**Would reopen if** the project ever wanted a public shared graph across strangers, which
is the thing Gun is actually excellent at and Privatium explicitly is not
(`docs/roadmap.md`, "Explicitly not on the roadmap").

## 2. RxDB

**Declined as core. Retained as a Tier 2 option an app author may choose.**

- **RxDB has no discovery layer at all.** It is a client database plus a replication
  *protocol*. Every replication plugin — GraphQL, CouchDB, Websocket, Supabase, Firestore,
  NATS, Google Drive — points at a backend you supply. WebRTC is the only peer-to-peer one
  and it needs a signalling server. There is no third option to find.
- **It solves the layer after the one that blocks us.** RxDB stores in IndexedDB, which is
  unreachable if the page will not load — see ADR 0003. Secure context is the constraint;
  RxDB does not address it.
- **Once secure context is solved, it earns little.** In a native shell the core is
  in-process and there is no replica to manage. On a real HTTPS origin a client replica is
  worth having, but the event log is already an append-only stream with `(dev, lam)`
  watermarks: fetch events above the watermark, apply, queue local writes to an outbox,
  push on reconnect. That is `spec/protocol.md §10` with an IndexedDB backing store, in
  roughly 300 lines, with no new dependency and no second merge model.
- Its conflict model is master/fork — the master rejects a stale push and returns the real
  state. That is a primary, which is legal at the client↔node edge and illegal node↔node
  (`AGENTS.md` invariant 9).
- The storage engines most people reach for (SQLite, IndexedDB) are paid plugins, though
  the free ones are more than adequate at personal-app scale.

**Would reopen if** the PWA client replica turns out to need real conflict handling — which
would mean the single-writer invariant had been broken somewhere, and that is the finding.

## 3. libp2p

**Declined in favour of `iroh`. The closest call in this document.**

libp2p is genuinely capable: Kademlia, AutoNAT, DCUtR hole punching, circuit relay v2,
Noise, QUIC, mDNS. The decision is fit, not quality.

| | iroh | rust-libp2p |
|---|---|---|
| Dial-by-public-key | built in | assembled |
| Mainline DHT discovery | `discovery-pkarr-dht` feature flag | write the pkarr layer |
| Concurrent discovery | `ConcurrentDiscovery` — this is `spec/protocol.md §6.5` | hand-rolled |
| Configuration surface | opinionated, small | large, modular |
| Browser | QUIC only; needs a WebTransport bridge | WebRTC transport exists |
| Multi-language | Rust-first | Go, JS, Rust |

libp2p's advantages — polyglot implementations, browser WebRTC, million-node routing — are
things Privatium does not need. Its DHT bootstrap also lands on Protocol Labs' bootstrap
peers by default, which is *more* operator-dependent than mainline, not less.

Decisive point: **iroh already ships `discovery-pkarr-dht` and `ConcurrentDiscovery`.**
The discovery design in ADR 0002 §"Discovery: everything at once" is a feature flag rather
than a build.

**Would reopen if** browser peers ever need to be first-class, since QUIC does not reach
them and libp2p's WebRTC transport does.

## 4. SharkTrustX

**Declined.** It is a DNS product, and DNS dependence is the thing pkarr exists to remove.

Its own documentation is unambiguous: the portal requires a dedicated domain plus name
server settings configured at a registrar; it runs on a Debian VPS with BIND; registering a
zone requires a domain you control with NS records pointed at the portal; and the software
is limited to a single VPS. The alternative — the vendor demo portal — requires signing in
with a third-party account from the same network as the server.

That is a domain, a registrar, a VPS, a payment method, and a single point of failure, to
obtain what pkarr provides from an Ed25519 key and nothing else.

**Would reopen if** the project ever acquires a domain for other reasons *and* pkarr proves
unreliable — at which point it is a good implementation of a thing we would then need.

## 5. BAS embedded in Rust

**Declined.** ADR 0001 declined Barracuda as the host. This declines the narrower proposal:
Rust as the executing binary, BAS linked in as a C library for LSP and Lua, p2p added
around it.

The argument that survives independently of licensing and platform support:

**BAS owns the event loop.** Its architecture is built around `SoDisp`, a platform-neutral
socket dispatcher. That leaves exactly two configurations:

- **(a) BAS owns the network.** Then iroh QUIC, mDNS, pkarr UDP, and the sync protocol must
  all be driven from inside `SoDisp`, from Rust, across FFI, under BAS's threading model —
  and DuckDB, the OS keyring, and the shell's IPC become guests of a C event loop. The
  failure mode is intermittent threading bugs rather than clean errors.
- **(b) Rust owns the network** (tokio, axum, iroh) and BAS runs as a guest. Then BAS is
  contributing a Lua VM (mlua has one), a template engine (~300 lines, ADR 0001), and a VFS
  that cannot be used anyway because the filesystem is an event log. In exchange: GPLv2, an
  FFI boundary, a second thread pool, SharkSSL's ECCN obligations, and an unported iOS
  target.

There is no configuration (c). This holds at zero licensing cost and would hold even if
every platform were officially supported.

The question that closes it is "where does p2p sync live?" Under (a) it is hand-written
async state machines in Lua against a C dispatcher — pkarr is feasible in Lua, magicsock-
class traversal is not (ADR 0001). Under (b) it is Rust, reached through
`Lua → BAS → FFI → Rust → FFI → Lua bindings`: three languages, two FFI boundaries, two
event loops, and all the difficult components on the far side.

**On licensing**, one nuance worth recording: RealTimeLogic's free commercial licence
plausibly applies to this project and would resolve the GPL web-content clause *for its
holder*. It does not resolve it for anyone forking Privatium, and a GPLv3 project cannot
ship a GPLv2-only dependency regardless. The choice is between an openly forkable framework
and a personally licensed one — a product decision, made deliberately here.

**Correction and open item.** It has been suggested that Android is an officially supported
BAS target. RealTimeLogic's published platform lists (embedded Linux, Zephyr, FreeRTOS,
lwIP, VxWorks, QNX, INTEGRITY, Windows/WinCE, ThreadX, Azure RTOS, Nucleus, embOS, Mac, and
other POSIX systems; Mako Server on Linux, Windows, macOS, QNX) do **not** name Android or
iOS. Android via the NDK is very plausible — it is Linux with bionic — but plausible is not
ported. **This claim is unverified and this ADR should be updated if a source is found.**
It does not change the outcome, because the event-loop argument above is independent of it.

**Would reopen if** the project's scope narrows to desktop, browser, and Android, under a
personal licence, with no shared native core — the conditions ADR 0001 already names.
Meanwhile Mako Server remains recommended as a *prototyping* vehicle, exactly as ADR 0001
says.

---

## 6. The cross-cutting reason the JavaScript options lose

Gun and RxDB are both JavaScript. That is fine *above* the data API and disqualifying
*below* it.

A Rust core compiles to a static or dynamic library with a C ABI, which means LÖVE reaches
it through LuaJIT's FFI, Godot through GDExtension, Unity through P/Invoke, Bevy as a
crate, and Swift or Kotlin through `uniffi` — **with no server, no localhost port, and no
daemon**. A LÖVE game cannot embed Node. A Godot export cannot embed Node. With a
JavaScript core, every one of those becomes "run a daemon and POST to it", which is a
legitimate fallback (`docs/frameworks.md §5.3`) and a poor default.

On mobile it is worse: there is no shippable Node runtime for iOS, so a JavaScript core
would mean embedding a separate JS engine to run sync logic and bridging it back to native
storage — a subordinate runtime doing what the binary could do directly.

**The rule, stated once:** JavaScript above the data API, always. JavaScript below it,
never.

---

Copyright © 2026 Gabriel Mongefranco
