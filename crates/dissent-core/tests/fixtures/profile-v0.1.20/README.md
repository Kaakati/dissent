# profile-v0.1.20

A profile written by the v0.1.20 release (commit `1bc781c`), the last release whose store files were plaintext.
`tests/it/upgrade_compat.rs` starts the current engine on a copy of it, checks that the first start seals every
plaintext file, and checks what is inside after each start. If that test fails after a change, an older profile no
longer loads, so fix the change (add a migration) rather than the fixture.

Everything here is generated. The identities are throwaway; nothing is a real user's key or message.

## Layout

- `identity.bin`: the keystore for Alice, sealed under the fixture vault.
- `engine/`: Alice's engine data dir. `files/` (the blob store) was removed, as for the older fixtures.

## Opening it

- Passphrase: `dissent fixture v0.1.20`
- Salt: the 16 bytes `dissent-fixture3`
- Vault: `Vault::from_passphrase(passphrase, salt)`, which is Argon2id.

## Expected contents

Alice (`ngm3pv37…`) has two contacts, bob (`dgdqn7eo…`) and carol (`2f5st7z4…`).

- DM with bob: `secret dm from alice` (edited to `secret dm from alice, edited`) and `secret dm from bob`.
- Group `secret crew` (`kpsqinvd…`) with bob and carol: `secret group hello`.
- Server `Fixture Server`, owned by alice: in `general`, `secret channel text`, and a 96x64 PNG with the text
  `a picture`, whose thumbnail is in `thumbnails/`.
- Alice's own avatar, decrypted in `avatars/`.
- One bookmark (the channel message), one reminder (the DM), the note `secret note about bob` and the nickname
  `Caz` for carol.

## How it was made

1. Extract commit `1bc781c` with `git archive` into a scratch folder, so the working tree is untouched.
2. Add a throwaway `#[ignore]` test there. It saves Alice's identity with `keystore::save`, starts her engine with
   relays off and `loopback_only = true`, starts bob and carol as test peers, makes them contacts, and creates
   everything above. Then it shuts all three down cleanly.
3. Run it with `DISSENT_FIXTURE_OUT=<dir>`. Copy `identity.bin` and `engine/` here, then delete `engine/files/`.

The generator is not kept in the repo.
