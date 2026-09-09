# Synthetic identity fixtures

`node.key` contains the bytes `00` through `1f`, in order. The certificate's signing key
is thirty-two bytes of `2a`. Neither is a real identity or a credential.

`certificate-message.txt` spells the five signed fields in protocol order, without
whitespace. Its final newline is file formatting, not part of the signed message.
`node.cert` adds the base64 Ed25519 signature. Issuance is 2026-09-04 at 12:00 UTC;
expiry is exactly 180 days later, 2027-03-03 at 12:00 UTC.

The fixture was constructed independently of `Certificate`, using `ed25519-dalek` to
sign the literal message. `pkarr-name.txt` encodes that signing key's public key with
the z-base32 alphabet `ybndrfg8ejkmcpqxot1uwisza345h769`, most significant bit first,
padding only the last five-bit group with zero bits. Its final newline is not part of
the name.

The signature was also verified with OpenSSL 3. The z-base32 name was independently
checked by translating Python's standard RFC base32 output to the z-base32 alphabet.
