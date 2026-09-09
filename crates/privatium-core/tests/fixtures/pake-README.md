# Synthetic pairing vectors

`pake-vectors.json` contains deterministic test secrets only. Never use them to pair.

The code is `0x728E` — the spec's own example row, 🦊 🍕 ⚡️ 🎲. The device's Ed25519
secret is thirty-two bytes of `05`, its X25519 secret thirty-two bytes of `08`, the
device's SPAKE2 secret `x` sixty-four bytes of `06` and the node's `y` sixty-four bytes
of `07`, each reduced as `spec/protocol.md §7.4.1` reduces `w`. The node is the identity
of `identity/`: its node key, its certificate, and the cluster key that README names.

`w`, `pA`, `pB`, `TT`, `Ke`, `Ka`, `KcA`, `KcB`, `cA` and `cB` are the values of RFC 9382
§3 for that run, with `M` and `N` in hex for a reader. `transcript` holds the six
messages of `§7.4.2` exactly as they travel — the two sealed ones base64 — and
`parse_cases` the inputs both parsers must read alike.

Regenerate explicitly from the repository root:

```sh
cargo test -p privatium-core --locked --test pair generate_pake_vectors -- --ignored --exact
```

Default tests never write the fixture. Rust and JavaScript each run both sides of the
PAKE against it; JavaScript replays the device's side of the transcript with a
test-only source of randomness and must reproduce every byte the device sends.
