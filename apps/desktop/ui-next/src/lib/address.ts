/* eslint-disable */
// @ts-nocheck -- Bridge: a verbatim copy of src/mock/address.js (the mock needs the plain JS file) so the new UI builds now.
// ui-next-d replaces this with a typed port (same export names).

// Bitcoin-style addresses (bech32m, BIP-350) for users, servers and devices: dsu1…, dss1…, dsd1….
// Matches crates/dissent-identity/src/address.rs. The payload is the full 32-byte key.
// Display and input only: IDs stay lowercase base32 in state, storage, links and the backend.

const B32 = 'abcdefghijklmnopqrstuvwxyz234567';
const CHARSET = 'qpzry9x8gf2tvdw0s3jn54khce6mua7l';
const BECH32M_CONST = 0x2bc830a3;
const CHECKSUM_LEN = 6;
const MAX_LEN = 90;

export const HRP = { user: 'dsu', server: 'dss', device: 'dsd' };
const KIND_OF_HRP = { dsu: 'user', dss: 'server', dsd: 'device' };

// A message fit for showing to the person who pasted the text.
export class AddressError extends Error {}

function polymod(values) {
  const GEN = [0x3b6a57b2, 0x26508e6d, 0x1ea119fa, 0x3d4233dd, 0x2a1462b3];
  let chk = 1;
  for (const v of values) {
    const top = chk >>> 25;
    chk = ((chk & 0x1ffffff) << 5) ^ v;
    for (let i = 0; i < 5; i++) if ((top >>> i) & 1) chk ^= GEN[i];
  }
  return chk >>> 0;
}

function hrpExpand(hrp) {
  const out = [];
  for (const c of hrp) out.push(c.charCodeAt(0) >> 5);
  out.push(0);
  for (const c of hrp) out.push(c.charCodeAt(0) & 31);
  return out;
}

// Regroup bits between widths (BIP-173 convertbits). Without padding, leftover bits must be zero.
function convertBits(data, from, to, pad) {
  let acc = 0;
  let bits = 0;
  const out = [];
  const maxv = (1 << to) - 1;
  for (const v of data) {
    if (v >> from) return null;
    acc = ((acc << from) | v) & 0xffffff;
    bits += from;
    while (bits >= to) {
      bits -= to;
      out.push((acc >>> bits) & maxv);
    }
  }
  if (pad) {
    if (bits > 0) out.push((acc << (to - bits)) & maxv);
  } else if (bits >= from || ((acc << (to - bits)) & maxv) !== 0) {
    return null;
  }
  return out;
}

// Bytes to a bech32m string under `hrp`.
export function encodeAddress(hrp, bytes) {
  const data = convertBits(bytes, 8, 5, true);
  const pm = polymod([...hrpExpand(hrp), ...data, 0, 0, 0, 0, 0, 0]) ^ BECH32M_CONST;
  for (let i = 0; i < CHECKSUM_LEN; i++) data.push((pm >>> (5 * (5 - i))) & 31);
  return `${hrp}1${data.map((v) => CHARSET[v]).join('')}`;
}

// A bech32m string to { hrp, bytes }. Throws AddressError with a message for the person.
export function decodeAddress(text) {
  const s = text.trim();
  if (s.length < CHECKSUM_LEN + 2 || s.length > MAX_LEN) throw new AddressError('That address has the wrong length.');
  if (/[^\x21-\x7e]/.test(s)) throw new AddressError('That address contains an invalid character.');
  if (/[a-z]/.test(s) && /[A-Z]/.test(s)) throw new AddressError('That address mixes upper and lower case.');
  const lower = s.toLowerCase();
  const sep = lower.lastIndexOf('1');
  if (sep < 1) throw new AddressError('That is not a complete address.');
  const hrp = lower.slice(0, sep);
  const rest = lower.slice(sep + 1);
  if (rest.length < CHECKSUM_LEN) throw new AddressError('That address is cut short.');
  const data = [];
  for (const c of rest) {
    const p = CHARSET.indexOf(c);
    if (p < 0) throw new AddressError('That address contains a character that is not allowed.');
    data.push(p);
  }
  if (polymod([...hrpExpand(hrp), ...data]) !== BECH32M_CONST) {
    throw new AddressError('That address has a typo. Check it and paste it again.');
  }
  const bytes = convertBits(data.slice(0, data.length - CHECKSUM_LEN), 5, 8, false);
  if (!bytes) throw new AddressError('That address is malformed.');
  return { hrp, bytes };
}

// RFC 4648 base32, lowercase, no padding. Same as the Rust `Display` of IDs.
export function base32Encode(bytes) {
  let out = '';
  let buffer = 0;
  let bits = 0;
  for (const b of bytes) {
    buffer = ((buffer << 8) | b) & 0xffffff;
    bits += 8;
    while (bits >= 5) {
      bits -= 5;
      out += B32[(buffer >>> bits) & 31];
    }
  }
  if (bits > 0) out += B32[(buffer << (5 - bits)) & 31];
  return out;
}

export function base32Decode(text) {
  const bytes = [];
  let buffer = 0;
  let bits = 0;
  for (const c of text.toLowerCase()) {
    const p = B32.indexOf(c);
    if (p < 0) return null;
    buffer = ((buffer << 5) | p) & 0xffffff;
    bits += 5;
    if (bits >= 8) {
      bits -= 8;
      bytes.push((buffer >>> bits) & 255);
    }
  }
  return bytes;
}

// The address of a base32 ID (the form the backend and state use), e.g. addressOf('user', root_id).
export function addressOf(kind, id) {
  const bytes = base32Decode(id ?? '');
  if (!bytes || bytes.length !== 32) return '';
  return encodeAddress(HRP[kind], bytes);
}

// The shortened form shown in tags and cards: first 8 and last 6 characters.
export function shortAddress(address) {
  if (!address) return '';
  return address.length > 15 ? `${address.slice(0, 8)}…${address.slice(-6)}` : address;
}

// The base32 ID a search box holds when it holds a valid address, else null. A bad address matches nothing.
export function searchId(text) {
  try {
    return parseAddressInput(text)?.id ?? null;
  } catch {
    return null;
  }
}

// Search: a person matches by name, by their tag, or by a pasted address. `q` is lowercase and trimmed.
export function matchesSearch(p, q) {
  if (!q) return true;
  if (p.display_name.toLowerCase().includes(q) || (p.suffix ?? '').includes(q)) return true;
  const id = searchId(q);
  return id !== null && id === p.root_id;
}

// What to check before a pasted address or short link goes to the backend to be looked up. Returns true
// for an address or short link, false for anything else (a long link is sent as it is). Throws AddressError
// for a bad checksum or an address of another kind, with a message for the person.
export function isAddressToAdd(text) {
  const found = parseAddressInput(text);
  if (!found) return false;
  if (found.kind !== 'user') {
    throw new AddressError(`That is a ${found.kind} address. Ask for a person's address or contact link.`);
  }
  return true;
}

// The short address shown in tags. Falls back to the base32 prefix if the ID does not convert.
export function shortOf(kind, id) {
  return shortAddress(addressOf(kind, id)) || (id ?? '').slice(0, 8);
}

// Reads text pasted where an ID is accepted. Returns null when the text is not an address at all
// (so it can be a name to search for). Returns { kind, id } with the base32 ID the backend expects,
// or throws AddressError for a wrong kind or a bad checksum. `kind` limits the accepted kinds.
export function parseAddressInput(text, kind = null) {
  // A short contact link is the address behind the prefix. Whitespace from wrapping is ignored.
  const s = (text ?? '').replace(/\s+/g, '').replace(/^dissent:\/\/contact\//i, '');
  if (!/^ds[a-z]1/i.test(s)) return null;
  const { hrp, bytes } = decodeAddress(s);
  const found = KIND_OF_HRP[hrp];
  if (!found) throw new AddressError('That is not a Dissent address.');
  if (kind && found !== kind) {
    throw new AddressError(`That is a ${found} address, not a ${kind} address.`);
  }
  return { kind: found, id: base32Encode(bytes) };
}
