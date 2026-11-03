# Security policy

Dissent is end-to-end encrypted, peer-to-peer software, and security reports are taken seriously. Thank you for helping keep its users safe.

## Supported versions

Dissent is early-stage and releases often. Only the [latest release](https://github.com/Kaakati/dissent/releases/latest) receives fixes; the app updates itself, so fixes ship as a new release rather than as patches to older versions.

## Reporting a vulnerability

**Please do not report security problems in public issues, discussions or pull requests.**

Report them privately through GitHub's private vulnerability reporting:

1. Open the [Security tab](https://github.com/Kaakati/dissent/security) of the repository.
2. Choose **Report a vulnerability**, or go directly to [the advisory form](https://github.com/Kaakati/dissent/security/advisories/new).

The report is visible only to you and the maintainers.

Please include as much of the following as you can:

- The affected component (desktop app, a crate, the seeder, relay, probe or push relay) and the version.
- What an attacker could achieve, and what position they need: a network attacker, a relay, a DHT node, a member of a server, someone holding an invite link, a former member, and so on.
- Steps to reproduce, a proof of concept or a failing test, and any logs.
- Whether the problem is already described as a known limit in [`docs/design/06-threat-model.md`](docs/design/06-threat-model.md). Reports showing that a documented limit is worse than described are welcome too.

## What to expect

- An acknowledgement of your report, normally within a few days. Dissent is maintained by volunteers, so please allow some time.
- An assessment, and questions if anything is unclear.
- A fix in a new release, followed by a published advisory. You will be credited in the advisory unless you prefer not to be.

Please give the maintainers a reasonable time to release a fix before disclosing the issue publicly.

## Scope

In scope: anything that breaks the properties described in the [threat model](docs/design/06-threat-model.md), such as reading or altering messages, calls or files without the keys, impersonating a device or member, bypassing permissions or bans, learning more metadata than documented, or compromising a device through crafted network input, links or files.

Out of scope, as stated in the threat model: an attacker with access to your unlocked computer, malware running as your user, and a compromised root key. Limits already listed as accepted or open in the threat model are known, though new ways to exploit them are still worth reporting.

Never test against other people's devices, servers or communities without their permission. Use your own devices, or several local copies with `DISSENT_LOOPBACK=1` and separate `DISSENT_DATA_DIR` profiles.
