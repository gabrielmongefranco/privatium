<!--
Project:  Privatium™
File:     docs/plans/phase-3.md
Authors:  Gabriel Mongefranco (@gabrielmongefranco)
Created:  2026-09-05
Modified: 2026-09-08
Summary:  Implementation plan for Phase 3 — more than one node: node admission over the
          pairing handshake, certificate renewal on sync, the sync protocol over the Phase
          2 channel, the foreign-log receiver, multi-writer materialization, logs that
          arrive by file sync, endpoint failover, attachments, the always-on node of
          Phase 3b, and the household profiles of Phase 3c.
          Non-normative. Where this plan and spec/ disagree, spec/ wins and this
          file is wrong. See main README.md for full license information.
-->

# Phase 3 Implementation Plan

Target: `docs/roadmap.md` Phase 3 — *more than one node* — and Phase 3b, *the always-on
node*, which the roadmap says adds no protocol and this plan treats as the last milestone.
Deliverable: desktop and laptop stay in sync with no server, and one pairing covers both.

## 0. How to use this

Read `AGENTS.md` in full first. Then read `docs/plans/phase-2.md`, because every decision
below starts from what that phase built rather than from what it planned: its §2 (eleven
decisions, all decided), its §3 rows 1–35 (the spec edits Phase 2 made, the hardening
rows 32–35 included), the implementation-status paragraphs of M14–M19, the paragraph
"Phase 2 hardening", and §7 and §8 (R9–R19). Then the contract: `spec/protocol.md §2.3,
§3, §4, §6, §7.4.2, §8.3, §8.3.1, §8.4, §9.2, §10, §13, §14`, `spec/data-dictionary.md
§2, §3.1–§3.3, §3.6–§3.8, §3.10, §4`, `spec/data-api.md §2, §3, §6`, `spec/app-contract.md
§6`, `spec/cli.md §1, §2, §7, §8`, `docs/decisions/0003` and `0005`, `docs/security.md`,
`docs/backup-and-restore.md` and `docs/deployment.md`. Then the code this phase builds
on: `identity.rs`, `registry.rs`, `sys.rs`, `pair/`, `session/`, `discover/`,
`wire/channel.rs`, `wire/owner.rs`, `http/auth.rs` and `lib.rs` in the core;
`run.rs`, `pair.rs`, `node.rs` and `lib.rs` in the binary; and the tests
`tests/{identity,pair,session,discover,devices}.rs` in the core and
`tests/channel.rs` in the binary.

One milestone per branch, one PR per milestone, in order — M20 to M26, continuing Phase
2's numbering, then one hardening round. A milestone is done when its named tests are
green on all three platforms and its checklist is ticked on that run, not when it
compiles. Write the named tests first. Do not start M(n+1) before M(n) merges.

Section 2 lists the decisions this plan makes. **All fourteen are decided.** Most follow
from what Phase 2's code and spec already fix; the ones that change the wire, the
security posture, or how data is stored, shared or identified — §2.1, §2.2, §2.11 and
§2.12 — were made by the owner on 2026-09-06 after this plan was reviewed, and each
records the alternative it rejected so it is not re-litigated. Section 3 is a fresh
record of the spec gaps Phase 3 will hit, one row per gap with the file and the milestone
that closes it. **This plan edits no `spec/` file**; the milestone that hits a row edits
it, regenerates `skills/` with `cargo xtask gen-skill-reference` in the same change, and
records what it found here.

Two rules from earlier plans carry more weight here than anywhere. *One writer per log
file, forever* (`AGENTS.md` 2): a sync receiver writes another device's file, and that is
the one exception `spec/protocol.md §10.2` allows, bounded exactly as it says. *No node is
primary* (`AGENTS.md` 9): if a milestone finds itself wanting a coordinator, an election,
or "the copy that is right", stop and re-read `§10.3`.

---

## 1. Scope

### In

Admitting a node over `/ws/pair` with `kind = "node"` and revoking one; certificate
renewal after a completed sync; the sync protocol of `§10.1`–`§10.2` over the Phase 2
channel, node to node; a receiver that writes other devices' logs byte for byte; the
Lamport fold of `§4.3` on receipt; materialization with more than one writer; the
endpoint candidate list and failover of `§10.4`; discovery filtered to the cluster by
`cl`; logs that arrive by file sync; the `animals` live demo; attachments
(`docs/roadmap.md` Phase 3, `spec/protocol.md §14` item 8); `privatium pair --join`; the
always-on node's documentation; `--version` claiming `pv/1 (partial: phase 3)`.

**Phase 3c, added after M21 landed:** household profiles as a partition, segment
directories under each app, `usr` in the envelope, shared tables declared in `app.toml`,
and an ephemeral message channel for apps that update many times a second. Decided in
`docs/decisions/0007-household-profiles.md`, whose §7 records that nothing is left open;
the milestone is M27.

### Out — do not implement, do not stub, do not leave TODOs referencing

pkarr, DNS discovery, iroh, relays, hole punching, onion services, HTTPS and
certificates from a CA, the PWA, native shells, `uniffi`, packaging, `privatium
firewall`, cluster rotation as a command (the "nuclear option" of `§2.3.5` stays a
documented procedure: delete `identity/cluster.*`, re-found, re-admit, re-pair),
node-key rotation, field-level merge, log compaction, blob garbage collection, a per-app
blob quota, re-keying a channel in place, a mobile client.

### The one-sentence test

If a Phase 3 change needs a machine outside the LAN to be reachable by anything but a
URL the owner typed, it is Phase 5's.

---

## 2. Decisions this plan makes — all decided

Fourteen, all decided. Each starts from a Phase 2 fact — a line of code, a `spec/`
section Phase 2 wrote, or a row of `docs/plans/phase-2.md §3` — and says what Phase 3
adds. §2.1, §2.2, §2.11 and §2.12 change the wire, the posture or what is stored, and
were the owner's call; §2.4 and §2.12 were reshaped by one fact found while making them
— a home node behind its router can dial a VPS and cannot be dialed by it — which is
why admission negotiates its direction and why either side may start the first pass.
The rest follow from what exists. Each ends with the rows that carry it.

### 2.1 A node is admitted over `/ws/pair` with `kind = "node"`, in whichever direction the network allows; both sides prove their keys, and the cluster key crosses once, under `K_pair`, from the established node to the disposable one — DECIDED

**Phase 2 facts.** `pair::handshake::Exchange::begin_with` refuses `kind: "node"` with
`PairError::NodeKind` and close code 4403 naming Phase 3 (`docs/plans/phase-2.md §2.7`,
`spec/protocol.md §7.4.2`). An accepted `pA` is bound to its code generation and its
window (row 32). The node's sealed message carries its X25519 static, its certificate,
the cluster ID and the cluster *public* key; the client's sealed message carries its
X25519 key, a label and a user agent. Nothing in the six messages proves that either
side holds the Ed25519 key its ID derives from: a browser's row is written from the key
it claims, and possession is proven later by the `/ws` handshake over the X25519 key. A
node's X25519 static is derived from its node key (`docs/plans/phase-2.md §2.6`) and
cannot be computed from its public key, so a peer learns it only from a message.
`Certificate::verify` binds `node_id`, `node_pub`, `cluster_id` and the 180-day
lifetime, and the certificate's canonical bytes are row 4's. `§7.4` calls the side
whose owner opened the window the node and the side that dialed the client; a PAKE is
symmetric, so which side is *admitted* is a fact the messages have to carry. A window
opens by the owner's standing alone, one code at a time, and is consumed by the first
success (`docs/plans/phase-2.md §2.7, §2.8`).

**Why the direction is negotiated.** A laptop on the LAN can dial the desktop and the
desktop can dial the laptop. A VPS can be dialed by a home node and cannot dial it back
through the home router. If the side that dials were always the side admitted, the
Phase 3b bullet — a VPS admitted with the same flow as a laptop — would need a tunnel.
So the exchange decides who joins whom from a fact each side already holds.

**Disposable.** A node is disposable while it has paired nothing and admitted nobody
since it entered its current cluster (`§2.3`), judged from its own `_sys` segments
alone: no `sys_device` put for another ID after the event that last set its own
`sys_node.cluster_id`. A restored or synced row another device wrote counts for
nothing (row 20's principle), which is what lets a rebuilt node join with its data
restored, and a re-founded node join after the procedure of `§2.3.5`. A node whose own
certificate has expired is judged the same way, and may re-join its own cluster without
the check (re-admission, below).

**The design.** The owner opens a window *for a node* on one machine — `privatium pair
--node`, the devices page's *Admit a node*, or `POST /api/v1/pair` with `{"ttl": …,
"node": true}` — and runs `privatium pair --join <url>` on the other (§2.12). A window
opened for a node answers `kind = "node"` alone; a window opened for devices refuses it
with 4403 exactly as today, so a window the owner opened for a phone can never hand out
the cluster key, and `PairingSnapshot` says which kind of window it is. The code, the
TTL, the five attempts and the per-source rule apply unchanged.

Messages 1–4 of `§7.4.2` are unchanged: the dialer sends `kind: "node"`, its node ID as
`dev` and its node Ed25519 key as `pub`. What is added for `kind = "node"` alone, and
nowhere for a browser:

1. **Both sealed messages carry `sig` and `disposable`.** The node's sealed message
   (message 5) adds `sig` — its Ed25519 signature over the PAKE transcript `TT` of
   `§7.4.1` — and `disposable`. The client's sealed message (message 6) carries its
   derived X25519 static as `x25519`, `sys_node.display_name` (or nothing) as `label`,
   no `ua`, and the same `sig` and `disposable`. Each side verifies the other's `sig`
   against the `pub` it already holds — the hello's for the node, the client start's
   for the client — and a missing or failing signature is 4403 and one audited
   failure. This is proof of possession: the certificate about to be issued names that
   key, and the `sys_device` row about to be written is keyed by it.
2. **The established side admits the disposable side.** After message 6 both sides
   hold both flags. One disposable and one not: the disposable side joins. Both
   disposable — two fresh machines — the dialer joins the node whose owner opened the
   window. Neither: the node refuses with 4403 after message 6, one audited failure
   naming the procedure of `§2.3.5`; two clusters are never merged.
3. **Message 7, sealed by the admitter: `admit`.** `{"cluster_key": "<base64 of the
   32-byte Ed25519 seed>", "cert": "<the joiner's certificate, base64>", "paired_at":
   "<RFC 3339 UTC>", "lam": <the admitter's _sys Lamport counter>}`. Sent only after
   the joiner's `sig` verified and, when the node is the admitter, after the registry
   check of `§7.4.2` — so the cluster private key never crosses to a peer that failed
   any check. The certificate is `Identity::sign_certificate(&joiner_pub, now)`;
   `paired_at` is the instant the admitter will write in the row; `lam` is folded by
   the joiner as `§4.3` folds a received counter, so everything the joiner writes from
   here is causally after its admission.
4. **Message 8, sealed by the joiner: `{"joined": true}`**, sent once it has verified
   `admit` — `cert` against the public half of `cluster_key`, that half against the
   `cluster_pub` it was sent when the node is the admitter, `cert.node_id` its own —
   and adopted the cluster (§2.2). The admitter then writes the joiner's `sys_device`
   row and `node.admitted` as one batch and closes 1000. An `admit` that draws no
   `joined` writes no row: the joiner, still disposable, runs `pair --join` again, and
   the owner opens a new window, since the window was consumed at message 6 as it is
   for every kind.

**Re-admission.** A node offline past its certificate's expiry (`§2.3.1`) dials, or is
dialed, like any joiner. The admitter finds the joiner's key already in `sys_device`,
active, with `sys_node.cluster_id` its own cluster and `sys_node.cert_expires_at` at or
before now: that is re-admission, allowed without the disposability check and without a
new row — `admit` carries a fresh certificate and the key the node already holds,
`node.admitted` says so, and the joiner replaces `node.cert` alone. A registered key
whose certificate has not expired, or whose row is revoked, is refused with 4403 as
`§7.4.2` says; a node that wants another cluster re-founds first.

**The alternative, so it is not re-litigated:** one direction only — the dialer joins,
the node's existing sealed message carries the cluster key, no `sig` — with an SSH
reverse tunnel in the Phase 3b quickstart so the VPS can dial home. Fewer fields and one
fewer message; but the cluster private key would leave the node before the joiner had
proven its key or been checked against the registry, a window opened for a phone could
admit a node, and the VPS flow would not be the laptop's.

*Decided: the negotiated form above. It changes the wire for `kind = "node"` (§3 rows 1,
5 and 30) and is the one place the cluster private key crosses a network, which `§2.3.3`
and invariant 10 make a posture decision; M20 writes it into `§2.3.1` and `§7.4.2`.*

### 2.2 What each side writes at admission, and the two writers of a node's registry rows — DECIDED

**Phase 2 facts.** Every node writes its own `sys_device` row at bootstrap (`kind =
'node'`, `replica = true`, both public keys; `sys::DeviceRow::this_node`,
`bootstrap_sys` in `lib.rs`) and its own `sys_node` row. `bootstrap_sys` also writes a
`sys_cluster` row for the current cluster whenever the log holds none, with `created_by`
this node, and audits `cluster.created` — correct for a founder and wrong for a joiner.
`registry.rs` amends a row by reading the winner across every segment of the `_sys` log,
applying the edit and putting the whole row, so owner-set and unknown fields survive
(`§4.2`); the keys of a row it writes are sorted, where `sys::DeviceRow` writes them in
declaration order. The admitting node writes a paired device's row (`pair/node.rs`,
`pairing_finish`) as `§7.4` step 5 says. Restore preserves other nodes' and clusters'
records and local keys select the current ones (row 20). The channel's lookup
(`wire/channel.rs`, `pins`) reads a device's `x25519_pub` and `revoked_at` from
`sys_device` and nothing else. `local/state.jsonl` holds one record per app, is never
synced and is not required for restore (`§3`).

**The problem.** Once nodes admit nodes, a node's `sys_device` row has two writers by
construction — the node itself at bootstrap and the admitting node at admission — and
`§4.5` picks between them by `(lam, ts, dev)`, where the two `lam` counters have never
met. A long-lived node whose founding cluster was empty could out-rank the admission row
with its own, which carries no `paired_at`. The same holds for `sys_cluster` if a joiner
writes one. And a joiner holds nothing in `sys_device` for its admitter until `_sys`
has synced, so an admitter that dials first — a home node driving the VPS it admitted
— would be refused at the joiner's `/ws` for want of pins.

**The design.**

- The admitter writes the joiner's `sys_device` row — `kind = 'node'`, `replica =
  true`, both public keys, `paired_at`, `paired_via = 'lan'`, `label`, no `user_agent`
  — and `node.admitted` (alert, `spec/data-dictionary.md §3.10`) as one batch, on
  message 8 (§2.1), as a browser's row is written on message 6. It writes nothing else.
  `paired_via` is `lan` for a URL the owner typed beyond the LAN too: `§3.2`'s list has
  no value for one, and adding one is not this phase's.
- The joiner, having verified `admit`, folds `lam` and re-asserts its own `sys_device`
  row with the same facts — the `paired_at` from `admit`, `paired_via = 'lan'`, the
  label it sent, cleaned by the same rule — through `amend_sys_row`, so the two writers'
  rows materialize to the same columns whichever `§4.5` picks. The re-assertion outranks
  the joiner's own bootstrap row by construction, being the same device at a higher
  `lam`. Every later amendment by either side reads the winner and carries every field
  forward, as today. Byte-for-byte equality of the two `d` values is not promised —
  the two writers order keys differently — and is not needed.
- The joiner replaces `identity/cluster.key`, `cluster.pub` and `node.cert` in a swap a
  crash cannot leave half done (row 4): the three new files are written and flushed
  beside their targets, the directory flushed, then each renamed into place;
  `Identity::load_or_create_at` finishes a swap it finds part way through before it reads
  anything, and never founds a cluster while such files exist. It tombstones the
  `sys_cluster` row of the cluster it founded, as `§2.3` allows a node that has paired
  nothing and admitted nobody; amends its `sys_node` with the new `cluster_id`, `cert`
  and `cert_expires_at`; publishes its discovery facts again, so `cl` changes on the
  wire; and records the admitter as a **peer hint** in `local/state.jsonl` — its ID,
  its X25519 public key and, when the joiner dialed, the URL — which the channel's
  lookup accepts for a `kind = 'node'` peer while `sys_device` holds no row for it, and
  which the row supersedes the moment it arrives, revocation included (§2.4, row 14). It
  writes no `sys_cluster` row for the cluster it joined. A re-admitted node replaces
  `node.cert` and amends `sys_node` alone.
- `bootstrap_sys` writes a `sys_cluster` row and audits `cluster.created` only when this
  start generated the cluster key — `Identity` reports it — never merely because the log
  holds no row. A joined node's log holds the founder's row after its first pass
  (§2.4), and until then `discovery_facts` and the manifest read the cluster ID from
  `identity/`, as they already do.
- Who may join is §2.1's disposability rule. Re-founding (`§2.3.5`) is the documented
  procedure for everything else, and the procedure ends by revoking the old cluster's
  device rows, since a row's pins are accepted by every node that holds it (row 31,
  R30).

*Decided: rows 3, 4 and 14 carry it; M20 writes it into `§2.3.1`, `§3`,
`data-dictionary.md §3.1b, §3.2, §3.7`.*

### 2.3 Certificates renew after a completed pass; an expired node starts for its owner alone and syncs nothing until re-admitted — DECIDED

**Phase 2 facts.** `Identity::load_or_create_at` renews an unexpired certificate with
fewer than ninety days left at start, writes `node.cert` through a temporary file and a
rename, and `bootstrap_sys` amends `sys_node.cert` and audits `cert.renewed` once
(rows 4 and 19). An expired certificate is `CertificateError::Expired` and the node does
not open. `§2.3.1` already says renewal happens at start and after every completed sync
and that self-renewal at or after expiry is refused.

**What Phase 3 adds.** `Identity::renew(now)` performs at runtime what startup performs,
and `Node::renew_certificate_if_due(now)` calls it after a pass that completed with any
peer (§2.4 defines *completed*), amends `sys_node` through `amend_sys_row` and audits
`cert.renewed`; the discovery facts and the channel's hello read the new certificate at
their next use. Nothing about the schedule changes: fewer than ninety days left, and
never at or after expiry.

An expired certificate no longer refuses `Node::open`. The node starts in a state it
reports on the node page and on standard error: it answers loopback and in-process
callers as the owner, presents no certificate on `/ws` — every non-loopback handshake
is refused with 4403 — opens no channel to a peer, advertises with `pair = 0`, and
audits `cert.expired` (warn) once. The owner re-admits it — `pair --join` from it
against a window a cluster node opened with `pair --node`, or the other way round when
only the cluster node can dial — which is the only way out; re-admission keeps the
node's key, its row and its cluster and issues a fresh certificate (§2.1). `§2.3.1` is
edited to say so (rows 2 and 30), because "MUST be re-admitted" is unreachable from a
node that cannot start.

A node that verifies a peer's certificate — as the client of a pass, or as a device's
node — refuses an expired one and audits `cert.expired` (warn) naming the peer, once
per peer per start.

### 2.4 Sync runs inside the Phase 2 channel, node to node, over `core::handle`; a node authenticates to a node with the `§8` handshake and its `kind = 'node'` pins — DECIDED, with one row

**Phase 2 facts.** The channel is an adapter over `core::handle`: a `req` frame becomes
a `Request` carrying `Peer`, `Session` and `Host`, and its `Response` streams back as
`res`, `chunk`s and `end` (`docs/plans/phase-2.md §2.1`, rows 2, 27, 28). The `/ws`
handshake is `Handshake::node(identity, lookup, hello)`, where `lookup` reads the
device's `x25519_pub` and `revoked_at` from `sys_device`; the client side is
`ClientHandshake::start(device, static, NodePins { id, cluster, x25519 })` and verifies
the node's certificate against the pinned cluster key before it confirms. The node's
own row cannot open a channel (row 35), the handshakes are time-bounded (row 35), and a
revocation closes an open channel through the handler's broadcast (M19 status).
`/api/v1/sync/heads`, `pull` and `push` are already rows of `§9.2`. `Node` is `Send`
and not `Sync`; the handler holds it behind one mutex taken for the synchronous part of
a request; the discovery mechanisms run on threads of their own and receive facts by
`publish_facts` (M18 status).

**The design.** No second transport and no second authentication. The node that starts
a pass is the client: it opens `/ws` on the peer's URL, runs `ClientHandshake` with its
own derived static, the peer's ID from discovery, the cluster public key from
`identity/` and the peer's `x25519_pub` from the peer's `sys_device` row, and verifies
the peer's certificate exactly as a browser does. The peer is the node: its `lookup`
finds the client's row — written at admission, or arrived by sync — and the Noise-KK
confirm proves the client holds its static. Inside the channel the pass is three
requests of `§9.2` through `handle`, answered under the peer's lock like every route.

Three consequences to state, because they shape the tests:

- **A node reaches a peer only once it holds that peer's pins.** The admitter holds the
  joiner's row from admission; the joiner holds the admitter's pins as the peer hint of
  §2.2 until `_sys` brings the row, so **either side may start the first pass** — which
  is what lets a home node behind its router drive the VPS it admitted — **and `_sys`
  is synced before any other app in every pass.** The channel's lookup answers a `kind
  = 'node'` peer from the hint only while `sys_device` holds no row for it; a row,
  revoked or not, supersedes the hint. A third node meets a peer it was not admitted by
  once `_sys` has carried both rows both ways, which is one pass with any node that has
  met both. The roadmap's first bullet — the phone reaches the second node without
  re-pairing — holds when that node has synced `_sys`: the phone verifies the node's
  certificate against its pinned cluster key, and the node admits the phone because the
  phone's row has arrived. Both halves are needed; neither is "the certificate alone".
- **A node session is confined.** A `Session` whose device row has `kind = 'node'` may
  use `/api/v1/sync/*`, `/api/v1/health` and `/api/v1/manifest` and nothing else; every
  other route answers 403. A node holds the cluster key and the owner's data already,
  so nothing is gained by letting it browse a peer's pages, and least privilege says
  refuse. The sync routes in turn answer a node session alone: a browser session and the
  owner's loopback standing are both refused, so nothing but a cluster member can ever
  ask a node to write another device's file. Row 9.
- **Completed** means: heads were exchanged for every app, every range this node asked
  for arrived whole, and every range it offered was accepted or refused with a reason.
  A pass that stopped early — a connect timeout, a torn answer, a 409 — is not
  completed, renews nothing, and is retried at the next trigger.

`spec/app-contract.md §6`'s row for `start_sync`/`sync_now` says "iroh + LAN peers";
iroh is Phase 5's transport for the same `§10.1` union (`§14` item 6), and in `pv/1
(partial: phase 3)` the row says LAN alone (row 26).

### 2.5 The engine is a thread of its own and reaches the node only through an inbox the node drains — DECIDED

**Phase 2 facts.** `start_sync(&mut self)` and `sync_now(&mut self)` return
`Error::Unimplemented` naming Phase 3 (`lib.rs`), and `AGENTS.md` forbids a stub that
succeeds. An embedder's `Node` sits in their `main` while `axum::serve` runs
(`spec/app-contract.md §2.3`), so a background engine cannot hold it. `SnapshotJob`
reads segments by length with no lock. `Node::refresh_app` is the per-request stat path:
it notices a segment that grew or appeared through `Store::is_stale`, folds `lam` and
the heads with `AppLog::rescan`, rebuilds and sends `resync` on the app's stream; the
stream's 30-second ping runs the same stat (`spec/data-api.md §3`). The discovery
mechanisms are the model: threads, no async runtime in the core's contract, facts pushed
to them by the node.

**The design.**

- `Node::start_sync(&mut self) -> Result<()>` starts `sync::Engine` on a thread with a
  current-thread tokio runtime of its own; the handle stops it when the node drops or
  `close` runs. `start_sync` keeps its signature. The engine holds what it needs and
  nothing it must not: the paths, the node ID, the derived X25519 static, the
  certificate, the cluster public key, and a `watch::Receiver<sync::PeerTable>` the node
  refreshes with `publish_peers` — the active `kind = 'node'` rows other than this one,
  each with its `x25519_pub`, the revocations, and the endpoints of §2.8 — whenever
  `_sys` is refreshed, a pass ends, discovery reports a node, or the owner types a URL.
  It reads the discovery table directly, since `discover::Shared` is already shared by
  threads.
- **Outbound** it reads this node's logs by segment length with no lock, as
  `SnapshotJob` does, and learns the local heads by reading each segment's last line.
- **Inbound** it never writes a log file. What it pulls goes into `sync::Inbox` — an
  `mpsc` channel the node holds the receiving end of — as raw bytes tagged with the app,
  the origin device and the peer. **`Node::refresh_app` drains the inbox for that app
  first**, and always `_sys` before it, so a pulled batch lands under the node's lock,
  is validated and appended by the receiver of §2.6, applied to the cache by the rank
  of §2.7, and put on the app's stream as `append` frames with `pv.on('append')` firing
  for it. A push *from* a peer arrives as a request through `handle` and reaches the
  same receiver directly. `sync_now(&mut self) -> Result<SyncReport>` runs one pass to
  completion on the engine's runtime, drains every app, and reports what moved and
  what refused.
- **Liveness.** `Node::sync_events() -> watch::Receiver<u64>` ticks when the inbox
  gains a batch; the stream's pump selects on it beside its ping and calls
  `refresh_app`, so a synced event reaches an open `/api/stream` in milliseconds; the
  daemon's run loop drains on the same tick for apps with no stream open. An embedder
  that calls nothing sees the batch at its next call, which the Tier 3 skill says.
- **Triggers.** At start; when the discovery table gains a cluster peer; every sixty
  seconds while a peer answers; and one second after any local append, debounced —
  `Node` bumps a `watch` the engine waits on — which is what makes the `animals` demo
  live. No timer is the only trigger (`§10.4`).

The engine's client side needs a WebSocket client in the core: `tokio-tungstenite`,
already in the core's graph beneath `axum`'s `ws` feature at the version `Cargo.lock`
pins, becomes a direct dependency (§5).

### 2.6 The receiver stores what the origin wrote, byte for byte, and completes a torn copy by its suffix — DECIDED, with three rows

**Phase 2 facts.** `log::Writer::check_is_ours` refuses a file that is not this node's,
and `Error::LogNotOurs` names the receiver as the one exception that will not come
through it. The reader treats `<dev>.jsonl` and `<dev>.<n>.jsonl` as one stream, ignores
a `.tmp` and every other name, skips a batch that reached the disk short and reports it
once (`§4.1`), skips a line that is not an envelope and reports it without an audit
(`Malformed`), and refuses to repair a `seq` gap. `backup::Plan` copies a log only when
this root lacks it or holds a strict prefix of it, and refuses a divergence before a byte
moves. A writer whose append failed stays closed until the file is re-read.

**The design.** `log::foreign::Receiver` is the one module that opens another device's
file: `open(paths, app, dev)`, `append(&mut self, lines) -> Result<Head>` validating
every line per `§10.2` before any is written — `dev` equal to the file, `seq` exactly
`head + 1` per envelope line, `app` equal to the directory, the envelope parses — then
one `write_all` and one `fsync`, as this node's own batches are. It refuses this node's
own ID, and `Writer` still refuses everything it writes.

Three cases the spec leaves open, each a row:

- **A future-dated line** (`§4.4`) is stored and skipped by the materializer, which
  already applies `§4.4` on read; the rejection is audited once as `event.rejected`.
  Not storing it would leave a permanent gap the receiver re-requests forever, and
  editing it is forbidden. Row 12.
- **A short batch and a line that is not an envelope** are forwarded and stored exactly
  as the origin holds them. `§4.1` says a reader MUST NOT forward the lines of a short
  batch, and read literally that makes a receiver's copy diverge from the origin's file
  — a permanent `seq` gap on the receiver, since the origin's next line follows the
  skipped ones — and defeats the prefix rule that `backup::Plan` and the torn-tail
  completion below rely on. Sync copies the file; every reader on every node skips the
  same lines by the same rule. A non-envelope line is bounded by the reader's own line
  limit, which M21 states, and counts toward `api.max_body`. Row 11.
- **A foreign segment that ends mid-line** — the receiver crashed between the write and
  the disk — is completed, never cut: the next pull asks for `seq = head + 1`, the bytes
  that arrive are compared with the torn tail, and if the tail is a prefix of them the
  remainder is appended, so the file is byte-identical to the origin's. Bytes that are
  not a prefix are a corrupt copy: sync for that device's log stops, the owner is told
  with the file name and offset, and nothing is written. Row 13.

### 2.7 The cache learns rank, so a received event is applied only if it wins — DECIDED

**Phase 2 facts.** `store::materialize::apply` overwrites blindly, and its comment says
why that is correct while this node is the only writer and names the sync receiver as
the moment it stops being (`store/mod.rs`, `apply`). `store::events::winners` already
computes `§4.5`'s winner per `(tbl, id)` for a replay. Phase 1's §2.5 property —
incremental equals replay — is held by `test_incremental_matches_full_replay`.

**The design.** M22 gives every app cache a `pv_rank(tbl, id, lam, ts, dev)` table kept
by every rebuild and every apply; an apply reads the row's rank, compares `(lam, ts,
dev)`, and writes only when the incoming event is later, for a `put` and a `del` alike.
This node's own append keeps its fast path — its event is always the latest, as the
comment proves — and the comment is rewritten to say when each path is taken. The data
API's conditional append (`§10.6`, `base`) reads the rank from `pv_rank` rather than
re-reading the log, with a test that the answer is the same. The §2.5 property is
extended to interleaved streams from three devices and stays the definition. `§4.5`
gains one sentence: an incremental apply MUST compare rank (row 19).

### 2.8 Peers and endpoints are learned by discovery and remembered lightly — DECIDED, with two rows

**Phase 2 facts.** `discover::Discovered` carries `id`, `cluster`, `addrs`, `port`,
`apps`, `pair` and `seen_at`, keyed by ID and validated off the wire (rows 33, 34);
`Node::discovered` lists it. `spec/data-dictionary.md §3.7, §3.7b, §3.8` describe
`sys_peer`, `sys_endpoint` and `sys_sync_state` as local-store tables, and `sys.sql`
deliberately holds none of them; `local/state.jsonl` holds one record per app and
`§3` shows `local/` with two files. `§10.4` lists six endpoint kinds and
`data-dictionary.md §3.7b` ten.

**The design.** The engine keeps the candidate list in memory: one entry per address
discovery reported (`lan-mdns`, `lan-udp`), plus the join URL and any URL the owner
typed on the settings node page (`static`, the kind row 15 adds for a URL the owner
typed, since `lan-ip` would misname a VPS), ordered by `last_ok` then kind, connect
timeout 2500 ms, ten seconds across all candidates, `endpoint.failover` audited through
the inbox when the first candidate fails and another answers. What survives a restart is
the join URL and the owner's URLs, written by the node into `local/state.jsonl` as a
`peers` record — "cached hints; may be stale, never authoritative", which is what
`data-dictionary.md §3.7` calls them (row 14). `sys_sync_state` is not persisted:
`their_seq` is one `heads` request away and `our_seq` is the file. `§10.4` and
`data-dictionary.md §3.7b` get one list of kinds, `static` added,
(row 15). The network-change and foreground re-attempts of `§10.4` are a native
client's; a node re-attempts on discovery, on the timer and on `sync_now`, and the
plan says which half of that checklist line Phase 3 claims (row 27).

### 2.9 No filesystem watcher; the stat is the watcher — DECIDED, with one row

**Phase 2 facts.** `docs/plans/phase-2.md §5` takes no `notify` crate and says Phase 3
does not need one either. `Store::take_inputs` lists `log/*.jsonl` on every stat,
`refresh_app` rescans the log when it moved, the stream's ping runs the same stat, and
the reader ignores a `.tmp` beside a segment (the facts of §2.5). `§10.5` says a node MUST
watch `data/` for externally-appeared files and re-materialize.

**The design.** A log Syncthing delivers — a foreign segment appearing whole by rename, a
foreign segment growing, a `.syncthing.*.tmp` beside it — is applied on the next
request or ping with no new machinery, and M23 proves it. `Node::query` calls
`refresh_app` first, so an embedder's read sees a segment that appeared on disk.
`notify` stays out; `deny.toml`'s allowance for its licence stays as a harmless line.
`§10.5`'s sentence is edited to say the watching is the stat (row 18).

One rule the documents must state: a folder under file sync **or** network sync, not
both. Two writers of one foreign segment — the engine and Syncthing — is the conflict
the single-writer rule exists to prevent. `docs/backup-and-restore.md §2` and
`docs/deployment.md §1, §2.3` say so, and how to tell which is in use: the node page
names the peers it syncs with, and a folder with a `.stfolder` is Syncthing's.

The roadmap's scope line says "filesystem watcher for externally-synced logs". It is
reachable in effect and not as written; §6 M23 proposes the wording.

### 2.10 Discovery filters to the cluster by `cl`; strangers are shown and never contacted — DECIDED, with one row

**Phase 2 facts.** `Facts::cluster` is advertised as `cl` and `Discovered::cluster` is
read and validated (row 34); `Node::discovered` returns every node seen; the node page
lists them by ID; `docs/plans/phase-2.md §7` says the `cl` filter needs a second node.

**The design.** `Node::peers()` is `discovered()` filtered to this node's cluster ID,
never this node's own ID, and `Node::strangers()` the rest, both by ID; the engine syncs
with `peers()` alone and the
node page shows both lists under their own headings, so a stranger's node on the LAN is
visible and never offered a pass. `§6.1`'s "a client that has paired MUST filter" gains
the node's own case (row 8).

### 2.11 Attachments: immutable, content-addressed files beside the log, streamed through the channel — DECIDED

**Phase 2 facts.** `§14` item 8 and the roadmap fix the constraints: `data/<slug>/blob/
<sha256>`, immutable, referenced from `d` by hash, a set union, inside the same backup,
never a mutable file sync. The channel carries a request's whole body in one `req`
frame bounded by `api.max_body` (4 MB), and a `chunk` from the client is reserved and
refused naming Phase 3 (`§8.3`, `ChannelError::Upload`). Request bodies stream in the
core (ADR 0003), so a large upload never lands in memory whole on loopback. `PV401`
to `PV407` are the accessibility rules; `PV408` is the next number. The scaffold maps
logical types to controls (`spec/data-dictionary.md §2`).

**The design.**

- **Storage.** `data/<slug>/blob/<sha256 hex>`, 64 lowercase hex characters, written to
  `<hash>.part` and renamed when the hash of what was written matches the name, then
  the directory flushed (`durable::sync_dir`). Never modified, never deleted in `pv/1`;
  a `.part` from a crashed write is removed at the next start. Not in snapshots — a
  snapshot is a cache of tables, and a blob is already immutable and self-verifying.
  In the backup by being under `data/`; `restore --from` copies blobs this node lacks,
  verifying each, and a mismatch refuses that blob by name while the rest proceed.
  **`blob/` sits beside `log/` at the app level, above the profile segments of
  `docs/decisions/0007-household-profiles.md` D6, and stays there.** A blob is named by its
  own hash, so two people who attach the same file share one copy; per-segment blobs would
  give up that deduplication and would still not isolate anything, since the hash is the
  name. The consequence has to be written down rather than discovered later: removing a
  profile's segment removes its events and **not** the blobs they referenced. Reclaiming
  those is the garbage collection this milestone already defers, and `§14` item 8 is where
  it is asked. `§4.7` says both sentences.
- **Reference.** A JSON object in `d`: `{"sha256":"<hex>","type":"image/png","bytes":
  1234,"name":"receipt.png"}`. `spec/data-dictionary.md §2` gains the logical type
  `attachment` — declared `JSON`, stored as text, the scaffold's control `input
  [type=file]` — and typed writes refuse a value that is not that shape. Presence is
  never checked on write: the blob may arrive after the event, or before it.
- **Data API.** `PUT <mount>api/blob` with the bytes as the body and `Content-Length`
  required, streamed to `.part` and hashed as it streams, refused past `api.max_blob`
  (32 MiB by default, a new `data-dictionary.md §3.6` key) before a byte is read when
  the length says so,
  answering the reference. `GET <mount>api/blob/<hex>?type=<mime>` serves the bytes as
  `application/octet-stream` with `Content-Disposition: attachment` unless `type` is one
  of `image/*` (never `image/svg+xml`, which is a document that can carry script),
  `audio/*`, `video/*`, `application/pdf`, `text/plain`, in which case inline under
  `nosniff`; the type is the URL's, never stored, so nothing on disk is trusted for it. `POST <mount>api/blob` as `multipart/form-data` with one file part and
  a `next` field answers 303 to `next` with `blob`, `type`, `bytes` and `name` in the
  query — the no-JavaScript path for a Tier 1 form, whose handler then writes the event.
  `pv.js` gains `pv.blob(file)` and `pv.blobUrl(reference)`; the outbox never queues a
  blob, since a file is not an event. `pv.js` stands a few bytes under the 12 KB
  `data-api.md §5` promises, so the two helpers move that sentence to 16 KB rather than
  split the one helper an app author imports (row 32).
- **The channel.** A `req` may carry `streaming: true` and no payload, followed by `chunk`
  frames from the client in order and a client `end`, mirroring the response side; the
  node feeds them to the request body as they arrive, bounded by `api.max_blob` for the
  blob routes and by `api.max_body` everywhere else. This is the reserved direction of
  `§8.3` defined, and a wire change (row 23).
- **Sync.** After the lines of an app, the engine lists the hashes a peer holds (`GET
  /api/v1/sync/blobs?app=&after=`, paged by hash), fetches what it lacks (`GET
  /api/v1/sync/blob?app=&sha256=`) into `.part` files it verifies before rename, and
  pushes what the peer lacks (`PUT /api/v1/sync/blob?app=&sha256=`, streamed); a
  receiver refuses a mismatch naming the peer. A set union of hashes, exactly as the
  logs are a set union of `(dev, seq)`.
- **Lint and accessibility.** `PV408`: every `<img>` in a template, and every one a Tier
  2 page ships, has `alt` (WCAG 1.1.1) — needed the day pictures appear. The scaffold
  emits a file input on its own form and an `<img alt>` or a download link on the detail
  page.
- **Not decided, deliberately:** garbage collection of blobs no event references, and a
  size quota per app. `§14` item 8 becomes those two questions.

*Decided. It is new normative surface — `protocol.md §4.7`, the `blob/` directory in
`§3`, the `attachment` type, `api.max_blob`, `data-api.md §8`, three sync routes,
`PV408`, the client `chunk` direction of `§8.3` and the `pv.js` bound (rows 22, 23 and
32) — and nothing before M25 depends on it. The alternative for the channel — capping
a blob at `api.max_body` and streaming only on loopback — would make an attachment
saved from a phone a different feature from one saved at the desk.*

### 2.12 A node joins with `privatium pair --join <url>` against a window opened with `privatium pair --node`, or from the settings pages — DECIDED

**Phase 2 facts.** `cli.md §8` gives `pair` two flags, `--open` and `--timeout`, and no
way for a machine to present a code or to say the window is for a node. `privatium pair`
talks to the running node over loopback through `/api/v1/pair`
(`docs/plans/phase-2.md §2.8`) because a data root is one process's. The owner's standing
— a loopback request or an in-process call, never a session — is what opens pairing,
names the node, labels and revokes (M19 status, `wire/owner.rs`).
`spec/app-contract.md §6` lists the areas of the core's API and `join` is not among them.

**The design.** Two commands, one on each machine, in the order reachability allows:

- On the machine that will be dialed: `privatium pair --node` — or the devices page's
  *Admit a node*, or `POST /api/v1/pair` with `"node": true` — opens a window that
  answers `kind = "node"` alone and prints the code as `pair` does.
- On the machine that can reach it: `privatium pair --join <url>` takes the URL the
  first machine printed, prompts on the terminal for the code — the two words, or the
  four glyph labels typed — and asks the running node over loopback to join: `POST
  /api/v1/join` with `{"url": …, "code": …}`, the owner's alone like `/api/v1/pair`,
  read as `application/json` and bounded as that route's body is, answering the outcome
  by name. The settings node page gets the same as *Join a cluster*, for an owner
  without a terminal. `--join` with `--node`, or with `--open`, is a usage error.

Which machine joins is decided by §2.1, not by which command it ran: on a LAN the fresh
laptop joins the desktop whichever was dialed; for Phase 3b the owner runs `pair --node`
on the VPS over SSH and `pair --join http://<vps>:8420` at home, and the VPS joins the
home cluster. `Node::join(url, code)` runs the client of §2.1 for an embedder; in the
daemon the socket work runs on a task outside the node's lock, as `serve_ws_pair` keeps
its own, and the lock is taken for the steps that read or write the node —
disposability, the identity, the adoption. The code never crosses loopback in a URL or a
log.

*Decided: it widens `cli.md §8`, `§9.2` and `app-contract.md §6` (row 5). The
alternative — a joining node that opens its own window and an admitting node that dials
it under a flag of its own — needs four flags where two and a negotiated direction do
the same, and lets the owner get the pair wrong.*

### 2.13 Two small shapes: what `--version` claims, and what the four methods do — DECIDED

`privatium --version` prints `pv/1 (partial: phase 3)` from M26, since the `§13` items
Phase 3 cannot claim are pkarr's and the network-change half of `§10.4` (§7).
`start_sync` and `sync_now` are real from M21, `sync_now` answers a `SyncReport`, and
`test_spec_app_contract_6_phase_2_methods_never_ok` is retired for a test that they do
what `§6` says; `join` joins the table (row 5). `pv.node().peers` and `/api/node`'s
`peers` already count active `kind = 'node'` rows other than this node
(`registry.rs`, `paired_node_count`) and become non-zero at M20 with no change.
`sys.v_health.unsynced_peers`, NULL since Phase 1, is filled at M21 with the count of
peers that have not completed a pass since this node started (row 21).

---

### 2.14 Cluster succession carries a paired device across a join — DECIDED, scheduled separately

A phone pins a cluster key, so copying its device row does not carry its pairing across
an adoption of another cluster. The chosen follow-up is cluster succession: a node
that founded its current cluster and has admitted no other node may join while keeping
its paired devices. Before discarding its old cluster key, it signs an amendment naming
the successor cluster and public key, using the old key and the certificate's canonical
encoding rule. The channel hello carries the chain; a client verifies each link from
its existing pin, then replaces that pin with the successor. Unverified mismatches
remain refused, and the chain has a small fixed bound, proposed as four links.

Only a cluster's founder may sign its succession. A node may not sign for a cluster it
joined, and two nodes that have each admitted nodes remain unable to merge clusters.
Rotation stays a hard cut requiring revocation and re-pairing. The old key already
could certify any node to its pinned devices; succession adds a verifiable transition,
not another holder of authority.

This is a separate milestone after M21, with its own spec rows and tests. It will touch
`protocol.md §2.3, §2.3.1, §2.3.2, §7.4.2, §8.1`,
`data-dictionary.md §3.1b`, admission, and browser and native pin verification. M21
adds no succession code, wire field, or normative text. Existing admission rules remain.

---

## 3. Spec gaps found — fixed in the milestone that meets them

Read against the sections §0 names, as the code stands after PR #38. Nothing here is
edited by this plan; each row names the milestone whose PR edits it, with `skills/`
regenerated in the same change and the row marked **Fixed** with the test that holds it,
as `docs/plans/phase-2.md §3` does.

**What Phase 2 already closed.** Three rows an earlier draft of this plan carried are
closed by `docs/plans/phase-2.md §3`: the certificate's canonical signed bytes and its
base64 in `sys_node.cert` (Phase 2 row 4); renewal at start only while unexpired, and the
refusal of self-renewal at or after expiry (row 19); restore preserving other nodes' and
clusters' records, with verified local keys selecting the current ones (row 20). Rows 31
(hourly `last_seen_at`) and 35 (the node's own row cannot open a channel, handshakes
time-bounded) apply to node sessions unchanged. Row 34's validation of records off the
wire is what the `cl` filter of §2.10 stands on. What those rows left — the message
shapes of admission, the runtime half of renewal, and what an expired node may do — is
rows 1, 2 and 7 below.

| # | Was | Proposed | Files | Milestone |
|---|---|---|---|---|
| 1 | `§2.3.1` says the admitting node sends the cluster private key and a certificate, and `§7.4.2` shows six messages whose sealed payloads carry neither; nothing says what a joining node puts in `dev`, `pub` and `x25519`, nothing proves either side holds the key its ID derives from, and `§7.4`'s node and client are the window's side and the dialer, which says nothing about which of two nodes is admitted | For `kind = "node"`: the dialer's `dev` and `pub` are its node identity and its `x25519` the derived static of `§8`; both sealed messages carry `sig`, an Ed25519 signature over `TT`, and `disposable`; the established side admits the disposable one, two disposable sides admit toward the window, two established sides are refused naming `§2.3.5`; a seventh sealed message, `admit`, from the admitter carries `cluster_key`, the joiner's `cert`, `paired_at` and the admitter's `_sys` `lam`; an eighth, `joined`, from the joiner precedes the row; *disposable* is defined from the node's own `_sys` segments (§2.1) | `protocol.md §2.3, §2.3.1, §7.4.2` | **Fixed**; M20; `test_spec_2_3_1_a_node_is_admitted_by_pairing_and_receives_the_cluster_key_and_a_certificate`, `test_spec_2_3_1_the_established_node_admits_the_disposable_one_whichever_dialed`, `test_spec_2_3_1_admit_is_sent_only_after_the_joiners_signature_verifies`; an `expired` flag joined `sig` and `disposable` on both messages (row 33) |
| 2 | `§2.3.1` "A node offline longer than 180 days MUST be re-admitted", and the reference node refuses to open with an expired certificate, so re-admission — which needs a running node — is unreachable | An expired node starts for its owner alone: loopback and in-process callers are served, every non-loopback handshake is refused with 4403, it opens no channel to a peer, advertises `pair = 0`, audits `cert.expired` (warn) once, and says so on the node page; re-admission is the only way out (§2.3) | `protocol.md §2.3.1`, `docs/backup-and-restore.md §1` | **Fixed**; M20; `test_spec_2_3_1_an_expired_node_starts_for_its_owner_and_refuses_every_channel`, `test_spec_2_3_1_an_expired_node_refuses_every_channel_over_a_socket` |
| 3 | `data-dictionary.md §3.1` has every node write its own `sys_device` row and `§7.4` step 5 has the admitting node write the joiner's, so a node's row has two writers that `§4.5` ranks by counters that have never met; `data-dictionary.md §3.1b` says who tombstones a founding cluster's row and not who writes the joined cluster's | The admitter writes the joiner's row and `node.admitted` once `joined` arrives; the joiner folds the `lam` it was sent and re-asserts its own row with the same facts from `admit`, so the two writers agree column for column; a `sys_cluster` row is written by the founding node alone, at founding, and `cluster.created` is audited then and never again; an admitted node writes none and receives the founder's by sync (§2.2) | `protocol.md §2.3.1, §4.3`, `data-dictionary.md §3.1b, §3.2` | **Fixed**; M20; `test_spec_2_3_1_the_joiner_and_the_admitter_write_the_same_device_facts`, `test_spec_2_3_1_a_joined_node_writes_no_cluster_row_and_audits_no_founding` |
| 4 | `§2.3` says a node admitted elsewhere "discards the one it founded" and not how; the reference node refuses an `identity/` whose three files disagree, which a crash mid-swap leaves | The swap writes the three files beside their targets, flushes them and the directory, then renames each; startup completes a swap it finds part way through before it reads anything and never founds a cluster while such files exist (§2.2) | `protocol.md §2.3` | **Fixed**; M20; `test_spec_2_3_a_join_interrupted_mid_swap_completes_at_the_next_start` — the swap stages the three files in `identity/adopt.tmp/` and commits by renaming that directory to `identity/adopt/`, which a start completes |
| 5 | `cli.md §8` has no way for a machine to present a code or to open a window for a node; `§9.2`'s `/api/v1/pair` body is `{"ttl"}` alone and no route joins; `app-contract.md §6` lists no `join` | `pair --node` and `pair --join <url>`, the latter prompting for the code; `"node": true` in `POST /api/v1/pair`'s body and `node` in the window it answers with; a window opened for a node answers `kind = "node"` alone and one opened for devices refuses it; `POST /api/v1/join`, the owner's alone; the two settings surfaces; `join(url, code)` in the `§6` table (§2.12) | `cli.md §8`, `protocol.md §7.1, §9.2`, `data-dictionary.md §3.3`, `app-contract.md §6` | **Fixed**; M20; `test_spec_9_2_join_and_node_windows_are_the_owners_alone_and_bound_their_bodies`, `test_spec_cli_8_pair_join_admits_this_node`, `test_spec_cli_8_pair_node_and_join_together_are_a_usage_error` |
| 6 | `§2.3.4` "A device that has synced since the revocation refuses that node" — a browser never syncs, so it trusts a revoked node's certificate until expiry; nothing says what a revoked node does when the revocation reaches it | The refusal is a replica's — a node, and a native replica when one exists; a browser holds the cluster public key alone and is covered by the 180-day bound `§2.3.4` already states; a node that finds its own ID in `sys_node_revocation` stops syncing, refuses every channel, and tells the owner (alert); a revoked node's key is never re-admitted, since a registered key is refused at pairing (`§7.4.2`), and it re-initializes instead (`§2.4`) | `protocol.md §2.3.4`, `docs/security.md §8` | **Fixed**; M20; `test_spec_2_3_4_a_node_refuses_a_peer_named_in_its_revocation_table`, `test_spec_2_3_4_a_node_that_finds_itself_revoked_stops_and_tells_the_owner` |
| 7 | `§2.3.1` "after every completed sync" — completed is undefined, and `data-dictionary.md §3.10` gives `cert.expired` no severity or subject | Completed per §2.4: heads exchanged for every app, every requested range arrived whole, every offered range accepted or refused with a reason; `cert.expired` is `warn`, its subject the node whose certificate expired, written once per peer per start by the refusing side and once by an expired node about itself | `protocol.md §2.3.1`, `data-dictionary.md §3.10` | **Fixed**; M20; the definition is in `§2.3.1` and the severities in `data-dictionary.md §3.10`; the audit rows are held by `test_spec_2_3_1_an_expired_node_starts_for_its_owner_and_refuses_every_channel` and `test_spec_2_3_4_revoking_a_node_writes_the_revocation_row_and_closes_its_channel` |
| 8 | `§6.1` "a client that has paired MUST filter discovery results by `cl`" — a node browsing is a client here, and nothing says its own list is filtered or what becomes of the rest | The node offers a pass to nodes whose `cl` is its own cluster and to no other; strangers are kept by ID, shown apart on the node page, and never contacted (§2.10) | `protocol.md §6.1` | **Fixed**; M20; `test_spec_6_1_peers_are_the_clusters_nodes_and_strangers_are_kept_apart_by_id` |
| 9 | `§9.2` gives the three sync routes the auth `session`, which today admits any paired device, and `§8.4` says nothing about what a node session may reach | The sync routes answer a session whose device row is active with `kind = 'node'` and nothing else — not a browser session, not the owner's loopback standing; such a session may use the sync routes, `/api/v1/health` and `/api/v1/manifest`, and every other route answers 403 (§2.4) | `protocol.md §9.2, §8.4` | **Fixed**; M21; `test_spec_9_2_sync_routes_answer_a_node_session_alone` |
| 10 | `§10.1` shows the exchange and `§9.2` the routes; neither says what `heads` answers for a device the node has never seen, what `pull`'s `after` means at zero, how a long `pull` is paged, what `push` answers, or how a node learns which apps a peer holds when its own `apps/` differs | `heads` without `app` answers `{slug: {dev: seq}}` for every slug under `data/`, mounted or not, `_sys` included, and omits devices it has never seen; `after=0` is the whole stream; `pull` answers a bounded batch of raw lines and a `next` equal to the last `seq` sent, and the client asks again until `next` equals the head; `push` answers the new head per device, or 409 naming the first line that did not follow `head + 1` with nothing of that batch written; a slug the receiver has no folder for gets `data/<slug>/log/` and is not mounted; `§2.3.1`'s definition of a completed pass says a refused range is not a completion, since a 409 means the peer's head moved and the next trigger starts from fresh heads | `protocol.md §2.3.1, §9.2, §10.1, §10.2` | **Fixed**; M21; `test_spec_10_1_heads_pull_and_push_are_a_set_union` |
| 11 | `§4.1` "MUST NOT materialize, serve or forward the lines of such a batch" against `§10.2`'s byte-for-byte copy: not forwarding a short batch leaves the receiver a permanent `seq` gap and a file that is not a prefix of the origin's; a line that is not an envelope has the same effect | Sync copies the segment as the origin holds it, short batches included. Every short-batch line carries a `seq` and counts in heads; every reader skips the batch by the same rule. A complete line without a `seq` after the last envelope travels with the next envelope; a torn partial line waits until terminated. Non-envelope lines pass through in position and count toward `api.max_body` per line. Pages never split a batch; the client bounds a page at twice `api.max_body` plus the head allowance. No byte cursor is added (§2.6) | `protocol.md §4.1, §10.2` | **Fixed**; M21; `test_spec_10_2_a_short_batch_and_a_non_envelope_line_are_copied_and_skipped_everywhere` |
| 12 | `§4.4` "reject on ingest" against `§10.2`'s "write received events to the origin's file" and `§3.1`'s "never modify": a rejected line is a permanent gap the receiver re-requests forever | A sync receiver stores the line as received; the rejection is the materializer's and is audited once as `event.rejected` (§2.6) | `protocol.md §4.4, §10.2` | **Fixed**; M21; `test_spec_4_4_a_future_dated_synced_line_is_stored_skipped_and_audited_once` |
| 13 | Nothing says what a receiver does with a foreign segment that ends mid-line; `§3.1` forbids truncating | Completed by its suffix from the origin when the torn tail is a prefix of what arrives; otherwise refused and reported with the file name and offset, nothing written (§2.6) | `protocol.md §10.2` | **Fixed**; M21; `test_spec_10_2_a_torn_foreign_segment_is_completed_by_its_suffix_never_truncated` |
| 14 | `data-dictionary.md §3.7, §3.7b, §3.8` describe local tables nothing in `local/state.jsonl` holds; `§3` shows `local/` with two files | `sys_peer`'s hints are a `peers` record in `state.jsonl` — the join URL, the owner's URLs, and for the node that admitted this one its ID and X25519 public key, which `§8.3`'s lookup accepts for a `kind = 'node'` peer while `sys_device` holds no row for it (§2.2, §2.4); endpoints and sync state are held in memory and rebuilt at start; the three sections say so and `§3`'s layout is unchanged (§2.8) | `data-dictionary.md §3.7–§3.8`, `protocol.md §3, §8.3` | **Fixed** for the hint; M20; `test_spec_2_3_1_a_node_is_admitted_by_pairing_and_receives_the_cluster_key_and_a_certificate` (the `peers` line, no secret in it); **Fixed** at M21 for lookup and in-memory state; `test_spec_8_3_either_side_can_start_the_first_pass_after_admission`, `test_spec_10_2_sync_state_is_never_an_event` |
| 15 | `§10.4` lists `kind` as six values; `data-dictionary.md §3.7b` lists ten; neither has a value for a URL the owner typed, which a join URL and a VPS's address are | One list, `data-dictionary.md §3.7b`'s plus `static`, in both places (§2.8) | `protocol.md §10.4`, `data-dictionary.md §3.7b` | **Fixed**; M21; `test_spec_10_4_candidates_are_ordered_by_last_ok_then_kind_and_bounded` |
| 16 | `§13` "Never writes to a log file for a device other than as specified in §10.2" — `§10.2` never says the receiver is the *only* such writer | It is: `log::foreign` is the one module that opens another device's file, `Writer` still refuses one, and `restore --from` copies a file rather than writing a line (§2.6) | `protocol.md §10.2` | **Fixed**; M21; `test_spec_10_2_the_receiver_is_the_only_writer_of_another_devices_file` |
| 17 | `lua-api.md §3.4` "when sync exists it fires for events arriving from other devices too" and `data-api.md §3` "including events arriving via sync" are promises in the future tense, and neither says in which VM a handler runs for a synced event | Both true; the tense changes; `lua-api.md §3.4` says the handler runs in a VM checked out by the drain that landed the event, with `pv.device()` the origin device, and never in a request's own VM | `lua-api.md §3.4`, `data-api.md §3` | **Fixed**; M21; `test_spec_lua_3_4_on_append_fires_for_synced_events_with_the_origin_device` |
| 18 | `§10.5` "A node MUST watch `data/` for externally-appeared files and re-materialize" reads as a watcher; `docs/backup-and-restore.md §2` says every file syncer works with no configuration, and with network sync on, two writers of one foreign file can exist | The watching is the stat every request and every stream ping already makes (§2.9); a folder is under file sync or network sync, never both, said in both documents with how to tell which is in use | `protocol.md §10.5`, `docs/backup-and-restore.md §2`, `docs/deployment.md §1, §2.3` | M23 |
| 19 | `§4.5` describes a replay and says nothing about an incremental apply with more than one writer | One sentence: an implementation that applies events incrementally MUST compare `(lam, ts, dev)` against the row's current winner and apply only a later event (§2.7) | `protocol.md §4.5` | M22 |
| 20 | `app-contract.md §6` says `query` reads the sandboxed connection and nothing about a segment that appeared on disk since the last request | `query` stats the app first, as a request does, so a log another machine delivered is seen (§2.9) | `app-contract.md §6` | M23 |
| 21 | `data-dictionary.md §4` `v_health` carries `unsynced peer count` with no definition, and the reference view answers NULL | The count of active `kind = 'node'` peers other than this node that have not completed a pass since this node started; NULL while `start_sync` has not run (§2.13) | `data-dictionary.md §4` | **Fixed**; M21; `test_spec_10_2_sync_state_is_never_an_event` |
| 22 | `§14` item 8 and the roadmap fix constraints for attachments and no shape | A new `protocol.md §4.7` for the blob directory, the write-verify-rename rule, the `.part` file and the reference; `blob/` in `§3`'s tree; `data-dictionary.md §2` gains `attachment` and `data-dictionary.md §3.6` gains `api.max_blob`; `data-api.md §8` the three routes; `§9.2` the three sync routes; `cli.md §5.1` `PV408` and `cli.md §7` restore copying blobs; `app-contract.md §4, §5` show `blob/` (§2.11) | those files | M25 |
| 23 | `§8.3` "A `chunk` from the client — a streamed request body — is reserved and refused in `pv/1`" — a 32 MiB blob cannot cross the channel in one `req` bounded by `api.max_body` | A `req` with `streaming: true` and no payload, then client `chunk`s in order and a client `end`; bounded by `api.max_blob` on the blob routes and `api.max_body` elsewhere (§2.11) | `protocol.md §8.3` | M25 |
| 24 | `§10.6` and `data-api.md §2` have the conditional append read "the row's events ranked past its `base`" from the log | It may read the row's current rank from the cache, provided the answer is the same, which the milestone proves (§2.7) | `protocol.md §10.6`, `data-api.md §2` | M22 |
| 25 | `apps/animals/README.md` says the live demo is "one attribute"; `docs/frameworks.md §3` has no row for the htmx SSE extension | The honest count once the file is open (M24), and the extension's row: version, size, no build step | `apps/animals/README.md`, `docs/frameworks.md §3` | M24 |
| 26 | `app-contract.md §6` `start_sync` / `sync_now` "iroh + LAN peers"; iroh is Phase 5's (`§14` item 6); `sync_now` returns `()` and a program cannot learn what a pass did | LAN peers in `pv/1 (partial: phase 3)`; `sync_now` answers a report; `Error::Unimplemented` is gone from both (§2.13) | `app-contract.md §6` | **Fixed**; M21; `test_spec_app_contract_6_start_sync_and_sync_now_are_real` |
| 27 | `§10.4` "Re-attempt on: network-change events, application foreground, and explicit user action" is written for a client with a UI; `§13`'s line bundles the timeout and the re-attempt | A node re-attempts on discovery, on the sixty-second timer and on `sync_now`; the network-change and foreground halves are a native client's (Phase 4), and the checklist line says which half a node satisfies (§2.8) | `protocol.md §10.4, §13` | **Fixed**; M21; `test_sync_wakes_an_idle_drain_and_debounces_local_appends` |
| 28 | `docs/deployment.md §2` and `docs/connectivity.md §2, §4.4` describe an always-on node in the future tense, and neither says that a VPS reached over plain HTTP from the public internet is `§7.7`'s exposure on an untrusted network | The quickstart of M26; node-to-node sync with a VPS is encrypted by `§8`; a browser pairing to it over plain HTTP across the internet carries `§7.7`'s gap, said plainly, and the certificate host that closes it is Phase 5 | `docs/deployment.md §2`, `docs/connectivity.md §2, §4.4` | M26 |
| 29 | `cli.md §1`'s example and `protocol.md`'s status line say `phase 2` | `pv/1 (partial: phase 3)` (§2.13) | `cli.md §1`, `protocol.md` status line | M26 |
| 30 | `§7.4.2` refuses a device key already in `sys_device`, active or revoked, and `§2.3.1` requires a node offline past 180 days to be re-admitted — with the key it has, since `§2.4` reserves re-initialization for a compromised key | A registered, active node key whose `sys_node.cert_expires_at` is at or before now and whose `sys_node.cluster_id` is the admitter's is re-admitted: no disposability check, no new row, a fresh certificate in `admit`, `node.admitted` saying so; a registered key that is revoked, or whose certificate has not expired, is refused as before (§2.1) | `protocol.md §2.3.1, §7.4.2` | **Fixed**; M20; `test_spec_2_3_1_an_expired_node_is_readmitted_with_its_own_key_and_no_new_row` |
| 31 | `§2.3.5` says rotation invalidates every certificate and "All devices must re-pair", and `docs/security.md §8` says a node key is re-initialized; neither says that a `sys_device` row's pins are accepted by every node that holds the row whatever cluster it was paired into, so an old cluster's rows carried by a re-founded node keep admitting old devices | The procedure is written out: on each node delete `identity/cluster.*` and restart, open `pair --node` on one, `pair --join` from the others, then revoke every device row of the old cluster from the devices page; a `pv/1` node cannot tell an old row from a new one by itself (R30) | `protocol.md §2.3.5`, `docs/security.md §8` | **Fixed**; M20; the procedure is `§2.3.5` and `docs/security.md §8`; `test_spec_7_4_2_a_revoked_nodes_key_cannot_be_admitted_again` holds the refusal it ends in |
| 32 | `data-api.md §5` promises `pv.js` "under 12 KB" and the file stands a few bytes under it; `pv.blob` and `pv.blobUrl` pass it | 16 KB, unminified and meant to be read as before (§2.11) | `data-api.md §5` | M25 |
| 33 | §2.1's direction rule decides from `disposable` alone, and an expired node that ever paired a device is not disposable — so under that rule an expired node with a paired phone could never be re-admitted: both sides established, 4403 | Both node-kind sealed messages carry `expired` beside `sig` and `disposable`; an expired side never admits; exactly one expired side is the joiner and the other must be established, which then applies row 30's registry check; two expired sides are refused; `disposable` stays honest | `protocol.md §2.3.1, §7.4.2` | **Fixed**; M20; `test_spec_2_3_1_an_expired_node_is_readmitted_with_its_own_key_and_no_new_row` (B pairs a phone before it expires), `test_spec_2_3_1_direction_rule_over_every_flag_combination` |
| 34 | `§8.4` made the owner's standing a loopback peer with a loopback `Host`, so a browser opened on the node's own LAN address from the node's keyboard was shown the pairing screen and would have paired the machine with itself (issue #46) | The owner is a request from this machine — loopback, or a peer address that is one of the node's own interface addresses — whose `Host` names this machine the same way; the `Host` rule is what defeats DNS rebinding; a peer on another machine is unchanged | `protocol.md §8.4, §9.2` | **Fixed**; M20; `test_spec_8_4_a_request_from_this_machines_own_address_is_the_owner` |
| 36 | `§10.2`'s page rule assumes a batch fits one page and `api.max_batch` bounds only the count, so `append_batch` — Lua, an embedder, or `sample/seed.jsonl` loaded whole — can write a batch no page can offer and no peer's request body can hold, which never converges | `§4.1` bounds one append at `api.max_body`, refused before anything is written (413 on the data API); `§10.2` says a range that still cannot be offered is refused for that log alone, the pass carries on with the rest and does not complete | `protocol.md §4.1, §10.2`, `data-api.md §2`, `app-contract.md §6`, `lua-api.md §3.3` | **Fixed**; M21; `test_spec_10_2_an_append_past_the_page_bound_is_refused_before_it_is_written`, `test_spec_10_2_a_log_that_cannot_be_offered_does_not_stop_the_apps_after_it` |
| 35 | `data-api.md §3` promises gap-free reconnect from a scalar Lamport mark, but a synced event can arrive below it | Live delivery includes every accepted synced event regardless of `lam`; the high-water mark never decreases. `after=` replays greater Lamport values only; clients re-read after reconnect and on `resync` | `data-api.md §3` | **Fixed**; M21; `test_spec_data_3_live_sse_delivers_below_the_resume_mark` |

Four are additions rather than corrections and deserve to be called out: **`sig`,
`disposable`, `admit` and `joined`** widen `§7.4.2` (rows 1 and 30); **`pair --node`,
`pair --join`, the `node` field of `/api/v1/pair` and `/api/v1/join`** widen `cli.md
§8` and `§9.2` (row 5); **`§4.7`, `data-api.md §8`, `api.max_blob`, `PV408` and the
three sync-blob routes** are new normative surface for attachments (row 22); and **the
client `chunk` direction** is wire meaning (row 23). Each is decided in §2.

**The rule from Phase 1 stands:** when implementation reveals a further gap, fix the spec
in the PR that found it. Do not accumulate a list and do not code around it.

---

## 4. Workspace layout — what the tree holds, and what Phase 3 adds

The tree after PR #38, with Phase 3's additions marked by milestone. Files without a
mark exist today.

```
crates/privatium-core/
├── src/
│   ├── lib.rs                Node::open, the bootstrap order, sys audit sinks, the §6 API;
│   │                         + join, renew_certificate_if_due, peers, strangers, receive,
│   │                           start_sync, sync_now, sync_events, publish_peers (M20, M21)
│   ├── identity.rs           node and cluster keys, the certificate, startup renewal, the
│   │                         X25519 and CSRF derivations; + adopt (the swap), renew, the
│   │                         expired state (M20)
│   ├── registry.rs           display name, label, revocation, the hourly mark, peer count;
│   │                         + revoke_node, the joiner's row re-assertion (M20)
│   ├── sys.rs                sys_* rows and audit kinds; + node.admitted, node.revoked,
│   │                         cert.expired, sync.peer_seen, endpoint.failover (M20, M21)
│   ├── local.rs              local/state.jsonl; + the peers record (M21)
│   ├── log/
│   │   ├── mod.rs, writer.rs, reader.rs, lamport.rs, batch.rs, envelope.rs
│   │   └── foreign.rs        the one writer of another device's file (M21)
│   ├── store/
│   │   ├── mod.rs, materialize.rs, events.rs, snapshot.rs, restore.rs, …, sys.sql
│   │   └── rank.rs           pv_rank and the rank-aware apply (M22)
│   ├── sync/
│   │   ├── mod.rs            start_sync, sync_now, Inbox, SyncHandle, SyncReport (M21)
│   │   ├── engine.rs         the pass: heads, pull, push, blobs; the peer table (M21, M25)
│   │   ├── endpoints.rs      the candidate list and failover of §10.4 (M21)
│   │   └── routes.rs         heads, pull, push, blobs, blob — answered through handle (M21, M25)
│   ├── blob/mod.rs           the store: write-verify-rename, read, list, parts (M25)
│   ├── pair/
│   │   ├── mod.rs, code.rs, spake2.rs, qr.rs
│   │   ├── handshake.rs      the six messages, both roles; + kind = "node", sig, admit (M20)
│   │   └── node.rs           the window and the handshake against it; + admission (M20)
│   ├── session/mod.rs, handshake.rs
│   ├── discover/mod.rs, txt.rs, mdns.rs, udp.rs
│   ├── wire/
│   │   ├── mod.rs            Handler, dispatch, fire_append; + the node-session confinement (M21)
│   │   ├── channel.rs        /ws and /ws/pair; + messages 7 and 8 either way, the hint in the
│   │                         lookup, client chunks (M20, M21, M25)
│   │   ├── owner.rs          the owner's acts; + /api/v1/join, the join form (M20)
│   │   ├── handoff.rs, router.rs, data.rs
│   │   └── blobs.rs          PUT, POST and GET api/blob beneath a mount (M25)
│   ├── http/
│   │   ├── auth.rs, devices.rs, pairing.rs, shell.rs, api.rs, …
│   │   └── join.rs           the join form and the node page's peers and strangers (M20)
│   ├── app/, lua/, lint/     + PV408 in lint/html.rs and lint/web.rs (M25)
│   └── backup.rs             + blobs in the plan (M25)
├── assets/shell/
│   ├── pv.js                 + blob, blobUrl (M25)
│   ├── client.js, channel.js, session.js, pair.js, htmx.min.js, vendor/noble/
└── tests/
    ├── identity.rs, pair.rs, session.rs, discover.rs, devices.rs, channel.rs, …
    ├── admission.rs          (M20)
    ├── sync.rs               the receiver, the fold, the drain — no socket (M21, M22)
    ├── filesync.rs           (M23)
    ├── blob.rs               (M25)
    └── js/pv.test.mjs        + blob (M25)
crates/privatium/
├── src/pair.rs               + --join (M20)
├── src/run.rs                + start_sync after the bind, the drain on sync_events (M21)
└── tests/
    ├── channel.rs, cli.rs, adapter.rs
    └── cluster.rs            real sockets, two and three nodes, a killable endpoint (M20–M26)
apps/animals/static/htmx-ext-sse.js, VENDOR.md   (M24)
apps/sketch/web/app.js        + save as picture (M25)
apps/_lint/{pass,fail}/PV408/ (M25)
```

`Node` is `Send` and not `Sync` (`wire/mod.rs`) and stays so. The engine, like discovery,
runs on a thread of its own and is handed what it needs — `publish_peers`, the inbox, the
`watch` channels of §2.5 — rather than reaching into the node. Nothing in Phase 3 needs a
shared node; if a milestone finds itself wanting `Arc<Mutex<Node>>` in the core, stop and
re-read §2.5.

---

## 5. Dependencies

The direct dependencies as `Cargo.lock` pins them after PR #38, and what Phase 3 changes.
Every Phase 2 addition carried its reason and its `cargo deny` result in
`docs/plans/phase-2.md §5`; nothing below re-argues them.

| Need | Crate / package | Version, licence | Phase 3 |
|---|---|---|---|
| HTTP types, streaming bodies, the WebSocket upgrade | `axum` (`tokio`, `ws`) | 0.8.9, MIT | unchanged |
| Middleware, `ServeDir` | `tower`, `tower-http` | 0.5.3, 0.7.1, MIT | unchanged |
| Runtime, channels, timers | `tokio` | 1.53.1, MIT | the engine's current-thread runtime is the `rt` feature already on; `sync` and `time` already on |
| Stream and sink helpers | `futures-core`, `futures-util` | 0.3.34, MIT OR Apache-2.0 | unchanged |
| WebSocket engine | `tokio-tungstenite` | 0.29.0, MIT | **promoted to a dependency of the core** at M20, for `join`'s client side (§2.12), and used by the engine from M21 (§2.5); the same crate `axum`'s `ws` feature already compiles, so the graph gained no package (`cargo deny check` clean at M20) |
| Interface list | `if-addrs` | 0.15.0, MIT OR BSD-3-Clause | **taken by the core** at M20 for the owner's standing of `§8.4` (row 34); the binary already listed interfaces for `--verbose`, so the graph gained no package |
| Separate IPv6 socket, interface list | `socket2`, `if-addrs` | 0.6.5, MIT OR Apache-2.0; 0.15.0, MIT OR BSD-3-Clause | unchanged |
| Lua 5.4 | `mlua` | 0.12.1, MIT | unchanged |
| SQLite | `rusqlite` | 0.40.2, MIT | unchanged; `pv_rank` is a table, not a feature |
| Signing, statics, AEAD, the PAKE's group | `ed25519-dalek`, `x25519-dalek`, `curve25519-dalek`, `chacha20poly1305` | 3.0.0, 3.0.0, 5.0.0, BSD-3-Clause; 0.11.0, Apache-2.0 OR MIT | unchanged; `sig` is `ed25519-dalek`'s `Signer` |
| Hash, MAC, KDF | `sha2`, `hmac`, `hkdf` | 0.11.0, 0.13.0, 0.13.0, MIT OR Apache-2.0 | `sha2` hashes blobs as they stream (M25); nothing new |
| CSPRNG, wiping | `rand`, `zeroize` | 0.10.2, 1.9.0, MIT OR Apache-2.0 | the cluster seed in `admit` is a `Zeroizing` buffer end to end |
| Time, paths, IDs, encoding | `jiff`, `directories`, `ulid`, `base64` | 0.2.35, Unlicense OR MIT; 6.0.0, MIT OR Apache-2.0; 3.0.0, MIT; 0.23.1, MIT OR Apache-2.0 | unchanged |
| Serde, TOML | `serde`, `serde_json`, `toml` | 1.0.229, 1.0.151, 1.1.4, MIT OR Apache-2.0 | raw lines stay bytes end to end (`§4.2`); the engine parses an envelope to read `seq` and never re-serializes |
| Lua AST for the linter | `full_moon` | 2.2.0, MPL-2.0 | `PV408` reads templates through the existing HTML synthesis |
| Errors | `thiserror`, `anyhow` | 2.0.20, 1.0.104, MIT OR Apache-2.0 | unchanged |
| Embedding | `include_dir` | 0.7.4, MIT | unchanged |
| mDNS | `mdns-sd` | 0.21.2, Apache-2.0 OR MIT | unchanged; R17 stands |
| QR | `qrcode` | 0.14.1, MIT OR Apache-2.0 | unchanged |
| Tests | `tempfile` | 3.27.0, MIT OR Apache-2.0 | unchanged |
| Windows file properties | `embed-resource` (build only) | 3.0.11 (2026-07-02), MIT | **taken by the binary**: `crates/privatium/build.rs` writes the `VS_VERSION_INFO` script from `[workspace.package]` and this crate compiles it, locating `rc.exe`, `llvm-rc` or `windres` itself. `winres` was declined — last release 0.1.12 in September 2021 — and `winresource`, its maintained fork, was not needed, because it reads a manifest of its own and the script here is generated. No RustSec advisory for any of the three; a no-op on every non-Windows target; `cargo deny check` clean with it in the graph |
| Multipart upload | `multer` | check the current release and its advisory history at M25; MIT | **decide at M25** against a bounded reader of one file part written by hand, with the crate's issue tracker open; the plan does not take it |
| htmx SSE extension | `htmx-ext-sse` | the release current at M24; licence confirmed from the `htmx-extensions` repository | vendored under `apps/animals/static/` with its own `VENDOR.md`, an app's file and not the framework's |

**Not taken, and why:** `notify` (§2.9 — the stat is the watcher); `image` (the QR
renders from the module matrix); a JSON-lines or NDJSON crate (a line is a `\n`); a
content-addressing or Merkle crate (a set union of hashes is a listing and a diff); a
retry or backoff crate (three timeouts and a table). `cargo deny check` runs in the PR
that promotes `tokio-tungstenite` and in M25's, and the result is recorded in the
milestone's status paragraph.

---

## 6. Milestones

Every milestone lists its tests by name with the spec section in the name. Each set
covers the empty input, the missing configuration, the invalid value, the boundary and
the unauthorized caller where the surface has one; the test that holds each is named in
the checklist. A checklist is ticked only on a three-platform run.

### M20 — Admission in either direction, the expired state, renewal at runtime, revocation of a node, discovery filtered to the cluster

- The window: `Pairing` gains `node: bool` (`pair --node`, *Admit a node*, `"node":
  true` in `POST /api/v1/pair`), `PairingSnapshot` reports it, and
  `Exchange::begin_with` accepts `kind: "node"` on a node window alone — a device
  window keeps refusing it with 4403, and a node window refuses every other kind the
  same way.
- `pair::handshake`: for a node client the node's sealed message adds `sig` and
  `disposable` and the client's carries `sig` and `disposable` in place of `ua`; each
  side verifies the other's `sig` over `TT` against the `pub` it holds and refuses 4403
  without it. A new `Admission` state holds both flags and decides the direction (§2.1
  item 2); `Admitter::admit(...)` seals message 7 and `Joiner::verify(ciphertext)` yields
  `ClientAdmitted { cluster_key, certificate, paired_at, lam }` — a `Zeroizing` seed,
  wiped on drop, `Debug` printing none of it — for whichever side joins;
  `Joiner::joined()` seals message 8. `pair/node.rs`: `pairing_finish` returns the
  node's decision for a node client instead of writing a row; `pairing_admit` seals
  message 7 — the one place the cluster private key ever leaves a node, and the test
  that proves a browser never gets it stays — and `pairing_admitted` writes the row
  (`kind = 'node'`, `replica = true`, `paired_via = 'lan'`, no `user_agent`) and
  `node.admitted` (alert) as one batch on message 8; `pairing_adopt` is the node side's
  adoption when the client admits. `wire/channel.rs` drives messages 7 and 8 in either
  direction within the existing pairing bound and closes 1000.
- `Node::is_disposable()` from this node's own `_sys` segments (§2.1); re-admission from
  the registry (§2.1, row 30): a registered, active node key with an expired
  `sys_node.cert_expires_at` in this cluster is re-admitted without a row.
- `identity::Identity::adopt(dir, seed, certificate)` — the crash-safe swap of §2.2 —
  and `load_or_create_at` completing a half-done swap; `Identity::founded()` reporting a
  key generated in this start, which is the only case `bootstrap_sys` writes a
  `sys_cluster` row and audits `cluster.created`; `Identity::renew(now)` writing
  `node.cert` as startup does; the expired state: `load_or_create_at` returns an identity
  whose `certificate()` is `Expired`, `Node::open` succeeds, `pairing_hello`, the `/ws`
  handshake and `discovery_facts` read the state and refuse or advertise accordingly,
  and `bootstrap_sys` audits `cert.expired` once (§2.3).
- `Node::join(&mut self, url: &str, code: &Code) -> Result<Joined>`: the client of §2.1
  over a WebSocket to `<url>/ws/pair`, run on a current-thread runtime the call creates;
  on `admit`, folds `lam`, adopts the cluster, tombstones the founded cluster's row,
  amends `sys_node`, re-asserts its own `sys_device` row from `admit`'s facts, records
  the admitter's hint in `local/state.jsonl` (ID, X25519 key, the URL), publishes the
  discovery facts, and sends `joined`. The daemon's `POST /api/v1/join` runs the same
  steps with the socket outside the lock (§2.12). `local::PeersRecord` and
  `Node::peer_hints()`, which M21's lookup reads.
- `Node::renew_certificate_if_due(&mut self, now)`: renewal at runtime with the
  amendment and the audit; called by M21 after a completed pass, by nothing here.
- Revoking a node: `Node::revoke_node(id, reason, now)` — `revoke_device` plus a
  `sys_node_revocation` row and `node.revoked` (alert); the devices page's *Revoke* on a
  `kind = 'node'` row calls it; the channel broadcast closes the node's open channel;
  the `/ws` lookup already refuses a revoked row; the client side of a handshake refuses
  a peer whose ID is in `sys_node_revocation`; a node that finds its own ID there enters
  the refused state of row 6.
- `POST /api/v1/join` in `wire/owner.rs`, the owner's alone, `application/json`, bounded
  as `/api/v1/pair`'s body is, answering `Joined` or the refusal by name; the settings
  node page's *Join a cluster* form (`http/join.rs`), `csrf()`, a URL field and a code
  field, both labelled; the devices page's *Admit a node* beside *Open pairing*;
  `privatium pair --node` and `privatium pair --join <url>` in
  `crates/privatium/src/pair.rs`, the latter prompting for the code on the terminal and
  posting it over loopback (§2.12). `tokio-tungstenite` becomes a dependency of the core
  (§5).
- `Node::peers()` and `Node::strangers()` (§2.10); the node page lists both under their
  own headings, by ID. `docs/security.md §8` gains the rotation procedure of row 31.

**Produces:** `pair::handshake::{Admission, Admitter, Joiner, ClientAdmitted}`,
`Node::{join, is_disposable, pairing_admit, pairing_admitted, pairing_adopt,
renew_certificate_if_due, revoke_node, peers, strangers, peer_hints}`,
`identity::Identity::{adopt, renew, founded, is_expired}`, `pair::Joined`,
`Pairing::node`, `local::PeersRecord`, `sys::{KIND_NODE_ADMITTED, KIND_NODE_REVOKED,
KIND_CERT_EXPIRED}`, `OwnerAction::{Join, AdmitNode}`.

**Tests** (`tests/admission.rs`; two nodes in one process, each in its own data root,
the handshake driven as data through `pair::handshake` and `Node::pairing_*`):
`test_spec_2_3_1_a_node_is_admitted_by_pairing_and_receives_the_cluster_key_and_a_certificate`,
`test_spec_2_3_1_the_established_node_admits_the_disposable_one_whichever_dialed` (the
fresh node dials the established one, and the established one dials the fresh one; the
same outcome, `admit` from the established side both times),
`test_spec_2_3_1_two_fresh_nodes_join_toward_the_window_and_two_established_ones_are_refused`,
`test_spec_2_3_1_admit_is_sent_only_after_the_joiners_signature_verifies` (a missing
`sig`, a signature by another key, and a signature over other bytes, on each side:
4403, one audited failure each, nothing sealed after it),
`test_spec_2_3_disposability_is_judged_from_this_nodes_own_segments` (a paired browser
makes a node non-disposable; a restored `_sys` full of other devices' rows does not; a
re-founded node is disposable again; an empty log is),
`test_spec_7_1_a_device_window_refuses_a_node_and_a_node_window_refuses_a_device`,
`test_spec_2_3_3_the_cluster_key_goes_to_a_node_and_never_to_a_browser`,
`test_spec_2_3_3_cluster_private_key_is_absent_from_every_event_snapshot_and_backup`
(Phase 2's test, run again over both nodes' roots after admission — the key crossed
the wire once, under `K_pair`, and landed only in `identity/`; the hint holds no
secret),
`test_spec_2_3_1_the_joiner_and_the_admitter_write_the_same_device_facts` (the two rows
materialize to the same columns whichever wins; the joiner's re-assertion outranks its
bootstrap row; the folded `lam` is past the admitter's),
`test_spec_2_3_1_a_lost_joined_leaves_no_row_and_the_join_is_retried` (the connection
cut after `admit`: no row, no audit of success, the joiner holds the key, a second
`pair --join` against a new window succeeds),
`test_spec_2_3_1_a_joined_node_writes_no_cluster_row_and_audits_no_founding`,
`test_spec_2_3_a_join_interrupted_mid_swap_completes_at_the_next_start` (each of the
three renames cut short, the node opens with the joined cluster, never the founded one,
and never a third),
`test_spec_2_3_1_an_expired_node_starts_for_its_owner_and_refuses_every_channel`
(loopback answered, `/ws` 4403 before the hello, `pair = 0`, `cert.expired` once),
`test_spec_2_3_1_an_expired_node_is_readmitted_with_its_own_key_and_no_new_row` (row
30; an unexpired registered key and a revoked one are refused),
`test_spec_2_3_1_certificate_renews_at_runtime_under_ninety_days_and_never_at_expiry`
(a fake clock at 91, 89 and 180 days),
`test_spec_2_3_4_revoking_a_node_writes_the_revocation_row_and_closes_its_channel`,
`test_spec_2_3_4_a_node_refuses_a_peer_named_in_its_revocation_table`,
`test_spec_2_3_4_a_node_that_finds_itself_revoked_stops_and_tells_the_owner`,
`test_spec_7_4_2_a_revoked_nodes_key_cannot_be_admitted_again`,
`test_spec_6_1_peers_are_the_clusters_nodes_and_strangers_are_kept_apart_by_id` (a
stranger's record with another `cl`, an invalid `cl`, this node's own registration,
and an empty table),
`test_spec_9_2_join_and_node_windows_are_the_owners_alone_and_bound_their_bodies` (a
session refused on both routes, an oversized body refused before it is read, a
malformed URL, an empty code and a `node` that is not a boolean refused by name),
`test_spec_cli_5_pv4xx_join_form_admit_button_and_node_page` (`tests/common/a11y.rs`).
In `crates/privatium/tests/cluster.rs`: `test_spec_cli_8_pair_join_admits_this_node`
(two binaries, two roots, `pair --node` on one, the code on the other's standard input,
exit 0 naming the cluster), `test_spec_cli_8_pair_join_from_the_established_node_admits_the_dialed_one`
(the Phase 3b direction), `test_spec_cli_8_pair_join_with_a_wrong_code_exits_1_without_writing_identity`,
`test_spec_cli_8_pair_node_and_join_together_are_a_usage_error`.

**Documentation:** rows 1–8, 30, 31 and the hint of row 14; `docs/deployment.md §3`
(the two commands, in either order); `docs/security.md §8` (revoking a node, the
rotation procedure); `docs/backup-and-restore.md §1` (what `identity/` holds after a
join, and the expired state); `skills/privatium-tier3-rust` (`join`);
`apps/*/README.md` where they name Phase 3.

**Acceptance checklist** — check only after the named tests pass on all three platforms:

- [ ] Admission in either direction, the key crossing once, after proof, and never to a
  browser:
  `test_spec_2_3_1_a_node_is_admitted_by_pairing_and_receives_the_cluster_key_and_a_certificate`,
  `test_spec_2_3_1_the_established_node_admits_the_disposable_one_whichever_dialed`,
  `test_spec_2_3_1_two_fresh_nodes_join_toward_the_window_and_two_established_ones_are_refused`,
  `test_spec_2_3_1_admit_is_sent_only_after_the_joiners_signature_verifies`,
  `test_spec_2_3_3_the_cluster_key_goes_to_a_node_and_never_to_a_browser`,
  `test_spec_2_3_3_cluster_private_key_is_absent_from_every_event_snapshot_and_backup`.
- [ ] Windows by kind, disposability from the node's own log, and the retry:
  `test_spec_7_1_a_device_window_refuses_a_node_and_a_node_window_refuses_a_device`,
  `test_spec_2_3_disposability_is_judged_from_this_nodes_own_segments`,
  `test_spec_2_3_1_a_lost_joined_leaves_no_row_and_the_join_is_retried`.
- [ ] The registry rows agree, the founder alone founds, and a swap a crash cannot
  spoil: `test_spec_2_3_1_the_joiner_and_the_admitter_write_the_same_device_facts`,
  `test_spec_2_3_1_a_joined_node_writes_no_cluster_row_and_audits_no_founding`,
  `test_spec_2_3_a_join_interrupted_mid_swap_completes_at_the_next_start`.
- [ ] The expired state, re-admission and renewal at runtime:
  `test_spec_2_3_1_an_expired_node_starts_for_its_owner_and_refuses_every_channel`,
  `test_spec_2_3_1_an_expired_node_is_readmitted_with_its_own_key_and_no_new_row`,
  `test_spec_2_3_1_certificate_renews_at_runtime_under_ninety_days_and_never_at_expiry`.
- [ ] Revocation of a node, both ways, and no second admission of its key:
  `test_spec_2_3_4_revoking_a_node_writes_the_revocation_row_and_closes_its_channel`,
  `test_spec_2_3_4_a_node_refuses_a_peer_named_in_its_revocation_table`,
  `test_spec_2_3_4_a_node_that_finds_itself_revoked_stops_and_tells_the_owner`,
  `test_spec_7_4_2_a_revoked_nodes_key_cannot_be_admitted_again`.
- [ ] The cluster filter and the owner's surfaces:
  `test_spec_6_1_peers_are_the_clusters_nodes_and_strangers_are_kept_apart_by_id`,
  `test_spec_9_2_join_and_node_windows_are_the_owners_alone_and_bound_their_bodies`,
  `test_spec_cli_5_pv4xx_join_form_admit_button_and_node_page`,
  `test_spec_cli_8_pair_join_admits_this_node`,
  `test_spec_cli_8_pair_join_from_the_established_node_admits_the_dialed_one`,
  `test_spec_cli_8_pair_join_with_a_wrong_code_exits_1_without_writing_identity`,
  `test_spec_cli_8_pair_node_and_join_together_are_a_usage_error`.

**Manual pass, recorded in the PR:** a second machine joined from its terminal and from
the settings form, dialing and dialed; the join form and the *Admit a node* button
keyboard-only at 200 % zoom with a screen reader on the code field; the node page's two
lists read in order.

**Implementation status, 2026-09-07, branch `m20-admission`:** implemented. Section 3
rows 1–8, 30 and 31 are fixed, row 14's hint is written, and two rows were found on the
way — 33, the `expired` flag, and 34, the owner's standing from this machine's own
address (issue #46). Four shapes differ from the sketch above, none of them wire meaning
beyond what rows 33 and 34 record. **The direction rule has three inputs per side**, not
two: `disposable`, `expired` and, on the admitting side alone, the registry's word; the
rule is `pair::handshake::direction`, pure, and unit-tested over every combination, which
is what keeps both sides from waiting for each other's `admit`. **The swap stages a
directory**: the three files go into `identity/adopt.tmp/`, that directory is renamed to
`identity/adopt/` — the one commit point — and `Identity::load_or_create_at` completes or
discards what it finds before it reads a byte, so no partial state is ambiguous. **`join`
is a trait over the node's two steps** (`pair::join::JoinNode`, `join_start_at` and
`join_apply_at`), implemented by `Node` for an embedder and by `wire::Handler` for the
daemon, whose `/api/v1/join` and *Join a cluster* form run the socket with the lock
released between them; the same two steps drive the admission tests as data, with no
socket. **A window's kind is a second method**, `pair_node(ttl)` beside `pair(ttl)`,
rather than a flag, since `spec/app-contract.md §6` names `pair(ttl)`. The label a node
offers is its display name, on both messages, so the two writers of its row agree on
every column. `sys_cluster` is written by the founder alone, where a founder is the start
that generated the key or a first bootstrap whose log holds no record of the node — the
zero-byte log a crash leaves comes back that way. `tokio-tungstenite` and `if-addrs` are
dependencies of the core (§5); `cargo deny check` passes advisories, bans, licences and
sources. Issue #48 — the sketch app's undo reversed only this tab's strokes — is an app
change in the same PR: every change a tab sees joins one history in arrival order,
batches as one, and undo reverses the latest wherever it came from
(`apps/sketch/web/history.js`, held by `sketch.test.mjs`).

**Verification on Windows (2026-09-07):**

| Command | Outcome |
|---|---|
| `cargo fmt --all --check` | clean |
| `cargo clippy --workspace --all-targets --locked -- -D warnings` | clean |
| `cargo test --workspace --locked` | 639 passed, 0 failed, 2 ignored (the two vector generators) |
| `cargo test -p privatium-core --locked --test admission` | 19 passed |
| `cargo test -p privatium --locked --test cluster` | 4 passed, two child binaries each |
| `cargo xtask header-check` | 336 files, all carry a header block |
| `cargo xtask gen-skill-reference --check` | 21 files match |
| `cargo xtask lint-spec-refs` | 37 rules, every reference resolves |
| `cargo deny check` | advisories, bans, licenses and sources ok |
| `bash .github/scripts/conformance.sh` | every named item passes, the admission and §8.4 items included |
| `node --test crates/privatium-core/tests/js/*.test.mjs` | 62 passed |
| `privatium lint apps/hello apps/animals apps/sketch` | 0 findings in 3 apps |

Two things wait on a person, as Phase 2's do. The manual pass above — a second machine
joined from its terminal and from the settings form, the join form and the *Admit a
space* button under a screen reader at 200 % zoom — is not recorded yet; the checker of
`tests/common/a11y.rs` holds the markup, and the two-binary socket tests are the
automated half of "joined from its terminal". The checklist below is ticked only on a
three-platform run of CI, which the PR that carries this paragraph requests.

---

### M21 — The sync protocol, the receiver, the engine, failover, and the routes

**Implementation status.** LAN synchronization, node-only routes, byte-preserving
reception, whole-batch paging, the bounded engine and drain, endpoint failover, and
certificate renewal after complete passes are implemented. Caches use full replay on
foreign receipt; incremental rank maintenance remains M22. The seq cursor is unchanged.
Real TCP fixtures exercise browser access on an unmet node, three peers, restart after a
power cut, route refusals, and ciphertext-only failover. The original child-binary
admission tests remain. Unit tests hold expiry, inbox ordering and revocation during a
drain. `ChannelError::Upload` names the capability rather than a phase now, so
`test_channel_refuses_a_request_chunk_naming_phase_3` is renamed
`test_channel_refuses_a_request_chunk_naming_the_capability`; the reserved client
`chunk` direction itself is unchanged and is still row 23's. Windows verification is recorded below; three-platform CI and the two-machine
human demonstration are still owed, so the acceptance boxes remain unchecked.

- `log::foreign::Receiver::open(paths, app, dev)`; `append(&mut self, bytes) ->
  Result<Head>` validating every envelope line per `§10.2` and row 11 before any is
  written, one `write_all`, one `fsync`; `complete_torn(&mut self, bytes)` per row 13;
  the segment's `head` read from its last envelope line. `Writer::check_is_ours` still
  refuses everything this module writes, and the test that says so is kept.
- `Node::receive(&mut self, app, dev, bytes) -> Result<Received>`: the receiver, then
  `AppLog::rescan` folds `lam` and the heads (`§4.3`), then a full rebuild (M22 replaces
  it with the rank-aware apply; a rebuild is correct and slow), then each line on the
  app's stream as `Append` and `pv.on('append')` through `fire_append` with the origin as
  `device`; `event.rejected` once for a future-dated line.
- The routes (`sync::routes`, through `handle`, a node session alone — row 9): `GET
  /api/v1/sync/heads[?app=]`, `GET /api/v1/sync/pull?app=&dev=&after=` as NDJSON, paged
  with `next`, byte for byte from the reader, `POST /api/v1/sync/push?app=&dev=` to the
  receiver, answering the new head or 409 naming the first line out of order (row 10).
  The node-session confinement in `wire/mod.rs` (`§8.4`).
- `sync::engine`: the pass — `_sys` first, then every slug under `data/` — heads, pull
  what the peer holds and this node lacks into the inbox, push what this node holds and
  the peer lacks; `sync.peer_seen` on the first success with a peer; the triggers of
  §2.5; `Node::renew_certificate_if_due` after a completed pass. `sync::endpoints`: the
  candidate table, ordering, timeouts, `endpoint.failover`. The `peers` record in
  `local/state.jsonl`. `publish_peers` from `Node::refresh`, from a pass, and from
  discovery's table.
- `Node::start_sync`, `sync_now -> Result<SyncReport>`, `sync_events`; the inbox drained
  by `refresh_app` and `sync_now`; `run.rs` starts sync after the bind and drains on
  `sync_events` for apps with no stream open; `v_health.unsynced_peers` filled (row 21).
- A `seq` gap in a push is refused and the range requested by the next pull, which is the
  ordinary path, not a repair.

**Produces:** `log::foreign::Receiver`, `Node::{receive, start_sync, sync_now,
sync_events, publish_peers}`, `sync::{Inbox, SyncHandle, SyncReport, PeerTable,
engine::Engine, endpoints::Candidates, routes}`, `sys::{KIND_SYNC_PEER_SEEN,
KIND_ENDPOINT_FAILOVER}`, `local::PeersRecord`.

**Tests.** `crates/privatium-core/tests/sync.rs` feeds the receiver bytes from another
root and checks raw storage, replay, recovery audits, SSE and Lua callbacks. Inbox,
health and certificate-renewal tests are unit tests in `src/sync/mod.rs`; candidate
ordering is in `src/sync/endpoints.rs`, and the public lifecycle test is in
`tests/embedded.rs`. These cover:
`test_spec_10_2_push_validates_dev_seq_app_and_envelope` (each refused, nothing written;
an empty body answers the head unchanged),
`test_spec_10_2_a_seq_gap_is_refused_and_the_range_is_pulled`,
`test_spec_10_2_received_lines_land_in_the_origin_devices_file_byte_for_byte` (digests
of the two files equal; unknown fields intact — `§4.2` across a wire),
`test_spec_10_2_a_short_batch_and_a_non_envelope_line_are_copied_and_skipped_everywhere`
(row 11; a line past the reader's bound refused),
`test_spec_10_2_the_receiver_is_the_only_writer_of_another_devices_file` (`Writer`
still refuses; `Receiver` refuses this node's own ID),
`test_spec_4_3_lamport_folds_received_events_and_stays_monotonic_across_restart`,
`test_spec_4_4_a_future_dated_synced_line_is_stored_skipped_and_audited_once`,
`test_spec_10_2_a_torn_foreign_segment_is_completed_by_its_suffix_never_truncated` (a
suffix that matches, one that does not, and an empty remainder),
`test_spec_10_2_sync_state_is_never_an_event` (no `sys_*` row for a cursor; the `peers`
record holds URLs alone),
`test_sync_inbox_is_drained_by_refresh_and_by_sync_now` (`_sys` before the app, an
empty inbox a no-op),
`test_spec_data_3_stream_carries_synced_events`,
`test_spec_lua_3_4_on_append_fires_for_synced_events_with_the_origin_device`,
`test_spec_app_contract_6_start_sync_and_sync_now_are_real` (replaces
`…_phase_2_methods_never_ok`; `sync_now` with no peer answers an empty report),
`test_spec_10_4_candidates_are_ordered_by_last_ok_then_kind_and_bounded` (an empty
list, a `kind` outside the list refused, ten seconds across all). In
`crates/privatium/tests/cluster.rs`, real sockets:
`test_spec_10_1_heads_pull_and_push_are_a_set_union` (two nodes, three apps, both
directions, digests equal),
`test_spec_9_2_sync_routes_answer_a_node_session_alone` (a browser session 403, the
owner on loopback 403, a node session admitted; a node session on `/a/hello/` 403),
`test_spec_2_3_2_a_device_pinned_to_the_cluster_reaches_an_unmet_node` (the Rust
channel client pairs with node A; `_sys` syncs to node B; the client opens `/ws` on B,
verifies B's certificate, and is admitted — and before the sync is refused),
`test_spec_10_3_power_cut_desktop_catches_up_through_the_laptop` (three nodes, one
"phone" — the Rust channel client — the desktop stopped, the phone writes to the laptop,
the desktop restarts and converges with no lost line and no duplicate),
`test_spec_10_3_offline_edits_on_both_nodes_converge`,
`test_spec_10_4_killing_the_active_endpoint_fails_over_in_under_five_seconds` (two
listeners for one peer, the first closed mid-pass, a clock on the second's first answer),
`test_spec_10_3_no_node_is_primary` (every node's app-log bytes equal after passes
started in both orders),
`test_spec_8_3_either_side_can_start_the_first_pass_after_admission` (the admitter
dials the joiner before any `_sys` has crossed and is admitted from the hint; after the
pass the row has arrived and a revocation of the admitter, synced in, is honoured over
the hint; a stranger's ID with no row and no hint is refused).

Whole-batch page boundaries and deferred no-seq tails are also held over the channel
by `test_spec_10_2_pull_keeps_batches_whole_and_defers_only_trailing_filler` and
`test_spec_10_2_short_tail_heads_and_filler_converge_by_sequence`. The receiver tests
include unmounted recovery without a local writer and a torn foreign tail across
restart. `test_spec_data_3_live_sse_delivers_below_the_resume_mark` holds row 35.

**Documentation:** rows 9–17, 21, 26, 27, 35, 36; `docs/architecture.md §6`;
`docs/deployment.md §1, §3`; `skills/privatium-tier1-lua` and `-tier2-web`
(`pv.on('append')` and the stream now carry other devices' events; the tense);
`skills/privatium-tier3-rust` (`start_sync`, `sync_now`, `subscribe` as they now behave).

**Acceptance checklist** — check only after the named tests pass on all three platforms:

- [ ] Validation, the gap, byte-for-byte copies and the one writer:
  `test_spec_10_2_push_validates_dev_seq_app_and_envelope`,
  `test_spec_10_2_a_seq_gap_is_refused_and_the_range_is_pulled`,
  `test_spec_10_2_received_lines_land_in_the_origin_devices_file_byte_for_byte`,
  `test_spec_10_2_a_short_batch_and_a_non_envelope_line_are_copied_and_skipped_everywhere`,
  `test_spec_10_2_the_receiver_is_the_only_writer_of_another_devices_file`.
- [ ] The fold, clock hygiene, the torn tail and no cursor as an event:
  `test_spec_4_3_lamport_folds_received_events_and_stays_monotonic_across_restart`,
  `test_spec_4_4_a_future_dated_synced_line_is_stored_skipped_and_audited_once`,
  `test_spec_10_2_a_torn_foreign_segment_is_completed_by_its_suffix_never_truncated`,
  `test_spec_10_2_sync_state_is_never_an_event`.
- [ ] The drain, the stream, the Lua handler and the real methods:
  `test_sync_inbox_is_drained_by_refresh_and_by_sync_now`,
  `test_spec_data_3_stream_carries_synced_events`,
  `test_spec_lua_3_4_on_append_fires_for_synced_events_with_the_origin_device`,
  `test_spec_app_contract_6_start_sync_and_sync_now_are_real`.
- [ ] The routes confined to node sessions, the union over sockets, the first pass from
  either side, the unmet node: `test_spec_9_2_sync_routes_answer_a_node_session_alone`,
  `test_spec_10_1_heads_pull_and_push_are_a_set_union`,
  `test_spec_8_3_either_side_can_start_the_first_pass_after_admission`,
  `test_spec_2_3_2_a_device_pinned_to_the_cluster_reaches_an_unmet_node`.
- [ ] No primary, the power cut, offline edits, failover under five seconds, renewal
  after a pass: `test_spec_10_3_no_node_is_primary`,
  `test_spec_10_3_power_cut_desktop_catches_up_through_the_laptop`,
  `test_spec_10_3_offline_edits_on_both_nodes_converge`,
  `test_spec_10_4_killing_the_active_endpoint_fails_over_in_under_five_seconds`,
  `test_spec_10_4_candidates_are_ordered_by_last_ok_then_kind_and_bounded`,
  `test_spec_2_3_1_certificate_renews_after_a_completed_pass`.

The roadmap's first bullet — a second node admitted with one pairing, the phone reaching
it without re-pairing — is complete here, not at M20, for the reason §2.4 gives.

**Verification on Windows (2026-09-07).** The pinned toolchain ran the Windows tests
natively. The full suite passed after fixing a scheduling race in the existing
`test_spec_4_5_a_reader_sees_a_batch_whole_or_not_at_all`: the reader now observes both
before and after the commit, while retaining the assertion that no partial batch is
visible. The final node-page prose change also passed its accessibility test.

| Command | Outcome |
| --- | --- |
| `cargo fmt --all --check` | Passed |
| `cargo clippy --workspace --all-targets --locked -- -D warnings` | Passed |
| `cargo test --workspace --locked` | 671 passed, 0 failed, 2 ignored (the two vector generators) |
| `cargo test -p privatium-core --locked --test sync` | 14 passed |
| `cargo test -p privatium --locked --test cluster -- --nocapture` | 17 passed, including the four original child-binary admission tests |
| `cargo test -p privatium-core --locked --test devices test_spec_cli_5_pv4xx_join_form_admit_button_and_node_page -- --exact` | 1 passed |
| `cargo xtask header-check` | 345 files checked |
| `cargo xtask gen-skill-reference` | Regenerated 21 reference files |
| `cargo xtask gen-skill-reference --check` | All 21 match |
| `cargo xtask lint-spec-refs` | All 37 rules resolve |
| `cargo deny check` | Advisories, bans, licenses and sources passed; duplicate-version warnings remain |
| `bash .github/scripts/conformance.sh` | Passed, including receiver, engine, lifecycle and real-channel sync tests by name |
| `target/debug/privatium --data-dir target/m21-lint-node lint apps/hello apps/animals apps/sketch` | All three apps: 0 errors, 0 warnings |
| `node --test crates/privatium-core/tests/js/*.test.mjs` | 62 passed |
| `cargo +stable check --workspace --all-targets --locked` | Passed on Windows; this does not substitute for the Linux stable job |
| `bash .github/scripts/embedded-example.sh target/m21-embedded-20260907` | Appended, queried, served, and refused a foreign Host |
| `bash .github/scripts/fresh-clone.sh target/m21-fresh-20260907` | Attempted; blocked by an existing listener on port 8420 (Windows error 10048), left running |
| `python -m unittest discover -s .github/scripts -p test_release_tools.py` | 7 passed |
| `cargo build --workspace --release --locked` | Passed |
| `target/release/privatium --version` and `target/release/privatium skill list` | Started successfully; 7 bundled skills; executable size 13,669,888 bytes |
| `python .github/scripts/release_tools.py package Windows` | Created the single-binary and portable ZIPs; `GITHUB_OUTPUT` pointed to a temporary build-output file |

Git Bash, browser-script workers, release-tool tests and the advisory fetch needed the
ordinary execution environment because the sandbox blocked their process or temporary
file access. No dependency was added. GitHub's Linux, macOS and Windows matrix has not
run for this change, and no acceptance box is checked on the strength of a local run.

The measured desktop catch-up, including full cache replay, took 241 ms (R23). First
app delivery after 1,000 system audit rows took 1.02 s (R28). These are one local
synthetic run, not performance guarantees. Failover's five-second assertion passed.
Foreign receipt still rebuilds caches; this change does not implement rank maintenance.

The node page gained explanatory text, no control. Its semantic accessibility check
passed, and the text was re-read; no additional control required a manual interaction
pass. The desktop/laptop/phone demonstration remains a person's check.

One append may write at most `api.max_body` bytes, refused before anything reaches the
log (row 36), so every batch fits a page and can be offered. A log written before that
bound, or by hand, can still hold a range no request body can carry: it is refused for
that log alone, the rest of the pass runs, and the pass does not complete, so nothing
renews on it. Neither case adds another cursor.

---

### M22 — Materialization with more than one writer

- `pv_rank(tbl, id, lam, ts, dev)` in every app cache and in `_sys`, written by
  `materialize`, `restore` and `apply`; `Store::apply_batch` takes the rank of each
  incoming event and applies it only when it is later than the row's, for a `put` and a
  `del` alike (§2.7); `Node::receive` uses it and `Node::append_batch` keeps its fast
  path; the comment in `materialize.rs` says when each path is taken.
- The data API's conditional append (`§10.6`, `base`) reads the row's rank from
  `pv_rank` rather than re-reading the log (row 24).
- Phase 1's §2.5 property test, extended: random event streams from three devices,
  interleaved and applied incrementally in arrival order, must equal a replay of the
  same logs, digest for digest, tombstones included.

**Tests** (`tests/store.rs`, `tests/sync.rs`):
`test_spec_4_5_incremental_apply_of_interleaved_devices_equals_replay`,
`test_spec_4_5_a_lower_ranked_received_event_does_not_overwrite_the_winner` (equal
`lam` broken by `ts`, equal `ts` broken by `dev`, and an event with no rank row yet),
`test_spec_4_6_a_synced_tombstone_and_a_later_put_from_another_device_resolve_by_rank`,
`test_spec_10_6_conditional_append_ranks_against_synced_events` (the answer equals the
log's, for a landed write, a moved row and a fresh one),
`test_spec_3_1_delete_cache_loses_nothing_with_three_writers`,
`test_spec_4_5_rank_table_survives_a_rebuild_and_a_schema_change`.

**Documentation:** row 19, row 24; `docs/plans/phase-1.md §2.3` is not edited — history
stays.

**Acceptance checklist** — check only after the named tests pass on all three platforms:

- [ ] Incremental equals replay with three writers:
  `test_spec_4_5_incremental_apply_of_interleaved_devices_equals_replay`,
  `test_spec_3_1_delete_cache_loses_nothing_with_three_writers`,
  `test_spec_4_5_rank_table_survives_a_rebuild_and_a_schema_change`.
- [ ] Rank decides, tombstones included:
  `test_spec_4_5_a_lower_ranked_received_event_does_not_overwrite_the_winner`,
  `test_spec_4_6_a_synced_tombstone_and_a_later_put_from_another_device_resolve_by_rank`.
- [ ] The conditional append reads the same answer from the cache:
  `test_spec_10_6_conditional_append_ranks_against_synced_events`.

R22 is measured here: a 50,000-line log's rebuild before and after `pv_rank`, recorded
in the PR.

---

### M23 — Logs that arrive by file sync

- No new machinery: the tests below prove Syncthing's shape — a foreign segment appearing
  whole by rename, a foreign segment growing, a `.syncthing.*.tmp` beside it, a segment
  that arrives torn and is completed by a later rename — converges through the existing
  stat path with `start_sync` never called (§2.9).
- `Node::query` calls `refresh_app` first, so an embedder's read sees a segment that
  appeared on disk (row 20).
- The two documents say "one or the other per folder" and how to tell which is in use
  (row 18); `§10.5`'s sentence says the watching is the stat.

**Tests** (`tests/filesync.rs`): `test_spec_10_5_copied_logs_converge_with_sync_disabled`
(two roots, files copied by hand both ways, digests equal, `start_sync` never called),
`test_spec_10_5_a_temp_file_beside_a_log_is_ignored` (a `.syncthing.*.tmp`, an editor's
backup, an empty file, a directory),
`test_spec_10_5_a_foreign_segment_that_grows_on_disk_is_applied_on_the_next_request`,
`test_spec_10_5_a_foreign_segment_that_arrives_torn_is_read_to_its_last_whole_line_and_completed_later`,
`test_spec_app_contract_6_query_sees_a_segment_that_appeared_on_disk`,
`test_spec_10_5_a_segment_named_for_this_node_that_was_not_written_by_it_is_refused`
(a file another machine synced in under this node's own ID: the writer stays poisoned,
the owner told, nothing appended — the unauthorized writer of this milestone).

**Documentation:** rows 18 and 20; `docs/backup-and-restore.md §2`; `docs/deployment.md
§1, §2.3`.

**Acceptance checklist** — check only after the named tests pass on all three platforms:

- [ ] Convergence by files alone: `test_spec_10_5_copied_logs_converge_with_sync_disabled`,
  `test_spec_10_5_a_foreign_segment_that_grows_on_disk_is_applied_on_the_next_request`,
  `test_spec_app_contract_6_query_sees_a_segment_that_appeared_on_disk`.
- [ ] What is ignored, what is completed, what is refused:
  `test_spec_10_5_a_temp_file_beside_a_log_is_ignored`,
  `test_spec_10_5_a_foreign_segment_that_arrives_torn_is_read_to_its_last_whole_line_and_completed_later`,
  `test_spec_10_5_a_segment_named_for_this_node_that_was_not_written_by_it_is_refused`.

**Manual pass, recorded in the PR:** Syncthing on two machines with `data/` shared and
sync off on both, an event written on each, both pages converging (R24).

**Roadmap wording, proposed and not applied.** The scope line "filesystem watcher for
externally-synced logs" is met by the per-request stat and not by a watcher; the
proposed line is "logs that arrive by file sync, noticed on the next request". The
acceptance bullet "Syncthing on `data/` alone produces the same convergence with sync
disabled" is reachable as written and is held by
`test_spec_10_5_copied_logs_converge_with_sync_disabled` plus the manual pass.

---

### M24 — The `animals` live demo, and the reference apps under sync

- `apps/animals`: the htmx SSE extension vendored under `static/` with its own
  `VENDOR.md` (version, hash, licence, how to reproduce it — as `alpine-csp.min.js` is
  recorded); the knowledge page's history panel gets `hx-ext="sse"
  sse-connect="<?= url('/api/stream') ?>"` on its container and `hx-trigger="sse:append"
  hx-get="<?= url('/knowledge/history') ?>"` on the panel — the README's "one attribute"
  is two, and that is the honest count (row 25). On a plain-HTTP LAN origin the
  extension's `EventSource` cannot be used, so `client.js` supplies one over the channel
  through the extension's `htmx.createEventSource` factory, which the source exposes for
  exactly this; if that hook is gone in the vendored version, `pv.js` fires `sse:append`
  on the panel itself and the attribute count is unchanged (R25).
- `apps/sketch` and `apps/hello` need nothing; the tests below run them across two nodes.
- The manual demo, recorded in the PR: desktop teaches an animal, the phone's history
  moves, no reload.

**Tests:** `test_animals_history_updates_from_a_synced_event` (through `handle`: a
received batch reaches the stream, and the panel's markup carries the trigger);
`test_reference_apps_converge_across_two_nodes` (`crates/privatium/tests/cluster.rs`:
hello's profile, animals' tree, sketch's strokes written on one node, read on the other);
`test_reference_apps_lint_clean` still passes with the vendored extension (`PV504` sees a
vendored file, not a CDN); `test_spec_cli_5_pv4xx_reference_apps` still passes with the
panel's new attributes; under `node --test`, `client.test.mjs` — the channel's event
source feeds the extension and a closed channel ends the stream without an error the
page shows.

**Documentation:** row 25; `apps/animals/README.md`, `apps/animals/SKILL.md`,
`docs/frameworks.md §3` (the SSE extension earns its row: version, size, no build step).

**Acceptance checklist** — check only after the named tests pass on all three platforms:

- [ ] The panel updates from a synced event and the apps converge:
  `test_animals_history_updates_from_a_synced_event`,
  `test_reference_apps_converge_across_two_nodes`.
- [ ] The corpus stays clean and accessible: `test_reference_apps_lint_clean`,
  `test_spec_cli_5_pv4xx_reference_apps`.
- [ ] The extension over the channel: `client.test.mjs`.

---

### M25 — Attachments

The design is §2.11; this is its checklist.

- `blob::Store::open(paths, slug)`; `write(&mut self, body: impl Stream) ->
  Result<Reference>` hashing as it streams to `.part`, renaming on match, refusing a
  declared or observed length past `api.max_blob`; `read(&self, hash) -> Result<impl
  Stream>`; `has`, `list(after)`, `remove_parts_on_start`. A hash that is not 64 lowercase
  hex characters is refused before the filesystem is touched.
- `data-dictionary.md §2` `attachment`; `store::normalize` refuses a reference of the
  wrong shape; the scaffold's control and detail rendering; `api.max_blob` in
  `data-dictionary.md §3.6`.
- `wire::blobs`: `PUT`, `POST` multipart with `next`, `GET` with the type allowlist and
  `nosniff`; beneath every mount, Tier 1 and Tier 2 alike, resolved before routes. The
  channel's client `chunk` direction (row 23) in `channel.rs` and `channel.js`, bounded
  before a byte is buffered.
- `pv.js`: `blob(file)`, `blobUrl(reference)`; the outbox does not queue a blob — a `PUT`
  that fails offline is reported and retried by the app; `spec/data-api.md §5`'s bound
  becomes 16 KB (row 32) and `pv.js` stays under it.
- Sync: the three routes and the engine's blob leg after the lines; the receiver's hash
  check; a peer's listing paged by hash.
- `restore --from` copies and verifies; `backup::Plan` lists blobs to copy and refuses a
  mismatch by name.
- `PV408` with its `pass` and `fail` fixtures under `apps/_lint/`; `tests/common/a11y.rs`
  checks `<img alt>` on rendered pages.
- `apps/sketch`: *Save as picture* — the canvas to a PNG, `pv.blob`, a `picture` event —
  and a *Pictures* list with an `<img alt>` per saved picture; the README's "how would I
  save my drawing" answered in the app that raised it.

**Tests** (`tests/blob.rs`): `test_spec_4_7_blob_is_stored_by_its_hash_and_a_mismatch_is_refused`
(an empty body is the hash of nothing and is stored; an invalid hash name refused),
`test_spec_4_7_blob_write_streams_and_a_length_past_the_limit_is_refused_before_it_is_read`
(the boundary at `api.max_blob` exactly, and one byte over),
`test_spec_4_7_a_part_file_from_a_crash_is_removed_at_start`,
`test_spec_data_8_blob_is_served_inline_for_an_allowlisted_type_or_as_a_download`
(`text/html` never inline),
`test_spec_data_8_blob_routes_refuse_a_cross_site_request_and_a_session_of_another_app`,
`test_spec_data_8_multipart_upload_redirects_to_next_with_the_reference` (two file
parts refused, `next` off-origin refused),
`test_spec_2_attachment_reference_is_validated_on_write`,
`test_spec_8_3_a_streamed_request_is_bounded_and_ended_by_the_client` (a `chunk`
without a streaming `req`, a stream past the bound, a stream never ended: each closes
the channel),
`test_spec_4_7_blobs_sync_as_a_set_union_and_a_corrupt_copy_is_refused`,
`test_spec_3_1_delete_cache_loses_nothing_with_blobs`,
`test_spec_cli_7_restore_copies_missing_blobs_and_refuses_a_mismatch`,
`test_scaffold_attachment_column_round_trips`, `test_lint_rule_pv408_passes`,
`test_lint_rule_pv408_fails`, `test_sketch_saves_a_picture_as_a_blob`; under
`node --test`: `pv.test.mjs` — `pv.blob` uploads and returns the reference, `pv.blobUrl`
builds beneath the mount, a blob is never queued; `channel.test.mjs` — the client
streams a body as chunks and ends it.

**Documentation:** rows 22, 23 and 32 in full; `docs/backup-and-restore.md §1` (`blob/` is
inside `data/`, so nothing changes for the owner — said explicitly);
`docs/architecture.md §2.1` (one paragraph: what is not text, and why it still copies);
`apps/sketch/README.md`; `skills/privatium-tier1-lua`, `-tier2-web`, `-accessibility`
(`PV408`, `alt`), `-security` (the type comes from the URL and is allowlisted).

**Acceptance checklist** — check only after the named tests pass on all three platforms:

- [ ] Storage by hash, the bound, the crash:
  `test_spec_4_7_blob_is_stored_by_its_hash_and_a_mismatch_is_refused`,
  `test_spec_4_7_blob_write_streams_and_a_length_past_the_limit_is_refused_before_it_is_read`,
  `test_spec_4_7_a_part_file_from_a_crash_is_removed_at_start`.
- [ ] The three routes and the streamed request:
  `test_spec_data_8_blob_is_served_inline_for_an_allowlisted_type_or_as_a_download`,
  `test_spec_data_8_blob_routes_refuse_a_cross_site_request_and_a_session_of_another_app`,
  `test_spec_data_8_multipart_upload_redirects_to_next_with_the_reference`,
  `test_spec_8_3_a_streamed_request_is_bounded_and_ended_by_the_client`.
- [ ] The reference, the union, the restore and the cache:
  `test_spec_2_attachment_reference_is_validated_on_write`,
  `test_spec_4_7_blobs_sync_as_a_set_union_and_a_corrupt_copy_is_refused`,
  `test_spec_3_1_delete_cache_loses_nothing_with_blobs`,
  `test_spec_cli_7_restore_copies_missing_blobs_and_refuses_a_mismatch`.
- [ ] The scaffold, the rule and the app: `test_scaffold_attachment_column_round_trips`,
  `test_lint_rule_pv408_passes`, `test_lint_rule_pv408_fails`,
  `test_sketch_saves_a_picture_as_a_blob`, `pv.test.mjs`, `channel.test.mjs`.

**Manual pass, recorded in the PR:** a picture saved on the desktop and shown on the
phone; the file input and the pictures list keyboard-only with a screen reader, every
`alt` read; 200 % zoom.

---

### M26 — Phase 3b: the always-on node, and closing the phase

- Documentation only, as the roadmap says: `docs/deployment.md §2` becomes a quickstart —
  install the binary on a VPS, `privatium` under a supervisor with the data root on the
  persistent disk, `pair --join` from the VPS with the code the home node shows, the one
  inbound port, the four-jobs table with "full node" as the only job this phase can run,
  and destroy-and-rebuild as `restore --from` a copy of `data/` plus a fresh `pair --join`
  from a re-initialized `identity/`. It says plainly that node-to-node sync with the VPS
  is encrypted by `§8`, that a phone reaching the VPS from cellular does so by a URL the
  owner typed over plain HTTP across the internet — `§7.7`'s exposure on an untrusted
  network, on every visit — and that the certificate host which closes it is Phase 5
  (row 28).
- `docs/connectivity.md §2`'s "any node → always-on machine" row and `§4.4`.
- `--version` → `pv/1 (partial: phase 3)`; `protocol.md`'s status line; `cli.md §1`'s
  example (row 29); `.github/scripts/conformance.sh` gains the Phase 3 names of §7 and
  the `§10.6` outbox line, held since Phase 1 by
  `test_spec_10_6_conditional_append_lands_conflicts_or_appends`; the roadmap's Phase 3
  and 3b boxes ticked, each naming its test; `README.md`'s status sentence.
- The acceptance bullet "nothing in the codebase distinguishes it" is a review item with
  one mechanical check: `grep -rn "primary\|leader\|authoritative" crates/*/src` finds
  only comments that forbid them, recorded in the PR.

**Tests:** `test_spec_cli_1_version_qualifies_protocol` (now `phase 3`);
`test_spec_10_3_a_node_destroyed_and_rebuilt_from_a_backup_rejoins_and_loses_nothing`
(`crates/privatium/tests/cluster.rs`: a root deleted, `data/` restored from a copy, a
fresh `identity/`, joined from the home node's side as §2.12 has a VPS joined,
converged — attachments included; the restored `_sys` rows other devices wrote do not
make the rebuilt node non-disposable);
`test_spec_2_3_4_the_destroyed_nodes_old_row_is_revoked_and_a_copy_with_its_key_is_refused`
(the owner revokes the destroyed node from the devices page; a copy that still holds
the old `identity/` is refused at every peer's `/ws` and cannot be re-admitted).

**Documentation:** rows 28 and 29; `docs/deployment.md §2`; `docs/connectivity.md §2,
§4.4`; `docs/roadmap.md` Phase 3 and 3b; `README.md`.

**Acceptance checklist** — check only after the named tests pass on all three platforms:

- [ ] The claim and the rebuild: `test_spec_cli_1_version_qualifies_protocol`,
  `test_spec_10_3_a_node_destroyed_and_rebuilt_from_a_backup_rejoins_and_loses_nothing`,
  `test_spec_2_3_4_the_destroyed_nodes_old_row_is_revoked_and_a_copy_with_its_key_is_refused`.
- [ ] The review item recorded, and the quickstart read once on a VPS by a person.

**Roadmap wording, proposed and not applied.** Phase 3b's bullet "Phone on cellular, both
home machines off, reads and writes still work" is reachable with the exposure row 28
states; the proposed addition is "over a URL the owner typed, with the plain-HTTP
exposure of `spec/protocol.md §7.7` until Phase 5's certificate host". Phase 3's bullet
"A second node is admitted with one pairing; the phone reaches it without re-pairing"
completes at M21 (§2.4), and the tick names both
`test_spec_2_3_1_a_node_is_admitted_by_pairing_and_receives_the_cluster_key_and_a_certificate`
and `test_spec_2_3_2_a_device_pinned_to_the_cluster_reaches_an_unmet_node`.

---

### M27 — Phase 3c: household profiles

Scope, decided in `docs/decisions/0007-household-profiles.md`: profiles as a partition
(D1–D3), framework-level scoping that composes with `sys_app_grant` rather than
duplicating it, with the grant gaining a `profile_id` (D2), segment directories under each
app with `_sys` exempt (D6), `usr` in the envelope (D7), PIN policy translated from `§7.5`
(D13), `[tables]` sharing declarations (D14), the ephemeral message channel as two
additions to the data API (D16), and the removal posture of D18. The ADR's §6 lists every
document each of those edits, and its §7 records that nothing is left open.

**There is no profile merge.** D9 and D11 are withdrawn. Two profiles that turn out to be
one person stay two profiles; an app that wants them together reads both. Nothing here
implements an alias, a `sys_profile_alias` table, or a union at read time.

**No node is ever told to delete data** (D18). A segment can be deleted locally; a peer
that still holds it hands it back at the next pass. Do not add a `sys_segment_removed`
marker, and do not let the interface claim a deletion is cluster-wide.

Five facts already settled, so none is relitigated:

- **`lam` stays per app; only `seq` is segmented** (D6). `seq` becomes per
  `(app, segment, device)`; `§4.3` is amended to say `lam` does not follow it, because
  `(lam, ts, dev)` has to order a `_shared` row against a private one.
- **The heads shape needs a nesting level, not a compound key.** Folding the segment into
  the outer key as `"<slug>/<segment>"` was considered and refused: `validate_destination`
  in `log/foreign.rs` admits `_sys` or a valid unreserved slug, and `§1.1`'s slug pattern
  has no `/`. Permitting one would loosen the guard that keeps a path separator out of a
  name used to build a path, which is the wrong direction for `§10.2`'s validation rule.
  `{slug: {dev: seq}}` becomes `{slug: {segment: {dev: seq}}}`.
- **The wire cost is real and already partly paid.** M21 shipped the heads shape and three
  routes keyed `?app=&dev=`; M25 adds three more. Six routes and the heads shape change
  together, or not at all.
- **`blob/` stays above the segment** (M25), so deleting a segment does not delete its
  attachments. That is the same GC question `§14` item 8 already defers.
- **The active profile is remembered per device, node-local, in `local/state.jsonl`** (D3
  part 5), never an event and never synced. A new handshake resumes it with no PIN;
  choosing a different profile asks for that profile's PIN.

**Order.** The envelope field and the segment path first, because later events depend on
them; then materialization and query scoping; then the grant's `profile_id`, the PIN
screen, the switcher under `/settings/profile` and `/api/node`; then `[tables]`; then the
ephemeral channel, which touches nothing the others touch and could equally go first.

**Two values this milestone chooses**, both left open by the ADR deliberately because
neither forecloses anything: the default rate limit for the ephemeral endpoint, which joins
`spec/data-api.md §7`'s table, and the PIN's length and character rules.

**Documentation:** the ADR's §6 table in full, `skills/` regenerated in the same change,
and this plan's §3 gaining a row per spec edit as the milestone meets it.

---

### Hardening after M26

Phase 1 needed four rounds and Phase 2 one; expect at least one here. The review reads
every path a node session can take through `auth.rs`, `wire/mod.rs`, `sync/routes.rs`
and `log/foreign.rs` against OWASP ASVS 5.0 V2, V9 and V12, the admission state machine
of §2.1 under concurrency and against `§7.5`'s limits, the blob routes against V12, the
receiver against a peer that lies about `seq`, `dev`, lengths and hashes, and the engine
against a peer that answers slowly, partially, or forever. Fix the spec in the same PR,
as always, and record the round here with its rows. M27 lands after this round and earns
its own: profile scoping is an authorization boundary in the framework's read path, the
grant's three-way resolution is where a wrong precedence shows the wrong person's rows, and
`AGENTS.md`'s fail-closed rule is what both have to be read against.

---

## 7. Conformance mapping

Phase 3 can satisfy these lines of `protocol.md §13` beyond what `docs/plans/phase-2.md
§7` claims, and `.github/scripts/conformance.sh` gains a `run` per binary naming the
tests:

| Checklist item (§13 wording) | Milestone | Test |
|---|---|---|
| Never writes to a log file for a device other than as specified in §10.2 | M21 | `test_spec_10_2_the_receiver_is_the_only_writer_of_another_devices_file` |
| Lamport counter is monotonic across restart and sync (§4.3) — the sync half | M21 | `test_spec_4_3_lamport_folds_received_events_and_stays_monotonic_across_restart` |
| Sync rejects `seq` gaps (§10.2) | M21 | `test_spec_10_2_a_seq_gap_is_refused_and_the_range_is_pulled` |
| Node certificates expire at 180 days and renew on sync (§2.3.1) — the renewal half | M20, M21 | `test_spec_2_3_1_certificate_renews_at_runtime_under_ninety_days_and_never_at_expiry`, `test_spec_2_3_1_certificate_renews_after_a_completed_pass` |
| A device pinned to a cluster trusts a node it has never met, if signed (§2.3.2) | M21 | `test_spec_2_3_2_a_device_pinned_to_the_cluster_reaches_an_unmet_node` |
| Discovery filters by TXT `cl` once paired (§6.1) | M20 | `test_spec_6_1_peers_are_the_clusters_nodes_and_strangers_are_kept_apart_by_id` |
| No node is designated primary or authoritative (§10.3) | M21 | `test_spec_10_3_no_node_is_primary`, `test_spec_10_3_power_cut_desktop_catches_up_through_the_laptop` |
| Endpoint failover uses ≤2500 ms connect timeouts (§10.4) — the timeout half; re-attempt on network change is a native client's, Phase 4 (row 27) | M21 | `test_spec_10_4_killing_the_active_endpoint_fails_over_in_under_five_seconds`, `test_spec_10_4_candidates_are_ordered_by_last_ok_then_kind_and_bounded` |
| `sys_device.replica` declared accurately (§10.7) — nodes | M20 | `test_spec_2_3_1_the_joiner_and_the_admitter_write_the_same_device_facts` |
| Cluster private key never leaves nodes; devices receive the public key only (§2.3.3) — with admission, the key crossing once | M20 | `test_spec_2_3_3_the_cluster_key_goes_to_a_node_and_never_to_a_browser`, `test_spec_2_3_3_cluster_private_key_is_absent_from_every_event_snapshot_and_backup` |
| Row-granularity LWW ordered by `(lam, ts, dev)` (§4.5) — with more than one writer | M22 | `test_spec_4_5_incremental_apply_of_interleaved_devices_equals_replay` |
| Deleting `cache/` and all `snap/` loses no data (§3.1, §5) — with attachments | M25 | `test_spec_3_1_delete_cache_loses_nothing_with_blobs` |
| Outbox replay relies on ULID idempotency, with no dedupe table (§10.6) — held since Phase 1, added to the script | M26 | `test_spec_10_6_conditional_append_lands_conflicts_or_appends` |

After M26 the unclaimed items of `§13` are exactly `§6.2`'s four pkarr lines and the
network-change half of `§10.4`: Phase 5 and Phase 4. `privatium --version` prints `pv/1
(partial: phase 3)` and must not claim conformance until they land.

**The roadmap's bullets, and the test that ticks each.** These are the acceptance
criteria; M26 ticks them in `docs/roadmap.md` naming these tests.

| `docs/roadmap.md` bullet | Milestone | Test |
|---|---|---|
| A second node is admitted with one pairing; the phone reaches it without re-pairing | M20, M21 | `test_spec_2_3_1_a_node_is_admitted_by_pairing_and_receives_the_cluster_key_and_a_certificate`, `test_spec_2_3_2_a_device_pinned_to_the_cluster_reaches_an_unmet_node` |
| Discovery filters to your own cluster on a LAN carrying a stranger's node | M20 | `test_spec_6_1_peers_are_the_clusters_nodes_and_strangers_are_kept_apart_by_id` |
| The power-cut case | M21 | `test_spec_10_3_power_cut_desktop_catches_up_through_the_laptop` |
| Edit offline on both machines, reconnect, both converge | M21 | `test_spec_10_3_offline_edits_on_both_nodes_converge` |
| Syncthing on `data/` alone produces the same convergence with sync disabled | M23 | `test_spec_10_5_copied_logs_converge_with_sync_disabled`, plus the manual pass |
| A `seq` gap is detected and repaired rather than appended | M21 | `test_spec_10_2_a_seq_gap_is_refused_and_the_range_is_pulled` |
| Lamport counters survive restart and remain monotonic | M21 | `test_spec_4_3_lamport_folds_received_events_and_stays_monotonic_across_restart` |
| Cluster private key is absent from every event, snapshot, and backup export | M20 | `test_spec_2_3_3_cluster_private_key_is_absent_from_every_event_snapshot_and_backup` |
| Killing the active endpoint fails over in under 5 seconds, not 30 | M21 | `test_spec_10_4_killing_the_active_endpoint_fails_over_in_under_five_seconds` |
| An attachment reaches every node and every restore; a mismatch is refused, never served | M25 | `test_spec_4_7_blobs_sync_as_a_set_union_and_a_corrupt_copy_is_refused`, `test_spec_cli_7_restore_copies_missing_blobs_and_refuses_a_mismatch` |
| `rm -rf cache/ data/*/snap/` then restart → identical state, attachments included | M25 | `test_spec_3_1_delete_cache_loses_nothing_with_blobs` |
| 3b: a VPS node is admitted with the same flow as a laptop | M20, M26 | `test_spec_cli_8_pair_join_from_the_established_node_admits_the_dialed_one`, and the quickstart read on a VPS |
| 3b: phone on cellular, both home machines off, reads and writes still work | M26 | manual, with the exposure row 28 states |
| 3b: destroying and rebuilding the VPS node loses nothing | M26 | `test_spec_10_3_a_node_destroyed_and_rebuilt_from_a_backup_rejoins_and_loses_nothing` |
| 3b: nothing in the codebase distinguishes it from any other node | M26 | the review item and its `grep`, recorded in the PR |

---

## 8. Risks

Numbered after `docs/plans/phase-2.md §8`, whose R17 (subtypes) and R18–R19 (the owner's
standing on the origin, the console pass) still stand.

**R20 — The inbox model.** Draining in `refresh_app` means a pulled batch waits for a
request, a ping, `sync_events` or `sync_now`. In the daemon the stream's pump and the run
loop react to `sync_events`, so latency is milliseconds; in an embedder with no traffic it
is "until you ask", which the Tier 3 skill says. If the review finds a path where a batch
can wait forever, the fix is a tick in the daemon's maintenance loop calling
`refresh_app`, not a shared node.

**R21 — Two writers of a registry row.** §2.2 makes the admitter and the joiner write
the same `sys_device` facts so that `§4.5`'s choice is invisible, and has the joiner
adopt and re-assert before it sends `joined`, so the admitter's row is never written
for a node that did not join. What remains is a joiner that adopted and then lost the
connection before `joined` arrived: it holds the cluster key with no row anywhere, and
the fix is to run `pair --join` again, which the plan says and the named test drives.
The hardening round reads the crash inside `adopt` itself.

**R22 — Rank in the cache.** `pv_rank` doubles the writes of an apply. Measure against
Phase 1's numbers in M22; if a 50,000-line log's rebuild slows by more than a third,
index `(tbl, id)` and stop there — never skip the comparison.

**R23 — Snapshots after a catch-up.** A pass that brings old events with a low `lam`
makes the newest snapshot inapplicable (`§5.3`, the cross-device case), so the next
start replays from zero until the daily pass writes a new one. Correct, audited as an
expected tier 3, and slow on a large log; M21 records the cost with the power-cut test,
and the mitigation, if one is needed, is a snapshot after a completed pass that moved a
foreign segment, not a change to `§5.3`.

**R24 — Syncthing races.** Syncthing can rewrite a foreign segment while the node reads
it. The reader's tolerance for a torn tail and the rename-on-complete Syncthing uses are
what M23 relies on; the tests reproduce the rename, not Syncthing itself, and the manual
pass runs the real thing on two machines.

**R25 — The demo.** `htmx.createEventSource` may not exist in the vendored extension. The
fallback in M24 keeps the attribute count and is decided with the file open.

**R26 — Blob size over the channel.** A 32 MiB default through the channel's frame loop is
fine on a LAN and slow on a phone's radio; Phase 5 may lower the default per transport.
The limit is a `sys_setting`, so nothing here is hard-coded, and the client `chunk`
direction is bounded before a byte is buffered.

**R27 — Two nodes in one test process.** Every socket test opens two or three data roots
and listeners; on the CI runners `cluster.rs` will be the slowest suite in the workspace.
Keep the through-`handle` tests as the bulk and the socket tests as the few named in §6;
gate nothing on wall-clock but the failover test, whose bound the spec fixes.

**R28 — `_sys` first, and audit volume.** Every pass syncs `_sys` before app data, and
`_sys` carries every node's audit rows — a pairing storm on one node delays the first
app's data on another by the size of those rows. Bounded by `§7.5`'s limits and by the
per-source rule; measured in M21 with a thousand audit rows, and if it matters the
mitigation is paging `pull` smaller for `_sys`, never skipping it.

**R29 — The VPS on plain HTTP.** Phase 3b's phone-on-cellular bullet is met over a URL
the owner typed across the public internet, where `§7.7`'s exposure is at its worst.
Row 28 says so in the quickstart; the certificate host is Phase 5, and nothing in this
phase pretends otherwise.

**R30 — The registry is a set union, and a row's pins are good everywhere.** A node that
joins brings its `_sys` segments with it, and every `sys_device` row they hold becomes a
pin every node of the cluster accepts, whatever cluster the row was paired into.
§2.1's disposability rule keeps a node's own admissions out of another cluster; it
cannot keep out rows the owner restored or synced in, and a `pv/1` node has no way to
tell an old cluster's row from a new one — `sys_device` carries no cluster and clocks
across machines are not comparable. Accepted for this phase: the rows are the owner's
own act, the rotation procedure of row 31 ends by revoking them, and a row-level
cluster binding, if one is ever wanted, is a `pv/2` question beside `§14` item 5.

---

## 9. PR sequence

| # | Branch | Depends on | Spec edits |
|---|---|---|---|
| 40 | `phase3-plan-revision` | PR #38 | none — this plan |
| 49 | `m20-admission` | PR #40 | §3 rows 1–8, 30, 31, the hint of row 14, and rows 33 and 34 found on the way; issues #46 and #48 |
| 50 | `m21-sync` | M20 | rows 9–17, 21, 26, 27, 35, 36; §2.14 records succession for a separate milestone |
| 51 | `m22-rank` | M21 | rows 19, 24 |
| 52 | `m23-filesync` | M22 | rows 18, 20 |
| 53 | `m24-animals-live` | M23 | row 25 |
| 54 | `m25-attachments` | M24 | rows 22, 23, 32 |
| 55 | `m26-always-on` | M25 | rows 28, 29; roadmap: tick Phase 3 and 3b |
| 56 | `phase3-hardening` | M26 | as found |
| 57 | `m27-profiles` | M26, and the hardening round after it | ADR 0007 §6 in full |

Cluster succession (§2.14) is a separate follow-up after M21; its PR number and final
spec rows are assigned when that milestone is planned.

The numbers from M21 on are a forecast in sequence after #49; a fix or a documentation
PR that lands between two milestones shifts them, as #43 and the two issues did before
M20.

---

Copyright © 2026 Gabriel Mongefranco
