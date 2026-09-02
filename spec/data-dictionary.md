<!--
Project:  Privatium™
File:     spec/data-dictionary.md
Authors:  Gabriel Mongefranco (@gabrielmongefranco)
Created:  2026-08-28
Modified: 2026-09-02
Summary:  NORMATIVE. System tables, app index, type mappings, and field definitions.
-->

# Data Dictionary — `pv/1`

Companion to `spec/protocol.md`. Defines the framework's own tables, the app index, and
the type system apps are written against.

---

## 1. Two stores

| Store | Path | Synced | In backup | Contents |
|---|---|---|---|---|
| **Replicated** | `data/_sys/log/<dev>.jsonl` | ✔ | ✔ | Device registry, app index, settings, grants, audit |
| **Local** | `local/state.jsonl` | ✘ | ✘ | Pairing codes, sync cursors, peer addresses, cache watermarks |

Anything an owner would expect to see on all their devices is replicated. Anything that
would be actively wrong to copy to another machine is local. Getting this split wrong is
how you leak a live pairing code into a backup.

`_sys` is a reserved slug and is materialized into the DuckDB schema `sys`. It uses the
identical event envelope as any app; the framework dogfoods its own storage layer.

---

## 2. Type system

App authors declare column types in `schema.sql` using DuckDB types directly. The scaffold
generator maps them to HTML controls as follows; hand-written templates may do anything.

| Logical type | DuckDB type | HTML control | Notes |
|---|---|---|---|
| `text` | `VARCHAR` | `input[type=text]` | `max_length` enforced server-side |
| `longtext` | `VARCHAR` | `textarea` | |
| `integer` | `BIGINT` | `input[type=number]` | |
| `decimal` | `DECIMAL(18,4)` | `input[type=number]` | precision overridable |
| `money` | `DECIMAL(18,2)` | `input[type=number]` | never a float |
| `percent` | `DECIMAL(9,4)` | `input[type=number]` | stored as `0.0725`, not `7.25` |
| `date` | `DATE` | `input[type=date]` | |
| `datetime` | `TIMESTAMPTZ` | `input[type=datetime-local]` | stored UTC |
| `time` | `TIME` | `input[type=time]` | |
| `duration` | `INTERVAL` | two numeric inputs | |
| `bool` | `BOOLEAN` | `input[type=checkbox]` | |
| `select` | `VARCHAR` | `select` | `options` list required |
| `multiselect` | `VARCHAR[]` | checkbox group | |
| `ref` | `VARCHAR` | `select` | ULID pointing at another row; `ref_view` required |
| `note` | — | rendered text | display-only, no column |

### 2.1 JSON encoding rules

Because `d` is JSON, values are encoded as follows on write and decoded on replay:

| DuckDB type | JSON |
|---|---|
| `VARCHAR`, `DATE`, `TIME`, `TIMESTAMPTZ`, `INTERVAL` | string |
| `DECIMAL`, `BIGINT` | **string**, not number |
| `BOOLEAN` | `true` / `false` |
| `VARCHAR[]` | array of strings |
| NULL | `null`, or key omitted (equivalent) |

`DECIMAL` and `BIGINT` are encoded as strings because JSON numbers are IEEE 754 doubles in
most parsers and would silently lose precision on money and large integers. This is
non-negotiable — it is the entire reason DuckDB was chosen over SQLite.

Dates are `YYYY-MM-DD`. Timestamps are RFC 3339 UTC with a literal `Z`.

---

## 3. System tables

All are event-sourced into the `sys` schema. `id` is a ULID unless stated otherwise.

### 3.1 `sys_node`

Singleton describing **this installation**. Exactly one row; `id` is the Node ID.

Every node also appears in `sys_device` with `kind = 'node'`, including this one. The two
tables answer different questions: `sys_node` is "who am I," `sys_device` is "who may talk to
this cluster." A node's `sys_device` row is what makes it revocable (§3.1c) and what carries
its `replica` flag.

| Column | Type | Notes |
|---|---|---|
| `id` | `VARCHAR` | Node ID (8-char Crockford Base32) |
| `display_name` | `VARCHAR` | Owner-set. Used as the mDNS instance name. |
| `pubkey` | `VARCHAR` | Ed25519 public key, base64 |
| `created_at` | `TIMESTAMPTZ` | |
| `protocol` | `VARCHAR` | e.g. `pv/1` |
| `build` | `VARCHAR` | `official` \| `custom` \| `fork:<name>` |
| `cluster_id` | `VARCHAR` | → `sys_cluster.id` |
| `cert` | `VARCHAR` | This node's cluster-signed certificate, base64 |
| `cert_expires_at` | `TIMESTAMPTZ` | Renewed on every successful sync |

### 3.1b `sys_cluster`

The set of nodes belonging to one owner (`spec/protocol.md §2.3`). Exactly one row.

| Column | Type | Notes |
|---|---|---|
| `id` | `VARCHAR` | Cluster ID (8-char Crockford Base32) |
| `pubkey` | `VARCHAR` | Ed25519 cluster public key, base64. **Never the private key.** |
| `pkarr_name` | `VARCHAR` | z-base32 encoding of the cluster public key — the DHT-resolvable name |
| `created_at` | `TIMESTAMPTZ` | |
| `created_by` | `VARCHAR` | Node ID of the founding node |
| `label` | `VARCHAR` | Owner-set, e.g. "Home" |

The cluster **private** key lives in `identity/cluster.key` on nodes only. It MUST NOT appear
in any event, log, snapshot, or backup export.

### 3.1c `sys_node_revocation`

| Column | Type | Notes |
|---|---|---|
| `id` | `VARCHAR` | Revoked Node ID |
| `revoked_at` | `TIMESTAMPTZ` | |
| `revoked_by` | `VARCHAR` | Node ID that issued the revocation |
| `reason` | `VARCHAR` | Nullable |

Replicated, so revocation propagates. Bounded by the 180-day certificate lifetime — see
`spec/protocol.md §2.3.4` for the honest statement of the gap.

### 3.2 `sys_device`

Every paired device, including browsers and other nodes.

| Column | Type | Notes |
|---|---|---|
| `id` | `VARCHAR` | Device Node ID |
| `label` | `VARCHAR` | Owner-set, e.g. "Pixel 9" |
| `kind` | `VARCHAR` | `browser` \| `desktop` \| `mobile` \| `node` |
| `replica` | `BOOLEAN` | Holds full logs and materializes locally. Nodes always; native mobile optionally; browsers never. Peers use this to decide whether to offer sync (`spec/protocol.md §10.7`). |
| `ed25519_pub` | `VARCHAR` | base64 |
| `x25519_pub` | `VARCHAR` | base64 |
| `paired_at` | `TIMESTAMPTZ` | |
| `paired_via` | `VARCHAR` | `lan` \| `iroh` \| `onion` \| `tunnel` |
| `last_seen_at` | `TIMESTAMPTZ` | Updated at most hourly — do not write an event per request |
| `user_agent` | `VARCHAR` | Nullable; browsers only |
| `revoked_at` | `TIMESTAMPTZ` | Nullable. Set = access denied immediately. |
| `revoked_reason` | `VARCHAR` | Nullable |

A row with `kind = 'node'` is a cluster member and holds the cluster private key; every other
kind holds the public key only (`spec/protocol.md §2.3.3`). Revoking a node also requires a
`sys_node_revocation` entry (§3.1c), because a node's certificate is presented to devices that
may not have synced `sys_device` recently.

Revocation is a `put` with `revoked_at` set, never a `del`. The historical record of what
was paired MUST survive.

### 3.3 `sys_pairing` — **local store only**

| Column | Type | Notes |
|---|---|---|
| `id` | `VARCHAR` | ULID |
| `code_hash` | `VARCHAR` | Argon2id of the 16-bit code + salt. **Never the code itself.** |
| `salt` | `VARCHAR` | base64 |
| `created_at` | `TIMESTAMPTZ` | |
| `expires_at` | `TIMESTAMPTZ` | `created_at` + 120s |
| `attempts` | `INTEGER` | Max 5 |
| `consumed_by` | `VARCHAR` | Device ID, nullable |
| `consumed_at` | `TIMESTAMPTZ` | Nullable |

Hashing a 16-bit value is not meaningfully preimage-resistant; the hash exists so that a
crash dump or stray log does not contain a live code, not as a security boundary. The real
protections are the 120-second TTL, the 5-attempt cap, and the PAKE.

### 3.4 `sys_app` — the app index

The registry that makes multi-app hosting work. One row per app folder the node knows
about.

| Column | Type | Notes |
|---|---|---|
| `id` | `VARCHAR` | The slug. Not a ULID — slugs are the natural key. |
| `title` | `VARCHAR` | Display name from `app.toml` |
| `version` | `VARCHAR` | Semver from `app.toml` |
| `api` | `INTEGER` | Framework API version the app targets |
| `tier` | `VARCHAR` | `lua` \| `web` \| `rust` |
| `icon` | `VARCHAR` | Icon token |
| `source` | `VARCHAR` | `bundled` \| `local` \| `url:<origin>` |
| `enabled` | `BOOLEAN` | Disabled apps keep their data and vanish from nav and mDNS |
| `nav_order` | `INTEGER` | Ascending. Ties broken by `title`. |
| `installed_at` | `TIMESTAMPTZ` | |
| `updated_at` | `TIMESTAMPTZ` | |
| `schema_hash` | `VARCHAR` | SHA-256 of `schema.sql`. Change triggers rematerialization. |
| `manifest_hash` | `VARCHAR` | SHA-256 of `app.toml` |
| `advertise` | `BOOLEAN` | Advertise a DNS-SD subtype for this app |
| `permissions` | `VARCHAR` | JSON. Non-default CSP and API permissions granted (Tier 2). |
| `last_error` | `VARCHAR` | Nullable. Load/validation failure text, shown in settings. |

**Rules:**

- The app index is replicated, so all your devices agree on which apps exist. The app
  *folders* are not replicated — a device that lacks the folder shows the app as
  unavailable rather than pretending it is gone.
- `source` says where the folder came from. `bundled` is a folder shipped with the
  framework — the repository's `apps/` in a development checkout, the package's at
  install. `local` is `<data-root>/apps/<slug>/`, the owner's, writable and surviving
  upgrades. `url:<origin>` is reserved: `pv/1` has no registry (`spec/app-contract.md
  §9`).
- One row per folder whose name is a valid, unreserved slug, written whether or not the
  app loaded: a refusal at any step of `spec/app-contract.md §8` sets `last_error` on it.
  `installed_at` is when the app first loaded cleanly and is NULL for a folder that never
  has; `enabled` is the owner's and survives every reload.
- Removing a folder MUST NOT delete the index row or the app's data. It sets
  `last_error = "folder missing"`.
- Uninstalling is an explicit owner action that sets `enabled = false`. Data deletion is a
  separate, separately-confirmed action.
- `id` is the slug, so an app renamed is an app replaced. Say so in the UI.

### 3.5 `sys_app_grant`

Which devices may access which apps. In `pv/1` the default is "all devices, all apps";
this table exists so that a shared-household node can hide the medication tracker from the
kitchen tablet without waiting for a protocol version.

| Column | Type | Notes |
|---|---|---|
| `id` | `VARCHAR` | ULID |
| `device_id` | `VARCHAR` | → `sys_device.id`, or `*` for all |
| `app_id` | `VARCHAR` | → `sys_app.id`, or `*` for all |
| `access` | `VARCHAR` | `none` \| `read` \| `write` |
| `granted_at` | `TIMESTAMPTZ` | |

Resolution: most specific match wins (`device+app` > `device+*` > `*+app` > `*+*`).
Absent any row, the default is `write`.

### 3.6 `sys_setting`

| Column | Type | Notes |
|---|---|---|
| `id` | `VARCHAR` | Setting key, dotted, e.g. `snapshot.retention_days` |
| `value` | `VARCHAR` | JSON-encoded scalar |
| `updated_at` | `TIMESTAMPTZ` | |

Settings are replicated. Node-specific things that must *not* replicate (listen port,
data directory, tunnel credentials) live in `config.toml` and the OS keyring, not here.

**Reserved keys:**

| Key | Default | Meaning |
|---|---|---|
| `snapshot.retention_days` | `365` | §5.4 |
| `snapshot.interval_days` | `7` | |
| `snapshot.min_events` | `100` | Also snapshot after N events, whichever first |
| `discovery.mdns` | `true` | LAN, multicast |
| `discovery.udp` | `true` | LAN, broadcast fallback |
| `discovery.pkarr` | `true` | Publish signed records to the mainline DHT (`spec/protocol.md §6.2`) |
| `discovery.pkarr_publish` | `true` | Publishing may be disabled independently of resolving |
| `discovery.dns` | `true` | Resolve pkarr records over DNS where the DHT is blocked |
| `discovery.dns_origin` | `""` | Empty uses the library default; set to a self-hosted pkarr relay |
| `p2p.enabled` | `true` | Hole punching for direct peer connections |
| `p2p.relay_url` | `""` | Empty uses the library default; set to a self-hosted relay |
| `p2p.relay_only` | `false` | Skip hole punching entirely — for hostile networks |
| `pairing.ttl_seconds` | `120` | |
| `pairing.max_attempts` | `5` | |
| `ui.locale` | `en-US` | |
| `ui.date_format` | `iso` | `iso` \| `us` \| `eu` |

### 3.7 `sys_peer` — **local store only**

| Column | Type | Notes |
|---|---|---|
| `id` | `VARCHAR` | Peer node ID |
| `p2p_node_id` | `VARCHAR` | Nullable; direct-transport identity where available |
| `last_addrs` | `VARCHAR[]` | Cached hints; may be stale, never authoritative |
| `last_sync_at` | `TIMESTAMPTZ` | |
| `transport` | `VARCHAR` | `lan` \| `p2p` \| `file` |

### 3.7b `sys_endpoint` — **local store only**

The candidate endpoint list (`spec/protocol.md §10.4`). Node-local because a good address on
one device is meaningless on another.

| Column | Type | Notes |
|---|---|---|
| `id` | `VARCHAR` | ULID |
| `url` | `VARCHAR` | e.g. `http://192.168.1.5:8420` |
| `kind` | `VARCHAR` | `lan-mdns` \| `lan-udp` \| `lan-ip` \| `pkarr` \| `dns` \| `ddns` \| `tunnel` \| `vpn` \| `p2p` \| `relay` |
| `via` | `VARCHAR` | Nullable. Which discovery mechanism produced this entry. |
| `node_id` | `VARCHAR` | Which node this reaches, once known |
| `last_ok` | `TIMESTAMPTZ` | Primary sort key — recency beats category |
| `last_fail` | `TIMESTAMPTZ` | Nullable |
| `rtt_ms` | `INTEGER` | Nullable |

Browser clients hold exactly one row: their own origin. Multi-endpoint failover is a
native-client capability.

### 3.8 `sys_sync_state` — **local store only**

| Column | Type | Notes |
|---|---|---|
| `id` | `VARCHAR` | `<peer_id>:<app>:<origin_dev>` |
| `peer_id` | `VARCHAR` | |
| `app_id` | `VARCHAR` | |
| `origin_dev` | `VARCHAR` | The device whose log this cursor tracks |
| `their_seq` | `BIGINT` | Highest `seq` we believe the peer holds |
| `our_seq` | `BIGINT` | Highest `seq` we hold |
| `updated_at` | `TIMESTAMPTZ` | |

### 3.9 `sys_snapshot`

Index of snapshots, mirroring what is on disk. Replicated so that any device can tell the
owner when the last good snapshot was taken, even if it does not hold the files.

| Column | Type | Notes |
|---|---|---|
| `id` | `VARCHAR` | Snapshot ID, e.g. `2026-W35-k7m2q9xf-8830` |
| `app_id` | `VARCHAR` | |
| `created_at` | `TIMESTAMPTZ` | |
| `hi_lam` | `BIGINT` | |
| `row_counts` | `VARCHAR` | JSON object, table → count |
| `bytes` | `BIGINT` | |
| `created_by` | `VARCHAR` | Node ID |
| `verified_at` | `TIMESTAMPTZ` | Nullable. Last successful checksum verification. |

### 3.10 `sys_audit`

Security-relevant events. Replicated, so a pairing on the laptop is visible from the phone.

| Column | Type | Notes |
|---|---|---|
| `id` | `VARCHAR` | ULID |
| `at` | `TIMESTAMPTZ` | |
| `kind` | `VARCHAR` | See below |
| `actor` | `VARCHAR` | Device ID or `system` |
| `subject` | `VARCHAR` | Nullable — device, app, or snapshot ID |
| `detail` | `VARCHAR` | JSON object |
| `severity` | `VARCHAR` | `info` \| `warn` \| `alert` |

**`kind` values (normative):**

`pair.opened`, `pair.attempt`, `pair.success`, `pair.failed`, `pair.expired`,
`node.admitted`, `node.revoked`, `cluster.created`, `cluster.rotated`, `cert.renewed`,
`cert.expired`, `endpoint.failover`, `outbox.replayed`, `pkarr.published`, `pkarr.blocked`,
`p2p.direct`, `p2p.relayed`, `discovery.method`,
`device.revoked`, `key.mismatch`, `app.installed`, `app.enabled`, `app.disabled`,
`app.load_failed`, `snapshot.created`, `snapshot.pruned`, `restore.tier2`,
`restore.tier3`, `clock.skew`, `event.rejected`, `sync.peer_seen`, `config.changed`.

`key.mismatch`, `node.admitted`, `cluster.rotated`, and `restore.tier3` MUST be `alert` and MUST surface in the UI, not only in
the log.

### 3.11 `sys_migration` — reserved

**Not implemented in `pv/1`.** The table and the `migrations/` folder
(`spec/app-contract.md §4`) are reserved so that adding them later is not a schema change.

Schema edits do not need migrations: changing `schema.sql` rematerializes from the logs, and
a new column is simply NULL for events that predate it (`spec/app-contract.md §4.5`).
Migrations are only needed when the *meaning* of stored data changes — a unit conversion, a
re-encoding, a renamed field whose old values must be rewritten on read.

Nothing in the reference apps needs that yet, and specifying a transform language before a
real case exists is how the declarative tier happened. When a case appears, the design
constraint is fixed: a migration transforms **events at replay time** and MUST NOT mutate a
log. See Open Question 8 in `spec/protocol.md §14`.

Reserved shape:

| Column | Type | Notes |
|---|---|---|
| `id` | `VARCHAR` | `<app>:<version>` |
| `app_id` | `VARCHAR` | |
| `from_version` | `VARCHAR` | |
| `to_version` | `VARCHAR` | |
| `applied_at` | `TIMESTAMPTZ` | |
| `schema_hash` | `VARCHAR` | Post-migration hash |

Migrations transform *events*, not tables — because tables are derived. An app migration is
a mapping applied at replay time, expressed as SQL. This row records that a node has
acknowledged the app is now at a given version; it does not record a mutation, because
none occurred.

---

## 4. Framework views

Views the framework exposes to apps and to the shell UI. Apps MAY read these; they MUST
NOT write to `sys` tables.

| View | Returns |
|---|---|
| `sys.v_app_nav` | Enabled apps, ordered, with icon and title — powers the launcher |
| `sys.v_device_active` | Devices where `revoked_at IS NULL` |
| `sys.v_health` | Restore tier in use, last snapshot age, log sizes, unsynced peer count |
| `sys.v_audit_recent` | Last 200 audit rows, newest first |

---

## 5. Naming conventions

Normative for both system and app tables.

| Rule | Example |
|---|---|
| Table names singular, `snake_case` | `fill`, not `fills` |
| System tables prefixed `sys_` | `sys_device` |
| Views prefixed `v_` | `v_upcoming_refill` |
| Primary key is always `id` | |
| Foreign keys are `<table>_id` | `device_id` |
| Timestamps end `_at`; dates end `_on` | `paired_at`, `filled_on` |
| Booleans read as assertions, no `is_` prefix | `enabled`, not `is_enabled` |
| Money columns end `_amount`, currency in a sibling `_currency` | `copay_amount` |
| No column named `date`, `time`, `order`, `group`, `user` | reserved words |

---

## 6. Worked example

The `hello` app's complete state after a name change, as it exists on disk:

`data/hello/log/k7m2q9xf.jsonl`
```
{"seq":1,"lam":1,"ts":"2026-08-28T14:03:11.412Z","dev":"k7m2q9xf","app":"hello","op":"put","tbl":"profile","id":"01J9YQ2W7C8XKF3M0N5RTVB6ZP","d":{"display_name":"Gabe"}}
{"seq":2,"lam":2,"ts":"2026-08-28T14:07:44.008Z","dev":"k7m2q9xf","app":"hello","op":"put","tbl":"profile","id":"01J9YQ2W7C8XKF3M0N5RTVB6ZP","d":{"display_name":"Gabriel"}}
```

Replay: group by `id`, order by `(lam, ts, dev)`, take last → one row,
`display_name = 'Gabriel'`. The first line is history and stays forever.

That file is the entire backup. That is the point.

---

Copyright © 2026 Gabriel Mongefranco
