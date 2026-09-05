// Project:  Privatium™  |  File: crates/privatium-core/assets/shell/pair.js
// Authors:  Gabriel Mongefranco (@gabrielmongefranco)
// Created:  2026-09-05  |  Modified: 2026-09-05
// Summary:  The browser's side of pairing (spec/protocol.md §7): the code in both
//           renderings and their parser, SPAKE2 as RFC 9382 spells it over the vendored
//           curve, the six messages of /ws/pair, and the record kept under `pv:device`.
//           No UI: the pairing screen drives `pair()` and shows what it throws.

import { ed25519, x25519 } from './vendor/noble/curves/ed25519.js';
import { sha256 } from './vendor/noble/hashes/sha2.js';
import { hmac } from './vendor/noble/hashes/hmac.js';
import { extract, expand } from './vendor/noble/hashes/hkdf.js';
import { bytesToNumberLE, numberToBytesLE, equalBytes, hexToBytes } from './vendor/noble/curves/utils.js';
import { Frame, nodeId, base64, decode64, verifyCertificate } from './session.js';

const utf8 = new TextEncoder();
const text = new TextDecoder('utf-8', { fatal: true });
const MAX_MESSAGE_BYTES = 8192;
const MAX_LABEL_CHARS = 80;
const MAX_USER_AGENT_CHARS = 512;

/** Where the reference client keeps its keys and pins (spec/protocol.md §7.6). */
export const STORAGE_KEY = 'pv:device';

/**
 * The sixteen glyphs of spec/protocol.md §7.3, in index order — wire meaning. The two
 * variation selectors are escapes so nothing can normalize them away.
 */
export const GLYPHS = Object.freeze([
  { glyph: '\u{1F984}', label: 'Unicorn' },
  { glyph: '\u{1F3A7}', label: 'Headphones' },
  { glyph: '\u{1F355}', label: 'Pizza' },
  { glyph: '\u{1F6F8}', label: 'UFO' },
  { glyph: '\u{1F3B8}', label: 'Guitar' },
  { glyph: '\u{1F344}', label: 'Mushroom' },
  { glyph: '\u{1F48E}', label: 'Diamond' },
  { glyph: '\u{1F98A}', label: 'Fox' },
  { glyph: '\u{26A1}\u{FE0F}', label: 'Lightning' },
  { glyph: '\u{1F336}\u{FE0F}', label: 'Hot Pepper' },
  { glyph: '\u{1F9A9}', label: 'Flamingo' },
  { glyph: '\u{1F3A8}', label: 'Artist Palette' },
  { glyph: '\u{1F34D}', label: 'Pineapple' },
  { glyph: '\u{1F341}', label: 'Maple Leaf' },
  { glyph: '\u{1F3B2}', label: 'Game Die' },
  { glyph: '\u{1F353}', label: 'Strawberry' },
].map(Object.freeze));

/** The spellings a typed label may take, longest first per glyph; see the Rust twin. */
const LABEL_SPELLINGS = [
  ['unicorn'], ['headphones'], ['pizza'], ['ufo'], ['guitar'], ['mushroom'], ['diamond'], ['fox'],
  ['lightning'], ['hotpepper', 'pepper'], ['flamingo'], ['artistpalette', 'palette'], ['pineapple'],
  ['mapleleaf', 'maple', 'leaf'], ['gamedie', 'die'], ['strawberry'],
];

/**
 * The 256 words of spec/pairing-words.txt in the file's order — wire meaning. A test holds
 * this copy to the file; the file is normative.
 */
export const WORDS = Object.freeze([
  'abyss', 'acid', 'afloat', 'afraid', 'again', 'agency', 'aghast', 'ajar', 'algae', 'aliens', 'also', 'always',
  'amoeba', 'amuser', 'anchor', 'animal', 'anklet', 'answer', 'aorta', 'apnea', 'apple', 'arena', 'ascend', 'asleep',
  'atlas', 'atom', 'attic', 'avenue', 'awhile', 'awning', 'awoke', 'azalea', 'bakery', 'bamboo', 'banana', 'basket',
  'blade', 'blimp', 'blouse', 'bobcat', 'body', 'boiler', 'bonnet', 'boots', 'bottle', 'breath', 'broom', 'buckle',
  'bunny', 'busboy', 'cabin', 'cactus', 'cage', 'camera', 'carrot', 'cashew', 'caviar', 'cedar', 'celery', 'cement',
  'census', 'chrome', 'chute', 'circle', 'clay', 'clock', 'cobweb', 'copier', 'cornea', 'cotton', 'couch', 'coyote',
  'crib', 'cuddly', 'curry', 'cymbal', 'dagger', 'dairy', 'dawn', 'dealer', 'debris', 'decal', 'degree', 'depot',
  'device', 'dibs', 'digit', 'dimple', 'ditto', 'doctor', 'dodge', 'doll', 'donut', 'dorsal', 'double', 'dozed',
  'drum', 'dryer', 'duffel', 'dugout', 'duplex', 'duvet', 'easel', 'ebook', 'edged', 'editor', 'eerie', 'eggnog',
  'elbow', 'elves', 'emcee', 'energy', 'engine', 'envoy', 'enzyme', 'essay', 'ethics', 'eulogy', 'exam', 'exhale',
  'exist', 'fabric', 'faded', 'falcon', 'family', 'fasten', 'faucet', 'feline', 'femur', 'fence', 'ferret', 'fiddle',
  'fillet', 'flight', 'focus', 'foggy', 'fondue', 'fossil', 'fridge', 'fruit', 'gadget', 'garlic', 'gecko', 'gerbil',
  'geyser', 'gizmo', 'glove', 'gnarly', 'gong', 'gooey', 'gothic', 'grape', 'grill', 'guitar', 'gusto', 'haiku',
  'hefty', 'helmet', 'herbs', 'hubcap', 'huff', 'human', 'hunter', 'hybrid', 'icing', 'iconic', 'idly', 'igloo',
  'iguana', 'iodine', 'ipad', 'iphone', 'iron', 'island', 'itunes', 'ivory', 'jaguar', 'jazz', 'jeep', 'jelly',
  'jersey', 'jetski', 'jiffy', 'jigsaw', 'john', 'jovial', 'juggle', 'juice', 'juror', 'kabob', 'karate', 'kayak',
  'kennel', 'khaki', 'kimono', 'kiosk', 'kite', 'koala', 'ladder', 'laptop', 'latch', 'lemon', 'length', 'levers',
  'lilac', 'lint', 'liquid', 'litter', 'lizard', 'llama', 'luau', 'lyrics', 'maimed', 'mammal', 'mango', 'modify',
  'movie', 'mower', 'mule', 'mummy', 'muppet', 'mural', 'myriad', 'myth', 'nail', 'napkin', 'nebula', 'nectar',
  'nephew', 'nest', 'neuron', 'niece', 'nugget', 'nylon', 'oasis', 'object', 'obtain', 'ocular', 'office', 'older',
  'onion', 'onward', 'onyx', 'oomph', 'opera', 'opium', 'ought', 'oven', 'owlish', 'oxford', 'oxygen', 'oyster',
  'ozone', 'palm', 'patio', 'pauper'

]);

const PREFIX = 3;
const error = message => new Error(message);
const UNRECOGNIZED = 'the pairing code was not recognized: expected the four emoji from the pad, their four labels, or the two words shown on the node';

/** A code as its two renderings (spec/protocol.md §7.2): four glyphs with labels, two words. */
export function encodeCode(code) {
  if (!Number.isInteger(code) || code < 0 || code > 0xFFFF) throw error('a pairing code is sixteen bits');
  const glyphs = [12, 8, 4, 0].map(shift => GLYPHS[(code >> shift) & 0xF]);
  return { emoji: glyphs.map(g => g.glyph), labels: glyphs.map(g => g.label), words: [WORDS[code >> 8], WORDS[code & 0xFF]] };
}

/**
 * Read a typed code in any accepted rendering — the four glyphs (with or without their
 * variation selectors), their four labels, or the two words, case-insensitively with
 * spaces, hyphens and punctuation ignored and each word abbreviable to three letters or
 * more. Returns the sixteen-bit integer; throws an error naming what was expected and
 * never repeating the input. Mirrors `pair::Code::parse` exactly.
 */
export function parseCode(input) {
  if (typeof input !== 'string') throw error(UNRECOGNIZED);
  const glyphs = [], tokens = [];
  let current = '';
  for (const ch of input) {
    if (/^[A-Za-z]$/.test(ch)) { current += ch.toLowerCase(); continue; }
    if (current) { tokens.push(current); current = ''; }
    const index = GLYPHS.findIndex(g => g.glyph.startsWith(ch));
    if (index >= 0) glyphs.push(index);
  }
  if (current) tokens.push(current);
  if (glyphs.length) {
    if (glyphs.length !== 4 || tokens.length) throw error(UNRECOGNIZED);
    return (glyphs[0] << 12) | (glyphs[1] << 8) | (glyphs[2] << 4) | glyphs[3];
  }
  if (!tokens.length) throw error('enter the pairing code: the four emoji, their four labels, or the two words');
  if (tokens.length === 2) {
    const [high, low] = tokens.map(wordIndex);
    if (high >= 0 && low >= 0) return (high << 8) | low;
  }
  const joined = tokens.join('');
  const words = joinedWords(joined);
  if (words !== null) return words;
  const labels = joinedLabels(joined);
  if (labels !== null) return labels;
  throw error(UNRECOGNIZED);
}

function wordIndex(token) {
  return token.length < PREFIX ? -1 : WORDS.findIndex(w => w.startsWith(token));
}

function joinedWords(joined) {
  let rest = joined; const bytes = [];
  while (rest.length) {
    if (bytes.length === 2 || rest.length < PREFIX) return null;
    const index = WORDS.findIndex(w => rest.startsWith(w) && w.startsWith(rest.slice(0, PREFIX)));
    if (index < 0) return null;
    bytes.push(index); rest = rest.slice(WORDS[index].length);
  }
  return bytes.length === 2 ? (bytes[0] << 8) | bytes[1] : null;
}

function joinedLabels(joined) {
  let rest = joined; const nibbles = [];
  while (rest.length) {
    if (nibbles.length === 4) return null;
    let best = null;
    LABEL_SPELLINGS.forEach((spellings, index) => {
      for (const s of spellings) if (rest.startsWith(s) && (!best || s.length > best.s.length)) best = { index, s };
    });
    if (!best) return null;
    nibbles.push(best.index); rest = rest.slice(best.s.length);
  }
  return nibbles.length === 4 ? (nibbles[0] << 12) | (nibbles[1] << 8) | (nibbles[2] << 4) | nibbles[3] : null;
}

// ---------------------------------------------------------------------------------------
// SPAKE2, RFC 9382 §3 over edwards25519 (spec/protocol.md §7.4.1)
// ---------------------------------------------------------------------------------------

const Point = ed25519.Point;
const ORDER = Point.Fn.ORDER;
const M_HEX = 'd048032c6ea0b6d697ddc2e86bda85a33adac920f1bf18e1b0c6d166a5cecdaf';
const N_HEX = 'd3bfb518f44f3430f29d0c92af503865a1ed3281dc69b35dd868ba85f886c4ab';
const INVALID = 'cannot pair: the other side sent an invalid key-exchange message';

/** Decode a point, refusing non-canonical encodings and small-order points. */
function point(bytes) {
  let p;
  try { p = Point.fromBytes(bytes, false); } catch { throw error(INVALID); }
  if (!equalBytes(p.toBytes(), bytes) || p.isSmallOrder()) throw error(INVALID);
  return p;
}

function scalarWide(bytes64) {
  const s = bytesToNumberLE(bytes64) % ORDER;
  if (s === 0n) throw error('cannot pair: a zero scalar was drawn; try again');
  return s;
}

function lengthPrefixed(...parts) {
  const out = [];
  for (const part of parts) { out.push(numberToBytesLE(BigInt(part.length), 8), part); }
  const total = out.reduce((n, p) => n + p.length, 0), joined = new Uint8Array(total);
  let at = 0;
  for (const p of out) { joined.set(p, at); at += p.length; }
  return joined;
}

/**
 * SPAKE2 as spec/protocol.md §7.4.1 fixes it, both sides, as pure functions over bytes.
 * `password(code)` is `w`; `start(side, w, secret64)` yields `{ message, finish }`; `finish`
 * takes the other side's 32-byte message and the two base64 Ed25519 keys and yields
 * `{ ke, confirmSend, verify(theirConfirm), transcript }`. Secrets are BigInts and cannot
 * be wiped; the caller drops the state as soon as it has `ke`.
 */
export const spake2 = Object.freeze({
  M: M_HEX,
  N: N_HEX,
  password(code) {
    if (!Number.isInteger(code) || code < 0 || code > 0xFFFF) throw error('a pairing code is sixteen bits');
    const ikm = Uint8Array.from([code >> 8, code & 0xFF]);
    return scalarWide(expand(sha256, extract(sha256, ikm), utf8.encode('pv/1 pake w'), 64));
  },
  start(side, w, secret64) {
    if (side !== 'A' && side !== 'B') throw error('side is A or B');
    if (typeof w !== 'bigint' || w <= 0n || w >= ORDER) throw error('invalid password scalar');
    const x = scalarWide(secret64);
    const blind = point(hexToBytes(side === 'A' ? M_HEX : N_HEX)), unblind = point(hexToBytes(side === 'A' ? N_HEX : M_HEX));
    const message = blind.multiply(w).add(Point.BASE.multiply(x)).toBytes();
    let used = false;
    return Object.freeze({
      message,
      finish(theirMessage, deviceEd25519Base64, nodeEd25519Base64) {
        if (used) throw error('a key-exchange state is single use');
        used = true;
        const their = point(theirMessage);
        const k = their.subtract(unblind.multiply(w)).multiply(x).clearCofactor();
        if (k.equals(Point.ZERO)) throw error(INVALID);
        const [pa, pb] = side === 'A' ? [message, theirMessage] : [theirMessage, message];
        const transcript = lengthPrefixed(
          utf8.encode('pv/1 device ' + deviceEd25519Base64), utf8.encode('pv/1 node ' + nodeEd25519Base64),
          pa, pb, k.toBytes(), numberToBytesLE(w, 32));
        const digest = sha256(transcript);
        const ke = digest.slice(0, 16), ka = digest.slice(16);
        const kc = expand(sha256, extract(sha256, ka), utf8.encode('ConfirmationKeys'), 32);
        const [sendKey, expectKey] = side === 'A' ? [kc.slice(0, 16), kc.slice(16)] : [kc.slice(16), kc.slice(0, 16)];
        const confirmSend = hmac(sha256, sendKey, transcript);
        return Object.freeze({ ke, confirmSend, transcript,
          verify: theirConfirm => theirConfirm instanceof Uint8Array && equalBytes(hmac(sha256, expectKey, transcript), theirConfirm) });
      },
    });
  },
});

/** The two frames of §7.4.2 over `K_pair`, for the client: `{ send, receive }`. */
function pairFrames(ke) {
  const prk = extract(sha256, ke, utf8.encode('pv/1 pair'));
  return { send: new Frame(expand(sha256, prk, utf8.encode('pv/1 c2s'), 32), 1),
    receive: new Frame(expand(sha256, prk, utf8.encode('pv/1 s2c'), 32), 2) };
}

// ---------------------------------------------------------------------------------------
// The six messages (spec/protocol.md §7.4.2)
// ---------------------------------------------------------------------------------------

const validId = id => typeof id === 'string' && /^[0-9abcdefghjkmnpqrstvwxyz]{8}$/.test(id);

function parseJson(data, limit = MAX_MESSAGE_BYTES) {
  if (typeof data !== 'string' || data.length > limit) throw error('cannot pair: the node sent an invalid message');
  let value;
  try { value = JSON.parse(data); } catch { throw error('cannot pair: the node sent an invalid message'); }
  if (value === null || typeof value !== 'object' || Array.isArray(value)) throw error('cannot pair: the node sent an invalid message');
  return value;
}

function clean(value, max) {
  if (typeof value !== 'string') return undefined;
  const kept = Array.from(value).filter(c => !/[\p{Cc}]/u.test(c)).slice(0, max).join('').trim();
  return kept || undefined;
}

/** Wrap a WebSocket-shaped object so its messages can be awaited in order. */
function inbox(socket) {
  const queue = [], waiters = [];
  let failure = null;
  const settle = () => { while (waiters.length && (queue.length || failure)) {
    const waiter = waiters.shift();
    if (queue.length) waiter.resolve(queue.shift()); else waiter.reject(failure);
  } };
  const closed = code => { failure ??= error(`cannot pair: the node closed the connection (${code})`); settle(); };
  socket.addEventListener('message', event => { queue.push(event.data); settle(); });
  socket.addEventListener('close', event => closed(event?.code ?? 'closed'));
  socket.addEventListener('error', () => closed('error'));
  return {
    next: () => new Promise((resolve, reject) => { waiters.push({ resolve, reject }); settle(); }),
  };
}

async function binary(data) {
  if (data instanceof Uint8Array) return data;
  if (data instanceof ArrayBuffer) return new Uint8Array(data);
  if (typeof Blob !== 'undefined' && data instanceof Blob) return new Uint8Array(await data.arrayBuffer());
  throw error('cannot pair: the node sent an invalid message');
}

function defaultRandom(n) {
  const bytes = new Uint8Array(n);
  globalThis.crypto.getRandomValues(bytes);
  return bytes;
}

/**
 * Pair this browser with the node on `socket` — a WebSocket already opened to `/ws/pair`
 * (or anything with `send` and `addEventListener` for `message`, `close` and `error`) —
 * using `code`, a sixteen-bit integer or a string `parseCode` accepts. Options: `label`
 * and `userAgent` for the devices page; `storage` (default `localStorage`), where the
 * record goes under `pv:device`; `now` in UTC milliseconds; `random(n)`, the CSPRNG,
 * for the vector tests alone.
 *
 * Resolves to the stored record: the device's ID and both keypairs, the node's ID, keys
 * and certificate, and the pinned cluster key (spec/protocol.md §7.6). Rejects — and
 * sends nothing further — when pairing is closed, when the code is wrong (the node's
 * confirmation fails before this side sends its own, §7.4.2), when storage is
 * unavailable (checked before any message is sent), and on any malformed or unverifiable
 * message. Never sends the code or `w`.
 */
export async function pair(socket, code, options = {}) {
  const value = typeof code === 'string' ? parseCode(code) : code;
  if (!Number.isInteger(value) || value < 0 || value > 0xFFFF) throw error(UNRECOGNIZED);
  const storage = options.storage ?? globalThis.localStorage;
  const probe = STORAGE_KEY + ':probe';
  try { storage.setItem(probe, '1'); storage.removeItem(probe); }
  catch { throw error('cannot pair: this browser will not keep the pairing; enable site storage and try again'); }
  const random = options.random ?? defaultRandom;
  const now = options.now ?? Date.now();
  const messages = inbox(socket);
  if ('binaryType' in socket) socket.binaryType = 'arraybuffer';

  const hello = parseJson(await messages.next());
  if (hello.v !== 1) throw error('cannot pair: the node speaks another protocol version; use a pv/1 client');
  if (hello.open !== true) throw error('pairing is closed on the node; open it there and try again');
  if (!validId(hello.id)) throw error('cannot pair: the node sent an invalid message');
  const nodePublic = decode64(hello.pub, 32);
  if (nodeId(nodePublic) !== hello.id || Point.fromBytes(nodePublic).isSmallOrder()) throw error('cannot pair: the node sent an invalid message');

  const edSecret = random(32), xSecret = random(32);
  const edPublic = ed25519.getPublicKey(edSecret), xPublic = x25519.getPublicKey(xSecret);
  const device = nodeId(edPublic), devicePublicB64 = base64(edPublic);
  const state = spake2.start('A', spake2.password(value), random(64));
  socket.send(JSON.stringify({ v: 1, dev: device, pub: devicePublicB64, kind: 'browser', pA: base64(state.message) }));

  const reply = parseJson(await messages.next());
  const shared = state.finish(decode64(reply.pB, 32), devicePublicB64, hello.pub);
  if (!shared.verify(decode64(reply.cB, 32))) throw error('the pairing code did not match; check the code on the node and try again');
  socket.send(JSON.stringify({ cA: base64(shared.confirmSend) }));

  const frames = pairFrames(shared.ke);
  const sealedIn = await binary(await messages.next());
  if (sealedIn.length > MAX_MESSAGE_BYTES) throw error('cannot pair: the node sent an invalid message');
  let facts;
  try { facts = JSON.parse(text.decode(frames.receive.open(sealedIn))); } catch { throw error('cannot pair: the node sent an invalid message'); }
  if (facts === null || typeof facts !== 'object' || !validId(facts.cluster_id)) throw error('cannot pair: the node sent an invalid message');
  const clusterPublic = decode64(facts.cluster_pub, 32);
  if (nodeId(clusterPublic) !== facts.cluster_id) throw error('cannot pair: the node sent an invalid message');
  verifyCertificate(facts.cert, { id: hello.id, cluster: clusterPublic }, hello.id, now);
  const cert = JSON.parse(text.decode(decode64(facts.cert)));
  if (cert.cluster_id !== facts.cluster_id || cert.node_pub !== hello.pub) throw error('cannot pair: the node sent an invalid message');
  const nodeX25519 = decode64(facts.x25519, 32);
  try { x25519.getSharedSecret(xSecret, nodeX25519); } catch { throw error('cannot pair: the node sent an invalid message'); }

  const out = { x25519: base64(xPublic) };
  const label = clean(options.label, MAX_LABEL_CHARS), ua = clean(options.userAgent, MAX_USER_AGENT_CHARS);
  if (label) out.label = label;
  if (ua) out.ua = ua;
  socket.send(frames.send.seal(utf8.encode(JSON.stringify(out))));
  frames.send.close(); frames.receive.close();

  const record = {
    v: 1,
    dev: device,
    ed25519: { secret: base64(edSecret), public: devicePublicB64 },
    x25519: { secret: base64(xSecret), public: base64(xPublic) },
    node: { id: hello.id, ed25519: hello.pub, x25519: facts.x25519, cert: facts.cert },
    cluster: { id: facts.cluster_id, pub: facts.cluster_pub },
    paired_at: new Date(now).toISOString(),
  };
  edSecret.fill(0); xSecret.fill(0);
  try { storage.setItem(STORAGE_KEY, JSON.stringify(record)); }
  catch { throw error('cannot pair: this browser will not keep the pairing; enable site storage and try again'); }
  return record;
}
