# Synthetic session vectors

`session-vectors.json` contains deterministic test secrets only. Never use them for
pairing or a real session.

The key-schedule fixture uses 32 repetitions of byte 1 for the client static, 2 for the
node static, 3 for the client ephemeral, and 4 for the node ephemeral. The ten frames
per direction begin with an empty plaintext. Strings are standard padded base64.

The handshake fixture uses the existing `identity/node.cert` and its synthetic key
material. Its node X25519 static is derived as `spec/protocol.md §8` requires. The two
hello strings are exact transcript bytes; their field order must not be changed without
regenerating the confirmation. The confirmation is a sealed JSON object, without the
application frame's length prefix (`spec/protocol.md §8.3`).

Regenerate explicitly from the repository root:

```sh
cargo test -p privatium-core --locked --test session generate_session_vectors -- --ignored --exact
```

Default tests never write the fixture. Rust and JavaScript each seal and open the same
frames. JavaScript also verifies the certificate and reproduces the Rust confirmation
with a test-only replacement for `crypto.getRandomValues`.
