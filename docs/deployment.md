<!--
Project:  Privatium™
File:     docs/deployment.md
Authors:  Gabriel Mongefranco (@gabrielmongefranco)
Created:  2026-08-28
Modified: 2026-08-28
Summary:  Topologies, the always-on node, and per-OS firewall behaviour.
-->

# Deployment

## 1. Topologies

Pick by what the owner actually has, not by what is most impressive.

| Topology | Nodes | Remote access | Setup |
|---|---|---|---|
| **Single** | one desktop | none, or a tunnel | trivial |
| **Household** | desktop + laptop | tunnel or VPN | pair the second node once |
| **Household + always-on** | desktop + laptop + VPS | free, always | ~$5/month |
| **Backup-only** | one node, file sync on `data/` | none | install Syncthing |

Nodes are peers. There is no primary and no server role
(`spec/protocol.md §10.3`). For which client kinds work under which network conditions, see
`docs/connectivity.md`.

## 2. The always-on machine — four separable jobs

A cheap VPS is useful, but "run a node on it" is only one of four things it can do, and it is
the only one that touches your data. Enable them independently.

| # | Function | Holds data? | Can decrypt? | Solves |
|---|---|---|---|---|
| 1 | **Relay** | No | No | Hole punching failures (~10%), CGNAT, symmetric NAT |
| 2 | **pkarr relay / DNS** | No | No | Discovery on networks that block the DHT |
| 3 | **Certificate host** | No | No | Browsers and PWAs needing a real HTTPS origin |
| 4 | **Full node** | **Yes — complete plaintext replica** | Yes | Reachability when every machine at home is off |

### 2.1 Start with 1–3, think hard about 4

Functions 1–3 are pure infrastructure. They forward, resolve, and terminate TLS; they store
nothing and can read nothing.

Function 4 puts your entire medication history in plaintext on rented hardware, indefinitely.
It buys exactly one thing the others do not: a peer that is reachable when your desktop and
laptop are both off.

**For health data, running 1–3 and declining 4 is the better default.** This inverts the
usual advice — a relay is a safer thing to place on someone else's hardware than a node,
because a node is the only one of the four that can be read if the machine is compromised.

### 2.2 What each function needs

| Function | Requires |
|---|---|
| Relay | The binary, a public address, one open port |
| pkarr relay / DNS | The above, plus port 53 |
| Certificate host | A domain and an ACME certificate |
| Full node | Admission to the cluster by pairing (`spec/protocol.md §2.3.1`) |

Only the fourth involves pairing, because only the fourth is a cluster member. The first
three never see the cluster key.

### 2.3 Alternatives

All legitimate, all retained, none removed by peer-to-peer:

| Option | Cost |
|---|---|
| **Peer-to-peer** (pkarr + hole punching) | **none** — no account, no domain, no payment. Native clients only. |
| Mesh VPN | account, client on every device |
| Tunnel service | account; a domain for a stable name |
| DDNS + port forward + DNS-01 certificate | router configuration, a domain. Required for browsers and PWAs. |
| Syncthing on `data/` | an external tool; global discovery servers and a relay pool. Backup, not a transport. |
| Nothing | LAN works; remote is offline-cached with an outbox |

## 3. Cluster growth

Adding a second node is one pairing, and it buys more than a second copy:

- The laptop keeps serving when the desktop is off. Your phone reaches it over mDNS with no
  reconfiguration, because it pinned the **cluster** key, not the node key.
- Discovery already returns multiple instances; clients key on Node ID and filter on the
  `cl` TXT field, so a LAN carrying several households' nodes still shows you only your own.

Do not add nodes for performance. Add them for availability.

## 4. Firewalls

**Only a node needs inbound connectivity.** Clients need none. Outbound is effectively never
blocked on any supported platform, so certificate issuance and tunnels work regardless.

| OS | Default state | Consequence |
|---|---|---|
| **Fedora Workstation** | firewalld, `FedoraWorkstation` zone | Easiest of all. Ports 1025–65535 TCP and UDP are open by default — a deliberate decision so that novice users can run network-facing applications — and `mdns` is among the zone's allowed services. Nothing to configure. |
| **Ubuntu / Debian desktop** | ufw present but inactive | Works out of the box. Server installs vary. |
| **macOS** | Application Firewall off by default | Usually works. If enabled, prompts to allow incoming connections; unsigned applications are flagged, so notarization materially improves this. Recent macOS versions add a **separate local-network privacy prompt**. |
| **Windows** | Defender Firewall, inbound blocked | An elevation prompt appears on the first non-loopback bind. Request the **Private** profile only, never Public. |
| **openSUSE** | firewalld, `public` zone | Restrictive. Requires an explicit rule. |

### 4.1 Two traps

**Windows network classification.** Home Wi-Fi is frequently classified as *Public* rather
than *Private*, which blocks the application even after the owner clicked Allow. Expect this
to be the most common support question. Detect the profile and say so in plain language;
an owner who is not told will conclude the software is broken.

**mDNS needs its own rule.** UDP 5353 inbound is separate from the TCP application port.
Opening one and forgetting the other produces "it works by IP but not by name," which looks
like a discovery bug and is not.

### 4.2 Never require administrator

The node MUST run as an ordinary user. Elevation is only ever needed to open a firewall
port, and that MUST be an optional helper the owner can decline — with the manual commands
shown so they can inspect what would be run.

```
Fedora / openSUSE   sudo firewall-cmd --permanent --add-port=8420/tcp \
                    --add-port=5353/udp && sudo firewall-cmd --reload
Ubuntu / Debian     sudo ufw allow 8420/tcp && sudo ufw allow 5353/udp
Windows             netsh advfirewall firewall add rule name="Privatium" \
                    dir=in action=allow protocol=TCP localport=8420 profile=private
```

Ports must be ≥ 1024 (`spec/protocol.md §6.3`) — required for sandboxed packaging, and the
default 8420 also sits inside the range Fedora already permits.

## 5. Network transitions

Switching Wi-Fi to cellular mid-session is ordinary, not exceptional. Two separate concerns:

**Reachability** is handled by endpoint failover (`spec/protocol.md §10.4`): short connect
timeouts, re-attempt on network-change events, offline mode with an outbox when nothing
answers. Queued writes replay idempotently because they carry ULIDs.

**Origin** is the subtler one (`§10.8`). A browser reaching the node at a LAN address and
then at a public name is on two different origins, with two service workers and two storage
buckets — the app does not degrade, it becomes a different installation. Native clients
sidestep this entirely. For browsers, one resolvable name across every path is the answer.

## 6. Packaging

| Stage | Artifact | Notes |
|---|---|---|
| Now | Linux tarballs (x86_64, aarch64) | |
| Now | AppImage | The "download and run" story |
| Now | `.exe` / `.msi` | Bundle the WebView2 bootstrapper for older Windows |
| Now | `.app` | Notarization improves the firewall prompt as well as Gatekeeper |
| Next | `.deb`, `.rpm` | Trivial once the binary is stable |
| Later | Flatpak | `--share=network` for mDNS; data under XDG or the portal; never `--filesystem=host` |

---

Copyright © 2026 Gabriel Mongefranco
