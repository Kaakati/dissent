# Testing on real networks

This guide is for collecting connection measurements from real home, mobile and office networks. It
answers three questions: how often two devices get a direct path, how long that takes, and which relay
carries the call when they do not. The results decide how Dissent picks relays.

You need two computers on different networks. A phone cannot run the probe, but a laptop on the phone's
hotspot measures the mobile network just as well.

## Get the probe

Download `dissent-tools-<version>-<platform>` from the latest GitHub release and unpack it. It holds
`dissent-probe` (`dissent-probe.exe` on Windows). Open a terminal in that folder.

- **Windows:** run `.\dissent-probe.exe` in PowerShell. Windows Firewall asks once whether to allow
  it. Allow it on private networks, and on public networks too if you test from a café or hotspot. A
  blocked probe still works through a relay, but it never gets a direct path, so the results would be
  wrong.
- **macOS:** the binary is not notarized. Clear the download flag once with
  `xattr -d com.apple.quarantine ./dissent-probe`, then run `./dissent-probe`. If macOS asks whether to
  accept incoming connections, choose Allow.
- **Linux:** `chmod +x dissent-probe`, then `./dissent-probe`.

The examples below write `dissent-probe`. Use `.\dissent-probe.exe` or `./dissent-probe` as above.

## On each network: the one-device check (1 minute)

Run this once on every network you test, on each device. It needs no second device.

```sh
dissent-probe check --network "home Wi-Fi"
```

Name the network in your own words, such as `"home Wi-Fi (Vodafone)"`, `"phone hotspot (O2, 5G)"` or
`"office guest Wi-Fi"`. Add `--note "anything odd"` for extra context.

It prints, and saves to `probe-results/`:

- **udp ipv4 / ipv6:** whether UDP gets out. "no / no" means a direct path is impossible from here and
  every call goes through a relay.
- **mapping varies:** "yes" means a hard NAT that gives a different public port for each destination.
  This is common on mobile networks and corporate firewalls. Direct paths then work only if the other
  side is easy.
- **verdict:** a one-line reading of the two lines above.
- **relays:** each relay, fastest first. `tcp rtt` is one network round trip to the relay, and it is
  the number to compare. The `net report` figure is a fresh HTTPS request, so it reads higher whenever
  UDP is blocked.
- **dht:** whether the Mainline DHT answers, and how fast. Dissent uses it to find communities and
  devices by ID.

## Between two networks: listen and dial (1 minute)

1. On device A, start the listener and name its network:

   ```sh
   dissent-probe listen --network "home Wi-Fi"
   ```

   It prints one line that starts with `dissent-probe dial`. Copy the whole line to device B (by chat,
   email or a note). The line holds A's addresses, so send it only to yourself.

2. On device B, paste the line. Replace the placeholder at the end with B's network name, and press
   Enter:

   ```sh
   dissent-probe dial <long address> --peer-network "home Wi-Fi" --network "phone hotspot"
   ```

   It takes about 45 seconds: connect, then 30 seconds of watching for a direct path, then 10 seconds
   of voice-rate traffic. The result is saved to `probe-results/` on device B.

3. Press Ctrl-C on device A.

Then swap roles once: listen on B and dial from A. NAT behaviour is not always symmetric.

## Which pairs to test

Each pair is a pair of networks, with one device on each. The more of these you can cover, the better.
Repeat each pair at least twice, at different times of day if you can.

| # | Device A | Device B | Why |
|---|---|---|---|
| 1 | Home broadband, ISP 1 | Home broadband, ISP 2 | The most common case: two friends at home. |
| 2 | Home broadband | Phone hotspot (mobile data) | Mobile networks often use carrier-grade NAT. |
| 3 | Phone hotspot, carrier 1 | Phone hotspot, carrier 2 | Usually the worst case: two hard NATs. |
| 4 | Home broadband | Office, school or guest Wi-Fi | Firewalls that block UDP or use hard NATs. |
| 5 | Same Wi-Fi network | Same Wi-Fi network | A control. Should be direct within a second. |
| 6 | IPv6-only network (if you have one) | Home broadband | Some mobile carriers are IPv6-only with NAT64. |
| 7 | Any of the above, far apart (other country) | | Shows which relay wins at distance. |

Run `check` on every network in the table, not only the dialling side.

## What to send back

Send the whole `probe-results/` folder from each device, zipped. Each file is small JSON. The files
hold your network labels, notes, NAT type and timings. They do not hold your IP addresses.

To look at your own results first:

```sh
dissent-probe summary            # reads ./probe-results
dissent-probe summary path/to/a/folder
```

It prints Markdown tables, one for checks and one for two-device runs, that you can paste into an
issue. To merge results from several devices, copy all their files into one folder and run `summary` on
it.

## Reading a two-device result

- **connect:** time to the first answer, and the path it used. Most first answers are relayed.
- **time to direct:** when a direct path first appeared. "never" means the call stays on the relay.
- **final path / direct %:** the path at the end, and the share of pings that went direct.
- **ping rtt:** each ping opens a new connection. One sample is the handshake plus one request, so it
  is about two network round trips.
- **media rtt / delivered / jitter:** voice-rate datagrams echoed back over one open link. This is the
  number that matters for calls. Media rtt is one round trip, so one-way delay is about half of it.
  Delivered below 98% or jitter above 20 ms means the path is poor for calls.
- **relay:** the relay in the listener's address. A relayed path goes through it.

## Testing a relay of your own

Add `--relay https://your.relay.example` to `check`, `listen` and `dial`. The relay is measured next to
number0's public ones. Add `--no-public-relays` to use only yours. Give the same relay flags to both
devices. `listen` copies them into the line it prints.

## Other options

- `check --seconds 120` watches longer. iroh re-measures every 20 to 26 seconds.
- `check --no-dht` skips the DHT check, and `check --no-tcp-rtt` skips the TCP timing.
- `--out DIR` saves somewhere other than `probe-results/`, and `--no-save` saves nothing.
- `--json` prints the result as JSON on stdout. Logs go to stderr.
- `RUST_LOG=debug` gives more detail when something fails. In PowerShell, set `$env:RUST_LOG="debug"` first.
