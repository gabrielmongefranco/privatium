---
name: privatium-security
description: Security rules for writing or reviewing any Privatium app, and what to tell an owner before they install one. Covers injection, escaping, sandbox limits, secrets, permissions, and the framework's honest threat model. Load alongside every tier skill and before recommending any third-party app.
---

# Privatium Security

Applies to every tier. Load alongside the tier skill.

## Never, in any tier

1. **Never build SQL by concatenation.** Bind parameters. The linter rejects concatenation
   and you should too.
2. **Never put a secret in `data/`.** Not keys, not pairing codes, not tokens, not API
   credentials. Logs are plain text, sync everywhere, and live in backups forever. Secrets
   go in the OS keyring or `identity/`.
3. **Never disable escaping.** `<?= ?>` in LSP escapes by default. `<?raw ?>` exists for the
   rare genuine case and every occurrence is a review trigger. In JS, use `textContent`,
   never `innerHTML`, with user data.
4. **Never omit `csrf()`** from a non-GET form.
5. **Never trust an event's origin.** Events arrive via sync from other devices. Validate
   on read as well as on write.
6. **Never claim data is deleted.** `del` writes a tombstone; the original line stays in the
   log forever. That is deliberate. The only way to destroy data is to destroy `data/` and
   every synced copy — say so plainly in any UI that offers deletion.

## Tier 1 (Lua)

`io`, `os.execute`, `os.getenv`, `os.remove`, `debug`, `load`, `loadstring`, `dofile`, and
`package.loadlib` are removed from the sandbox. Do not attempt to reach them; do not
suggest a workaround. `require` is confined to the app's own `lib/`.

The DuckDB connection runs with `enable_external_access = false`, extension autoload off,
and `lock_configuration = true`. This is not adjustable hardening — unsandboxed, DuckDB can
read `identity/node.key`.

Watch instruction and memory limits. An unbounded loop over synced data aborts the request.

## Tier 2 (Web)

Default CSP is `script-src 'self'` scoped to the app's path. Inline `<script>` does not run.
Put JavaScript in external files.

Every non-default permission is shown to the owner at install:

| Permission | Ask for it only if |
|---|---|
| `inline_script` | You genuinely cannot use an external file. Almost never. |
| `wasm` / `eval` | A WASM loader requires it |
| `sql` | The app needs ad-hoc queries rather than named views |
| `cross_origin_isolated` | Solo mode only; see `privatium-games` |
| `remote` | **The app phones out.** This is the one thing the project exists to avoid. Expect the owner to refuse. |

Vendor libraries into `web/vendor/`. A CDN is a `remote` permission, an offline failure, and
an IP leak.

## Tier 3 (Rust)

Not sandboxed. Full filesystem and network access, in the owner's session, on their data.
Say so when recommending one. No `unsafe` without a comment naming the invariant it upholds.

## Reviewing a third-party app

Before recommending an owner install anything:

- [ ] Read `app.toml`. Every non-default permission justified by something visible in the code?
- [ ] Any `remote` origins? What is sent, and why?
- [ ] Grep for `<?raw`, `innerHTML`, `eval`, string-concatenated SQL
- [ ] Does it read `sys.v_*` views beyond what its function needs?
- [ ] Tier 3? Then it is unsandboxed native code — treat accordingly
- [ ] `privatium lint` clean?

Tell the owner in plain terms: **installing an app means running someone else's code on
your data.** It is sandboxed from the filesystem, but within its own scope it sees
everything. Treat it like a script a stranger emailed you.

## Three properties — do not conflate them

| # | Property | Mechanism | Missing anywhere? |
|---|---|---|---|
| 1 | Program authenticity | Signature / notarization / CA chain | **Yes** — browser on plain-HTTP LAN |
| 2 | Device authentication | PAKE, then pinned keys | No |
| 3 | Transport security | Derived session key | No |

**The PAKE does 1 job, not 2.** Authentication and key agreement are one operation —
deriving a key is the proof. Never write code that runs a PAKE for the channel and then
sends the code as a bearer token to log in; that is strictly weaker.

A guest on the Wi-Fi is blocked by property 2, on every path including plain HTTP. Pairing
mode is closed by default, and the code is 16 bits with 5 attempts in 120 seconds.

Pairing persists: the client stores its keypair and the pinned cluster key under the origin
(browser) or the OS keyring (native). Lost storage means re-pairing — never write a recovery
path that skips it.

## The framework's honest gap

This is **property 1 only**. A browser loading `http://192.168.1.14:8420` receives its
JavaScript over plaintext, so an attacker actively on-path during **both** the first page
load and the 120-second pairing window can substitute their own client and win. Passive
sniffing cannot, and properties 2 and 3 are unaffected. After first
pairing the node's key is pinned and any later attacker is refused with no override.

This is the SSH trust-on-first-use model. It is defensible, and it must be stated rather
than buried. Do not paper over it, and do not add a verification screen — the PAKE already
authenticates, and against a poisoned bundle a verification string is useless because the
attacker's client renders whatever it likes.

Closing it entirely means pairing over a native client, Tailscale, or Tor.

## Cluster keys

The cluster private key lives in `identity/cluster.key` on nodes only. It MUST NOT be sent to
a phone, tablet, or browser, and MUST NOT appear in any event, log, snapshot, or backup
export. Devices receive the cluster *public* key and pin it.

Consequence to state when asked: **compromising any one node compromises the cluster**, since
every node holds the key. That is the accepted trade — the alternative is node admission
failing whenever one particular machine is off. Nodes are machines the owner physically
controls; devices are not.

Revocation is bounded by the 180-day certificate lifetime, not instant. An owner needing a
hard cut must rotate the cluster and re-pair everything.

## Discovery and relays

| Component | Leaks | Reads data |
|---|---|---|
| Relay | addresses, timing, volume | **no** — ciphertext only |
| Mainline DHT (pkarr) | that a key published an address | no |
| pkarr DNS server | which keys are resolved | no |

- pkarr exposure equals dynamic DNS: whoever has the key resolves the address. Not worse.
- **BEP44 mutable items, never BEP5 infohash announcements.** A node must never look like a
  torrent peer.
- Publishing is optional and separately disableable. A LAN-only node should not publish.
- Records are under 1000 bytes and carry no application data, ever.
- When recommending rented hardware: **relay yes, full node think twice.** A relay stores
  nothing; a node stores a complete plaintext replica.

## Reporting

Suspected vulnerabilities go to <privatium@mongefranco.com>. Never a public issue.

Detail: `docs/security.md`, `spec/protocol.md §7–9`.
