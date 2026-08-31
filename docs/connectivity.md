<!--
Project:  Privatium™
File:     docs/connectivity.md
Authors:  Gabriel Mongefranco (@gabrielmongefranco)
Created:  2026-08-28
Modified: 2026-08-28
Summary:  Per-deployment matrices for bootstrap and reachability, and the
          resulting decision on peer-to-peer transport.
-->

# Connectivity

What each kind of client can do, under what network conditions, and why `pv/1` does not
implement NAT traversal.

Normative behaviour lives in `spec/protocol.md` §6 (discovery), §7 (pairing), and §10 (sync).
This document is the map.

---

## 1. Bootstrap — first load, authentication, discovery

Three distinct properties, easily conflated. See `spec/protocol.md §7.0`.

| Deployment | How the client code arrives | **1. Program authenticity** | Finds a node via | **2. Device authentication** |
|---|---|---|---|---|
| **Mobile native** | App store or sideload | Store signature / notarization | mDNS (NSD / Bonjour) + UDP fallback | PAKE → pins cluster key |
| **Browser, LAN, plain HTTP** | Node serves it | **None.** TOFU gap (§7.7) | Owner types the IP; `.local` unreliable on Android | PAKE over plaintext |
| **PWA** | HTTPS origin, cached by service worker | CA chain, then SW cache | **Cannot discover** — origin fixed at install | Token over TLS |
| **Laptop node (Wi-Fi)** | Installer / package | Package signature, notarization | mDNS + UDP | Admitted to the cluster once |
| **Desktop node (Ethernet)** | Installer / package | Package signature, notarization | mDNS + UDP | Founds the cluster |
| **VPS node** | Package or tarball | Package signature | No LAN; reached by DNS name | Admitted once, by pairing |

Two consequences worth stating plainly.

**Only one row lacks program authenticity**, and only against an attacker actively tampering
with traffic during first load. Device authentication and transport security hold on every
row, including that one — a guest on your Wi-Fi is a passive adversary and is excluded by the
PAKE regardless of how the page was served.

**A browser cannot discover anything.** No browser API exists for mDNS. The owner supplies an
address, and for a PWA that address is frozen at install time. This capability gap — not
rendering — is the substantive argument for building native shells.

---

## 2. Reachability by network state

✔ works · ◐ offline-cached reads, writes queued in the outbox · ✘ unreachable

| Client → target | Same LAN | Remote, **p2p** (pkarr + hole punching) | Remote, **VPN / tunnel / DDNS** | Remote, **nothing configured** | All nodes off |
|---|---|---|---|---|---|
| Mobile native → any node | ✔ mDNS | **✔ account-free** | ✔ | ◐ | ◐ |
| Browser page → node | ✔ IP or mDNS | ✘ no raw sockets | ✘ LAN-only by design | ✘ | ✘ |
| PWA → node | ✔ if the origin resolves to the LAN address | ✘ no raw sockets | ✔ needs a certificate | ✘ | ◐ |
| Laptop node → desktop node | ✔ | **✔ account-free** | ✔ | ◐ | n/a |
| Desktop node (was powered off) → cluster | ✔ | ✔ on waking | ✔ | ◐ until a peer appears | n/a |
| Any node → always-on machine | ✔ | ✔ | ✔ | ✔ | ✔ |

**Peer-to-peer closes every native-client failure without an account, a domain, or a
payment.** What it cannot close is the browser column: a browser has no raw sockets and
cannot treat a public key as a name, so remote browser access still needs a real domain and
a certificate.

The plain-HTTP LAN page remains the zero-dependency floor. Reaching *that* from outside
would require exactly the third parties the floor exists to avoid, so it stays LAN-only by
design.

---

## 3. What peer-to-peer would actually buy

| Scenario | Without p2p | With p2p |
|---|---|---|
| Two nodes on the same LAN | ✔ mDNS | no change |
| Node ↔ VPS node | ✔ public address | no change |
| Phone on cellular → home node, **VPS or VPN present** | ✔ | no change |
| Phone on cellular → home node, **nothing else** | ✘ | ✔ *if hole punching succeeds* |
| Laptop elsewhere → desktop at home, **nothing else** | ✘ | ✔ *if hole punching succeeds* |
| Power cut, desktop off, phone on cellular | ✘ | ✘ — p2p cannot reach a machine that is off |

Four rows unchanged, one it cannot help, **two it wins — and those two are the reason to
build it**, because it wins them with no account, no domain, and no money.

### 3.1 What it costs

- **A rendezvous is required.** pkarr on the mainline DHT supplies it: 10M+ nodes, 15 years,
  no operator. Not a company's fleet.
- **Relay fallback is required**, because hole punching fails against symmetric NAT and
  carrier-grade NAT. Roughly 10% of connections. The relay sees ciphertext only, and can be
  the owner's own.
- **Cellular is the weak case.** CGNAT is standard on carriers, so relay use is more likely
  there. Phone-to-home-node is still materially easier than phone-to-phone.
- **Managed networks block DHT traffic**, which is why DNS discovery runs concurrently
  (`spec/protocol.md §6.5`).

None of these reintroduces an account. The worst case is traffic through a relay that cannot
read it, and that relay can be yours.

---

## 4. Decision: peer-to-peer is the account-free path

**`pv/1` implements peer-to-peer**, because it is the only route to remote access requiring
no account, no domain, and no payment. That property is the point of the project, not a
nicety.

An earlier draft deferred this on the grounds that an always-on node solved the same
problem. That reasoning was flawed: a VPS requires a provider account, a card, and roughly
$5 a month, so recommending it while dismissing a mesh VPN "because it needs an account" was
not a fair comparison.

### 4.1 The stack

```
Discovery    mDNS on the LAN
             + pkarr on the BitTorrent mainline DHT
             + DNS, concurrently — never chained
Traversal    QUIC hole punching
Fallback     relay — the owner's, or a public default
Identity     the existing cluster Ed25519 key, unchanged
```

**A library is not a network.** The traversal library supplies code; the networks it talks to
are chosen separately:

| Piece | Whose network |
|---|---|
| Discovery via pkarr | **BitTorrent mainline** — 10M+ nodes, 15 years, owned by nobody |
| Discovery via DNS | a public pkarr server, or the owner's own |
| Hole punching | nobody's — direct peer to peer |
| Relay fallback | a public default, or the owner's own |

Configured with a self-hosted relay, the library vendor supplies code and no infrastructure.
Discovery runs on the largest and oldest DHT in existence.

### 4.2 Where the public key becomes the address

The cluster already has an Ed25519 keypair. pkarr makes that key a name directly — no
registrar, no DDNS account. What it replaces:

| Function | Normally | With pkarr |
|---|---|---|
| Name → current address | DDNS account | ✅ free, no account |
| External address discovery | STUN server | ✅ DHT |
| Rendezvous for hole punching | signaling server | ✅ DHT |
| Relay when punching fails | TURN | ❌ still required (~10% of the time) |
| **Browser-trusted certificate** | CA + real domain | ❌ **no CA signs a public key** |

That last row is a hard boundary, and it decides who gets the account-free story:

| Client | Account-free remote access? |
|---|---|
| Native desktop and mobile | ✅ pkarr + hole punching. Pin keys; no CA involved. |
| Browser and PWA | ❌ Still needs a real domain and a certificate. |

Browsers cannot browse mDNS, cannot open raw sockets, and cannot trust a public key as a
name. Every account-free capability lands in the native shells.

### 4.2.1 What mobile clients can and cannot do with apps

A separate axis, and the one most often misread.

| | Tier 1 (Lua) | Tier 2 (Web) |
|---|---|---|
| Works on iOS and Android | ✔ | ✔ |
| Delivered dynamically; updates when the node changes | ✔ (HTML) | ✔ (HTML/JS/WASM) |
| Requires rebuilding or resubmitting the client | **No** | **No** |
| Offline: cached reads, queued writes | ✔ | ✔ |
| Offline: render a view not yet visited | ✘ | ✔ |

**No app of either tier requires a per-user build.** Tier 1 sends the phone HTML, which is
content by any platform's reckoning; Tier 2 sends web assets rendered in the platform web
view, which is the explicitly permitted path.

The single thing that is not permitted is shipping a native Lua interpreter that downloads
and runs an app's source. That would only buy offline rendering for Tier 1 — and Tier 2
already provides it, legally, for apps that need it.

### 4.3 Routes retained

Peer-to-peer is added alongside, not instead of. All of these remain first-class:

| Route | For |
|---|---|
| mDNS + LAN address | The zero-dependency floor. Unchanged, still required. |
| Mesh VPN | Owners who prefer a managed answer |
| DDNS + DNS-01 certificate | Browsers and PWAs needing a real HTTPS origin |
| Tunnel | Owners who cannot forward a port |
| Always-on machine | See §4.4 |
| Nothing | LAN works; remote is offline-cached with an outbox |

The last row stays legitimate rather than degraded. A medication tracker is not a real-time
system: a phone showing this morning's cached refill list and queueing today's fill is
sufficient until the owner is home.

### 4.4 What an always-on machine is actually for

It has four separable functions, and only one of them touches your data:

| Function | Holds data? | Can decrypt? |
|---|---|---|
| Relay for failed hole punching | No | No |
| pkarr relay serving discovery over DNS | No | No |
| Certificate host — a real domain for browsers and PWAs | No | No |
| Full cluster node | **Yes, a complete plaintext replica** | Yes |

The first three are pure infrastructure. **The fourth is the one to think hardest about.** An
owner may sensibly run the first three and decline the fourth — and for health data that is
the better default. This inverts the usual advice: a relay is a *safer* thing to put on
rented hardware than a node.

### 4.5 A smaller first step

Full traversal is not required to get the account-free property. **pkarr alone** replaces
DDNS for any owner willing to forward one port:

| | Removes | Needs |
|---|---|---|
| pkarr only | DDNS account, domain | a forwarded port |
| pkarr + hole punching | DDNS, domain, **and** port forwarding | relay fallback |

Shipping pkarr first is small, useful on its own, and does not constrain the second step.

### 4.6 Caveats

- **The DHT is blocked on many managed networks.** DHT traffic resembles BitTorrent to deep
  packet inspection, so corporate, campus, and hotel networks often block it. Concurrent DNS
  discovery is required, not optional (`spec/protocol.md §6.5`).
- **Records expire in hours.** A sleeping laptop vanishes from discovery. Correct behaviour —
  no stale addresses — but it means a laptop is not a dependable discovery target.
- **1000-byte limit.** Discovery only. Never application data.
- **Publishing must be optional.** A LAN-only node has no reason to publish, and some owners
  will not want to.
- **Exposure equals DDNS.** Anyone holding the key resolves the address, exactly as anyone
  holding a hostname does. BEP44 targets derive from the key, so the space is not
  enumerable. Publishing under a per-period derived key limits long-term tracking.
- **BEP44, never BEP5.** Mutable items, not infohash peer announcements. A node must never be
  made to look like a torrent peer.

---

Copyright © 2026 Gabriel Mongefranco
