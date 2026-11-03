# Contributing to Dissent

Thank you for your interest in Dissent. This guide explains how to help, how to set up a development environment, and what a change needs before it can be merged.

By taking part you agree to follow the [Code of Conduct](CODE_OF_CONDUCT.md). Security problems are not reported here: see [SECURITY.md](SECURITY.md).

## Ways to help

- **Report bugs.** Open an issue with the [bug report template](https://github.com/Kaakati/dissent/issues/new?template=bug_report.md). Include your Dissent version (Settings › About), operating system, and what you expected to happen.
- **Test on real devices and networks.** This is one of the most valuable contributions. Continuous integration runs every networked test on loopback only, so behaviour across home routers, carrier-grade NAT, mobile hotspots, VPNs, corporate firewalls and different countries is mostly untested. Try calls, file transfers and joining servers between machines on different networks, and run the [connectivity probe](apps/probe/README.md) ([real-network notes](apps/probe/REAL_NETWORKS.md)) to report whether two devices connect directly or through a relay.
- **Run a relay or seeder** for a community and report how it behaves: see [`packaging/README.md`](packaging/README.md).
- **Improve documentation.** The README, the tool guides under `apps/*/README.md` and the design notes in [`docs/design/`](docs/design/) all benefit from clearer wording and corrections.
- **Suggest features.** Open an issue with the [feature request template](https://github.com/Kaakati/dissent/issues/new?template=feature_request.md). For anything large, please discuss the design before writing code.
- **Write code.** See [Good first areas](#good-first-areas) below.

The interface is English only and has no translation framework yet. Proposals for adding one are welcome as an issue first.

## Development setup

You need:

- [Rust](https://rustup.rs) (stable), with `clippy`.
- [Node.js](https://nodejs.org) 24 or newer.
- The Tauri CLI: `cargo install tauri-cli --version "^2" --locked`.
- Platform packages, listed per OS in the README's [Build from source](README.md#build-from-source) section. On Linux these are the same packages `.github/workflows/release.yml` installs.
- Optional: NASM, libclang and `git submodule update --init` to build AV1 encoding (the `svt-av1` feature of `dissent-video`).

Then:

```sh
git clone https://github.com/<your-username>/dissent.git
cd dissent
npm --prefix apps/desktop/ui-next ci
scripts/dev-next.sh                       # desktop app against the Vite dev server
npm --prefix apps/desktop/ui-next run dev # the interface alone in a browser, with a mock backend
```

On Windows, run the `scripts/*.sh` helpers from Git Bash.

To run several copies on one machine, give each its own profile and keep them on loopback:

```sh
DISSENT_DATA_DIR=/tmp/dissent-a DISSENT_LOOPBACK=1 scripts/dev-next.sh
DISSENT_DATA_DIR=/tmp/dissent-b DISSENT_LOOPBACK=1 DISSENT_NEXT_PORT=1430 scripts/dev-next.sh
```

`DISSENT_LOOPBACK=1` binds 127.0.0.1 only, with no relays and no DHT. Binding every interface makes Windows Firewall ask for permission for every newly built executable.

### Project layout

The crate map and dependency direction are in the README's [Architecture](README.md#architecture) section. In short: `dissent-identity` ← `dissent-events`, `dissent-net`, `dissent-discovery` ← `dissent-core` ← the desktop app. Crates never depend on Tauri. Each feature has a design note in [`docs/design/`](docs/design/); read the relevant one before changing that area.

## Workflow

1. **Open or find an issue** for anything beyond a small fix, so the approach can be agreed before you invest time.
2. **Fork** the repository and create a branch from `main` with a descriptive name, for example `fix-invite-card-counts`.
3. **Make focused changes.** One logical change per pull request is much easier to review than several.
4. **Run the checks** in [Testing](#testing) for every crate you touched.
5. **Open a pull request against `main`** and fill in the template's checklist.
6. **Review.** A maintainer reviews, may ask for changes, and merges when it is ready.

### Why maintainers merge

Every push to `main` that passes clippy and the test suite is released automatically as the next patch version, with installers for every platform and in-app updates for every user. Because of that, only maintainers merge, and they batch and time merges so a release run is never interrupted. Please don't ask for a merge to be rushed.

Release notes are generated from commit messages (see below), so your commit subjects become what users read.

## Commit messages

Release notes are built by [git-cliff](https://git-cliff.org) from [`cliff.toml`](cliff.toml), which groups commits by the **first word of the subject**:

| First word | Release notes section |
|---|---|
| `Add`, `Introduce`, `Implement`, `Support`, `Create` | Added |
| `Fix`, `Repair`, `Correct`, `Prevent` | Fixed |
| `Harden`, `Secure`, `Protect`, `Encrypt` | Security |
| `Remove`, `Drop`, `Delete` | Removed |
| `Doc`, `Docs`, `Document` | Documentation |
| anything else | Changed |

Write the subject in the imperative mood, describing the change from a user's or reader's point of view, without a trailing period. Body lines that start with `- ` are copied into the release notes under the subject; other body text is kept in history only.

```text
Fix invite cards showing a stale member count

- The card now refreshes its counts when the server's member list changes
```

Merge commits and `Release v...` commits are skipped.

## Code conventions

These keep the codebase consistent and the wire and disk formats stable.

- **Rust edition 2021, workspace dependencies only.** Use `name.workspace = true` in a crate's `Cargo.toml`, and add any new dependency to the root `Cargo.toml` `[workspace.dependencies]` first.
- **Errors.** Each crate has its own `thiserror` enum and `pub type Result<T>`. `anyhow` is allowed only in the app crates and in tests.
- **Encoding.** Use `postcard` for anything hashed, signed or stored, and `serde_json` only across the Tauri boundary.
- **Hashing** uses `blake3`. Ids are `[u8; 32]` newtypes with lowercase, unpadded base32 `Display` and `FromStr` (see `dissent-identity`).
- **Domain separation.** Prefix the bytes of every signature and hash with `b"dissent/<thing>/v1"`.
- **Wire enums are append-only.** Never reorder, remove or insert variants in an enum that is encoded with postcard: add new variants at the end. The `wire_order` tests in `dissent-events` and `dissent-core` pin the order.
- **New event kinds** go through the `SealedControl` carrier, so older clients show them as locked rather than failing to decode. See "Compatibility rule for new control payloads" in [`05-encrypted-servers.md`](docs/design/05-encrypted-servers.md#compatibility-rule-for-new-control-payloads).
- **Sealed stores.** Every file a profile stores is sealed under the vault. A new store must use the same sealing, never plaintext.
- **Stored data keeps loading.** `crates/dissent-core/tests/it/upgrade_compat.rs` loads a profile from an older release. If your change alters an on-disk format, keep old files readable and extend that test.
- **Async** code uses tokio. Keep crate APIs `Send + Sync`, with no global state.
- **Time** is `u64` milliseconds since the Unix epoch everywhere.
- **Platform-specific code** must be clean on every OS: put imports used only under `#[cfg(windows)]` (or unix or macos) behind the same `cfg`.

For the interface in `apps/desktop/ui-next`, follow the existing components and the visual system in [`07-visual-system.md`](docs/design/07-visual-system.md), and keep the app fully usable from the keyboard.

## Testing

Tests live beside the code (`#[cfg(test)]`) or in `crates/*/tests/`. A change should come with tests that would fail without it.

| What | Command |
|---|---|
| One crate | `cargo test -p <crate>` |
| Lints for one crate | `cargo clippy -p <crate> --all-targets -- -D warnings` |
| Whole workspace | `cargo test --workspace` and `cargo clippy --workspace --all-targets -- -D warnings` |
| Networked tests | `scripts/fwtest.sh <package> <test-target or lib> --ignored [filter]` |
| Interface driver | `npm --prefix apps/desktop/ui-next run test:ui` |
| Interface unit tests, types and lint | `npm --prefix apps/desktop/ui-next run test:unit`, `run typecheck`, `run lint` |

### Networked tests

Anything that needs the network is `#[ignore]`. Run those tests **only** through `scripts/fwtest.sh`, which copies the test executable to one fixed path (`target/fw/dissent-test.exe`) so Windows Firewall needs a single permission, and serializes runs. Examples:

```sh
scripts/fwtest.sh dissent-core it --ignored            # all of dissent-core's integration tests
scripts/fwtest.sh dissent-core it --ignored calls::    # only the call suite
scripts/fwtest.sh dissent-discovery lib --ignored      # a crate's own ignored unit tests
```

`dissent-core`'s integration tests are one binary, `it`, in `crates/dissent-core/tests/it/`. They must stay on loopback (`NodeConfig.loopback_only = true`) and use `test_support::fast_timings()`. Production intervals live in `Timings::default()` in `crates/dissent-core/src/timings.rs`; change a production interval there, not in a test. Tests that need the public internet are in `internet.rs` or named `internet_*`, and CI skips them.

On a slow machine, set `DISSENT_TEST_SCALE` (for example `DISSENT_TEST_SCALE=2`) to multiply test waits and limits. CI runs with `2`.

### Interface driver

`npm run test:ui` builds the interface, serves it against the mock backend and drives it through headless Chrome or Edge (`CHROME_PATH` selects the browser; on Windows Edge is the default). `SECTIONS=core,calls` runs only some sections. When you add a UI feature, add it to the mock and the driver; see `apps/desktop/ui-next/README.md` and `PARITY.md`.

### Linux

CI runs clippy with `-D warnings` on Linux, so code gated to Windows or macOS must not leave unused imports there. For pure-Rust crates you can check with:

```sh
cargo clippy --target x86_64-unknown-linux-gnu -p <crate> --all-targets -- -D warnings
```

For crates with C dependencies, run clippy in a Linux container with the packages listed in `release.yml`, or rely on the CI result of your pull request.

### Other notes

- Tests that use the OS credential store are `#[ignore]`, because CI's Linux runner has none.
- `Cargo.lock` pins `rust-ini` to 0.21.1, because `tauri-plugin-deep-link` fails to build on Linux against newer versions. Keep the pin through `cargo update`.

## Security-sensitive changes

Changes to cryptography, key handling, the event DAG and replay rules, permissions, MLS commits, sealing, network exposure or anything that changes what another party can learn need extra care:

- Describe the design in the relevant note under [`docs/design/`](docs/design/), or add a new note, before or with the code.
- Update [`06-threat-model.md`](docs/design/06-threat-model.md): what the change protects against, and what remains.
- Add adversarial tests, for example a malicious member, a banned device or a peer without the invite secret, as in "How the fixes are verified" in the threat model.
- Consider mixed versions. Older clients will see your events and messages; follow the compatibility rule above.

If you find a vulnerability while working, report it privately as described in [SECURITY.md](SECURITY.md) rather than in a pull request.

## Good first areas

- **Documentation:** fixes and clarifications in the README, the tool guides and the design notes.
- **Real-network reports:** probe results, call quality and file transfer behaviour on networks CI never sees.
- **Interface polish** in `apps/desktop/ui-next`: accessibility, keyboard navigation, tooltips and layout at narrow window sizes, with driver coverage.
- **Tests:** more coverage for an existing behaviour, or a regression test for a reported bug.
- **Tools:** the seeder, relay and probe in `apps/`, and their service packaging in `packaging/`.
- **Known gaps** listed in the threat model and design notes, once discussed in an issue.

Issues suitable for newcomers are labelled `good first issue` when available.

## Licensing of contributions

Dissent is dual-licensed under the [MIT License](LICENSE-MIT) and the [Apache License 2.0](LICENSE-APACHE). Unless you explicitly state otherwise, any contribution you intentionally submit for inclusion in Dissent is dual-licensed as above, without any additional terms or conditions.
