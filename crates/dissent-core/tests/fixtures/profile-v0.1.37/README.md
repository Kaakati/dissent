# profile-v0.1.37

A profile written by the v0.1.37 release (commit `a9de5fe`), the first release with file size limits: it holds
`size_limits.bin`, a new sealed store file, and a server with a `ServerFileSizeLimit`. `tests/it/upgrade_compat.rs`
starts the current engine on a copy of it and checks that both limits read back after each start. If that test fails
after a change, a v0.1.37 profile no longer loads, so fix the change (add a migration) rather than the fixture.

Everything here is generated. The identity is throwaway; nothing is a real user's key or message.

## Layout

- `identity.bin`: the keystore for Alice, sealed under the fixture vault.
- `engine/`: Alice's engine data dir. `files/` (the blob store) was removed, as for the older fixtures.

## Opening it

- Passphrase: `dissent fixture v0.1.37`
- Salt: the 16 bytes `dissent-fixture4`
- Vault: `Vault::from_passphrase(passphrase, salt)`, which is Argon2id.

## Expected contents

Alice (`uf4lu7za…`), display name `alice`.

- This device's limits: largest upload 20 GiB, largest download 2 GiB.
- Server `Fixture Server` (`udo5whpq…`), owned by Alice, with a server file size limit of 3 GiB, and in `general` the
  message `limits are set`.

## How it was made

1. Extract commit `a9de5fe` with `git archive` into a scratch folder, so the working tree is untouched.
2. Add a throwaway `#[ignore]` test there. It saves Alice's identity with `keystore::save`, starts her engine with
   relays off and `loopback_only = true`, sets the limits and makes the server above, then shuts it down cleanly.
3. Run it with `DISSENT_FIXTURE_OUT=<dir>`. Copy `identity.bin` and `engine/` here, then delete `engine/files/`.

The generator is not kept in the repo.
