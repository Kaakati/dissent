# Dissent probe

The probe measures how two devices connect. It answers one question: when two people on different networks talk, do their messages go straight between them, or do they go through a relay in the middle, and how fast is that?

## What it measures

A run has three steps.

1. **Connect.** How long from start to the first successful reply, and which path that reply used.
2. **Upgrade.** For 30 seconds, it checks the path every half second. Devices often start through a relay and then find a direct path. The result shows when that happened, or "never".
3. **Media.** It streams 50 small packets per second for 10 seconds, the rate a voice call uses. It reports how many came back, how long each took, and how much the delay changed from one packet to the next.

Two terms matter for reading the result.

- **Direct** means the devices talk straight to each other. This is the fastest path, with no middleman.
- **Relayed** means traffic goes through a relay server. A relay is needed when a router or mobile network blocks direct contact. It adds latency, so a relay close to both people is faster. A **mixed** path has both a direct route and a relay.

## Running it on two networks

You need two computers or phones, each on a different network. For example, one is at home on Wi-Fi and the other is on a phone's hotspot. Both need the probe binary. Build it with `cargo build --release -p dissent-probe`.

1. On the first device, start the listener:

   ```sh
   dissent-probe listen
   ```

   It prints an address that starts with letters and digits. Copy the whole line.

2. On the second device, dial that address:

   ```sh
   dissent-probe dial <paste the address here>
   ```

   To collect results from many runs, add `--json`. It prints one JSON object, and the schema is in `apps/probe/src/lib.rs`. Add `--duration 10` to shorten the upgrade step.

3. Stop the listener with Ctrl-C when you are done.

Both sides use iroh's public relays and the public DHT by default. The listener must keep running until the dial finishes.

### Other relay options

- `--relay https://your-relay.example.org` adds your own relay. You can repeat the flag. Public relays stay on.
- `--no-public-relays` drops the public relays. Use it with at least one `--relay`, or the probe refuses to start.
- `--loopback` binds only to this machine, with no relays and no DHT. Use it only to test on one computer.

Give the same relay options to both sides if you want to compare them.

## Reading the summary

```text
connect           812 ms, path relayed, rtt 96 ms
time to direct    4120 ms (over 30 s, 60 samples)
media path        relayed at start, direct at end
media delivered   497/500 (99.4%), round trip
media rtt         median 41.2 ms, p95 88.0 ms
media jitter      3.1 ms
```

- **Time to direct** is the moment the first direct sample appeared. "never" means the two networks never found a direct route. That is common when one side is behind a strict NAT, and the relay carries the call.
- **Media delivered** is the share of packets that came back. The probe sends each packet to the other side, which sends it back. A loss of a few percent is normal on mobile. Much worse than that means the path is unreliable for calls.
- **Media rtt** is the full round trip, so one-way delay is about half.
- **Jitter** is how much the delay changes from packet to packet. Calls get choppy when it grows.

## Limits

- The listener logs when a media link opens and closes. It cannot log each ping, because the networking layer answers pings itself.
- The media numbers are round trips through an echo, not one-way measurements.
- A single run is one sample. Repeat it at different times and places before drawing conclusions.
