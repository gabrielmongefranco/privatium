<!--
Project:  Privatium™
File:     docs/decisions/0007-household-profiles.md
Authors:  Gabriel Mongefranco (@gabrielmongefranco)
Created:  2026-09-07
Modified: 2026-09-07
Summary:  Decision record. Household profiles as a pv/1 partition, at-rest encryption
          declined, an ephemeral app channel for per-frame traffic, and the reservations
          that keep real multi-user reachable in pv/2. Status: DECIDED for the pv/1
          hooks; the pv/2 model and the ephemeral channel are DEFERRED.
          See main README.md for full license information.
-->

# ADR 0007 — Household profiles, segmented data, and the pv/2 multi-user path

**Status: DECIDED for the `pv/1` hooks. The `pv/2` multi-user model is DEFERRED, and the
ephemeral channel of D16 is DEFERRED, with the reservations of section 5 held open
deliberately.**

**Nothing in this record is scheduled for Phase 3.** `docs/plans/phase-3.md` is in flight
and its milestones own `spec/protocol.md §10`, `§2.3.1` and `§3` for the duration. Section
6 below is a list of edits a later phase makes, not a backlog for the current one. An
agent reading this while Phase 3 is open should change nothing but this file.

## Context

`pv/1` is single-owner by construction. `docs/architecture.md §8` lists multi-user
accounts as deliberately absent, and `docs/roadmap.md` lists multi-user sharing under
"Explicitly not on the roadmap". Both entries stand as descriptions of `pv/1`.

What changed is intent, not scope. The owner now intends real multi-user in `pv/2`, and
wants a household-facing feature sooner. Two things drive it: family members who each
want their own view of an app, and apps that exchange state between nodes many times a
second, which the log cannot carry. This ADR answers one question: **which `pv/1`
decisions would make either of those unreachable, and what does `pv/1` reserve so that it
does not?**

Nothing here bolts multi-user onto `pv/1`. Section 3 is a partition and a UI. Section 5 is
a short list of reservations that cost almost nothing now and cannot be retrofitted later,
because log lines are immutable, `pv/1` never rewrites a log, and `migrations/` is
reserved and unimplemented (`spec/data-dictionary.md §3.11`).

## 1. Terms, used precisely throughout

| Term | Meaning |
|---|---|
| **Profile** | A household member as `pv/1` models them. A partition key. |
| **Principal** | An identity that can be *authenticated and proven to a third party*. `pv/2`. |
| **Segment** | The on-disk subtree holding one profile's data for one app. |
| **Owner** | Unchanged from `spec/protocol.md §1`. The single human who controls the cluster. |

The distinction that matters: a partition key answers "which rows are hers." A principal
proves "this is her." Inside one trust domain the first is sufficient. Across trust
domains only the second works, and that is the entire content of the `pv/1` to `pv/2` gap.

## 2. What is being built, stated honestly

> Profiles keep household members out of each other's data *in the app*. They do not hide
> anything from anyone with access to the node's files. Everyone's data is backed up on
> every node equally.

That sentence is normative UI copy, not commentary. Following D8 it is permanently true
rather than provisional pending encryption, and it should be written that way.

It is the same deal a family NAS or a shared media server offers, and it is defensible
precisely because it is stated rather than implied. Same disclosure posture as
`spec/protocol.md §7.7`, which requires an implementation to state the plain-HTTP
bootstrap exposure rather than imply it away, and `§4.6`, where a tombstone is openly not
a deletion.

## 3. Decisions — `pv/1`

### D1. Profiles are a partition. They are never called accounts.

A PIN-gated selector choosing a partition key is not an access-control boundary on a node
whose `data/` is plaintext JSONL and whose cluster key is held by every node. Calling it
an account makes a promise the storage layer does not keep.

### D2. Enforcement lives in the framework, not in apps. Default deny.

The query layer scopes every read to the active profile before app code sees a row. An app
that wants otherwise declares it (D14). Rationale: N app-level implementations means the
partition is only as strong as the worst one, and the failure is silent.

**This is not the first per-subject access check in the system, and it must not duplicate
the one that exists.** `sys_app_grant` (`spec/data-dictionary.md §3.5`) already answers
"which devices may access which apps", with `device_id` and `app_id` each accepting `*`,
resolution by most-specific match, and `write` as the default when no row matches. Its
stated purpose is this exact scenario — hiding the medication tracker from the kitchen
tablet. Profile scoping is a **row filter within an app**; `sys_app_grant` is **whether
the app opens at all**. They compose: the grant decides the mount, the segment decides the
rows. Any implementation that finds itself re-deriving grant resolution has taken a wrong
turn.

**Tier 3 is explicitly out of scope for this enforcement.** A Tier 3 app links
`privatium-core` and is not sandboxed; it can already read `identity/node.key` and every
log file directly. Profile scoping there is a convenience of the core API, not a boundary,
and the Tier 3 skill must say so in the same terms it already uses for filesystem access.

The scope belongs on the calls that read and write, not on `Node::open`. `Node::open`
takes a data root (`spec/app-contract.md §2.3`) and embedded mode has no session to carry
a profile; `query`, `append` and `open_app` already take a slug, and the segment travels
beside it. A Tier 3 app that passes no segment sees everything, exactly as it does today.

### D3. Profile is a property of the session, and switching is the shell's job.

**Problem this resolves:** a solo-mode or Tier 2 app renders no framework chrome by
design, because `docs/architecture.md §7` promises solo mode is indistinguishable from a
purpose-built application. If profile display required a Privatium header, solo mode would
be dead and Tier 3 would be unreachable.

Resolution, in five parts, none of which mandates chrome:

1. **The profile is selected before the app loads.** A session carries exactly one active
   profile, chosen at the PIN screen. An app never chooses, and cannot change, the profile
   it runs under.
2. **The switcher lives under `/settings`, and adds no route prefix.** `spec/protocol.md
   §9.1` reserves exactly six framework prefixes and `/settings` is one of them; it
   survives solo mode, where a colliding app route is shadowed in favour of the framework
   with a warning at load. A switcher at `/settings/profile` therefore needs no change to
   §9.1 at all. **`pair` is not a precedent for a seventh prefix** — it is a reserved slug
   (§1.1) covering the mounts of `/ws/pair` and `/api/v1/pair`, and there is no top-level
   `/pair` route. Any app links the switcher with `url()` or `pv.url()`.
3. **`profile` is added to the reserved slug list** (`spec/protocol.md §1.1`) — not
   because the route design needs it, which under (2) it does not, but because a slug is a
   one-way door: reserving it now costs nothing, and reserving it after someone has
   installed an app called `profile` breaks that app. §1.1's closing sentence names the
   last two entries as route prefixes and must be reworded when the list grows.
4. **`GET /a/<slug>/api/node` and `pv.node()` gain the active profile's ID, display name,
   and role.** The response today carries `id`, `dev`, `name`, `app`, `solo`, `peers` and
   `restore_tier` (`spec/data-api.md §4`); the profile joins it as a sibling of `dev`, and
   is `null` on a node with no profiles configured. Apps MAY display it; the launcher and
   the native shells MUST.
5. **A profile choice must survive a reconnect, and the channel does not carry it.**
   `spec/protocol.md §8.3` is explicit: "The session is the connection: no cookie carries
   it, and a new connection is a new handshake", and the handshake authenticates `dev`
   alone. A profile held only against a WebSocket is lost every time the network blips.
   Either the client re-asserts the profile as its first framed request after each
   handshake, or the node remembers the last profile per device. The second is friendlier
   and the first is safer; the choice is OQ6.

Consequence to accept rather than fix: a Tier 2 app owning its whole viewport can render
whatever it likes and is not obliged to tell the user which profile is active. It also
cannot read another profile's data, so this is a UX failure and not a security one.
`privatium lint` SHOULD warn when a Tier 2 app neither links `url('/settings/profile')`
nor calls `pv.node()` on a node with profiles configured. **There is no `fullscreen`
declaration in `app.toml` and this ADR does not add one** (`spec/app-contract.md §3`); the
real distinctions are the tier and the deployment mode, both of which already exist.

### D4. Household members SHOULD use thin clients. Member nodes remain supported.

A node holds the cluster private key and a complete plaintext replica
(`spec/protocol.md §2.3.3`). Admitting a member's laptop to the cluster gives that member
everything, on hardware the owner does not administer. Mechanically identical to running a
full node on a VPS, which `docs/deployment.md §2` already covers with its four-functions
table; the difference is that the VPS threat is compromise and this one is ordinary use.

**This is a recommendation, not a rule, and the documentation MUST present it as a
choice.** Households that share everything are the common case, and a second node is the
best answer for replication and availability. What the docs owe the owner is the
consequence stated plainly at the moment of admission, in the same register as §2's table:

| Member client | Holds | Can read | Replication contribution |
|---|---|---|---|
| Browser or PWA (`replica = false`) | nothing | only their own profile, via the node | none |
| Caching mobile | cached views, outbox | only their own profile | none |
| **Full node** | **complete plaintext replica, cluster key** | **everything, every profile** | **full** |

The admission flow (`spec/protocol.md §2.3.1`) MUST show the third row before the pairing
code is accepted. Recommend thin; do not forbid full. Which trade a household wants is not
the project's decision to make.

### D5. Every node replicates every segment. Sync is never scoped by profile.

Restricting sync inside a cluster buys nothing and costs durability. Every node holds the
cluster key and can request any log at any time, so a sync restriction is a preference
between machines that already fully trust each other, not a boundary.

Everyone's data backs up on every node equally. This is a stated project goal, not a side
effect.

### D6. Segment-per-profile on disk, not a column.

```
data/<slug>/<segment-id>/log/<device-id>.jsonl
data/<slug>/<segment-id>/snap/<snapshot-id>/
data/<slug>/_shared/log/<device-id>.jsonl
```

`_sys` is **not** segmented. It is the node's own registry — devices, apps, settings,
audit — and it belongs to the cluster rather than to a person.

Single-writer holds unchanged: the device is still the sole writer of its own file.

This decision was originally justified largely by per-segment encryption, which D8 now
declines. It was re-examined against a plain `profile` column and **stands**, on three
remaining grounds:

1. **Removal is otherwise impossible.** `spec/protocol.md §4.6` states that the supported
   way to destroy data irrecoverably is to destroy `data/` — the whole directory, every
   app, every person. With a column, a member who leaves the household can never have
   their data removed from any node, ever, because that would require rewriting logs. With
   a directory it is one path, on each node, and the append-only invariant is intact. Note
   what this is: not a narrower spelling of something §4.6 already allows, but the first
   removal narrower than the whole store. OQ5 is where that is paid for.
2. **`pv/2` sharing is a subtree sync.** Sync is already a set union over files keyed by
   `(app, dev, seq)`. Scoping it to a subtree is natural; filtering every line by a field
   is a different algorithm.
3. **Isolation is verifiable with `ls`** rather than by reading the query planner.

Honest accounting of the cost, which is real: `seq` is already per `(app, device)`
(`spec/protocol.md §4.1`) and must become per `(app, segment, device)`, because
`spec/protocol.md §10.2` requires a receiver to refuse a gap in `seq`, and a device
writing to two segments of one app would otherwise produce gaps in both files. §10.2 also
requires app and device names to be validated before a path is constructed; the segment ID
becomes a third component under the same rule. That is a normative change to §10 and the
main complexity this decision buys.

### D7. Reserve `usr` in the event envelope now.

`pv/1` stamps it and otherwise ignores it. `spec/protocol.md §4.2` already requires readers
to accept and preserve unknown top-level fields, and names this as the mechanism by which
a `pv/1` node and a `pv/2` node share a log without either losing information. So this is
backward compatible by construction.

Named `usr` rather than `user` because every envelope key is short and lowercase — `seq`,
`lam`, `ts`, `dev`, `app`, `op`, `tbl`, `id` — and a four-letter outlier reads as a
different kind of field. `spec/data-dictionary.md §5` separately forbids a *column* named
`user` as a reserved word, which is corroboration rather than the reason: envelope keys
are not columns.

Rationale: `dev` answers "which device wrote this." Once two people share a node, nothing
answers "which person," and no later change can answer it for lines already written.

**This is the only identity reservation `pv/1` needs.** An earlier draft of this ADR also
required an Ed25519 identity keypair per profile from day one. That was wrong and is
withdrawn. `pv/1` does not sign individual events at all — attribution rests on the
authenticated session and the single-writer file, for devices as much as for profiles — so
a profile key would prove nothing retroactive that device keys do not already fail to
prove. `pv/2` can mint a keypair then and bind it to the profile ULID that `usr` already
records.

### D8. No at-rest encryption. Not optional, not later.

**Rejected because it breaks a hard project constraint: copying `data/` must be a complete
backup that a non-technical person can perform.** An encrypted segment copies fine and
restores to nothing, because the key is deliberately not in `data/` and therefore not in
the backup. Fixing that means the restore instructions grow a key-management step, which
is the constraint failing. `docs/backup-and-restore.md` is the page that would have to
carry it, and `AGENTS.md` names that page as the bar for a document an owner reads under
stress. Usability wins.

Two consequences worth stating so nobody re-derives them:

- Every open question in the previous revision concerning locked segments, key escrow,
  escrow visibility, owner-role key transfer, snapshotting ciphertext, and where an
  unwrapped key lives in memory is **closed by deletion**. Five of ten went away.
- The honest statement in section 2 is now permanent rather than provisional. Profiles are
  never a boundary against anyone with the node's files, at any protocol version.

**One factual correction for the record**, because it arose in discussion and should not
survive into the repository: small or highly structured plaintext does **not** weaken
modern authenticated encryption. AES-256-GCM and ChaCha20-Poly1305 are not meaningfully
easier to attack on a 40-byte JSON line than on a 40 GB file, and known plaintext structure
is not an attack on either. What small structured records genuinely leak, even under
correct encryption, is *metadata*: file sizes, line counts, and write timing expose
activity patterns. That was never the reason to decline encryption, and this decision does
not rest on it. The backup constraint does all the work on its own.

### D9. Merging profiles is an alias. Nothing moves and nothing is rewritten.

**Why events cannot simply be rewritten and re-synced**, since this is the obvious
question:

- Sync is a set union over `(app, dev, seq)` (`spec/protocol.md §10.1`). That union is
  correct only because those three fields uniquely determine a line's content. Rewrite a
  line's `usr` on one node and two nodes hold different content for the same identity,
  with no conflict-resolution rule anywhere in the protocol, because the design
  deliberately has none. The result is silent divergence, not a merge.
- Peers already hold the old lines. Rewriting on one node does not unwrite them elsewhere.
- `§10.2` requires a receiver not to re-serialize, normalize or remove anything: bytes in,
  bytes out.
- Snapshot manifests carry SHA-256 of the materialized output (`§5.2`) and would mismatch.

So: a `sys_profile_alias` event asserts that segment B belongs to the same person as
segment A. The read path unions both. No bytes move, the invariant holds, and the
assertion is reversible because it is just another event.

The escape hatch, if a physical merge is ever genuinely wanted: append *copies* of the rows
into the target segment as new events with new ULIDs. That is a fork, not a move. The
originals remain readable forever and the data is duplicated. The alias is better in every
case identified so far.

### D10. Profile ID is a ULID. Display name is mutable and may collide.

`[node]-[user]` is rejected: it bakes a mutable label into an identifier that appears in
immutable log paths, so renaming a node would orphan a segment. The same lesson is already
recorded for `sys_app.id` being the slug, where renaming an app is replacing an app.

Display collisions are a UI problem. The picker shows "Ana (desktop)" and "Ana (laptop)".

### D11. Moving a profile between nodes reuses the §7 pairing flow.

Same PAKE, same 16 bits, same 120-second TTL, same 5-attempt cap, same `sys_audit` event.
Different subject: a profile rather than a device.

With D8 there is no key material to transfer, so this reduces to authorizing the alias of
D9 across two nodes rather than moving secrets. The ceremony is still warranted: an alias
asserts that two data sets belong to one person, which is not something an unauthenticated
party should be able to claim.

Two §7 rules carry over unchanged. The PAKE authenticates and derives in one operation, so
an implementation MUST NOT run it and then send the code as a bearer credential. And §7.8
forbids a short-authentication-string confirmation screen; a profile move does not earn one
either.

### D12. `role` is `owner` or `member`.

Needed so D14's `shared-readonly` has a subject to name, and so D3's shell UI knows what to
offer. It is not the `owner` of `spec/protocol.md §8.4`, which is a request's standing —
loopback, this machine's own addresses, or an in-process call — and is unrelated to which
person is at the keyboard. The spec text must keep the two apart by name.

### D13. PIN policy mirrors pairing.

Translated from `spec/protocol.md §7.5` rather than copied, since a PIN is long-lived and a
pairing code is not:

| Rule | Value |
|---|---|
| Failed attempts before lockout | 5 |
| Lockout duration | 120 seconds, then reset |
| Rate limit | no more than 1 attempt per 2 seconds per source |
| Audit | every attempt, success or failure, writes a `sys_audit` event |
| Replication | lockout state is node-local; audit events replicate |

A PIN has no TTL, because unlike a pairing code it is not single-use. §7.5's 120 seconds
becomes the lockout window rather than the code's lifetime, and §7.5's "the code is
destroyed and a new one issued" has no analogue: a PIN survives its own lockout.

`sys_audit.kind` is a **normative closed list** (`spec/data-dictionary.md §3.10`), so this
adds to it rather than reusing `pair.*`: `profile.created`, `profile.attempt`,
`profile.failed`, `profile.locked`, `profile.switched`, `profile.aliased`. The PIN itself
never reaches `detail`, which is the general rule in `AGENTS.md` about secrets and row
contents, not a special case for this table.

### D14. Shared tables are declared in the manifest.

```toml
[tables]
recipe = "shared"            # all profiles read, all profiles write
chores = "shared-readonly"   # all profiles read, role = owner writes
meds   = "private"           # default, per segment
```

- Default is `private`. Sharing is explicit, shown to the owner at install like any other
  permission (`spec/app-contract.md §5.4` is the established pattern), and `privatium lint`
  flags an unscoped read in an app that declared nothing.
- Shared data lives in the reserved `_shared` segment (D6). Single-writer still holds.
- `shared` inherits last-write-wins at row granularity by `(lam, ts, dev)`
  (`spec/protocol.md §4.5`). Two people editing one row moves from theoretical to routine,
  so apps using `shared` should model rows small. §4.5's existing instruction — an app
  needing field-level merge MUST model each field as its own row — stops being an edge case
  here.

### D15. One node per OS user stays documented as the hard-isolation option.

It already works with no new code: `spec/cli.md §1` provides `--data-dir` and `§2` provides
`--port`. Two OS users on one desktop already get two independent nodes today, and the
isolation is enforced by the kernel rather than by our code. `spec/protocol.md §3.1`'s
`local/lock` makes the separation total: one process per data root, refused otherwise.

With D8 declining encryption, this is now the **only** configuration in which one household
member's data is genuinely unreadable by another. It should be documented that way, plainly.

| | Profiles on one node | One node per OS user |
|---|---|---|
| Isolation | convention, enforced by the framework | kernel |
| Unreadable by other members | no | yes |
| Shared apps and data | native | only via cross-cluster sharing, `pv/2` |
| Works on a shared tablet | yes | no |
| Backup | one `data/` | N of them |
| Path to multiplayer | yes | none |

These are the two ends of a slider, not competing designs.

### D16. Real-time app traffic is an ephemeral channel, not the log.

Not implemented in `pv/1`, and not in Phase 3. Recorded here because it is half the reason
the profile work matters and because its shape constrains D6 and D7.

Per-frame state MUST NOT be appended. Four players at 20 Hz for three minutes is roughly
14,000 permanent, replicated, replayed-forever lines, each one syncing to every device.
`skills/privatium-games/SKILL.md` already says to save on meaningful boundaries — level
complete, checkpoint, quit — and that guidance is the shape of this decision, not a
placeholder for it.

The eventual shape: app-scoped messages riding the authenticated session, never stamped,
never appended, never snapshotted, never synced. This violates no invariant, because
`AGENTS.md` invariant 1 says JSONL is the only *truth* and a position hint is not truth.

**The shape is the data API, not a new transport.** A new SSE event type on
`GET /a/<slug>/api/stream` outbound, and one small POST inbound. It is deferred, but it is
decided, because the alternative was costing more than it bought:

- **It needs no change to `spec/protocol.md §8.3` at all.** §8.3 defines five frame kinds
  and says the channel "defines no route of its own"; a sixth kind looked unavoidable and
  is not. Outbound, SSE already rides `res` plus interleaved `chunk`s — §8.3 says so in
  as many words, naming `spec/data-api.md §3` as the reason answers to different ids may
  interleave. Inbound, a small JSON POST is an ordinary `req`. Only a *streamed request
  body* is reserved and refused in `pv/1`, and an ephemeral message is not one.
- **The LAN case is the channel case, which is the same code path.** On a plain-HTTP LAN
  origin the data API is reachable only through the channel (`spec/data-api.md §5`,
  `spec/protocol.md §8.4`), and this is expected to be where the feature lives. There is no
  second implementation for LAN and remote: the same two endpoints work on loopback, in a
  native shell, on an HTTPS origin, and framed inside the channel.
- **The security fences come for free.** `spec/data-api.md §2.1` already refuses a POST
  that is not `application/json`, and refuses any request a browser marks
  `Sec-Fetch-Site: cross-site`, on every route before anything is read. An ephemeral
  endpoint inherits both and adds no new surface. It carries no token, for the reason §2.1
  gives.
- **It is node-local.** Messages fan out to sessions on this node and are not relayed to
  peers. The ordinary household shape is one node and several paired devices, so this
  covers the case that motivates it; cross-node relay is deferred with the rest of `pv/2`.
- **There is no direct browser-to-browser path anywhere in the stack, and this ADR does
  not add one.** No WebRTC, no data channel. The route is device → node → device. On a LAN
  that is fine. Anyone modelling a twitch game across two houses should measure before
  designing, and `AGENTS.md` invariant 9 forbids solving it with a primary node.

Two things it must not do quietly. `spec/data-api.md §2` ends with **"Nothing else"** —
there is no framework-defined action endpoint, and `POST /a/<slug>/api/x/<action>` was
removed with the declarative tier. An ephemeral endpoint is not that: it invokes no named
server-side action and runs no app logic, it hands a message to the other sessions of the
same app. The spec edit has to say so in that paragraph, or a future session correctly
reading "Nothing else" will delete it — the same failure D17 guards against. And it needs
its own rate setting beside `spec/data-api.md §7`'s, because none of the existing ones
bounds it: 20 Hz per player is well under `api.sql_rate` and unrelated to
`api.max_batch`. One SSE stream per player sits comfortably inside `api.max_streams`
(8 per device).

The honest cost of the cheap shape: inbound is request/response, so each message is a
`req`/`res` pair rather than a fire-and-forget push. On a LAN that is affordable. A c2s
push frame is the optimization to reach for if measurement ever demands it — an
optimization, not a prerequisite, and not a reason to design for it now.

**Address it to a profile pair, not to a cluster.** In `pv/1` that resolves to profiles on
one node and is nearly a no-op; in `pv/2` the same API means two people, and no app has to
be rewritten. This is why D16 belongs in this record rather than a separate one.

### D17. Amend `docs/architecture.md §8` **and** `docs/roadmap.md`.

Two documents say multi-user is not happening, and both must be corrected together or the
tree contradicts itself:

- `docs/architecture.md §8`'s "deliberately absent" entry for multi-user accounts becomes
  deferred to `pv/2` **with the reservations of section 5 held open**, listing them.
- `docs/roadmap.md`'s "Explicitly not on the roadmap" list opens with multi-user sharing.
  The honest correction is narrower than deletion: multi-user *sharing as a hosted
  service* stays off the roadmap, which is what the paragraph's own reason says — each
  item there "turns a personal tool into a service". Household profiles do not. They
  belong under the roadmap's "Open questions, not yet scheduled", whose framing — worth
  prototyping before worth specifying, none a deliverable — is exactly right for both this
  and D16.

Without this, a future session correctly following the project's own no-invented-spec rule
will find unused machinery, conclude it is scope creep, and delete it. `docs/decisions/0004`
records the inverse failure (an invented `--bind` flag, an invented `kind = "console"`);
this is the same class of error running the other way.

## 4. Rejected

| Option | Why |
|---|---|
| At-rest encryption of any segment | Breaks the "copy `data/` is a complete backup" constraint (D8) |
| A per-profile data key, and escrow | Follows from the above |
| A per-profile identity key in `pv/1` | Proves nothing retroactive; mintable in `pv/2` against the ULID `usr` already records (D7) |
| Per-profile sync scoping | No security gain inside a cluster; costs durability (D5) |
| A `profile` column instead of a segment directory | Re-examined after D8 and still rejected: makes per-person removal impossible, and makes `pv/2` sharing a line filter rather than a subtree sync (D6) |
| Enforcement in each app | Partition is only as strong as the worst app, and it fails silently (D2) |
| A second access-resolution table beside `sys_app_grant` | It already resolves device × app with `*` and most-specific-match; segments filter rows within an app and compose with it (D2) |
| A seventh framework route prefix for `/profile` | `/settings` already survives solo mode and is already shadow-resolved in the framework's favour; `pair` is a reserved slug, not a top-level route (D3) |
| A `fullscreen` key in `app.toml` | No such key exists, and a config key describing a UI is what `AGENTS.md` forbids; tier and deployment mode already carry the distinction (D3) |
| **Forbidding** household members from running nodes | Not the project's call. Recommend thin, state the consequence, let the owner decide (D4) |
| Rewriting `usr` on existing events and re-syncing | Breaks the `(app, dev, seq)` set-union property; silent divergence with no resolution rule (D9) |
| Deleting a profile | Append-only; alias instead (D9) |
| `[node]-[user]` as a profile ID | Mutable label inside an immutable path (D10) |
| A mandatory framework header for profile display | Kills solo mode and cannot reach Tier 3 (D3) |
| Appending per-frame state, at any rate | 20 Hz is thousands of permanent replicated lines per session (D16) |
| A sixth channel frame kind for D16 | SSE already rides `res` and interleaved `chunk`s, and an inbound message is an ordinary `req`; §8.3 needs no change (D16) |
| A direct browser-to-browser transport for D16 | Not in the stack, and not justified by one deferred feature (D16) |
| Cross-node relay of ephemeral messages in `pv/1` | The household case is one node and several devices; relay is `pv/2`'s (D16) |
| Calling any of this an "account" | Promises a boundary the storage layer does not provide, at any version (D1, D8) |

## 5. Reservations that keep `pv/2` reachable

Reduced from six to four by D7 and D8, then restored to five by what `sys_audit` needs.

| # | Reservation | Cost if skipped |
|---|---|---|
| R1 | `usr` in the envelope | Historical events are permanently unattributable to a person |
| R2 | Segment directories rather than a column | `pv/2` sharing becomes a line filter; per-person removal becomes impossible |
| R3 | A segment scope parameter on the sync endpoints, even if `pv/1` always passes `*` | `/api/v1/sync/heads`, `pull` and `push` are keyed by `app` and `dev` alone and assume total trust; the shape becomes a one-way door |
| R4 | `sys_app_grant` resolution written as "resolve subject", not "look up device" | A polymorphic subject becomes a rewrite rather than a column |
| R5 | `sys_audit.actor` documented as a subject, not as "device ID or `system`" | Every security-relevant act in the household is permanently attributed to a machine rather than a person |

**R4 is about code shape, not about growing the table.** `spec/protocol.md §14` item 3 is
explicit that multi-owner "will need a capability model, not an ACL bolted onto
`sys_app_grant`", and this ADR does not reopen that. What R4 buys is that the *resolution
function* does not hard-code "the subject is a device", so a capability model can replace
what it consults without rewriting every caller.

The ephemeral channel's profile-pair addressing (D16) is not listed, because nothing in
`pv/1` implements the channel and therefore nothing can foreclose it. Its outbound event
type is additive to `spec/data-api.md §3` and its inbound endpoint is additive to `§2`;
neither touches the wire, so both are reachable from HEAD at any time.

## 6. Spec changes this implies

**A later phase's work, not Phase 3's.** The rows naming `spec/protocol.md §2.3.1`, `§3`,
`§4.1`, `§10` and `§14` touch sections `docs/plans/phase-3.md` is editing now.

| File | Change |
|---|---|
| `spec/protocol.md §1.1` | `profile` added to reserved slugs; the "last two" sentence reworded (D3) |
| `spec/protocol.md §2.3.1` | Node admission shows the full-replica consequence (D4) |
| `spec/protocol.md §3` | Segment level in the storage layout; `_sys` exempt (D6) |
| `spec/protocol.md §4.1` | `usr` in the envelope; `seq` per `(app, segment, device)` (D6, D7) |
| `spec/protocol.md §10` | Heads, pull and push scoped by segment; the segment name validated before a path is built (D6, R3) |
| `spec/protocol.md §14` | The open questions below appended |
| `spec/data-api.md §2` | The inbound ephemeral endpoint, and why "Nothing else" still holds (D16) |
| `spec/data-api.md §3` | An ephemeral SSE event type carrying no `seq` (D16) |
| `spec/data-api.md §7` | A rate setting for ephemeral messages; no existing `api.*` limit bounds them (D16) |
| `spec/data-api.md §4` | `/api/node` returns the active profile; `pv.node()` likewise (D3) |
| `spec/data-dictionary.md §3.5` | `sys_app_grant` resolution restated over a subject (R4) |
| `spec/data-dictionary.md §3.10` | Six `profile.*` audit kinds; `actor` restated as a subject (D13, R5) |
| `spec/data-dictionary.md §3` | `sys_profile`, `sys_profile_alias`, `role` (D9, D10, D12) |
| `spec/app-contract.md §3` | `[tables]` sharing declarations in `app.toml` (D14) |
| `spec/cli.md §5.1` | New rules appended, never renumbered: `PV1xx` for an undeclared `[tables]` entry, `PV2xx` for an unscoped read, `PV3xx` for a profile affordance a Tier 2 app never offers (D2, D3, D14) |
| `docs/architecture.md §8` | Rewrite the absent-features entry (D17) |
| `docs/roadmap.md` | Narrow the not-on-roadmap entry; add profiles and D16 to the open questions (D17) |
| `docs/deployment.md §2` | Member nodes get the same treatment as a VPS full node, framed as a choice (D4) |
| `docs/security.md §1` | A household member on this node joins the adversary table (D1, D8) |
| `docs/security.md §9` | Per-segment removal, and why it is not a hard delete (D6, OQ5) |
| `skills/privatium-security/SKILL.md` | The above, plus Tier 3 exempt from profile scoping (D2) |
| `skills/privatium-tier3-rust/SKILL.md` | The core's segment scope is a convenience, not a boundary (D2) |
| `skills/privatium-games/SKILL.md` | The ephemeral channel, once it exists, beside the save-on-boundaries rule (D16) |

`AGENTS.md` makes the last three rows mandatory rather than optional: a change to `spec/`
that is not reflected in `skills/` is an incomplete change, and every skill's `reference/`
is regenerated by `cargo xtask gen-skill-reference`.

## 7. Open questions

Reduced from ten to six. Five of the removed were consequences of encryption; OQ6 is new.

**OQ1. `lam` under segmented logs.** D6 settles `seq` as per `(app, segment, device)`.
`lam` is the ordering clock for last-write-wins and is per app today
(`spec/protocol.md §4.3`). Segmenting it too would prevent `(lam, ts, dev)` from ordering a
`_shared` row against a private one, so leaving it per app is the working assumption and
needs stating normatively. The heads exchange carries `seq`, not `lam`, and its shape today
is `{dev: seq}` for one app and `{slug: {dev: seq}}` without one (`§10.1`); segments extend
the nested form to `{slug: {segment: {dev: seq}}}`.

**OQ2. How deep does an alias union go?** Reads, clearly. Snapshots? Sync heads? Does an
aliased-away segment still accept writes, or become read-only at the alias event?

**OQ3. Does `sys_app_grant` gain a profile subject in `pv/1` or `pv/2`?** R4 keeps the code
shape open either way, so this is scheduling, not design — bounded by §14 item 3, which
rules out growing it into an ACL.

**OQ4. What a member sees when their profile is not the active one.** Resolved in principle:
an explanatory screen, with a countdown when the reason is a D13 lockout. Open in detail:
whether the app is hidden from `sys.v_app_nav` (`spec/data-dictionary.md §4`) or shown and
gated.

**OQ5. Per-person removal in practice.** D6 makes removing a segment possible, but it must
happen on every node, and a node that is offline at the time will re-sync the segment back
from a peer on reconnect. Removal therefore needs a replicated `sys_segment_removed` marker
that peers honour, and that marker is the first thing in the system that instructs a node to
*not* hold data it is entitled to. It also sits directly against `spec/protocol.md §10.2`,
which requires a receiver to refuse a gap rather than skip it: a removed segment is a gap
every peer must agree to stop asking about. Worth designing carefully; it is the one place
this ADR touches the edge of the append-only invariant.

**OQ6. Where the active profile lives across a reconnect.** From D3 part 5. A WebSocket
session is the connection (`§8.3`) and a new connection is a new handshake authenticating
`dev` alone. Re-asserting the profile per connection is safer; remembering the last profile
per device is friendlier and means a dropped connection does not throw a child back to the
PIN screen mid-game. This interacts with D16, where a reconnect during play must not change
who the player is.

## Would reopen if

- Household use never materializes, or the multiplayer motivation is dropped. R2 and R3
  carry real `pv/1` complexity justified only by `pv/2` being genuinely intended.
- The backup story gains a documented, non-technical key step. That is the only thing that
  would reopen D8, and it should be reopened for that reason and no other.
- A direct peer transport between two clients arrives for another reason. D16's relayed
  shape is a consequence of what the stack has, not a preference.

---

Copyright © 2026 Gabriel Mongefranco
