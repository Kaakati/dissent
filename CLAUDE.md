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
crates/dissent-files      encrypted file sharing: encrypt-then-chunk, content-addressed transfer via iroh-blobs
crates/dissent-audio      voice pipeline: mic/speaker (cpal), noise suppression + echo cancellation, Opus with FEC, adaptive jitter buffer
crates/dissent-video      call video: capture, AV1 (SVT-AV1 encode behind the `svt-av1` feature, off by default; rav1d decode) and H.264 (OpenH264), bitrate ladder
apps/seeder               headless always-on member: keeps community history available (uses dissent-core)
apps/relay                open-source iroh relay server binary anyone can run
apps/probe                connectivity probe: measures direct vs relayed paths, RTT and time-to-direct between two devices
apps/push-relay           content-blind wake-up relay prototype for mobile push (UnifiedPush / APNs bridge)
apps/desktop/src-tauri    Tauri 2 app (commands + state); apps/desktop/ui-next = React + shadcn/ui frontend (Vite)
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
- Stored files must keep loading: `tests/it/upgrade_compat.rs` loads a v0.1.4 profile. When a release changes on-disk formats, add a new fixture from that release.
- New event kinds go through the `SealedControl` carrier (generation `CARRIER_GENERATION`), so older clients show them as locked instead of failing to decode the batch or the events log. See `docs/design/05-encrypted-servers.md`, "Compatibility rule".
- Keystore and DHT records are plaintext; harden before 1.0.
- Default log filter is `info,mainline=error,mainline::rpc=off`: the Mainline DHT warns on every unresponsive stranger (Windows os error 10060/10054 on its UDP socket), and its routing table logs a false "Could not bootstrap" at startup that recovers within seconds. Dissent warns itself when the DHT stays empty after fresh nodes.
- Tests and local multi-instance checks must set `NodeConfig.loopback_only = true` (desktop: env `DISSENT_LOOPBACK=1`). Binding all interfaces makes Windows Firewall show an admin prompt for every new exe. Run networked `#[ignore]` tests ONLY through `scripts/fwtest.sh <pkg> <test|lib> --ignored ...`, which copies the test exe to the one firewall-allowed path `target/fw/dissent-test.exe`. dissent-core's integration tests are one binary, `it` (`crates/dissent-core/tests/it/`), so e.g. `scripts/fwtest.sh dissent-core it --ignored calls::` runs the call suite and `scripts/fwtest.sh dissent-core it --ignored typing::` the typing suite. Integration tests run engines with `test_support::fast_timings()`. The production intervals live in `Timings::default()` (`crates/dissent-core/src/timings.rs`), which a unit test pins, and `Engine::start` (used by the desktop app) runs on them. Change a production interval there, not in a test.

## Releases

- Every push to `main` (except docs-only) that passes clippy and tests is released automatically as the next patch version; a running build always finishes and a newer push waits behind it. Never push to main while a release run is in progress. For a specific version push a tag `vX.Y.Z`. `.github/workflows/release.yml` tests, builds installers (Windows, macOS arm64/x64, Linux) and tool archives, and publishes a release whose notes come from `cliff.toml` (git-cliff, grouped by the commit subject's first verb: Add, Fix, Harden, Remove, ...). Keep commit subjects imperative and descriptive.
- In-app updates use `tauri-plugin-updater` and the release's signed `latest.json`. The signing key lives outside the repo (`~/.tauri/dissent-updater.key`); CI reads it from the `TAURI_SIGNING_PRIVATE_KEY` secret. Never commit or print the private key.
- CI releases run clippy `-D warnings` on Linux, so platform-gated code must be clean on every OS: put imports used only under `#[cfg(windows)]` (or unix/macos) behind the same cfg. Check other platforms before a stable checkpoint with `cargo clippy --target x86_64-unknown-linux-gnu -p <crate> --all-targets -- -D warnings` (pure-Rust crates) or, for crates with C deps, in Docker: `docker run --rm -e CARGO_TARGET_DIR=/w/target/linux -e CARGO_BUILD_JOBS=12 -v "$PWD":/w -w /w rust:1 bash -c "apt-get update -qq && apt-get install -y -qq libwebkit2gtk-4.1-dev libappindicator3-dev librsvg2-dev libasound2-dev >/dev/null && cargo clippy -p <crate> --all-targets -- -D warnings"`.
