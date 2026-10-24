# Dissent

Open-source, decentralized Discord alternative. Rust + Tauri 2. Spec: `docs/Decentralized Discord Alternative — Architecture Spec.md`.

## Layout

```
crates/dissent-identity   Ed25519 root/device/community keys, DeviceCert, seed phrase, keystore
crates/dissent-events     signed hash-linked event DAG, HLC, permission state replay
crates/dissent-net        iroh endpoint wrapper: dial by DeviceId, ping ALPN, gossip per channel
crates/dissent-discovery  pkarr community records on Mainline DHT, invite links
crates/dissent-core       Engine: multi-community state, gossip, sync, DMs, persistence (no tauri)
crates/dissent-mls        OpenMLS group encryption; core seals message text per community (see docs/design/05-encrypted-servers.md)
crates/dissent-media      voice/video spike: str0m (sans-IO WebRTC) sessions with SFrame end-to-end media encryption
apps/seeder               headless always-on member: keeps community history available (uses dissent-core)
apps/relay                open-source iroh relay server binary anyone can run
apps/probe                connectivity probe: measures direct vs relayed paths, RTT and time-to-direct between two devices
apps/push-relay           content-blind wake-up relay prototype for mobile push (UnifiedPush / APNs bridge)
apps/desktop/src-tauri    Tauri 2 app (commands + state); apps/desktop/ui = static HTML/JS frontend
```

Dependency direction: identity ← events, net, discovery ← core ← desktop. Crates never depend on tauri.
Current feature design: `docs/design/02-communities-and-dms.md`.

## Conventions

- Edition 2021, workspace deps only (`x.workspace = true`); add new deps to root `Cargo.toml` `[workspace.dependencies]` first.
- Errors: per-crate `thiserror` enum + `pub type Result<T>`. `anyhow` only in the app crate and tests.
- Wire/disk encoding: `postcard` (binary) for anything hashed, signed or stored; `serde_json` only across the Tauri boundary.
- Hashing: `blake3`. IDs are `[u8; 32]` newtypes with lowercase base32 (no pad) `Display`/`FromStr` (see `dissent-identity`).
- Domain-separate every signature/hash: prefix bytes with `b"dissent/<thing>/v1"`.
- Async: tokio. Keep crate APIs `Send + Sync`, no global state.
- Tests live beside code (`#[cfg(test)]`) or in `crates/*/tests/`. Anything needing network/DHT is `#[ignore]`.
- Verify with `cargo test -p <crate>` and `cargo clippy -p <crate> -- -D warnings` before finishing.
- Crate sources for API lookup (do not guess APIs, read these):
  `C:/Users/Kaaka/.cargo/registry/src/index.crates.io-1949cf8c6b5b557f/{iroh-1.4.1,iroh-gossip-0.101.0,pkarr-8.1.0,tauri-2.12.5,bip39-3.0.0,ed25519-dalek-3.0.0}`
  (`examples/` dirs in iroh, iroh-gossip, pkarr are the fastest reference).

## Shared identity contract (`dissent-identity`, already implemented)

```rust
RootId / DeviceId / CommunityId   // [u8;32] pubkey newtypes: Display/FromStr base32, .suffix() 8 chars, .verify(msg,&Signature)
Signature([u8;64])                // serde: base64url when human-readable, raw bytes otherwise
SecretKey { generate, from_seed([u8;32]), seed(), public_bytes(), sign(&[u8]) -> Signature }
RootKey(SecretKey)      { generate, from_seed, id() -> RootId, certify(DeviceId, issued_at_ms) -> DeviceCert }
DeviceKey(SecretKey)    { generate, from_seed, id() -> DeviceId, sign }   // seed() is also the iroh SecretKey
CommunityKey(SecretKey) { generate, from_seed, id() -> CommunityId, sign } // seed() is also the pkarr Keypair
DeviceCert { root, device, issued_at_ms, sig }  .verify() -> Result<()>
Identity { root: Option<RootKey>, device: DeviceKey, cert: DeviceCert }  ::create(now_ms), root_id(), device_id()
IdentityError { InvalidKey, BadSignature, BadCert, Encoding(String), Io, Serde(String) }
```

Time is `u64` milliseconds since Unix epoch everywhere.

## Cross-crate gotchas

- `Dag::insert` checks only the event id. Run `Event::verify(&cert)` before inserting; `replay` trusts the DAG and assumes one community per DAG.
- `GossipMsg.from` is the relaying neighbour, not the author. Authorship comes only from the event signature.
- Call `Node::remember(&peer)` before `subscribe` with that peer as bootstrap; gossip dials by id only.
- A channel's id is the id of its `ChannelCreate` event.
- Known gap: a message concurrent with its channel's `ChannelCreate` can sort before it and be rejected. Set parents to current heads to keep that window small.
- Keystore and DHT records are plaintext; harden before 1.0.
- Tests and local multi-instance checks must set `NodeConfig.loopback_only = true` (desktop: env `DISSENT_LOOPBACK=1`). Binding all interfaces makes Windows Firewall show an admin prompt for every new exe. Run networked `#[ignore]` tests ONLY through `scripts/fwtest.sh <pkg> <test|lib> --ignored ...`, which copies the test exe to the one firewall-allowed path `target/fw/dissent-test.exe`.

## Releases

- Bump `version` in the root `Cargo.toml`, commit, then `git tag vX.Y.Z && git push origin vX.Y.Z`. `.github/workflows/release.yml` tests, builds installers (Windows, macOS arm64/x64, Linux) and tool archives, and publishes a release whose notes come from `cliff.toml` (git-cliff, grouped by the commit subject's first verb: Add, Fix, Harden, Remove, ...). Keep commit subjects imperative and descriptive.
- In-app updates use `tauri-plugin-updater` and the release's signed `latest.json`. The signing key lives outside the repo (`~/.tauri/dissent-updater.key`); CI reads it from the `TAURI_SIGNING_PRIVATE_KEY` secret. Never commit or print the private key.
