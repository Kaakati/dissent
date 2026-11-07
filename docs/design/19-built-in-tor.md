# Design 19: the built-in Tor

"Hide my IP through Tor" (design 06, "Hide through Tor") first used only the Tor the person runs. This adds a Tor
built into the app, the default for that choice: arti, the Tor Project's Rust Tor, shipped as a program next to the
app's own.

## Why a separate program

arti-client, linked into the app, calls `std::process::exit(1)` when a new consensus requires a protocol it lacks
(arti-client 0.48.0, `client.rs`, `enforce_protocol_recommendations`). Nothing in its config turns that off. So arti
runs as a sidecar: `arti proxy`, which ends only itself. The bridge that P1 added (`dissent_net::socks_bridge`) stays
as it is, and follows arti's SOCKS port instead of a fixed one.

## How it runs

`dissent_net::TorSidecar` (`crates/dissent-net/src/tor_sidecar.rs`):

- **Config.** Written to `<profile>/tor/arti.toml` at each start: `socks_listen = "127.0.0.1:auto"`,
  `dns_listen = false`, `enable_http_connect = false`, and `state_dir`, `cache_dir` and `port_info_file` inside the same
  folder, as literal paths. On Unix the folder is 0700.
- **Port.** arti writes the port the system gave it to `port_info.json`. The supervisor removes a stale one before each
  start, reads the new one, and publishes the address on a watch channel. `SocksBridge::follow` reads it for each
  tunnel: `None` (arti starting, restarting or stopped) answers 502.
- **Exits.** Its stderr goes to the app's log (target `arti`). When a line contains `OBSOLETE_MARKER`, the exit that
  follows is final: state `Obsolete`, no restart, and the indicator says "Tor stopped: update Dissent". Any other exit
  restarts after 1 s, doubling to 60 s, back to 1 s after a run of a minute. A program that can't start (missing from a
  development build) is `Failed`, and Tor mode stays offline and says why.
- **Lifetime.** The desktop app starts it in `tor::proxy_for` before the engine, keeps it while the profile and the
  choice stay the same, and stops it when Tor mode is turned off, on a profile switch, on quit (`RunEvent::Exit`) and
  before an update installs (Windows can't replace a running program). When the app dies, Windows kills it through a
  job object and Linux through `PR_SET_PDEATHSIG(SIGKILL)`, set in `pre_exec`. That signal comes when the thread that
  started arti ends, so arti is always started from one thread the supervisor owns, never a tokio thread (a
  blocking-pool one ends after a few idle seconds). On Linux and macOS each start writes
  `arti.pid`, and the next start first kills a process with that pid, but only when its executable (`/proc/<pid>/exe`,
  `proc_pidpath` on macOS) is this same arti program. On macOS, with no death signal, that is what ends an arti a crash
  left behind, at the next start.
- **Program.** `arti` beside the app's executable, where the bundle's `externalBin` puts it, or `DISSENT_ARTI` for
  development.

## Settings

`NetworkSettings.tor_builtin`: true for the built-in Tor, false for the person's own at `tor_socks`. A file from before
it has no value: with Tor on it keeps its own Tor, otherwise the built-in one is used when Tor is turned on. Settings ›
Network shows "Which Tor" under "Hide through Tor": Built into Dissent, or My own Tor with its SOCKS port.

## Build and release

`scripts/build-arti.sh [<target>]` downloads `arti-<ARTI_VERSION>.crate` from crates.io, checks its sha256, fetches its
locked dependencies, checks that arti-client's source still contains `ARTI_MARKER`, and builds it with
`--no-default-features --features tokio,rustls-ring,compression,static-sqlite,harden` into
`apps/desktop/src-tauri/binaries/arti-<target>[.exe]`. The release workflow runs it in each desktop job and adds
`externalBin: ["binaries/arti"]` to the bundle config, so every installer and the portable zip carry it. A test in
dissent-net keeps the script's `ARTI_VERSION` and `ARTI_MARKER` equal to the constants the supervisor uses.

**Monthly bump.** The Tor network stops accepting old arti versions, so arti is updated each month: set
`ARTI_VERSION` and `ARTI_SHA256` (sha256 of the new `.crate`), read the release's changelog for config, CLI and
`port_info.json` changes, and check the obsolete-version line in arti-client's `client.rs`. Then update
`ARTI_VERSION` (and, if it changed, `OBSOLETE_MARKER`) in `tor_sidecar.rs`, and run
`DISSENT_ARTI=<built arti> cargo test -p dissent-net --test tor_sidecar -- --ignored` once against the real network.

## Tests

- `crates/dissent-net/tests/tor_sidecar.rs`, a test without a harness whose own executable is the fake arti: tunnels
  through the reported port, a killed arti restarted with a 502 meanwhile, an obsolete one that stays stopped, a missing
  program, and stop; on Unix an arti left running is killed at the next start, and on Linux arti dies with an app that
  is killed. The unit test `a_left_over_pid_is_ours_only_when_it_runs_our_program` checks that a pid running any other
  program is never taken for arti. With `-- --ignored` and `DISSENT_ARTI` set it also runs the real arti against the Tor network.
- `socks_bridge.rs`: a followed proxy used where it is now and refused while it is down or off this machine.
- `tor_sidecar.rs` unit tests: the config, the port file, and the version pin.
- Desktop: the settings default and migration, the indicator's messages. UI driver: section `hideip`, H6 to H10.

## Measured (arti 2.8.0, Windows 11)

The binary is 17 MB on Windows (thin LTO makes it no smaller). A cold bootstrap took 9 to 16 s, a warm one under 1 s; the real-network test above
runs in about 15 s.
