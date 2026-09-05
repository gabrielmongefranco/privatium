// Project:  Privatium™  |  File: crates/privatium-core/assets/shell/session.js
// Authors:  Gabriel Mongefranco (@gabrielmongefranco)
// Created:  2026-09-05  |  Modified: 2026-09-05
// Summary:  Browser session keys, authenticated frames, and single-use client
//           handshake for spec/protocol.md §8. No storage or network side effects.

import { x25519, ed25519 } from './vendor/noble/curves/ed25519.js';
import { sha256 } from './vendor/noble/hashes/sha2.js';
import { extract, expand } from './vendor/noble/hashes/hkdf.js';
import { concatBytes, bytesToHex } from './vendor/noble/hashes/utils.js';
import { chacha20poly1305 } from './vendor/noble/ciphers/chacha.js';

const utf8 = new TextEncoder();
const text = new TextDecoder('utf-8', { fatal: true });
const MAX_HELLO_BYTES = 8192;
const LIMIT = 2 ** 32;
const error = code => new Error(`cannot use session: ${code}; close connection and reconnect`);
const pinned = () => new Error('cannot establish session: pinned identity verification failed; explicitly re-pair to proceed');
const validId = id => typeof id === 'string' && id.length === 8 && /^[0-9abcdefghjkmnpqrstvwxyz]{8}$/.test(id);
const isObject = value => value !== null && typeof value === 'object' && !Array.isArray(value);

function keyBytes(value) {
  if (!(value instanceof Uint8Array) || value.length !== 32) throw error('invalid key');
  return value;
}

function base64(value) {
  return btoa(Array.from(value, b => String.fromCharCode(b)).join(''));
}

function decode64(value, length) {
  if (typeof value !== 'string' || value.length > MAX_HELLO_BYTES ||
      (length !== undefined && value.length !== Math.ceil(length / 3) * 4)) throw error('invalid encoding');
  const bytes = Uint8Array.from(atob(value), c => c.charCodeAt(0));
  if ((length !== undefined && bytes.length !== length) || base64(bytes) !== value) throw error('invalid encoding');
  return bytes;
}

function parseHello(hello) {
  if (typeof hello !== 'string' || hello.length > MAX_HELLO_BYTES || utf8.encode(hello).length > MAX_HELLO_BYTES) throw error('invalid hello');
  let value;
  try { value = JSON.parse(hello); } catch { throw error('invalid hello'); }
  if (!isObject(value)) throw error('invalid hello');
  if (value.v !== 1) throw error('protocol version differs; use a pv/1 client');
  return value;
}

/**
 * Derive c2s/s2c Uint8Array keys from four X25519 keys and the two protocol IDs.
 * Ephemerals must be fresh for each connection. Throws on invalid or low-order keys;
 * never logs, persists, or transmits secrets. Caller must wipe returned keys after use.
 */
export function derive(myStatic, theirStatic, myEphemeral, theirEphemeral, nodeId, deviceId) {
  if (!validId(nodeId) || !validId(deviceId)) throw error('invalid identity');
  let ss, ee, input, prk;
  try {
    ss = x25519.getSharedSecret(keyBytes(myStatic), keyBytes(theirStatic));
    ee = x25519.getSharedSecret(keyBytes(myEphemeral), keyBytes(theirEphemeral));
    input = concatBytes(ss, ee);
    const salt = sha256(utf8.encode([nodeId, deviceId].sort().join('') + 'pv/1 session'));
    prk = extract(sha256, input, salt);
    return { c2s: expand(sha256, prk, utf8.encode('pv/1 c2s'), 32),
      s2c: expand(sha256, prk, utf8.encode('pv/1 s2c'), 32) };
  } catch { throw error('invalid key agreement'); }
  finally { for (const bytes of [ss, ee, input, prk]) bytes?.fill(0); }
}

/** One directional key/counter. Any failure requires closing the whole connection. */
export class Frame {
  #key; #direction; #counter = 0; #limit; #closed = false;

  /**
   * Copy a 32-byte key and own its counter from zero. Direction is 1 (c2s) or 2 (s2c).
   * Optional lower frame budget supports boundary tests; it cannot exceed 2^32.
   * Never construct a second sender with the same key. Throws on invalid inputs.
   */
  constructor(key, direction, limit = LIMIT) {
    keyBytes(key);
    if (![1, 2].includes(direction) || !Number.isSafeInteger(limit) || limit < 1 || limit > LIMIT) throw error('invalid frame parameters');
    this.#key = key.slice(); this.#direction = direction; this.#limit = limit;
  }

  #crypt(bytes, opening) {
    if (this.#closed || this.#counter >= this.#limit) { this.close(); throw error('closed'); }
    try {
      if (!(bytes instanceof Uint8Array)) throw error('invalid frame');
      const nonce = new Uint8Array(12), view = new DataView(nonce.buffer);
      view.setUint32(0, this.#direction, false);
      view.setBigUint64(4, BigInt(this.#counter++), false);
      const cipher = chacha20poly1305(this.#key, nonce);
      const result = opening ? cipher.decrypt(bytes) : cipher.encrypt(bytes);
      if (this.#counter === this.#limit) this.close();
      return result;
    } catch { this.close(); throw error('frame authentication failed'); }
  }

  /** Seal one Uint8Array with no associated data; returns ciphertext plus tag. */
  seal(plaintext) { return this.#crypt(plaintext, false); }

  /** Authenticate next ciphertext; throws permanently on tampering, replay, or disorder. */
  open(ciphertext) { return this.#crypt(ciphertext, true); }

  /** Wipe owned key and permanently refuse future frames. No I/O. */
  close() { this.#closed = true; this.#key.fill(0); }

  /** True at exhaustion or after failure/close; transport closes even after a valid last frame. */
  get closed() { return this.#closed; }
}

function nodeId(publicKey) {
  const alphabet = '0123456789abcdefghjkmnpqrstvwxyz';
  let bits = 0n;
  for (const byte of sha256(publicKey).slice(0, 5)) bits = (bits << 8n) | BigInt(byte);
  let id = '';
  for (let i = 7; i >= 0; i--) id += alphabet[Number((bits >> BigInt(i * 5)) & 31n)];
  return id;
}

function verifyCertificate(encoded, pins, id, now) {
  try {
    if (!Number.isFinite(now) || !validId(id) || pins.id !== id) throw pinned();
    const raw = decode64(encoded);
    if (raw.length > 4096) throw pinned();
    const cert = JSON.parse(text.decode(raw));
    if (!isObject(cert)) throw pinned();
    const fields = ['node_id', 'node_pub', 'cluster_id', 'issued_at', 'expires_at', 'sig'];
    if (fields.some(k => typeof cert[k] !== 'string')) throw pinned();
    const publicKey = decode64(cert.node_pub, 32), signature = decode64(cert.sig, 64);
    const canonical = JSON.stringify({ node_id: cert.node_id, node_pub: cert.node_pub,
      cluster_id: cert.cluster_id, issued_at: cert.issued_at, expires_at: cert.expires_at });
    if (!ed25519.verify(signature, utf8.encode(canonical), pins.cluster, { zip215: false }) ||
        ed25519.Point.fromBytes(publicKey).isSmallOrder() ||
        cert.node_id !== id || cert.node_id !== nodeId(publicKey) || cert.cluster_id !== nodeId(pins.cluster)) throw pinned();
    const issued = Date.parse(cert.issued_at), expires = Date.parse(cert.expires_at);
    if (!Number.isFinite(issued) || !Number.isFinite(expires) ||
        new Date(issued).toISOString() !== cert.issued_at || new Date(expires).toISOString() !== cert.expires_at ||
        expires - issued !== 180 * 86400000 || now < issued || now >= expires) throw pinned();
  } catch { throw pinned(); }
}

/**
 * Start a single-use client handshake using device ID, static secret, and stored pins
 * {id, cluster, x25519}. Public keys and secret are Uint8Arrays; pins come only from pairing.
 * Returns {hello, finish(nodeHello, utcMilliseconds), close()}. finish returns {confirm,
 * send, receive}; first authenticated inbound frame proves the node holds its static key.
 * Throws without a confirmation on certificate mismatch. Uses getRandomValues, no subtle,
 * network, storage, or UI. Caller closes the connection on every error and disposes frames.
 */
export function clientHandshake(deviceId, staticKey, storedPins) {
  if (!validId(deviceId) || !validId(storedPins?.id)) throw error('invalid identity');
  const pins = { id: storedPins.id, cluster: keyBytes(storedPins.cluster).slice(), x25519: keyBytes(storedPins.x25519).slice() };
  const secret = keyBytes(staticKey).slice(), ephemeral = new Uint8Array(32);
  let closed = false, hello;
  function close() { closed = true; secret.fill(0); ephemeral.fill(0); }
  try {
    globalThis.crypto.getRandomValues(ephemeral);
    hello = JSON.stringify({ dev: deviceId, e: base64(x25519.getPublicKey(ephemeral)), v: 1 });
  } catch { close(); throw error('secure random source unavailable'); }
  return Object.freeze({ hello, close, finish(nodeHello, now = Date.now()) {
    if (closed) throw error('closed');
    let keys, send, receive, completed = false;
    try {
      const node = parseHello(nodeHello);
      verifyCertificate(node.cert, pins, node.id, now);
      keys = derive(secret, pins.x25519, ephemeral, decode64(node.e, 32), node.id, deviceId);
      send = new Frame(keys.c2s, 1); receive = new Frame(keys.s2c, 2);
      const confirm = send.seal(utf8.encode(JSON.stringify({ confirm: bytesToHex(sha256(utf8.encode(hello + nodeHello))) })));
      completed = true;
      return { confirm, send, receive };
    } finally {
      keys?.c2s.fill(0); keys?.s2c.fill(0); close();
      if (!completed) { send?.close(); receive?.close(); }
    }
  } });
}
