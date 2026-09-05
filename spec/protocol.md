<!--
Project:  Privatium™
File:     spec/protocol.md
Authors:  Gabriel Mongefranco (@gabrielmongefranco)
Created:  2026-08-28
Modified: 2026-09-05
Summary:  NORMATIVE. Wire formats, event log, discovery, pairing, session crypto, sync.
-->

# Privatium Protocol Specification — `pv/1`

**Status:** Draft 0.1 — Phase 1 complete (`docs/plans/phase-1.md`), the later phases of
`docs/roadmap.md` not yet begun; a build that does not yet satisfy every item of §13
identifies itself as `pv/1 (partial: phase 1)` (`spec/cli.md §1`)
**Protocol identifier:** `pv/1`

The key words MUST, MUST NOT, REQUIRED, SHALL, SHALL NOT, SHOULD, SHOULD NOT, RECOMMENDED,
MAY, and OPTIONAL are to be interpreted as described in BCP 14 (RFC 2119, RFC 8174) when,
and only when, they appear in all capitals.

---

## 1. Terminology

| Term | Definition |
|---|---|
| **Node** | One installation of the Privatium server. Owns an identity keypair. |
| **Device** | Anything that has paired with a node: a browser, a phone, another node. |
| **Owner** | The single human who controls a node. `pv/1` has no concept of multiple owners. |
| **Cluster** | The set of nodes belonging to one owner, sharing a cluster keypair (§2.3). |
| **Replica** | A client holding the full log set and materializing locally. Nodes always; native mobile optionally; browsers never. |
| **App** | A folder conforming to `spec/app-contract.md`, mounted at a slug. |
| **Slug** | An app's identifier. `^[a-z][a-z0-9-]{1,30}$`. |
| **Event** | One line of JSONL. The atom of state. |
| **Log** | `data/<slug>/log/<device-id>.jsonl`. Append-only, single-writer. |
| **Replay** | Reconstructing tables from events. |
| **Snapshot** | A materialized, immutable copy of app tables at a watermark. Cache only. |

### 1.1 Reserved slugs

`_sys`, `api`, `a`, `ws`, `static`, `health`, `pair`, `well-known`, `settings`, `skills`.
Implementations MUST reject an app folder using any of these. The last two are reserved
because they are framework route prefixes (§9.1), not because of the mount path.

---

## 2. Identity

### 2.1 Node identity

On first run a node MUST generate an Ed25519 keypair.

- Private key: `identity/node.key`, mode `0600`. MUST NOT appear in `data/`, in any log,
  in any backup export, or in any snapshot.
- Public key: `identity/node.pub`.
- **Node ID**: the first 40 bits of `SHA-256(public_key)`, encoded as 8 characters of
  Crockford Base32 (lowercase). Example: `k7m2q9xf`.

Node IDs are the `dev` value in events and the filename of that node's log files. They
MUST be treated as opaque by all other nodes.

### 2.2 Device identity

Every paired device — including browsers — MUST have its own Ed25519 keypair and its own
Node ID computed the same way. A browser generates its keypair during pairing and stores
it in `localStorage` (or IndexedDB) under the origin.

A browser that loses its keypair MUST re-pair. Implementations MUST NOT provide a recovery
path that bypasses pairing.

### 2.3 Cluster identity

A **cluster** is the set of nodes belonging to one owner. Without it, a device would have to
pair separately with every node — tedious with two machines and unworkable with a household.

- The first node generates an Ed25519 **cluster keypair**.
- **Cluster ID**: first 40 bits of `SHA-256(cluster_public_key)`, 8 characters of Crockford
  Base32. Distinct from any Node ID.
- Every node holds the cluster private key. See §2.3.3 for the trade-off.

#### 2.3.1 Admitting a node

A new node joins by pairing with an existing node using the ordinary §7 flow. The owner
opens pairing mode on the existing node; physical presence is the authorization, exactly as
for a phone.

On success the admitting node sends:

1. The cluster private key.
2. A **node certificate**: `{node_id, node_pub, cluster_id, issued_at, expires_at, sig}`
   where `sig` is the cluster key's Ed25519 signature over the other fields.

`expires_at` MUST be `issued_at + 180 days`. Certificates renew automatically whenever two
nodes complete a sync, so an in-use node never expires. A node offline longer than 180 days
MUST be re-admitted.

#### 2.3.2 What devices pin

A device pairing with any node pins the **cluster public key**, not the node key.

On every subsequent connection a node presents its node certificate. The client MUST verify
the signature against the pinned cluster key and MUST reject an expired certificate. A
client that has never met a node before therefore still trusts it, provided the cluster
admitted it.

This is what makes §10.4 work: pair a phone once, and it reaches the desktop, the laptop,
and any future node without further ceremony.

A cluster-key mismatch is handled identically to a node-key mismatch (§8.1): full-screen
refusal, no override.

#### 2.3.3 Trade-off, stated plainly

Distributing the cluster private key to every node means **compromising any one node
compromises the cluster**. The alternative — a single signing node — would mean node
admission fails whenever that machine is off, which defeats the purpose.

For a single owner whose nodes are all machines they physically control, distribution is
correct. Implementations MUST NOT distribute the cluster key to non-node devices: phones,
tablets, and browsers receive the public key only.

#### 2.3.4 Revoking a node

Revocation writes a `sys_node_revocation` event, which replicates. A device that has synced
since the revocation refuses that node.

There is no CRL and no online check. The gap is bounded by the 180-day certificate lifetime:
a revoked node stays trusted by a device that never syncs, for at most that long. An owner
who needs a hard cut MUST rotate the cluster (§2.3.5).

#### 2.3.5 Cluster rotation

Generating a new cluster keypair and re-admitting the good nodes invalidates every
outstanding certificate. All devices must re-pair. This is the nuclear option and the only
one available in `pv/1`.

### 2.4 Key rotation

Node key rotation is not specified in `pv/1`. A node whose key is compromised MUST be
re-initialized and re-admitted. Cluster rotation is §2.3.5.

---

## 3. Storage layout

All paths are relative to the node's data root, which MUST be
`$XDG_DATA_HOME/privatium` on Linux (falling back to `~/.local/share/privatium`), the
platform equivalent elsewhere, or an owner-selected directory obtained through the
platform's file-chooser portal.

```
<data-root>/
├── identity/
│   ├── node.key                 Ed25519 private key, 0600, NEVER synced
│   ├── node.pub
│   ├── cluster.key              Ed25519 cluster private key, 0600, NODES ONLY
│   ├── cluster.pub
│   └── node.cert                this node's cluster-signed certificate
├── config.toml                  node configuration
├── apps/                        owner-installed app folders
│   └── <slug>/
├── data/                        ← THE ONLY THING THAT MUST BE BACKED UP
│   ├── _sys/
│   │   ├── log/<device-id>.jsonl
│   │   └── snap/
│   └── <slug>/
│       ├── log/<device-id>.jsonl
│       └── snap/<snapshot-id>/
├── local/                       node-local state, NEVER synced, NOT required for restore
│   ├── lock                     held exclusively by the process that has this root open (§3.1)
│   └── state.jsonl
└── cache/                       fully disposable
    ├── <slug>.sqlite
    └── ...
```

### 3.1 Rules

- A node MUST append only to `data/<slug>/log/<its-own-node-id>.jsonl`.
- A node MUST hold an exclusive lock on `local/lock` for as long as it has the root open,
  and MUST refuse to open a root whose lock another process holds: two processes on one
  root would both mint `seq` for the same log (§4.1). The lock is advisory — every writer
  goes through the node — and is released when its holder exits, so a crash leaves nothing
  stale behind.
- A node MUST NOT modify, truncate, reorder, or delete any line of any log file, including
  its own.
- A node MUST treat `local/` and `cache/` as excluded from sync and from backup.
- Deleting `cache/` and every `snap/` directory MUST result in zero data loss.

### 3.2 Log rotation

A node MAY roll its own log at a size threshold, producing
`log/<device-id>.<n>.jsonl` with `n` starting at `2`. Rolled files are still append-only
and still immutable. Readers MUST treat `log/<device-id>*.jsonl` as one logical stream
ordered by `seq`.

---

## 4. The event

### 4.1 Envelope

One JSON object per line. UTF-8. No trailing whitespace. `\n` terminated (`0x0A`, never
`\r\n`). Object keys SHOULD be emitted in the order below for greppability, but readers
MUST NOT depend on key order.

```json
{"seq":1041,"lam":8830,"ts":"2026-08-28T14:03:11.412Z","dev":"k7m2q9xf","app":"hello","op":"put","tbl":"profile","id":"01J9YQ2W7C8XKF3M0N5RTVB6ZP","d":{"display_name":"Gabriel"}}
```

| Field | Type | Req | Definition |
|---|---|---|---|
| `seq` | integer | ✔ | Per-device, per-app monotonic counter. Starts at 1. A **writer** MUST emit it gapless. |
| `lam` | integer | ✔ | Lamport counter. See §4.3. |
| `ts` | string | ✔ | RFC 3339 UTC with millisecond precision and a literal `Z`. |
| `dev` | string | ✔ | Node ID of the writer. MUST equal the log filename. |
| `app` | string | ✔ | App slug. MUST equal the containing directory. |
| `op` | string | ✔ | `put` or `del`. |
| `tbl` | string | ✔ | Table name within the app. |
| `id` | string | ✔ | Row key. Unique within `(app, tbl)` and stable across amendments to the same row. A ULID unless the table defines its own key — see below. |
| `batch` | integer | first of a batch | On the first line of a batch of two or more events, how many lines the batch has. Absent on every other line — see *Batches* below. |
| `d` | object | put | Column values. MUST be absent when `op` is `del`. |

A **reader** MUST NOT reject, reorder, or repair a `seq` gap it finds in a local log file.
Gap rejection belongs to sync (§10.2), where the missing range can actually be requested; a
reader that refuses to materialize a locally edited log turns a curiosity into an outage.

**On `id`.** A ULID is the default, and is what the framework mints when a caller supplies
no key. It is not the only legal value. `sys_node` and `sys_device` are keyed by Node ID
(`spec/data-dictionary.md §3.1`, `§3.2`), and `spec/lua-api.md §3.3` lets a server-side
caller pass its own key — `apps/animals` uses the constant `'cursor'` for a singleton row.
Events accepted over the HTTP data API remain restricted to ULIDs (`spec/data-api.md §2`),
because a browser client is not trusted to choose row keys.

Two writers that choose the same `id` for the same `(app, tbl)` converge on one row under
§4.5. For a deliberately shared singleton that is the intent; anywhere else it is a silent
cross-device merge, which is why minting is the default. `id` plays no part in sync itself:
§10.1 is a set union over `(dev, seq)`, and §10.6 depends only on a retry carrying the *same*
`id`, not on its shape.

**Batches.** Several events written as one act — `pv.batch` (`spec/lua-api.md §3.3`), a
data API append (`spec/data-api.md §2`) — share one `ts`, take contiguous `seq` and `lam`,
and reach the file in one write. The first line of a batch of `n ≥ 2` events carries
`"batch": n`; no other line carries the key. A reader that finds, after such a header,
fewer than `n` consecutive lines with that `ts` and a `seq` one past the previous — the
segment ended, a line with another `ts` came first, a new header began — has a batch that
reached the disk short, which a crash between the write and the disk leaves. A reader MUST
NOT materialize, serve or forward the lines of such a batch, MUST NOT remove them, and MUST
continue past them; the writer continues after them with the next `seq`, and the node
records the batch once in `sys_audit` as `batch.incomplete` (`spec/data-dictionary.md
§3.10`). A single event, a tombstone on its own, and a line appended by hand are batches of
one and carry no marker: nothing about a line a person writes changes.

**A failed append.** A writer whose write or flush fails MUST NOT append again until it
has re-read its own file: the failed line may be on disk in whole or in part, and the
next `seq` is whatever the file now ends with. A file that then ends mid-line stays
closed to writes until the owner resolves it — §3.1 forbids the writer to truncate.

### 4.2 Forward compatibility

Readers MUST accept and **preserve** unknown top-level fields and unknown keys inside `d`.
When a node re-emits or forwards an event during sync it MUST transmit the original line
byte-for-byte. Implementations MUST NOT normalize, re-serialize, or "clean" events.

This is the mechanism by which a `pv/1` node and a `pv/2` node can share a log without
either losing information.

### 4.3 Lamport clock

Each node maintains one Lamport counter per app.

- On write: `lam = max(lam_local, lam_max_seen) + 1`.
- On receiving events during sync: `lam_local = max(lam_local, max(received.lam))`.

`lam` establishes causal order. `ts` is for humans and for last-write-wins tie-breaking
only.

### 4.4 Clock hygiene

A node MUST reject on ingest any event whose `ts` is more than 24 hours in the future
relative to its own clock, and MUST record the rejection in `sys_audit`. A node SHOULD warn
the owner when its own clock appears to have moved backwards more than 60 seconds.

### 4.5 Replay and merge

To materialize table `T` of app `A`:

1. Read every event where `app = A` and `tbl = T` from every log file under
   `data/A/log/`, skipping the lines of a batch that reached the disk short (§4.1).
2. Group by `id`.
3. Within each group, order by `(lam, ts, dev)` ascending. `dev` is a deterministic
   lexicographic tie-break and carries no meaning.
4. Take the last event in each group. If its `op` is `del`, the row does not exist.
   Otherwise the row is its `d`.

Materialization projects only the columns named in `schema.sql`. This does not conflict with
§4.2: preservation is a property of the log file, which is never rewritten. Keys in `d` that
no column matches are simply not projected, and are still there the next time the log is
read. Implementations MUST NOT attempt to round-trip unknown keys through the query
engine.

Last-write-wins is at **row** granularity, not field granularity. An app that requires
field-level merge MUST model each field as its own row.

### 4.6 Deletion

`op: "del"` writes a tombstone. Tombstones are permanent; they are never garbage
collected in `pv/1`.

**An `id` the framework minted MUST NOT be reused as the key of a different row.** Once a
ULID has identified one row it identifies that row forever, deleted or not. Reusing it
would merge two unrelated histories under §4.5 — silently, and across devices, since a
tombstone written on one node and a `put` written on another converge on a single row.
Implementations MUST reject a client-supplied `id` naming a tombstoned row; §9.2's data
API is the surface where this applies, because it is the only one that accepts an `id`
from something untrusted, and it already restricts `id` to ULIDs
(`spec/data-api.md §2`).

This does **not** forbid deleting and re-asserting a *caller-chosen* key. `sys_node` and
`sys_device` are keyed by Node ID, and `apps/animals` deletes its `'cursor'` singleton at
the end of a round and writes it again at the start of the next — both blessed by §4.1.
Such a key names one logical row for the life of the app, so re-asserting it is an
amendment rather than a reuse, and §4.5 gives the expected answer with no special case.
Server-side callers choose their own keys and are trusted to mean it.

Materialization does not enforce any of this. A replay follows §4.5 over whatever survives
§4.4's clock hygiene — the only filter a reader is required to apply. §4.1's mercy for a
`seq` gap is the same principle: a reader materializes what it finds rather than judging
it. The constraint is on writers.

Implementations MUST NOT offer a "hard delete" that rewrites logs. The supported way to
destroy data irrecoverably is to destroy the `data/` directory.

---

## 5. Snapshots

Snapshots are a read-path optimization. They carry no authority.

### 5.1 Layout

```
data/<slug>/snap/<snapshot-id>/
├── MANIFEST.json
├── schema.sql          CREATE TABLE statements with the storage types
├── <table>.sqlite      one SQLite database holding that table
└── <table>.csv
```

`<snapshot-id>` MUST be `<ISO-year>-W<week>-<dev>-<hi_lam>`, e.g. `2026-W35-k7m2q9xf-8830`.
The high-water `lam` in the name means the read predicate is derivable from a directory
listing alone. `<week>` is two digits, zero-padded, as ISO 8601 spells it — `W05`,
never `W5`.

### 5.2 MANIFEST.json

```json
{
  "v": 1,
  "snapshot_id": "2026-W35-k7m2q9xf-8830",
  "app": "hello",
  "created": "2026-08-30T03:00:00.000Z",
  "hi_lam": 8830,
  "hi_seq": {"k7m2q9xf": 1041, "b3nn8t2q": 87},
  "engine": "sqlite 3.53.2",
  "tables": [
    {"name":"profile","rows":1,
     "sqlite_sha256":"...","csv_sha256":"..."}
  ]
}
```

`engine` is the string `sqlite ` followed by the engine's own reported version. The value
above is illustrative: it names whichever engine wrote the `.sqlite` files, and a reader
MUST NOT expect a particular version.

### 5.3 Read precedence

An implementation MUST attempt, in order, and MUST record which tier succeeded:

| Tier | Source | Condition to proceed to next tier |
|---|---|---|
| 1 | `<table>.sqlite` + log tail | file unreadable or SHA mismatch |
| 2 | CSV + `schema.sql` + log tail | CSV unreadable or SHA mismatch |
| 3 | Full log replay from `lam` 0 | (terminal) |

Tier 2 MUST create tables from `schema.sql` before loading CSV. Implementations MUST NOT
use CSV type inference: every value is typed from the declared column
(`spec/data-dictionary.md §2.1`).

**The log tail.** For a snapshot with high-water marks `hi_lam` and `hi_seq`, the log tail is
every event that survives §4.4 and has `lam > hi_lam`. A snapshot **applies** to a log only
when all three hold:

1. The events its `hi_seq` did not cover — `seq > hi_seq[dev]`, or a `dev` absent from
   `hi_seq` — are exactly the events with `lam > hi_lam`. An event the snapshot never saw
   whose `lam` is not above `hi_lam` is §4.1's cross-device case: the snapshot's rows carry
   no `(lam, ts, dev)` to compare it against, so no tier but the replay can place it.
2. The log holds every event `hi_seq` claims — `max(seq) ≥ hi_seq[dev]` for every `dev`
   listed. A snapshot carries no authority and MUST NOT resurrect an event the log has lost.
3. The snapshot's `schema.sql` matches the app's current declared types
   (`spec/app-contract.md §4.5`: a changed `schema.sql` rematerializes from the logs).

A snapshot that does not apply is skipped for tiers 1 and 2 and the replay is used. That is
the expected outcome, not a failure of the snapshot; falling through to tier 3
*unexpectedly* (`spec/cli.md §7`) means a snapshot that applied could not be read. The tier
used is node-local state — a fact about this node's cache — and is never an event; a tier 2
and an unexpected tier 3 are additionally recorded in `sys_audit` as `restore.tier2` and
`restore.tier3` (`spec/data-dictionary.md §3.10`).

### 5.4 Retention

- Default retention: 365 days, configurable.
- The pruner MUST NOT delete the oldest surviving snapshot for an app.
- The pruner MUST assert that snapshot retention does not exceed log retention. Because
  `pv/1` never deletes logs, this assertion always passes; it exists so that a future log
  compaction feature cannot silently cause data loss.

---

## 6. Discovery

### 6.1 DNS-SD / mDNS

Service type: `_privatium._tcp.local.`
Per-app subtype: `_<slug>._sub._privatium._tcp.local.`

A node MUST advertise the parent type and SHOULD advertise one subtype per enabled app
whose slug is ≤ 15 characters. Slugs longer than 15 characters MUST NOT be advertised as
subtypes (DNS label constraint) and this MUST be surfaced as a warning at app load.

**Instance name:** the owner-set display name, ≤ 63 bytes UTF-8. Collision handling is the
mDNS stack's responsibility; implementations MUST NOT invent their own suffixing.

**TXT record keys:**

| Key | Example | Meaning |
|---|---|---|
| `v` | `1` | Protocol major version |
| `id` | `k7m2q9xf` | Node ID — the stable identifier, use this, not the name |
| `cl` | `q4w8rt2n` | Cluster ID. A client browsing for its own cluster filters on this. |
| `nm` | `Gabriel's Node` | Display name |
| `apps` | `hello,animals` | Comma-separated enabled slugs |
| `build` | `official` | `official` \| `custom` \| `fork:<name>` |
| `pair` | `0` | `1` when pairing mode is currently open |
| `p` | `8420` | HTTP port (also in SRV; TXT copy is for cheap filtering) |

Total TXT SHOULD stay under 1300 bytes so the record fits one packet. If `apps` would
exceed the budget it MUST be truncated and terminated with `,…`.

Clients MUST key discovered nodes on `id`, never on instance name.

A client that has paired MUST filter discovery results by `cl` matching its pinned cluster.
On a LAN carrying several households' nodes this is what keeps the list to your own machines
without any per-node pairing.

### 6.2 pkarr — remote discovery on the mainline DHT

mDNS ends at the broadcast domain. For a client that is not on the LAN, a node publishes its
current addresses as **pkarr** records: DNS resource records signed by an Ed25519 key and
stored on the BitTorrent mainline DHT using BEP44 mutable items.

The public key *is* the name. There is no registrar, no account, no domain, and no payment.

#### 6.2.1 What is published

A node SHOULD publish a signed packet under its **node** keypair containing:

| Record | Value |
|---|---|
| `_pv.addr` | Space-separated `IP:port` list — IPv4 and IPv6 |
| `_pv.cl` | Cluster ID |
| `_pv.v` | Protocol major version |
| `_pv.relay` | Relay URL, when one is configured |

The packet MUST stay under the DHT's 1000-byte mutable-item limit. pkarr is a discovery
layer; it MUST NOT be used to carry application data of any kind.

#### 6.2.2 Republishing

DHT records are dropped after a few hours. A publishing node MUST republish on a timer, and
SHOULD republish immediately on any address change.

A consequence worth designing around: **a node that sleeps disappears within hours.** This is
correct behaviour — it prevents stale addresses — but it means a laptop is not a dependable
discovery target. An always-on node (`docs/deployment.md §2`) is.

#### 6.2.3 Privacy

Anyone holding a node's public key can resolve its current address. This is **exactly the
exposure of dynamic DNS**, where anyone holding the hostname can do the same. BEP44 targets
derive from the public key, so the keyspace is not enumerable, and records expire rather than
accumulating.

Two things implementations MUST get right:

- pkarr uses **BEP44 mutable items, not BEP5 infohash announcements.** A node is not
  announcing as a peer for any content and MUST NOT be made to do so.
- Publishing is OPTIONAL and MUST be individually disableable. A node reachable only on the
  LAN has no reason to publish, and some owners will not want to.

Implementations MAY publish under a key derived per-period from the cluster key — for example
`HMAC(cluster_key, date)` — so that an observer who learns the key once cannot track the node
indefinitely. Cluster members compute the same derivation. This is an enhancement over
plain pkarr, not part of it.

#### 6.2.4 When the DHT is unavailable

Mainline DHT traffic is UDP to many peers and resembles BitTorrent to deep packet
inspection. Corporate, campus, and hotel networks frequently block or throttle it.

Implementations MUST therefore treat pkarr as one discovery service among several and MUST
NOT depend on it alone. Running several concurrently is REQUIRED behaviour, not an
optimisation (§6.5).

### 6.3 DNS discovery

The same pkarr signed packets MAY be published to an HTTP pkarr relay that serves them over
ordinary DNS. This resolves in environments where the DHT is blocked, because it is a normal
DNS query on port 53.

- Default resolver origin is the library's public server.
- An owner MAY point this at their own pkarr relay — for example one running on their
  always-on node (`docs/deployment.md §2`).
- Records are identical in both channels; only the transport differs.

### 6.4 UDP broadcast fallback

For networks with multicast filtered or AP client isolation enabled.

- Probe: UDP broadcast to `255.255.255.255:52525`, payload the 8 ASCII bytes `PVDISCO1`
  followed by a 4-byte random nonce.
- Response: unicast UDP to the source, payload `PVDISCO1` + the same nonce + a JSON object
  with the same key set as the TXT record.
- Nodes MUST rate-limit responses to 1 per source IP per second.
- Nodes MUST NOT respond to a probe arriving from outside RFC 1918 / RFC 4193 space.

### 6.5 Running discovery services concurrently

A node MUST run every configured discovery mechanism **at the same time**, not in a fallback
chain. They fail in different environments and none dominates:

| Mechanism | Reaches | Fails when |
|---|---|---|
| mDNS (§6.1) | same broadcast domain | different subnet, multicast filtered, AP isolation |
| UDP broadcast (§6.4) | same subnet | multicast and broadcast both filtered |
| pkarr / DHT (§6.2) | anywhere | DHT blocked by DPI; node asleep |
| DNS (§6.3) | anywhere | resolver unreachable |
| Static / DDNS | anywhere | address changed and DDNS not updated |

Results merge into the endpoint candidate list (§10.4) and are ordered there by last success.
A client MUST NOT wait for a slow mechanism before trying a fast one; mDNS typically answers
in milliseconds while a DHT lookup takes seconds.

### 6.6 Default ports

| Purpose | Port | Notes |
|---|---|---|
| HTTP | 8420 | Configurable. MUST be ≥ 1024. |
| UDP discovery | 52525 | Fixed in `pv/1`. |
| Peer transport (QUIC/UDP) | ephemeral | Chosen by the transport layer; not fixed |
| DHT | ephemeral | Outbound only; no inbound rule required |

---

## 7. Pairing

### 7.0 Three separate properties

Discussions of pairing routinely conflate three distinct guarantees. They are separate, they
are provided by different mechanisms, and one of them is not provided at all on one path.
Implementations MUST NOT treat them as interchangeable.

| # | Property | Question it answers | Mechanism |
|---|---|---|---|
| **1** | **Program authenticity** | Is the code I am running the real client? | App store signature, notarization, or a CA chain |
| **2** | **Device authentication** | Is this device permitted to talk to this cluster? | PAKE on first contact, pinned keys thereafter (§7.4, §8) |
| **3** | **Transport security** | Is this channel confidential and tamper-evident? | The session key derived by §8 |

Coverage by client:

| Client | 1. Program | 2. Device | 3. Transport |
|---|---|---|---|
| Native desktop / mobile | ✔ signed package | ✔ | ✔ pinned keys |
| Browser or PWA over TLS | ✔ CA chain | ✔ | ✔ TLS + session |
| **Browser over plain HTTP on LAN** | **✘ — see §7.7** | ✔ | ✔ session key |

Only property 1 is missing on the plain-HTTP path, and only against an attacker who is
*actively tampering with traffic at first load*. Properties 2 and 3 hold on every path.

#### Property 2 is not a separate login step

A password-authenticated key exchange performs authentication and key agreement as **one
operation**. Deriving a usable key *is* the proof that both sides held the code:

```
code (16 bits) ──▶ CPace / SPAKE2 ──▶ strong session key
                          │
                          └── produces nothing at all if either side had the wrong code
```

Implementations MUST NOT run a PAKE to establish a channel and then transmit the code as a
bearer credential to "log in." That is strictly weaker: it puts the code on the wire, makes
it replayable, and discards the property the PAKE was chosen for.

#### What this means for an untrusted device on the LAN

Discovery is public. Anyone on the network can see that a node exists (§6), and that is
inherent to mDNS. What they cannot do is get any application data:

1. They reach the node and receive the client.
2. Pairing mode is closed unless the owner opened it (§7.1), so the handshake is refused
   outright.
3. With pairing mode open, they face 16 bits, 5 attempts, a 120-second window, and rate
   limiting — roughly 1 in 13,000 — and every attempt writes a replicated `sys_audit` event
   visible on every device the owner owns.

A guest on the Wi-Fi is a **passive** adversary, so property 1 is irrelevant to them.
Property 2 is what excludes them, and it does so on every path including plain HTTP.

### 7.1 Requirements

Pairing MUST NOT be possible unless the owner has explicitly opened pairing mode on the
node (a button press, a CLI flag, or first-run). Physical presence is the authorization.

### 7.2 The code

The pairing secret is **16 bits** of CSPRNG output. It is rendered two ways, both of which
MUST be displayed simultaneously and both of which MUST be accepted as input:

| Rendering | Encoding | Example |
|---|---|---|
| Emoji | 4 symbols from the 16-glyph set (§7.3), big-endian nibbles | 🦊 🍕 ⚡️ 🎲 |
| Words | 2 words from the 256-word list, big-endian bytes | `amber otter` |

Both encode the identical 16-bit integer. Word input MUST be case-insensitive and MUST
ignore spaces, hyphens, and punctuation.

Implementations MUST NOT allow the owner to choose the code. It is always node-generated.

### 7.3 Glyph set

Index order is normative. Changing it changes the wire meaning of a code.

| # | Glyph | Codepoints | Label |
|---|---|---|---|
| 0 | 🦄 | U+1F984 | Unicorn |
| 1 | 🎧 | U+1F3A7 | Headphones |
| 2 | 🍕 | U+1F355 | Pizza |
| 3 | 🛸 | U+1F6F8 | UFO |
| 4 | 🎸 | U+1F3B8 | Guitar |
| 5 | 🍄 | U+1F344 | Mushroom |
| 6 | 💎 | U+1F48E | Diamond |
| 7 | 🦊 | U+1F98A | Fox |
| 8 | ⚡️ | U+26A1 U+FE0F | Lightning |
| 9 | 🌶️ | U+1F336 U+FE0F | Hot Pepper |
| 10 | 🦩 | U+1FAB0 | Flamingo |
| 11 | 🎨 | U+1F3A8 | Artist Palette |
| 12 | 🍍 | U+1F34D | Pineapple |
| 13 | 🍁 | U+1F341 | Maple Leaf |
| 14 | 🎲 | U+1F3B2 | Game Die |
| 15 | 🍓 | U+1F353 | Strawberry |

Rendering rules:

- Index 8 and 9 carry U+FE0F. Implementations MUST NOT apply Unicode normalization or any
  transformation that could strip a variation selector. Store as bytes.
- The label MUST be shown beneath every glyph on both the node display and the input pad.
  This is what makes cross-vendor rendering differences a non-issue.
- No ZWJ sequences, no skin-tone modifiers, no flags. Do not add any.
- Index 10 (🦩, Emoji 12.0, 2019) has the narrowest device support. If tofu is reported it
  is the designated replacement candidate; replacing it is a **breaking protocol change**.

### 7.4 Handshake

Transport: WebSocket at `/ws/pair`, or an equivalent framed channel on native transports.

1. Client connects. Node responds with its protocol version and Node ID.
2. Both sides run a balanced PAKE — **CPace** (RECOMMENDED) or **SPAKE2** (RFC 9382) —
   keyed on the 16-bit code, using X25519/Ristretto255.
   Augmented variants (SPAKE2+, RFC 9383) MUST NOT be used: the code is ephemeral and
   single-use, so there is no verifier to protect and augmentation buys nothing.
3. On success both sides hold a shared secret `K_pair`.
4. Over `K_pair`, each side sends its Ed25519 public key and X25519 public key. The
   transcript MUST bind the node's static public key.
5. Both sides persist the other's static keys. The node writes a `sys_device` event.
   The client pins the node's key.
6. The code is marked consumed. It MUST NOT be reusable.

### 7.5 Constraints

- Code TTL: 120 seconds. MUST be enforced node-side.
- Maximum attempts per code: 5. On exhaustion the code is destroyed and a new one issued.
- Failed attempts MUST be rate-limited to no more than 1 per 2 seconds per source.
- Every pairing attempt, success or failure, MUST produce a `sys_audit` event.

### 7.6 Persistence and re-pairing

After a successful pairing a client stores its own long-term keypair and the pinned cluster
public key, and uses them on every subsequent connection with no code and no prompt. This is
the smart-television model: pair once, trusted thereafter.

- Browsers MUST store these under the page origin (IndexedDB or equivalent, §2.2).
- Native clients MUST use platform secure storage — Keychain, Keystore, or the OS keyring.
- A client that loses this material MUST re-pair. Implementations MUST NOT provide a recovery
  path that bypasses pairing.
- Each device is its own `sys_device` row and is revocable independently. A laptop browser
  and a phone browser are two devices.

**Browser storage is per-origin.** A device paired at `http://192.168.1.5:8420` has no
credential at `https://node.example.com` and must pair again. This is the strongest everyday
argument for exposing one resolvable name across every path (§10.8).

### 7.7 Program authenticity on the plain-HTTP path

A browser loading the client over plain HTTP has no way to verify that what it received is
genuine. An attacker able to modify traffic on that first load can substitute their own
client, observe the code as the owner enters it, complete the handshake themselves, and
proxy everything afterwards.

No amount of in-page cryptography closes this. Trust cannot be bootstrapped over an
untrusted channel without an out-of-band anchor, and a browser has nowhere to keep one before
the code runs. Implementations MUST NOT claim otherwise.

This is trust-on-first-use, identical to accepting an unknown SSH host key. What narrows it:

- The attacker must be *active* on-path, not merely listening.
- They must be present during the specific 120-second window the owner opened.
- Pairing is permanently recorded in replicated `sys_audit` events.
- After first pairing the cluster key is pinned and any later substitution is refused (§8.1).

**What closes it:** pair over a native client, or over any transport with independent
authentication — a valid CA-issued certificate, a mesh VPN, or an onion service. An
implementation SHOULD say so on the pairing screen when it is serving over plain HTTP.

### 7.8 No verification string

`pv/1` deliberately has **no** short-authentication-string confirmation step. The PAKE
authenticates both parties; an attacker without the code cannot complete the handshake, so
a SAS adds no security. Implementations MUST NOT add one. (Against the one attack a PAKE
cannot stop — the substituted client of §7.7 — a SAS is also useless, because the attacker's
client renders whatever it likes.)

---

## 8. Session cryptography

After pairing, every session uses the pinned static keys. No code is involved.

```
ss     = X25519(my_static_priv, their_static_pub)
salt   = SHA-256(sorted(node_id, device_id) || "pv/1 session")
prk    = HKDF-Extract(salt, ss || X25519(my_eph_priv, their_eph_pub))
k_c2s  = HKDF-Expand(prk, "pv/1 c2s", 32)
k_s2c  = HKDF-Expand(prk, "pv/1 s2c", 32)
AEAD   = ChaCha20-Poly1305, 96-bit nonce = 32-bit direction tag || 64-bit counter
```

- This is structurally Noise_KK. Implementations MAY use a Noise library instead of the
  above and MUST document which.
- ChaCha20-Poly1305 is REQUIRED rather than AES-GCM because browser clients have no
  hardware AES available through pure-JS implementations.
- Nonce counters MUST NOT repeat under a key. Rekey or reconnect before exhaustion.
- Browser clients MUST use audited pure-JS implementations. `crypto.subtle` is unavailable
  on plain-HTTP origins and MUST NOT be depended upon.
- `crypto.getRandomValues` IS available on insecure origins and MUST be the CSPRNG source.

### 8.1 Pinned key mismatch

If a peer presents a static public key that differs from the pinned value, the client MUST
refuse the connection, MUST display a full-screen non-dismissible warning naming the node
and both key fingerprints, and MUST require explicit re-pairing to proceed. It MUST NOT
offer a "continue anyway" affordance.

### 8.2 Transport exemptions

When the transport already provides authenticated encryption bound to the peer's identity
— Tailscale, TLS from a trusted CA, or a Tor onion service — the session layer MAY be
skipped. Implementations MUST NOT skip it on plain HTTP under any circumstances.

---

## 9. HTTP interface

### 9.1 Route namespaces

The framework reserves five prefixes and hands everything else to apps.

| Prefix | Owner | Notes |
|---|---|---|
| `/` | framework | Shell and app launcher in host mode. **In solo mode this is the app's root.** |
| `/settings` | framework | Node settings, devices, apps, backup |
| `/api/v1/*` | framework | §9.2 |
| `/skills/*` | framework | Skill bundles for assistants (`spec/cli.md §6`) |
| `/static/*` | framework | Shell assets and `pv.js` |
| `/a/<slug>/**` | **the app** | Everything beneath a mount point |

**The framework does not define an app's routes.** A Tier 1 app registers its own with
`pv.get`, `pv.post`, and so on (`spec/lua-api.md §3.1`); a Tier 2 app serves its `web/`
directory and defines its own paths. Beneath `/a/<slug>/` only `api/` is reserved, for the
data API (`spec/data-api.md`), and it is resolved before a Tier 1 route table or a Tier 2
`web/` is consulted. In solo mode the mount is `/`, so `/api/…` is the solo app's data API
too; `/api/v1/*` (§9.2) stays the framework's.

**Framework prefixes take precedence in both modes.** In host mode this never arises, since
apps live under `/a/<slug>/`. In solo mode the app owns `/`, so an app route matching a
reserved prefix — `pv.get('/settings')`, say — is shadowed. Implementations MUST resolve in
favour of the framework and MUST warn at load, naming the route and the prefix. It is a
warning rather than a refusal because the same app is legal in host mode. A Tier 2 app's
routes are the paths under `web/`, so what is shadowed there is a top-level entry named
after a prefix — `web/settings/`, `web/static/` — and the warning names it the same way.

`/static/*` is the one prefix with a fall-through. The framework answers from its own
embedded assets, and in solo mode a name it does not have is served from the mounted Tier 1
app's `static/` (`spec/lua-api.md §2`) — `url('/static/app.css')` has to reach the app's
stylesheet when the mount is `/`. The framework's names still win, since they are tried
first; a Tier 2 app's `web/static/` is shadowed as above.

An earlier draft listed fixed `/v/<view>`, `/f/<form>`, and `/x/<action>` routes. Those
belonged to a declarative tier that was removed (`spec/app-contract.md §1`), and
implementations MUST NOT reintroduce them.

In solo mode the mount prefix is absent and the app owns `/` directly, which is why every
internal link MUST go through `url()` or `pv.url()` rather than a literal path.

### 9.2 API routes

| Method | Path | Auth | Purpose |
|---|---|---|---|
| GET | `/api/v1/health` | none | Liveness. Returns `{"v":1,"id":"..."}` only. |
| GET | `/api/v1/manifest` | none | Node ID, display name, app index, `pair` flag (below). No data. |
| GET | `/ws/pair` | code | Pairing handshake |
| GET | `/ws` | session | Encrypted application channel |
| GET | `/api/v1/sync/heads?app=` | session | `{dev: hi_lam}` per device |
| GET | `/api/v1/sync/pull?app=&dev=&after=` | session | NDJSON stream of raw event lines |
| POST | `/api/v1/sync/push` | session | NDJSON body of raw event lines |

Unauthenticated endpoints MUST expose no application data of any kind. `/api/v1/manifest`
returns app slugs and titles because discovery requires them; it MUST NOT return row
counts, timestamps of last activity, or any app content.

The manifest is one JSON object:

```json
{"v":1,"id":"k7m2q9xf","name":"Study","apps":[{"slug":"hello","title":"Hello","icon":"chat-heart"}],"pair":false}
```

- `name` is `sys_node.display_name` (`spec/data-dictionary.md §3.1`); while the owner has
  set none it is the Node ID, so a client always has something to show.
- `apps` lists the apps a device could open — those with a mount in the node's current
  mode — as `slug`, `title` and `icon` (the `app.toml` icon name, absent when none). In
  solo mode that is the one app.
- `pair` is whether the node is accepting a pairing at this moment (§7.1: pairing requires
  explicit owner action). A build without pairing reports `false`.

### 9.3 Headers

- `Cache-Control: no-store` on every response containing app data — which is every
  response except the framework's own embedded assets under `/static/*` and the skill
  documents under `/skills/*`, neither of which carries any. App responses, the shell's
  pages, and `/api/v1/*` all carry it.
- `Content-Security-Policy: default-src 'self'; script-src 'self'; object-src 'none';
  base-uri 'none'; form-action 'self'; frame-ancestors 'none'` — as written, on every
  response the framework itself renders, which is why the shell keeps its scripts and
  styles in files under `/static/` and inlines nothing. An app response carries the
  app's own policy (`spec/app-contract.md §5.4`): this one with `script-src` scoped to the
  app's path, widened only by its `[permissions]`, and never relaxed for anything else.
- `X-Content-Type-Options: nosniff`
- `Referrer-Policy: no-referrer`

All four are on every response, including refusals and errors.

In solo mode, when the app at `/` declares `permissions.cross_origin_isolated`
(`spec/app-contract.md §5.4`), every response of the origin — the framework's own
included, since both headers are document-level and the solo app owns the origin — also
carries `Cross-Origin-Opener-Policy: same-origin` and `Cross-Origin-Embedder-Policy:
require-corp`. Never in host mode, where the loader refuses the permission.

---

## 10. Sync

### 10.1 Model

Sync is a set union over `(dev, seq)` pairs. There is no conflict resolution step; conflict
is impossible because logs are single-writer and append-only.

```
A → B  GET /api/v1/sync/heads?app=hello
B → A  {"k7m2q9xf": 1041, "b3nn8t2q": 87}
A      for each dev where A.head[dev] > B.head[dev]:
A → B    POST /api/v1/sync/push  (raw lines, seq > B.head[dev])
A      for each dev where B.head[dev] > A.head[dev]:
A → B    GET /api/v1/sync/pull?dev=<dev>&after=<A.head[dev]>
```

### 10.2 Requirements

- Pushed lines MUST be validated: `dev` matches the claimed log, `seq` is exactly
  `head + 1`, `app` matches the request, envelope parses.
- A receiver MUST reject a gap in `seq` and request the missing range rather than
  appending out of order.
- A receiver MUST write received events to `data/<app>/log/<origin-dev>.jsonl`, never to
  its own log. This is the one case where a node writes a file named for another device;
  it is still single-writer, because only that origin device ever produces those lines.
- A receiver MUST NOT re-serialize. Bytes in, bytes out (§4.2).
- Sync state (per-peer heads) lives in `local/`, MUST NOT be an event, and MUST NOT sync.

### 10.3 Multi-node clusters

Because logs are single-writer and append-only, a cluster of nodes needs no coordination and
no primary. Every node holds every log it has seen; sync is a union.

**Worked example — the power-cut case.** Desktop and laptop are both nodes. Power fails;
the desktop stays off, the laptop's battery keeps it up. The phone, having pinned the cluster
key (§2.3.2), discovers the laptop over mDNS and syncs to it: the phone's own log lines are
written to `data/<app>/log/<phone-id>.jsonl` on the laptop. When the desktop boots it sees a
peer with higher heads for `<phone-id>` and pulls the missing range.

No conflict is possible at any point, because the phone remained the only writer of its own
log. The desktop being days behind is not a special case — it is the ordinary `seq` catch-up
in §10.1.

Implementations MUST NOT designate a primary, elect a leader, or treat any node's copy as
authoritative.

### 10.4 Endpoint selection and failover

A client keeps a **candidate endpoint list**, persisted, one entry per reachable path:

```json
{"url":"http://192.168.1.5:8420","kind":"lan-ip","last_ok":"2026-08-28T14:03:11Z","rtt_ms":4}
```

`kind` is one of `lan-mdns`, `lan-ip`, `dns`, `tunnel`, `vpn`, `p2p`.

Requirements:

- Order by `last_ok` descending, then by `kind` in the order above. Recency beats category:
  the endpoint that worked a minute ago is the best guess now.
- Connect timeout MUST be short — 2500 ms RECOMMENDED — so a dead endpoint fails over
  quickly rather than hanging. Total attempt budget across all candidates SHOULD be ≤ 10 s.
- Re-attempt on: network-change events, application foreground, and explicit user action.
  A client MUST NOT rely on a background timer alone.
- After exhausting candidates, enter offline mode (§10.6) rather than blocking the UI.
- Discovery results merge into the list; they do not replace it.

**Browser clients are restricted to their own origin.** An HTTPS page cannot fetch an
`http://` LAN endpoint — mixed content forbids it — so a browser client MUST hold exactly
one endpoint, its own origin. Multi-endpoint failover is a native-client capability. See
§10.7.

### 10.5 Transports

| Transport | When | Notes |
|---|---|---|
| HTTP on LAN | Peers on the same network | Discovered by §6.1, §6.4 |
| Direct peer (QUIC) | Peers anywhere | Hole punching after §6.2/§6.3 discovery; relay fallback (§10.5.1) |
| HTTP over VPN / tunnel | Peers anywhere | Tailscale, Cloudflare, or the owner's own |
| HTTP via DDNS + certificate | Peers anywhere | DuckDNS or a real domain with DNS-01 |
| File sync | Any | Syncthing, rsync, or a USB stick on `data/`; no protocol involvement |

#### 10.5.1 Relay fallback

Hole punching does not always succeed — symmetric NAT and carrier-grade NAT, which is
standard on mobile carriers, are the common failures. When it fails, traffic is relayed.

A relay MUST NOT be able to read anything it forwards. Transport encryption is end-to-end
between peers, so a relay sees ciphertext, source and destination addresses, and timing.

Implementations MUST allow the relay to be configured, and SHOULD encourage an owner with an
always-on node to run their own (`docs/deployment.md §2`). A default public relay is
acceptable as an unconfigured fallback and MUST be disableable.

**A relay is a strictly better trust position than a node.** A relay holds nothing and can
decrypt nothing; a node holds a full plaintext replica. An owner with one machine to spare
should run a relay on it before considering a full node there.

File-level sync is a legitimate transport precisely because of the single-writer rule. A
node MUST watch `data/` for externally-appeared files and re-materialize.

**An always-on machine is the RECOMMENDED answer to remote access**, and it can serve up to
four independent functions. Each is separately enableable; none is a protocol-level role.

| Function | What it does | Holds data? |
|---|---|---|
| **Relay** | Forwards ciphertext when hole punching fails (§10.5.1) | No |
| **pkarr relay / DNS** | Serves signed discovery packets over DNS where the DHT is blocked (§6.3) | No |
| **Certificate host** | A real domain with an ACME certificate, giving browsers and PWAs a usable HTTPS origin (§10.8) | No |
| **Full node** | An ordinary cluster member holding a complete replica | **Yes** |

The first three hold no application data and can decrypt nothing. **The fourth holds a full
plaintext replica and is the one to think hardest about** — an owner may reasonably run the
first three and decline the fourth.

Implementations MUST NOT give any node a protocol-level distinction. If a "server" role, a
leader election, or an authoritative-copy check appears anywhere, that is a defect.

### 10.6 Offline behaviour

A client that cannot reach any endpoint queues writes in an **outbox** and replays them on
reconnection, in the order they were queued.

The outbox requires no deduplication table, no transaction identifiers, and no
acknowledgement protocol, because **ULIDs make replay idempotent at the row**: a write that
may or may not have landed carries the same `(app, tbl, id)` on retry, and row-granularity
last-write-wins (§4.5) converges to one row either way — never two. Implementations MUST
NOT add a dedupe mechanism; doing so indicates a misunderstanding of §4.5.

Whether a write *did* land is decided by reading the log, never by remembering. A queued
entry carries the Lamport high-water mark the client held when it queued it; before
replaying, the client reads each of its rows' events past that mark
(`spec/data-api.md §1`, `/api/events?tbl&id&after`). An exact copy of its event means it
landed — the node wrote it and the response was lost — and the entry is dropped rather
than sent again. Another event on one of its rows means a newer change the client did not
see, and the whole entry is refused and reported to the app, never written over that
change. No event on any row means it is sent. That read is the whole of the bookkeeping,
and it is not remembered either.

A browser client is not a device (§10.7): it holds no log and stamps nothing, so what it
does send is stamped by the node as it arrives, and what it cannot do is overwrite what
it did not see. A native replica's own log carries its own `lam` and merges by causal
order under §4.5 instead.

Reads in offline mode come from whatever the client cached. A client MUST show its offline
state explicitly and MUST NOT present stale data as current.

### 10.7 Client capability tiers

Not every client can do everything. What a client can do is a property of its runtime, not
of its configuration.

| Capability | Node | Native desktop | Native mobile | PWA / browser |
|---|---|---|---|---|
| Full replica (holds logs, materializes) | ✔ | ✔ | OPTIONAL | ✘ |
| mDNS / DNS-SD browsing | ✔ | ✔ | ✔ | ✘ — no browser API exists |
| UDP broadcast fallback | ✔ | ✔ | ✔ | ✘ |
| Pinned TLS / raw sockets | ✔ | ✔ | ✔ | ✘ — needs a CA in the trust store |
| Direct peer transport (§10.5) | ✔ | ✔ | OPTIONAL | ✘ |
| Multi-endpoint failover (§10.4) | ✔ | ✔ | ✔ | ✘ — single origin |
| Background sync | ✔ | ✔ | ✔ | ✘ on iOS |
| Durable storage | ✔ | ✔ | ✔ | ⚠ iOS evicts after ~7 days unused |

The first row is the significant one. **A browser client cannot find a node** — the user
supplies an address. Native clients discover it. That capability gap, not rendering, is what
justifies native shells.

A native mobile client MAY be a full replica by embedding the core library. It is not
required to be, and the default is a caching client with an outbox. Implementations MUST
declare which they are in `sys_device.replica` so a peer knows whether to offer sync.

#### 10.7.1 Where an app renders

Orthogonal to the capability table above, and frequently confused with it.

| | Rendered by | Delivered as | Offline reach |
|---|---|---|---|
| Tier 1 | the node | HTML | Cached pages and an outbox only |
| Tier 2 | the client | HTML, JS, WASM | Full, against the local replica |

Both are delivered dynamically and update as soon as the node's files change. Neither
requires rebuilding or redistributing a client.

Implementations MUST NOT ship a client that downloads and executes a Tier 1 app's Lua
locally. The exemption permitting dynamic delivery on restrictive platforms covers scripts
executed by the platform web view; native interpreters running downloaded source do not
qualify. Tier 2 already occupies the permitted path.

A generic client SHOULD therefore present itself as a client for the owner's own node, which
is what it is, rather than as a host for third-party applications.

### 10.8 The origin problem

A client that reaches a node at `http://192.168.1.5:8420` on the LAN and
`https://node.example.com` on cellular is talking to **two different origins**. For a browser
client that means a different service worker, a different storage bucket, and a different
session — switching networks does not degrade the app, it becomes a different installation.

Two mitigations, and an implementation SHOULD support both:

1. **Native clients own their storage** and merely swap endpoints (§10.4). Network
   transitions are invisible.
2. **One resolvable name for every path** — a DNS name answering with the LAN address at home
   and a reachable address away — gives a single origin, LAN latency at home, and a working
   PWA on cellular. Note that some routers' DNS-rebind protection strips private addresses
   from responses; document this rather than working around it.

Implementations MUST NOT attempt to work around mixed-content restrictions, and MUST NOT
present a browser client with an endpoint list it cannot legally use.

---

## 11. Onion service

A node MAY host a Tor onion service in-process via `arti-client` with the
`onion-service-service` feature. When enabled:

- The `.onion` address MUST be displayed in settings and offered as a QR code.
- The onion service MUST route to the same HTTP server and MUST NOT bypass authentication.
- Implementations MUST NOT enable the `static` cargo feature (it pulls in native-tls);
  use `rustls`.
- `arti-client` may terminate the process on an obsolete-consensus signal. It MUST run
  supervised such that this does not take down the node.
- Nodes MUST also document the manual alternative (a `HiddenServicePort` directive in a
  system `torrc`) so owners can opt out of the bundled implementation entirely.

---

## 12. Version negotiation

- The major version appears in the mDNS TXT `v` key and the `/api/v1/` path prefix.
- A node MUST refuse a session with a peer advertising a different major version and MUST
  say so in plain language.
- Minor additions MUST be backward compatible by §4.2 (preserve unknown fields).
- `app.api` in an app manifest declares the framework API the app was written against. A
  node MUST refuse to load an app declaring a higher `api` than it implements.
- `api` names the version of `spec/app-contract.md` — its `api = 1` — and MUST be a
  positive integer. A build that speaks `pv/1` implements `api = 1` whether or not it
  satisfies every item of §13; the qualifier `spec/cli.md §1` puts on `--version` is a
  statement about conformance, not about which contract an app may target.

---

## 13. Conformance checklist

An implementation claiming `pv/1` conformance MUST satisfy all of:

- [ ] Deleting `cache/` and all `snap/` directories loses no data (§3.1, §5)
- [ ] Never writes to a log file for a device other than as specified in §10.2
- [ ] Preserves unknown envelope and `d` fields byte-for-byte (§4.2)
- [ ] Lamport counter is monotonic across restart and sync (§4.3)
- [ ] Rejects events > 24h in the future (§4.4)
- [ ] Row-granularity LWW ordered by `(lam, ts, dev)` (§4.5)
- [ ] Three-tier read fallback, with the tier used recorded (§5.3)
- [ ] Never prunes the oldest snapshot (§5.4)
- [ ] Advertises `_privatium._tcp` with the full TXT key set (§6.1)
- [ ] pkarr packets stay under 1000 bytes and carry no application data (§6.2.1)
- [ ] pkarr publishing is optional and individually disableable (§6.2.3)
- [ ] Uses BEP44 mutable items, never BEP5 infohash announcements (§6.2.3)
- [ ] Republishes pkarr records on a timer and on address change (§6.2.2)
- [ ] Runs all configured discovery mechanisms concurrently, not chained (§6.5)
- [ ] UDP fallback refuses non-private source addresses (§6.2)
- [ ] Pairing requires explicit owner action (§7.1)
- [ ] Pairing code is node-generated, 16-bit, 120s TTL, 5 attempts (§7.2, §7.5)
- [ ] Both emoji and word encodings accepted (§7.2)
- [ ] Variation selectors preserved on glyphs 8 and 9 (§7.3)
- [ ] No SAS confirmation step exists (§7.8)
- [ ] The pairing code is never transmitted as a bearer credential (§7.0)
- [ ] Pairing state persists per origin; loss requires re-pairing with no bypass (§7.6)
- [ ] Plain-HTTP pairing screens disclose the property-1 gap (§7.7)
- [ ] Pinned key mismatch has no override path (§8.1)
- [ ] Session layer never skipped on plain HTTP (§8.2)
- [ ] Unauthenticated endpoints leak no app data (§9.2)
- [ ] Sync rejects `seq` gaps (§10.2)
- [ ] Refuses apps declaring a higher `api` (§12)
- [ ] Cluster private key never leaves nodes; devices receive the public key only (§2.3.3)
- [ ] Node certificates expire at 180 days and renew on sync (§2.3.1)
- [ ] A device pinned to a cluster trusts a node it has never met, if signed (§2.3.2)
- [ ] Cluster-key mismatch has no override path (§2.3.2, §8.1)
- [ ] Discovery filters by TXT `cl` once paired (§6.1)
- [ ] No node is designated primary or authoritative (§10.3)
- [ ] Endpoint failover uses ≤2500 ms connect timeouts and re-attempts on network change (§10.4)
- [ ] Browser clients hold exactly one endpoint (§10.4, §10.8)
- [ ] Outbox replay relies on ULID idempotency, with no dedupe table (§10.6)
- [ ] `sys_device.replica` declared accurately (§10.7)

---

## 14. Open questions

Tracked, not decided. Do not implement speculatively.

1. **Log compaction.** `pv/1` never deletes events. A decade of daily use is perhaps tens
   of megabytes of text, so this is likely fine forever. If it is not, compaction must be
   designed together with a retention assertion (§5.4).
2. **Field-level merge.** Row-granularity LWW is a real limitation for concurrently edited
   free text. An `automerge`-backed column type is the likely answer, at the cost of a
   binary blob inside `d`.
3. **Sharing.** Multi-owner is out of scope. When it arrives it will need a capability
   model, not an ACL bolted onto `sys_app_grant`.
4. **Key rotation** (§2.3).
5. **Node key rotation** (§2.4) and a real revocation mechanism narrower than the 180-day
   certificate window (§2.3.4).
6. **NAT traversal.** Direct peer connections between nodes on different networks are not
   specified. The recommended answer is an always-on node (§10.5) or a VPN. Hole punching is
   a native-client capability that MAY be added without a protocol change, since it is a
   transport for the same §10.1 union.
7. **App data migrations.** Reserved, not implemented (`spec/data-dictionary.md §3.11`).
   The constraint is already fixed — a migration transforms events at replay and never
   mutates a log — but the transform language is undesigned, deliberately, until a real case
   exists.
8. **Attachments.** Binary blobs have no home in a JSONL log. Probably a content-addressed
   `blobs/` directory with hashes referenced from `d`, but that reintroduces a binary
   format into the backup story and needs thought.

---

Copyright © 2026 Gabriel Mongefranco
