// Run with: node --test apps/desktop/ui-next/src/mock/address.test.js
// The fixed vectors are the ones asserted in crates/dissent-identity/src/lib.rs (address_fixed_vectors).
import { test } from 'node:test';
import assert from 'node:assert/strict';
import { addressOf, base32Decode, base32Encode, decodeAddress, parseAddressInput, shortAddress, AddressError } from './address.js';

// The public key of the Ed25519 seed [7; 32]. The same key backs the user, server and device forms.
const KEY_B32 = '5jfgyy7ctrjavpxvkb5rglwf7gkuo5vox27hxescd3vgsfcg2iwa';
const KEY_BYTES = base32Decode(KEY_B32);

test('base32 round-trips the 32-byte key', () => {
  assert.equal(KEY_BYTES.length, 32);
  assert.equal(base32Encode(KEY_BYTES), KEY_B32);
});

test('addresses match the Rust encoding of the same keys', () => {
  assert.equal(addressOf('user', KEY_B32), 'dsu1af9xcclzn3fq40h42pa3xtk9lx25wa4wh6l8hyjzrm4xj9zx6gkq8syqmr');
  assert.equal(addressOf('server', KEY_B32), 'dss1af9xcclzn3fq40h42pa3xtk9lx25wa4wh6l8hyjzrm4xj9zx6gkqhrl784');
  assert.equal(addressOf('device', KEY_B32), 'dsd1af9xcclzn3fq40h42pa3xtk9lx25wa4wh6l8hyjzrm4xj9zx6gkqy0w8n3');
});

test('an address decodes back to the key under its prefix', () => {
  const { hrp, bytes } = decodeAddress(addressOf('server', KEY_B32));
  assert.equal(hrp, 'dss');
  assert.deepEqual(bytes, KEY_BYTES);
});

test('BIP-350 valid vector decodes and the invalid ones are refused', () => {
  assert.equal(decodeAddress('abcdef1l7aum6echk45nj3s0wdvt2fg8x9yrzpqzd3ryx').hrp, 'abcdef');
  assert.equal(decodeAddress('A1LQFN3A').hrp, 'a');
  assert.throws(() => decodeAddress('M1VUXWEZ'), AddressError);
  assert.throws(() => decodeAddress('qyrz8wqd2c9m'), AddressError);
  assert.throws(() => decodeAddress('in1muywd'), AddressError);
});

test('a single-character typo is refused with a friendly message', () => {
  const a = addressOf('user', KEY_B32);
  const typo = a.slice(0, -1) + (a.endsWith('q') ? 'p' : 'q');
  assert.throws(() => parseAddressInput(typo, 'user'), (e) => e instanceof AddressError && /typo/.test(e.message));
});

test('a server address pasted as a user is refused', () => {
  const server = addressOf('server', KEY_B32);
  assert.throws(() => parseAddressInput(server, 'user'), /That is a server address, not a user address/);
});

test('pasted addresses convert to base32, in any letter case', () => {
  const user = addressOf('user', KEY_B32);
  assert.deepEqual(parseAddressInput(`  ${user}  `, 'user'), { kind: 'user', id: KEY_B32 });
  assert.deepEqual(parseAddressInput(user.toUpperCase(), 'user'), { kind: 'user', id: KEY_B32 });
  assert.throws(() => decodeAddress(`D${user.slice(1)}`), /mixes upper and lower case/);
});

test('plain text is not an address, so it is left to search', () => {
  assert.equal(parseAddressInput('Maya'), null);
  assert.equal(parseAddressInput(''), null);
});

test('short form keeps the first 8 and last 6 characters', () => {
  const a = addressOf('user', KEY_B32);
  assert.equal(shortAddress(a), `${a.slice(0, 8)}…${a.slice(-6)}`);
  assert.equal(shortAddress(a).length, 15);
  assert.equal(shortAddress(''), '');
});
