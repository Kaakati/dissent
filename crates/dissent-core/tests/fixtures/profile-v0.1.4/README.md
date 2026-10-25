# profile-v0.1.4

A profile written by the v0.1.4 release (tag `v0.1.4`, commit `7328b97`). `tests/it/upgrade_compat.rs`
starts the current engine on a copy of it and checks what is inside. If that test fails after a change,
an older profile no longer loads, so fix the change (add a migration) rather than the fixture.

Everything here is generated. The identity and the peers are throwaway; nothing is a real user's key.

## Layout

- `identity.bin`: the keystore for Alice, sealed under the fixture vault.
- `engine/`: Alice's engine data dir. The `files/` blob store was removed. It is recreated empty on start.

## Opening it

- Passphrase: `dissent fixture v0.1.4`
- Salt: the 16 bytes `dissent-fixture1`
- Vault: `Vault::from_passphrase(passphrase, salt)`, which is Argon2id.

## Expected contents

Alice's display name is `alice`.

- Community `Fixture Server`, owned by alice (role `owner`), with two members: alice and bob.
  - Channels: `general` (text) and `lounge` (voice). Channel order is not part of the contract.
- `general` has 4 messages:
  - `welcome to the fixture server`, with a 👍 reaction from bob.
  - `second message from alice`.
  - `bob says hi`.
  - `replying to the welcome message`, a reply to the welcome message.
- DM with bob: `dm from alice` and `dm from bob`. The DM is closed in the sidebar.
- Group `fixture crew` with bob, carol and dave, holding `group hello`. The group is pinned.
- Contacts: bob, carol and dave.
- Status: `Dnd`, with no end time.
- Transfer settings: auto-accept at 262144 bytes.
- Ignored users: one unknown identity. Its id is not kept, so the test only checks the count.
- Addresses: bob, carol and dave are learned in `engine/addresses.bin`.

## How it was made

1. Check out the `v0.1.4` tag in a scratch worktree.
2. Add a throwaway `#[ignore]` test in that worktree's `crates/dissent-core/tests/it`. It does the following:
   - Makes Alice's identity and saves it with `dissent_identity::keystore::save`.
   - Starts Alice and three throwaway peers with relays off and `loopback_only = true`.
   - Makes all of them contacts. Creates the community, the voice channel, the messages, reaction, reply, DM, and group.
   - Sets status, transfer settings, pin, closed DM and ignored user.
   - Shuts everyone down cleanly.
3. Run it with `DISSENT_FIXTURE_OUT=<dir>` and copy `identity.bin` and `engine/` here, then delete `engine/files/`.

The generator is not kept in the repo. Rebuild a fixture the same way from a new tag when a release
changes an on-disk format.
