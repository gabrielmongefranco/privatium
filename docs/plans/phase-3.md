<!--
Project:  Privatium™
File:     docs/plans/phase-3.md
Authors:  Gabriel Mongefranco (@gabrielmongefranco)
Created:  2026-09-05
Modified: 2026-09-05
Summary:  Implementation plan for Phase 3 — more than one node: cluster admission, the
          sync protocol over LAN HTTP, multi-writer materialization, externally synced
          logs, endpoint failover, attachments, and the always-on node of Phase 3b.
          Non-normative. Where this plan and spec/ disagree, spec/ wins and this file is
          wrong.
-->

# Phase 3 Implementation Plan

Target: `docs/roadmap.md` Phase 3 — *more than one node* — and Phase 3b, *the always-on
node*, which the roadmap says adds no protocol and this plan treats as the last milestone.
Deliverable: desktop and laptop stay in sync with no server, and one pairing covers both.

## 0. How to use this

Read `AGENTS.md` in full, then `docs/plans/phase-2.md` — this phase builds on its channel,
its pairing and its cluster — and `spec/protocol.md §2.3, §4.3–§4.5, §10, §13, §14`,
`spec/data-dictionary.md §3.1–§3.2, §3.7–§3.8, §3.10`, `spec/data-api.md §2–§3`,
`spec/app-contract.md §6`, `docs/backup-and-restore.md`, `docs/deployment.md`, and
`docs/decisions/0005-mobile-role.md`.

One milestone per branch, one PR per milestone, in order — M20 to M26. Named tests first;
green on all three platforms before merge. **Confirm §2 before M20.** §3 is fixed
milestone by milestone, in the PR that meets each row, with `skills/` regenerated in the
same change.

Two rules from earlier plans carry more weight here than anywhere. *One writer per log
file, forever* (`AGENTS.md` 2): a sync receiver writes another device's file, and that is
the one exception `§10.2` allows, bounded exactly as it says. *No node is primary*
(`AGENTS.md` 9): if a milestone finds itself wanting a coordinator, an election, or "the
copy that is right", stop and re-read `§10.3`.

---

## 1. Scope

### In

Admitting a node to the cluster and revoking one; certificate renewal on sync; the sync
protocol of `§10.1`–`§10.2` over the Phase 2 channel; a receiver that writes other
devices' logs byte for byte; multi-writer materialization; the endpoint candidate list and
failover of `§10.4`; logs that arrive by file sync; discovery filtered to the cluster; the
`animals` live demo; attachments (`docs/roadmap.md` Phase 3, `protocol.md §14` item 8);
`privatium pair --join`; the always-on node's documentation.

### Out — do not implement, do not stub, do not leave TODOs referencing

pkarr, DNS discovery, iroh, relays, hole punching, onion services, HTTPS, the PWA, native
shells, `uniffi`, packaging, `privatium firewall`, cluster rotation as a command (the
"nuclear option" of `§2.3.5` stays a documented procedure: delete `identity/cluster.*`,
re-found, re-admit, re-pair), field-level merge, log compaction, blob garbage collection.

### The one-sentence test

If a Phase 3 change needs a machine outside the LAN to be reachable by anything but a
URL the owner typed, it is Phase 5's.

---

## 2. Decisions this plan makes — confirm before M20

### 2.1 The sync client rides the Phase 2 channel, and the sync routes are routes of `core::handle`

`§9.2` puts `heads`, `pull` and `push` under `/api/v1/sync/` with session auth. They are
answered by `Handler::handle` like every route, so a test reaches them with no socket and
a peer reaches them through `/ws`. Node-to-node, the session is Phase 2's handshake with
both sides presenting a certificate: the client verifies the peer's against the cluster
public key it holds, and the peer verifies the client's — a node with a certificate is a
`sys_device` row of `kind = 'node'` once `_sys` has synced, and before that its
certificate alone admits it, which is `§2.3.2` working as designed. No second transport,
no second authentication.

### 2.2 The sync engine is a client of files and sockets, and reaches the node only through an inbox the node drains itself

`Node` is `Send` and not `Sync`, `start_sync` and `sync_now` take `&mut self`
(`app-contract.md §6`), and an embedder's `Node` sits in their `main` while `axum::serve`
runs (`§2.3`). A background engine therefore cannot hold the node. It does not need to:

- **Outbound** it reads this node's own log by segment length with no lock, as
  `SnapshotJob` does, and learns the per-device heads by reading each segment's last
  line — `seq` is monotonic within a file even across `§4.1`'s gaps.
- **Inbound** it never writes a log file. What it pulls from a peer it puts in an inbox
  (`sync::Inbox`, an `mpsc` channel the node holds the receiving end of) as raw lines
  tagged with app and device, and **`Node::refresh_app` drains the inbox first** — the
  same per-request stat path every request, every stream ping and `Node::query` already
  go through — so a pulled batch lands under the node's lock, is validated and appended
  by the receiver of §2.3, applied to the cache, and broadcast on the app's stream, with
  `pv.on('append')` firing for it. `sync_now(&mut self)` runs one pass to completion,
  drains, and returns what happened. A push *from* a peer arrives as a request through
  `handle` and reaches the same receiver directly.
- **Liveness** for the daemon: `Node::sync_events() -> watch::Receiver<u64>` ticks when
  the inbox gains a batch, and the stream's pump selects on it beside its 30-second ping,
  so a synced event reaches an open `/api/stream` in milliseconds rather than at the
  next ping. An embedder that never calls anything sees the batch at its next call.

The engine runs on its own thread with its own current-thread runtime, holds a clone of
what it needs — paths, the identity's session keys and certificate, the cluster public
key, the peer table — and stops when the node's `SyncHandle` drops. `start_sync` keeps
its signature.

### 2.3 The receiver stores what the origin wrote, byte for byte, and completes a torn copy by its suffix

`§10.2`: a received line lands in `data/<app>/log/<origin-dev>.jsonl`, never
re-serialized, only after `dev`, `seq = head + 1`, `app` and the envelope's shape are
checked. Two cases the spec leaves open:

- **A future-dated line** (`§4.4`) is stored anyway and skipped by the materializer, which
  already applies `§4.4` on read; the rejection is recorded once as `event.rejected`. Not
  storing it would leave a permanent `seq` gap the receiver re-requests forever, and
  editing it is forbidden. §3 row 1.
- **A foreign segment that ends mid-line** — the receiver crashed between the write and
  the disk — is completed, never cut: the next pull asks for `seq = head + 1`, the line
  that arrives is compared with the torn bytes, and if the bytes are a prefix of it the
  remainder is appended, so the file is byte-identical to the origin's. Bytes that are
  not a prefix are a corrupt copy: the app's sync for that device stops, the owner is
  told with the path and offset, and nothing is written. The same rule
  `backup::Plan::apply` follows for a log that is a prefix of a backup's. §3 row 2.

Every pushed or pulled batch is one `write_all` and an `fsync`, as this node's own
batches are.

### 2.4 A node joins with `privatium pair --join <url>`, or from the settings page

`cli.md §8` gives `pair` two flags and no way for the *joining* node to present a code.
`--join <url>` is the smallest addition to a surface `§10` keeps narrow: the command
takes the URL the admitting node printed and prompts for the code (the two words, or the
four glyph labels typed), runs `§7.4` as the client with `kind = "node"` over `/ws/pair`,
and writes what comes back. The settings node page gets the same as a form, for an owner
without a terminal. Both call `Node::join(url, code)`, which runs inside the node — the
only writer of `identity/` and `_sys` — with the root's lock held. §3 row 3.

### 2.5 What each side writes at admission

The admitting node writes the joiner's `sys_device` row (`kind = 'node'`, `replica =
true`, `paired_via = 'lan'`, both public keys) and `node.admitted` (alert, `§3.10`), and
sends over `K_pair` what `§2.3.1` lists — the cluster private key and a certificate it
signs for the joiner — plus the cluster public key and ID. The joiner writes
`identity/cluster.key` (`0600`, `create_new`), `cluster.pub`, `node.cert`, amends its own
`sys_node` with `cluster_id`, `cert`, `cert_expires_at`, and records the admitting node's
URL as its first endpoint (§2.7). It writes no `sys_cluster` row — the founder's arrives by
sync, keyed by the same ID, and two writers of one row is `§4.1`'s silent merge — and
tombstones the row of the cluster it founded at its own first start (`protocol.md §2.3`),
so one row remains. The rule for who may join: a node that has paired a device or
admitted a node refuses to join another cluster; a lone node, its founding cluster empty,
joins and discards the `identity/cluster.*` it founded. Re-founding is the documented
procedure for everything else. §3 row 4.

### 2.6 Certificates renew after any sync, by the node itself, under ninety days

`§2.3.1`: certificates "renew automatically whenever two nodes complete a sync". Every
node holds the cluster key, so after a pass that exchanged anything with a peer, a node
whose certificate has fewer than ninety days left re-issues its own, writes `node.cert`,
amends `sys_node`, and audits `cert.renewed`. An expired certificate is refused at the
handshake with `cert.expired` (warn) on the refusing side, and the node re-joins. §3 row 5.

### 2.7 Peers and endpoints: learned by discovery, remembered lightly

`sys_peer` and `sys_endpoint` (`§3.7`, `§3.7b`) are local-only. The engine keeps them in
memory: candidates from mDNS (`lan-mdns`), the UDP probe (`lan-udp`), and the join URL
(`lan-ip`), ordered by `last_ok` then kind, 2500 ms connect timeout, ten seconds across
all, `endpoint.failover` audited through the inbox when the first candidate fails and
another answers. What survives a restart is the join URL and any URL the owner typed on
the settings page, written by the node into `local/state.jsonl` as a `peers` record —
"cached hints; may be stale, never authoritative", which is what `§3.7` calls them.
`sys_sync_state` is not persisted at all: `their_seq` is one `heads` request away and
`our_seq` is the file. §3 row 6 says so.

Sync runs at start, when discovery sees a cluster peer, every sixty seconds while a peer
answers, and one second after any local append (debounced) — the last is what makes the
`animals` demo live. No timer is the only trigger (`§10.4`).

### 2.8 The cache learns rank, so a received event is applied only if it wins

`store::materialize::apply` overwrites blindly because this node's own event always has
the highest `lam` — and its comment names Phase 3 as the moment that stops being true. M22
gives the cache a `pv_rank(tbl, id, lam, ts, dev)` table kept by every rebuild and every
apply, and an apply becomes: read the row's rank, compare `(lam, ts, dev)`, write only if
the incoming event is later. Phase 1's §2.5 property — incremental equals replay — is
extended to interleaved multi-device streams and stays the definition.

### 2.9 No filesystem watcher; the stat is the watcher

The roadmap names "a filesystem watcher for externally-synced logs". Phase 1 (M8, M9)
decided that a stat per request, plus the stream's ping, notices any segment that grew or
appeared — `Store::take_inputs` lists `log/*.jsonl` each time, `refresh_app` rescans the
log when it moved, and the reader ignores a `.tmp` beside a segment. A log Syncthing
delivers is therefore applied on the next request or ping with no new machinery, and M23
proves it. `notify` stays out; `deny.toml`'s allowance for its licence stays as a
harmless line.

One rule the documents must state: a folder under file sync **or** network sync, not
both. Two writers of one foreign segment — the engine and Syncthing — is the conflict the
single-writer rule exists to prevent.

### 2.10 Attachments: immutable, content-addressed files beside the log

`docs/roadmap.md` Phase 3 and `protocol.md §14` item 8 fix the constraints; this is the
shape M25 builds, and the largest decision in this plan.

- **Storage.** `data/<slug>/blob/<sha256 hex>`, 64 lowercase hex characters, written to
  `<hash>.part` and renamed when the hash of what was written matches the name, then the
  directory flushed (`durable::sync_dir`). Never modified, never deleted in `pv/1`; the
  `.part` of a crashed write is removed on the next start. Not in snapshots — a snapshot
  is a cache of tables, and a blob is already immutable and self-verifying. In the backup
  by being under `data/`.
- **Reference.** A JSON object in `d`: `{"sha256":"<hex>","type":"image/png","bytes":1234,
  "name":"receipt.png"}`. `data-dictionary.md §2` gains the logical type `attachment` —
  declared `JSON`, stored as text, the scaffold's control `input[type=file]` — and typed
  writes refuse a value that is not that shape. Presence is never checked on write: the
  blob may arrive after the event, or before it.
- **Data API.** `PUT <mount>api/blob`, body the bytes, `Content-Length` required,
  streamed to `.part` and hashed as it streams — the request-body streaming ADR 0003 paid
  for — refused past `api.max_blob` (32 MiB by default, `§3.6`) before a byte is read
  when the length says so; answers the reference. `GET <mount>api/blob/<hex>?type=<mime>`
  serves the bytes as `application/octet-stream` with `Content-Disposition: attachment`
  unless `type` is one of `image/*`, `audio/*`, `video/*`, `application/pdf`,
  `text/plain`, in which case inline under `nosniff`; the type is the URL's, never stored,
  so nothing on disk is trusted for it. `POST <mount>api/blob` as `multipart/form-data`
  with one file part and a `next` field answers 303 to `next` with `blob`, `type`,
  `bytes` and `name` in the query — the no-JavaScript path for a Tier 1 form, whose
  handler then writes the event. `pv.js` gains `pv.blob(file) -> reference` and
  `pv.blobUrl(reference)`.
- **Sync.** After the lines of an app, the engine lists (`GET /api/v1/sync/blobs?app=`,
  the hashes a peer holds, paged) and fetches what it lacks (`GET
  /api/v1/sync/blob?app=&sha256=`) into the inbox, and pushes what the peer lacks (`PUT
  /api/v1/sync/blob?app=&sha256=`); the receiver hashes every blob it stores and refuses
  a mismatch, naming the peer. A set union of hashes, exactly as the logs are a set union
  of `(dev, seq)`.
- **Restore.** `restore --from` copies blobs this node lacks, verifying each; a
  mismatch refuses that blob and names it, and the rest proceed. `rm -rf cache/ snap/`
  loses nothing, blobs included, and the roadmap bullet holds by the existing test with
  a blob added.
- **Lint and accessibility.** `PV409`: every `<img>` in a template, and every one a Tier
  2 page ships, has `alt` (WCAG 1.1.1) — needed the day pictures appear. The scaffold
  emits a file input on its own form and an `<img alt>` or a download link on the detail
  page.
- **Not decided, deliberately:** garbage collection of blobs no event references, and a
  size quota per app. `§14` item 8 becomes those two questions.

### 2.11 What `--version` claims, and what the four methods do

`privatium --version` prints `pv/1 (partial: phase 3)` from M26; `start_sync` and
`sync_now` are real from M21 and `test_spec_app_contract_6_phase_2_methods_never_ok` is
retired for a test that they do what `§6` says. The remaining `§13` items are all Phase
5's (§7).

---

## 3. Spec gaps found — fixed in the milestone that meets them

| # | Was | Proposed | Files | Milestone |
|---|---|---|---|---|
| 1 | `§4.4` "reject on ingest" against `§10.2`'s "write received events to the origin's file" and `§3.1`'s "never modify" — a rejected line is a permanent gap | A sync receiver stores the line as received; rejection is the materializer's and is recorded once (§2.3) | `protocol.md §4.4, §10.2` | M21 |
| 2 | Nothing says what a receiver does with a foreign segment that ends mid-line; `§3.1` forbids truncating | Completed by its suffix from the origin, or refused and reported (§2.3) | `protocol.md §10.2` | M21 |
| 3 | `cli.md §8` has no way for a joining node to present a code | `pair --join <url>`, prompting for the code; the settings form | `cli.md §8` | M20 |
| 4 | `§2.3.1` says what the admitting node sends and not what either side writes | §2.5 | `protocol.md §2.3.1`, `data-dictionary.md §3.1, §3.1b, §3.2` | M20 |
| 5 | `§2.3.1` "renew automatically whenever two nodes complete a sync" — by whom, when, from what age | §2.6 | `protocol.md §2.3.1` | M20 |
| 6 | `§3.7`, `§3.7b`, `§3.8` describe local tables nothing in `local/state.jsonl` holds | `sys_peer` hints are a `peers` record in `state.jsonl`; endpoints and sync state are held in memory and rebuilt at start; the three sections say so | `data-dictionary.md §3.7–§3.8`, `protocol.md §3` | M21 |
| 7 | `§10.4` lists `kind` as six values; `§3.7b` lists ten | One list, `§3.7b`'s, in both places | `protocol.md §10.4`, `data-dictionary.md §3.7b` | M21 |
| 8 | `§10.1` shows the exchange and `§9.2` the routes; neither says what `heads` returns for a device the node has never seen, what `pull`'s `after` means at zero, or what `push` answers | `heads` omits unknown devices; `after=0` is the whole stream; `push` answers the new head per device and a 409 naming the first line that did not follow `head + 1`, with nothing of that batch written | `protocol.md §9.2, §10.1, §10.2` | M21 |
| 9 | `lua-api.md §3.4` "when sync exists it fires for events arriving from other devices too" and `data-api.md §3` "including events arriving via sync" are promises | Both true; the tense changes, and `§3.4` says the handler runs in a VM the receiving request checked out | `lua-api.md §3.4`, `data-api.md §3` | M21 |
| 10 | `§14` item 8 and the roadmap fix constraints for attachments and no shape | A new `protocol.md §4.7` for the blob directory and the reference; `data-dictionary.md §2` gains `attachment`; `data-api.md §8` the three routes and `api.max_blob`; `§9.2` the three sync routes; `cli.md §5.1` `PV409`; `app-contract.md §4` and `§5` show `blob/` | those files | M25 |
| 11 | `§13` "Never writes to a log file for a device other than as specified in §10.2" — `§10.2` never says the receiver is the *only* such writer | It is; `log::foreign` is the one module that opens another device's file, and `Writer` still refuses one | `protocol.md §10.2` | M21 |
| 12 | `§6.1` "a client that has paired MUST filter discovery results by `cl`" — a node is a client here and nothing says its own browse list is filtered | The node's browse filters to its cluster for sync and shows strangers separately on the settings page, by ID | `protocol.md §6.1` | M20 |
| 13 | `docs/backup-and-restore.md §2` says every file syncer works with no Privatium configuration; with network sync on, two writers of one foreign file can exist | One or the other per folder (§2.9), said in that section and in `docs/deployment.md §1` | `docs/backup-and-restore.md`, `docs/deployment.md` | M23 |

Additions to call out: **`pair --join`** widens `cli.md §8`; **`§4.7`, `data-api.md §8`
and the three sync-blob routes** are new normative surface for attachments. Reject either
and §2.4 or §2.10 needs rewriting.

---

## 4. Workspace layout — what Phase 3 adds

```
crates/privatium-core/
├── src/
│   ├── log/foreign.rs        the one writer of another device's file (M21)
│   ├── sync/
│   │   ├── mod.rs            start_sync, sync_now, the inbox, SyncHandle (M21)
│   │   ├── engine.rs         the pass: heads, pull, push, blobs; the peer table (M21, M25)
│   │   ├── endpoints.rs      the candidate list and failover of §10.4 (M21)
│   │   └── routes.rs         heads, pull, push, blobs, blob — through handle (M21, M25)
│   ├── blob/mod.rs           the store: write-verify-rename, read, list (M25)
│   ├── wire/blobs.rs         PUT/POST/GET api/blob beneath a mount (M25)
│   ├── store/rank.rs         pv_rank and the rank-aware apply (M22)
│   ├── identity.rs           + join, renewal (M20)
│   └── http/join.rs          the settings join form and the node page's peers (M20)
├── assets/shell/pv.js        + blob, blobUrl (M25)
└── tests/
    ├── admission.rs          (M20)
    ├── sync.rs               through handle, two nodes in one process (M21, M22)
    ├── filesync.rs           (M23)
    ├── blob.rs               (M25)
    └── js/pv.test.mjs        + blob (M25)
crates/privatium/
├── src/pair.rs               + --join (M20)
└── tests/cluster.rs          real sockets, two and three nodes, a killable endpoint (M20, M21, M24)
apps/animals/static/htmx-ext-sse.js, VENDOR.md   (M24)
apps/sketch/web/app.js        + save as picture (M25)
apps/hello/schema.sql         + an attachment column, if the scaffold demo lives there (M25)
```

---

## 5. Dependencies

| Need | Crate / package | Version, licence | Note |
|---|---|---|---|
| WebSocket client in the core | `tokio-tungstenite` | the version axum's `ws` feature already pins (0.29.0), MIT | The engine's client side; the same crate the server side already uses |
| Multipart upload | `multer` | check the current release at M25; MIT | One file part, streamed; decide at M25 against writing a bounded reader by hand, with the crate's advisory history in front of you |
| htmx SSE | `htmx-ext-sse` | 2.2.4 | Vendored under `apps/animals/static/`, an app's own file with its own `VENDOR.md`; licence confirmed from the `htmx-extensions` repository at M24 |

Nothing else. `sha2` hashes blobs; `serde_json` reads references; the channel, the session
and discovery are Phase 2's. `notify` is not taken (§2.9).

---

## 6. Milestones

### M20 — Admission, certificates on sync, revocation of a node, discovery filtered to the cluster

- `Node::join(&mut self, url: &str, code: &Code) -> Result<Joined>`: the client of
  `§7.4` with `kind = "node"` over `/ws/pair`, writing what §2.5 lists; refused if this
  node already has a cluster. `identity::Identity::adopt_cluster(key, cert)`.
- The admitting side of `pair::handshake` accepts `kind = "node"`: writes the row and the
  alert, issues the certificate, sends the cluster key over `K_pair` — the one place that
  key ever leaves a node, and the test that proves a browser never gets it stays.
- `privatium pair --join <url>` (§2.4), prompting on the terminal for the words or the
  glyph labels; the settings node page's *Join a cluster* form.
- Renewal (§2.6): `Node::renew_certificate_if_due(&mut self, now)` called by the engine's
  inbox after a pass that exchanged anything; `cert.renewed`, `cert.expired`.
- Revoking a node: the devices page's *Revoke* on a `kind = 'node'` row also writes
  `sys_node_revocation` (`§3.1c`); `node.revoked` audit; a handshake from a revoked node
  is refused once the revocation has synced, and a node refuses a peer whose ID is in
  its `sys_node_revocation`.
- Discovery: `Node::discovered()` splits into `peers()` — `cl` equal to this cluster — and
  `strangers()`, both by ID (§3 row 12); the node page shows both.

**Produces:** `Node::{join, peers, strangers, renew_certificate_if_due}`,
`identity::Identity::adopt_cluster`, `pair::Joined`, `sys::{KIND_NODE_ADMITTED,
KIND_NODE_REVOKED, KIND_CERT_EXPIRED}`.

**Tests** (`tests/admission.rs`; two nodes in one process, each in its own data root,
the handshake driven as data):
`test_spec_2_3_1_a_node_is_admitted_by_pairing_and_receives_the_cluster_key_and_a_certificate`,
`test_spec_2_3_3_the_cluster_key_goes_to_a_node_and_never_to_a_browser`,
`test_spec_2_3_3_cluster_private_key_is_absent_from_every_event_snapshot_and_backup`
(Phase 2's test, run again over both nodes' roots after admission and a sync — the key
crossed the wire once, under `K_pair`, and landed only in `identity/`),
`test_spec_2_3_1_a_joined_node_writes_no_second_cluster_row`,
`test_spec_2_3_a_node_in_a_cluster_refuses_to_join_another`,
`test_spec_2_3_1_certificate_renews_after_a_sync_and_an_expired_one_is_refused`,
`test_spec_2_3_4_a_revoked_node_is_refused_after_the_revocation_syncs`,
`test_spec_3_1c_node_revocation_is_replicated`,
`test_spec_6_1_browse_filters_to_the_cluster_and_keys_on_id` (a stranger's TXT with
another `cl`). In `crates/privatium/tests/cluster.rs`:
`test_spec_cli_8_pair_join_admits_this_node` (two binaries, two roots, the code passed on
standard input), `test_spec_2_3_2_a_device_pinned_to_the_cluster_reaches_an_unmet_node`
(the Rust client of Phase 2's channel test, paired with node A, opens `/ws` on node B and
is admitted on B's certificate alone).

**Documentation:** rows 3, 4, 5, 12; `docs/deployment.md §3`; `docs/security.md §8`;
`docs/backup-and-restore.md §1` (what `identity/` now holds).

---

### M21 — The sync protocol, the receiver, the engine, and failover

- `log::foreign::Receiver::open(paths, app, dev) -> Receiver`; `append(&mut self, lines:
  &[Bytes]) -> Result<Head>` validating every line per `§10.2` before any is written,
  one `write_all`, one `fsync`; `complete_torn(&mut self, line)` per §2.3; the segment's
  `head` read from its last line. `Writer::check_is_ours` still refuses everything this
  module writes, and the test that says so is kept.
- `Node::receive(&mut self, app, dev, lines) -> Result<Received>` — the receiver, then
  `AppLog::rescan` folds `lam` and heads (`§4.3`), then the rank-aware apply of M22 (until
  M22 lands: a full rebuild, which is correct and slow), then each line on the app's
  stream as `Append`, then `pv.on('append')` through the same `fire_append` the seed uses,
  with `device` the origin. `event.rejected` once for a future-dated line.
- The routes (`sync::routes`, through `handle`, session `kind = 'node'` only): `GET
  /api/v1/sync/heads?app=` → `{dev: head}` for every segment; `GET
  /api/v1/sync/pull?app=&dev=&after=` → NDJSON, the lines with `seq > after` from the
  reader, byte for byte, skipping a short batch; `POST /api/v1/sync/push` → the receiver,
  answering the new heads or 409 naming the first line out of order (§3 row 8).
- `sync::engine`: a pass per peer per app — heads, pull what they have and we lack into
  the inbox, push what we have and they lack — over a channel session to the peer's
  `/ws`; `sync.peer_seen` on the first success. The inbox and `Node::refresh_app`
  draining it (§2.2); `Node::sync_events()`. Triggers per §2.7.
- `sync::endpoints`: the candidate table, ordering, timeouts, `endpoint.failover`.
- `Node::start_sync(&mut self) -> Result<()>` and `Node::sync_now(&mut self) ->
  Result<SyncReport>`; `SyncHandle` dropped by `close`.
- A `seq` gap in a push is refused and the range requested by the next pull, which is
  the ordinary path, not a repair.
- `local/state.jsonl` gains the `peers` record (§3 row 6).

**Produces:** `log::foreign::Receiver`, `Node::{receive, start_sync, sync_now,
sync_events}`, `sync::{Inbox, SyncHandle, SyncReport, engine::Engine,
endpoints::Candidates}`, `sys::{KIND_SYNC_PEER_SEEN, KIND_ENDPOINT_FAILOVER}`.

**Tests** (`tests/sync.rs`; two nodes in one process, their handlers wired socket to
socket by a test-only pair of channels, or three where named):
`test_spec_10_1_heads_pull_and_push_are_a_set_union`,
`test_spec_10_2_push_validates_dev_seq_app_and_envelope` (each refused, nothing written),
`test_spec_10_2_a_seq_gap_is_refused_and_the_range_is_pulled`,
`test_spec_10_2_received_lines_land_in_the_origin_devices_file_byte_for_byte` (digests of
the two files equal; unknown fields intact — `§4.2` across a wire),
`test_spec_10_2_the_receiver_is_the_only_writer_of_another_devices_file` (`Writer` still
refuses; `Receiver` refuses this node's own ID),
`test_spec_4_3_lamport_folds_received_events_and_stays_monotonic_across_restart`,
`test_spec_4_4_a_future_dated_synced_line_is_stored_skipped_and_audited_once`,
`test_spec_10_2_a_torn_foreign_segment_is_completed_by_its_suffix_never_truncated`,
`test_spec_10_2_sync_state_is_never_an_event` (no `sys_*` row for a cursor),
`test_sync_inbox_is_drained_by_refresh_and_by_sync_now`,
`test_spec_data_3_stream_carries_synced_events`,
`test_spec_lua_3_4_on_append_fires_for_synced_events`,
`test_spec_app_contract_6_start_sync_and_sync_now_are_real` (replaces
`…_phase_2_methods_never_ok`). In `crates/privatium/tests/cluster.rs`:
`test_spec_10_3_power_cut_desktop_catches_up_through_the_laptop` — three nodes, one
"phone" (the Rust channel client), the desktop stopped, the phone writes to the laptop,
the desktop restarts and converges with no lost line and no duplicate;
`test_spec_10_3_offline_edits_on_both_nodes_converge`;
`test_spec_10_4_killing_the_active_endpoint_fails_over_in_under_five_seconds` (two
listeners for one peer, the first closed mid-pass, a clock on the second's first answer);
`test_spec_10_3_no_node_is_primary` (every node's `data/` digests equal after the passes,
whichever started first).

**Documentation:** rows 1, 2, 6, 7, 8, 9, 11; `docs/architecture.md §6`; `docs/deployment.md
§1, §3`; `skills/privatium-tier1-lua` and `-tier2-web` (`pv.on('append')` and the stream
now carry other devices' events; the tense in the skills' text);
`skills/privatium-tier3-rust` (`start_sync`, `sync_now`, `subscribe` as they now behave).

---

### M22 — Materialization with more than one writer

- `pv_rank(tbl, id, lam, ts, dev)` in every app cache, written by `materialize`, `restore`
  and `apply`; `Store::apply_batch` takes the rank of each incoming event and applies it
  only when it is later than the row's, for a `put` and a `del` alike (§2.8).
- `Node::receive` uses it; `Node::append_batch` keeps its fast path, since this node's
  own event is always the latest — the comment in `materialize.rs` is rewritten to say
  when each path is taken.
- The data API's conditional append (`§10.6`, `base`) reads the row's rank from `pv_rank`
  rather than re-reading the log, with the property that the answer is the same.
- Phase 1's §2.5 property test, extended: random event streams from three devices,
  interleaved and applied incrementally in arrival order, must equal a replay of the same
  logs, digest for digest, tombstones included.

**Tests** (`tests/store.rs`, `tests/sync.rs`):
`test_spec_4_5_incremental_apply_of_interleaved_devices_equals_replay`,
`test_spec_4_5_a_lower_ranked_received_event_does_not_overwrite_the_winner`,
`test_spec_4_6_a_synced_tombstone_and_a_later_put_from_another_device_resolve_by_rank`,
`test_spec_10_6_conditional_append_ranks_against_synced_events`,
`test_spec_3_1_delete_cache_loses_nothing_with_three_writers`.

**Documentation:** `protocol.md §4.5` gains a sentence that an incremental apply MUST
compare rank; `docs/plans/phase-1.md §2.3` is not edited — history stays.

---

### M23 — Logs that arrive by file sync

- No code beyond what M21 needs: the tests below prove Syncthing's shape — a foreign
  segment appearing whole by rename, a foreign segment growing, a `.syncthing.*.tmp` beside
  it — converges through the existing stat path with `start_sync` never called.
- `Node::query` calls `refresh_app` first, so an embedder's read sees a segment that
  appeared on disk.
- The two documents say "one or the other per folder" (§2.9, §3 row 13) and how to tell
  which is in use: the node page names the peers it syncs with, and a folder with a
  `.stfolder` is Syncthing's.

**Tests** (`tests/filesync.rs`): `test_syncthing_copied_logs_converge_with_sync_disabled`
(two roots, files copied by hand both ways, digests equal),
`test_a_temp_file_beside_a_log_is_ignored`,
`test_a_foreign_segment_that_grows_on_disk_is_applied_on_the_next_request`,
`test_spec_app_contract_6_query_sees_a_segment_that_appeared_on_disk`.

**Documentation:** `docs/backup-and-restore.md §2`, `docs/deployment.md §1, §2.3`.

---

### M24 — The `animals` live demo, and the reference apps under sync

- `apps/animals`: the SSE extension vendored under `static/` with a `VENDOR.md`; the
  history panel gets `hx-ext="sse" sse-connect="<?= url('/api/stream') ?>"` on its
  container and `hx-trigger="sse:append" hx-get="<?= url('/history') ?>"` on the panel —
  the roadmap's "one attribute" is two, and that is the honest count. On a plain-HTTP LAN
  origin the extension's `EventSource` cannot be used, so `client.js` supplies one over
  the channel through the extension's `htmx.createEventSource` factory, which the source
  exposes for exactly this; if that hook is gone in the vendored version, `pv.js` fires
  `sse:append` on the panel itself, and the attribute count is unchanged.
- `apps/sketch` and `apps/hello` need nothing; the tests below run them across two nodes.
- The manual demo, recorded in the PR: desktop teaches an animal, the phone's history
  moves, no reload.

**Tests:** `test_animals_history_updates_from_a_synced_event` (through `handle`: a
received batch reaches the stream, and the panel's markup carries the trigger);
`test_reference_apps_converge_across_two_nodes` (`crates/privatium/tests/cluster.rs`:
hello's profile, animals' tree, sketch's strokes written on one node, read on the other);
`test_reference_apps_lint_clean` still passes with the vendored extension (`PV504` sees a
vendored file, not a CDN).

**Documentation:** `apps/animals/README.md`, `apps/animals/SKILL.md`, `docs/frameworks.md
§3` (the SSE extension earns its row: version, size, no build step).

---

### M25 — Attachments

The design is §2.10; this is its checklist.

- `blob::Store::open(paths, slug)`; `write(&mut self, body: impl Stream) -> Result<Reference>`
  hashing as it streams to `.part`, renaming on match, refusing a declared or observed
  length past `api.max_blob`; `read(&self, hash) -> Result<impl Stream>`; `has`, `list`,
  `remove_parts_on_start`.
- `data-dictionary.md §2` `attachment`; `store::normalize` refuses a reference of the
  wrong shape; the scaffold's control and detail rendering.
- `wire::blobs`: `PUT`, `POST` multipart with `next`, `GET` with the type whitelist and
  `nosniff`; beneath every mount, Tier 1 and Tier 2 alike, resolved before routes.
- `pv.js`: `blob(file)`, `blobUrl(reference)`; the outbox does not queue a blob — a `PUT`
  that fails offline is reported and retried by the app, since a file is not an event.
- Sync: the three routes and the engine's blob leg after the lines; the receiver's hash
  check.
- `restore --from` copies and verifies; `backup::Plan` lists blobs to copy.
- `PV409` with its `pass` and `fail` fixtures; `tests/common/a11y.rs` checks `<img alt>`
  on rendered pages.
- `apps/sketch`: *Save as picture* — the canvas to a PNG, `pv.blob`, a `picture` event —
  and a *Pictures* list with an `<img alt>` per saved picture; the README's "how would I
  save my drawing" answered in the app that raised it.

**Tests** (`tests/blob.rs`): `test_spec_4_7_blob_is_stored_by_its_hash_and_a_mismatch_is_refused`,
`test_spec_4_7_blob_write_streams_and_a_length_past_the_limit_is_refused_before_it_is_read`,
`test_spec_4_7_a_part_file_from_a_crash_is_removed_at_start`,
`test_spec_data_8_blob_is_served_inline_for_a_whitelisted_type_or_as_a_download`,
`test_spec_data_8_multipart_upload_redirects_to_next_with_the_reference`,
`test_spec_2_attachment_reference_is_validated_on_write`,
`test_spec_4_7_blobs_sync_as_a_set_union_and_a_corrupt_copy_is_refused`,
`test_spec_3_1_delete_cache_loses_nothing_with_blobs`,
`test_spec_cli_7_restore_copies_missing_blobs_and_refuses_a_mismatch`,
`test_scaffold_attachment_column_round_trips`, `test_lint_rule_pv409_passes`,
`test_lint_rule_pv409_fails`, `test_sketch_saves_a_picture_as_a_blob`; under
`node --test`: `pv.blob uploads and returns the reference`, `pv.blobUrl builds beneath the
mount`.

**Documentation:** row 10 in full; `docs/backup-and-restore.md §1` (`blob/` is inside
`data/`, so nothing changes for the owner — said explicitly); `docs/architecture.md §2.1`
(one paragraph: what is not text, and why it still copies); `apps/sketch/README.md`;
`skills/privatium-tier1-lua`, `-tier2-web`, `-accessibility` (`PV409`, `alt`),
`-security` (the type comes from the URL and is whitelisted).

---

### M26 — Phase 3b: the always-on node, and closing the phase

- Documentation only, as the roadmap says: `docs/deployment.md §2` becomes a quickstart —
  install the binary on a VPS, `privatium` under a supervisor with the data root on the
  persistent disk, `pair --join` from the VPS with the code the home node shows, the one
  inbound port, and the sentence that a plain-HTTP VPS is `§7.7`'s gap on the open
  internet until Phase 5's certificate host; the four jobs table with "full node" as the
  only one this phase can run; destroy-and-rebuild as `restore --from` a copy of `data/`
  and a fresh `pair --join`.
- `docs/connectivity.md §2`'s "any node → always-on machine" row and `§4.4`.
- `--version` → `pv/1 (partial: phase 3)`; `protocol.md`'s status line;
  `.github/scripts/conformance.sh` gains the Phase 3 names of §7; the roadmap's Phase 3
  and 3b boxes ticked, each naming its test.
- The acceptance bullet "nothing in the codebase distinguishes it" is a review item with
  one mechanical check: `grep -rn "primary\|leader\|authoritative" crates/*/src` finds
  only comments that forbid them.

**Tests:** `test_spec_cli_1_version_qualifies_protocol` (now `phase 3`);
`test_spec_10_3_a_node_destroyed_and_rebuilt_from_a_backup_rejoins_and_loses_nothing`
(`crates/privatium/tests/cluster.rs`: a root deleted, restored from a copy of `data/`,
re-joined, converged).

---

## 7. Conformance mapping

| Checklist item (§13 wording) | Milestone | Test |
|---|---|---|
| Never writes to a log file for a device other than as specified in §10.2 | M21 | `test_spec_10_2_the_receiver_is_the_only_writer_of_another_devices_file` |
| Lamport counter is monotonic across restart and sync (§4.3) — the sync half | M21 | `test_spec_4_3_lamport_folds_received_events_and_stays_monotonic_across_restart` |
| Sync rejects `seq` gaps (§10.2) | M21 | `test_spec_10_2_a_seq_gap_is_refused_and_the_range_is_pulled` |
| Node certificates renew on sync (§2.3.1) — the renewal half | M20 | `test_spec_2_3_1_certificate_renews_after_a_sync_and_an_expired_one_is_refused` |
| A device pinned to a cluster trusts a node it has never met, if signed (§2.3.2) | M20 | `test_spec_2_3_2_a_device_pinned_to_the_cluster_reaches_an_unmet_node` |
| Discovery filters by TXT `cl` once paired (§6.1) | M20 | `test_spec_6_1_browse_filters_to_the_cluster_and_keys_on_id` |
| No node is designated primary or authoritative (§10.3) | M21 | `test_spec_10_3_no_node_is_primary`, `test_spec_10_3_power_cut_…` |
| Endpoint failover uses ≤2500 ms connect timeouts (§10.4) — the timeout half; re-attempt on network change is a native-client behaviour of Phase 4 | M21 | `test_spec_10_4_killing_the_active_endpoint_fails_over_in_under_five_seconds` |
| `sys_device.replica` declared accurately (§10.7) — nodes | M20 | `test_spec_2_3_1_a_node_is_admitted_…` |
| Row-granularity LWW ordered by `(lam, ts, dev)` (§4.5) — with more than one writer | M22 | `test_spec_4_5_incremental_apply_of_interleaved_devices_equals_replay` |
| Deleting `cache/` and all `snap/` loses no data (§3.1, §5) — with attachments | M25 | `test_spec_3_1_delete_cache_loses_nothing_with_blobs` |

After M26 the unclaimed items of `§13` are exactly `§6.2`–`§6.3`'s pkarr and DNS lines and
the network-change re-attempt of `§10.4`: Phase 5 and Phase 4.

---

## 8. Risks

**R17 — The inbox model.** Draining in `refresh_app` means a pulled batch waits for a
request, a ping, or `sync_now`. In the daemon the stream's pump reacts to `sync_events`,
so latency is milliseconds; in an embedder with no traffic it is "until you ask", which
is documented. If the review finds a path where a batch can wait forever, the fix is a
tick in the daemon's maintenance loop calling `refresh_app`, not a shared node.

**R18 — Two nodes in one test process.** Every socket test opens two or three data roots
and two or three listeners; on the CI runners that is the slowest suite in the workspace.
Keep the through-`handle` tests as the bulk and the socket tests as the few named in §6.

**R19 — Rank in the cache.** `pv_rank` doubles the writes of an apply. Measure against
Phase 1's numbers in M22; if a 50,000-line log's rebuild slows by more than a third, index
`(tbl, id)` and stop there — never skip the comparison.

**R20 — Blob size on a phone.** A 32 MiB default through a WebSocket frame loop is fine on
a LAN and slow on cellular; Phase 5 may lower the default per transport. The limit is a
`sys_setting`, so nothing here is hard-coded.

**R21 — Syncthing races.** Syncthing can rewrite a foreign segment while the node reads
it. The reader's tolerance for a torn tail and the rename-on-complete Syncthing uses are
what M23 relies on; the tests reproduce the rename, not Syncthing itself, and the manual
pass runs the real thing on two machines.

**R22 — The demo.** `htmx.createEventSource` may not exist in the vendored extension. The
fallback in M24 keeps the attribute count and is decided with the file open.

---

## 9. PR sequence

| # | Branch | Depends on | Spec edits |
|---|---|---|---|
| 23 | `m20-admission` | Phase 2 | §3 rows 3, 4, 5, 12 |
| 24 | `m21-sync` | M20 | rows 1, 2, 6, 7, 8, 9, 11 |
| 25 | `m22-rank` | M21 | `§4.5` sentence |
| 26 | `m23-filesync` | M22 | row 13 |
| 27 | `m24-animals-live` | M23 | — |
| 28 | `m25-attachments` | M24 | row 10 |
| 29 | `m26-always-on` | M25 | roadmap: tick Phase 3 and 3b |
| 30 | `phase3-hardening` | M26 | as found |

---

Copyright © 2026 Gabriel Mongefranco
