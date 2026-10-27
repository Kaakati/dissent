# profile-v0.1.11

A profile written by the v0.1.11 release (tag `v0.1.11`, commit `f655c16`). `tests/it/upgrade_compat.rs`
starts the current engine on a copy of it and checks what is inside. If that test fails after a change,
an older profile no longer loads, so fix the change (add a migration) rather than the fixture.

Everything here is generated. The identity is throwaway; nothing is a real user's key.

## Layout

- `identity.bin`: the keystore for Alice, sealed under the fixture vault.
- `engine/`: Alice's engine data dir, with one community and no peers.

## Opening it

- Passphrase: `dissent fixture v0.1.11`
- Salt: the 16 bytes `dissent-fixture2`
- Vault: `Vault::from_passphrase(passphrase, salt)`, which is Argon2id.

## Expected contents

Alice's display name is `alice`.

- Community `Fixture Server`, owned by alice (role `owner`), with one member: alice.
  - Channel `general` (text).
- `general` has 2 messages: `welcome to the fixture server` and `second message from alice`.

## How it was made

1. Extract `v0.1.11` with `git archive` into a scratch folder, so the working tree is untouched.
2. Add a throwaway `#[ignore]` test there. It makes Alice's identity, saves it with `keystore::save`, starts
   the engine with relays off and `loopback_only = true`, creates the community, the channel and the two
   messages, and shuts down cleanly.
3. Run it with `DISSENT_FIXTURE_OUT=<dir>`. Copy `identity.bin` and `engine/` here, then delete `engine/files/`.

The generator is not kept in the repo. Rebuild the fixture from the tag when a release changes an on-disk
format.
