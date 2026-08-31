<!--
Project:  Privatium™
File:     docs/security.md
Authors:  Gabriel Mongefranco (@gabrielmongefranco)
Created:  2026-08-28
Modified: 2026-08-28
Summary:  Threat model, protections, and honest statements of what is not protected.
-->

# Security Model

Non-normative narrative. Normative requirements live in `spec/protocol.md §7–9`.

## 1. Who this protects against, and who it does not

| Adversary | Outcome |
|---|---|
| A passive sniffer on your LAN | **Defeated.** All traffic is encrypted, including plain-HTTP browser sessions. |
| Someone who finds your node on the network — a guest on your Wi-Fi | **Defeated.** No pairing, no data. Unauthenticated endpoints return an ID and nothing else. See §2.1. |
| An active on-path attacker arriving after you paired | **Detected.** Pinned key mismatch, hard refusal, no override. |
| An active on-path attacker present at your first browser page load *and* first pairing | **Wins.** See §4. |
| Someone with your unlocked laptop | **Wins.** Privatium is not disk encryption. Use LUKS/FileVault/BitLocker. |
| Someone who steals your backup folder | **Wins.** Backups are plain text by design. Encrypt the destination. |
| A malicious app folder you installed | **Contained, not eliminated.** See §6. |
| A nation-state | Out of scope. This is a refill tracker. |

## 2. Three properties, often confused

Almost every muddled conversation about pairing comes from collapsing three separate
guarantees into one. Normative version: `spec/protocol.md §7.0`.

| # | Property | Question | Mechanism |
|---|---|---|---|
| **1** | Program authenticity | Is this client genuine? | Package signature, notarization, or a CA chain |
| **2** | Device authentication | May this device talk to my cluster? | PAKE first contact, pinned keys after |
| **3** | Transport security | Is the channel confidential? | The derived session key |

| Client | 1 | 2 | 3 |
|---|---|---|---|
| Native desktop / mobile | ✔ | ✔ | ✔ |
| Browser or PWA over TLS | ✔ | ✔ | ✔ |
| **Browser over plain HTTP on LAN** | **✘ §3** | ✔ | ✔ |

**Property 2 is not a login step layered on property 3.** A password-authenticated key
exchange does both at once — deriving a usable key *is* the proof that both sides held the
code. There is no "authenticate, then encrypt." An attacker without the code does not get a
rejected login; the handshake produces nothing at all.

The practical consequence: never run a PAKE to open a channel and then send the code as a
bearer token to log in. That puts the code on the wire, makes it replayable, and throws away
the reason for choosing a PAKE.

### 2.1 The guest on your Wi-Fi

The common worry, and the one property 2 exists for.

Discovery is public — anyone on the network can see a node exists, which is inherent to
mDNS. What a guest gets from that:

1. They load the page.
2. Pairing mode is closed unless you pressed the button. The handshake is refused.
3. Even with it open: 16 bits, five attempts, a 120-second window, rate limiting. Roughly
   1 in 13,000 — and every attempt writes a replicated audit event that appears on your
   phone.

A guest is a **passive** adversary, so property 1 never enters into it. Property 2 excludes
them, and it does so identically on plain HTTP and on TLS.

### 2.2 Pair once, trusted after

The browser generates its own keypair during pairing and stores it with the pinned cluster
key under the page origin. Subsequent visits use those directly — no code, no prompt. Native
clients use Keychain, Keystore, or the OS keyring.

Lose that storage and you re-pair. There is deliberately no recovery path that skips
pairing. Each device is separately revocable, so your laptop's browser and your phone's
browser are two entries.

Browser storage is per-origin, so a device paired at a LAN address has no credential at a
public name and must pair again — the everyday argument for one resolvable name across all
paths.

## 3. What is encrypted where

| Path | Confidentiality | Peer authentication |
|---|---|---|
| Native client ↔ node | X25519 + HKDF + ChaCha20-Poly1305, pinned statics | Pinned key, TOFU at pairing |
| Browser ↔ node, LAN HTTP | Same, in pure JS | Pinned key, TOFU at pairing |
| Browser ↔ node, Tailscale | WireGuard + TLS on `ts.net` | Tailnet identity |
| Browser ↔ node, DuckDNS + LE | TLS | webPKI |
| Tor Browser ↔ node | Tor | Onion address is the key |
| Node ↔ node, direct | QUIC (TLS 1.3), then session layer | Pinned key |
| Node ↔ node, via relay | Same — the relay forwards ciphertext | Pinned key |
| `data/` at rest | **None.** Plain text, deliberately. | — |

The at-rest decision is the whole product. Encrypting the logs would defeat the restore
story, which is the reason the project exists. The correct place for at-rest encryption is
the filesystem, where the OS already does it well.

## 3b. Relays, discovery, and what leaks

Three pieces of infrastructure sit outside your machines. None can read your data; each
leaks something.

| Component | Sees | Does not see |
|---|---|---|
| **Relay** (hole-punching fallback) | Source and destination addresses, timing, volume | Anything inside — transport encryption is end to end |
| **Mainline DHT** (pkarr discovery) | That a key published an address, and which address | Who you are, what the node is, any content |
| **pkarr DNS server** | Which keys are being resolved, and by whom | The same — nothing readable |

### A relay is a safer place than a node

This is worth stating plainly because the intuition runs the other way. A relay forwards
ciphertext and stores nothing. A **full node holds a complete plaintext replica**. If you
have one rented machine and health data, host the relay and decline the node
(`docs/deployment.md §2.1`).

### pkarr exposure equals dynamic DNS

Anyone holding the key resolves the current address — exactly as anyone holding a DDNS
hostname resolves theirs. It is not worse. Specifically:

- BEP44 targets derive from the public key, so the keyspace is **not enumerable**
- Records expire within hours, so an offline node leaves no trail
- pkarr uses **BEP44 mutable items, not BEP5 infohash announcements** — a node never appears
  as a peer for any content and must never be made to
- Publishing is optional and separately disableable; a LAN-only node should not publish

For better than DDNS-equivalent, publish under a key derived per period from the cluster key.
An observer who captures the key once then cannot track the node indefinitely.

### One operational note

DHT traffic resembles BitTorrent to deep packet inspection and is blocked on many corporate
and campus networks. Nothing is being shared, but on a managed network the traffic pattern
alone may draw attention. Owners in those environments should disable `discovery.pkarr` and
rely on DNS discovery.

## 4. Property 1: the honest gap

This section is about **property 1 only**. Properties 2 and 3 hold on this path.

A browser loading `http://192.168.1.14:8420` receives its JavaScript over plaintext. An
attacker who can modify traffic on that first load can substitute their own client, learn
the pairing code as you type it, complete the handshake themselves, and proxy everything.

No amount of in-page cryptography fixes this. You cannot bootstrap trust over an untrusted
channel without an out-of-band anchor, and the browser has nowhere to keep one before the
code runs.

This is exactly the model you accept every time you type `yes` at:

```
The authenticity of host 'server (10.0.0.4)' can't be established.
ED25519 key fingerprint is SHA256:...
Are you sure you want to continue connecting (yes/no)?
```

Every system administrator on earth relies on it. It is a defensible position, not a
compromise — but it must be stated, not buried.

**What narrows the window:**

- The attacker must be *actively* on-path — ARP spoofing, a rogue AP, a compromised
  router or IoT device. Passive sniffing does not suffice.
- They must be present during the specific 120-second pairing window.
- Pairing mode is off by default and requires a deliberate action to open.
- Every pairing writes a permanent, replicated `sys_audit` event. A surprise pairing is
  visible on every device you own, forever.
- After first pairing the node's key is pinned; the attacker's later arrival is refused.

**What closes it entirely:** pair over a native client, or over Tailscale, or over Tor —
any transport with independent authentication. The settings page should say so.

## 5. Why there is no verification screen

An earlier draft included a short-authentication-string step: both screens show three
emoji, the human confirms they match.

It was removed because **a PAKE already does that job**. SAS exists in ZRTP and Signal
because their Diffie-Hellman is unauthenticated — there is no shared secret, so a human has
to supply the authentication out of band. Privatium has a shared secret: the pairing code.
An attacker without it cannot complete the handshake at all.

And against the one attack the PAKE cannot stop — the substituted client in §4 — a SAS is
useless, because the attacker's client displays whatever numbers it wants.

So the SAS would have been a screen that adds friction, teaches users to click through
security dialogs, and protects against nothing. It is normatively forbidden
(`spec/protocol.md §7.6`).

## 6. Pairing code strength

16 bits, node-generated, single-use, 120-second TTL, 5 attempts maximum, rate-limited to
one attempt per two seconds.

An online attacker gets 5 guesses out of 65,536 — a 0.008% success rate — and must be
attacking during a window the owner deliberately opened. Because the code goes through a
PAKE rather than being sent as a bearer token, there is no offline dictionary attack: each
guess costs a full network round trip.

For comparison, a 6-digit banking OTP is roughly 20 bits with far worse ergonomics and
frequently a 5-minute window.

The code is never owner-chosen. Owner-chosen codes get reused, get written on the monitor,
and get set to `0000`.

## 7. App folders are trusted-ish code

Installing an app folder means running its SQL on your node. The sandbox
(`spec/app-contract.md §7`) blocks the dangerous part — DuckDB with external access
enabled can read `identity/node.key` — but within its own schema an app sees everything.

Therefore:

- `enable_external_access=false`, extension autoload disabled, `lock_configuration=true`.
  These are hard requirements, not defaults to be overridden.
- Apps may read `sys.v_*` views but MUST NOT write to `sys` tables.
- The install flow warns the owner in the same terms you would warn them about running a
  script from a stranger.
- There is no app registry in `pv/1`, deliberately. A registry implies curation, curation
  implies a trust signal, and a false trust signal is worse than none.

## 8. Revocation

Revoking a device sets `revoked_at` on its `sys_device` row. Because that row is
replicated, revocation propagates to every node. Sessions are checked per request, so
revocation is immediate on any node that has received the event.

Revocation is not a delete. The record of what was paired and when survives permanently —
that history is a security feature.

There is no key rotation in `pv/1` (`spec/protocol.md §2.3`). A compromised *node* key
means re-initializing the node and re-pairing every device. This is a known gap.

## 9. Data destruction

There is no "hard delete." `op: "del"` writes a tombstone; the original event remains in
the log forever, and that is intentional — an append-only log you can secretly rewrite is
not an append-only log.

The supported way to destroy data irrecoverably is to destroy `data/`, including every
copy your sync tool has made. Say that plainly in the UI. Anyone whose threat model
requires deniable deletion should not use an append-only system.

## 10. Reporting

Report suspected vulnerabilities to <privatium@mongefranco.com>. Do not open a public
issue for a security defect.

---

Copyright © 2026 Gabriel Mongefranco
