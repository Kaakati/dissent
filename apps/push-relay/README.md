# Dissent push relay

Phones stop receiving network traffic when they sleep. To wake one, a peer asks this relay to send an empty push notification to the phone. The push is only a nudge. The phone then connects and fetches what it missed.

This is a prototype. Run it, read what it can see below, and decide whether that is acceptable before relying on it.

## What it can see

- The push endpoint of each registered device: a UnifiedPush URL, or an APNs device token. Either one identifies the device to the push provider.
- A random wake token per registration. A peer that knows the token can wake the device.
- The source IP address of each request, and when it arrives.
- How often each token is woken.

## What it cannot see

- Who sent a wake. Requests carry no sender identity.
- Who the device talks to, or about what. Wakes carry no content, and the phone's messages never pass through the relay.
- The contents of anything. The push is empty.

The source IP is the one exception. It shows the address of whoever sent a wake. A peer who wants to hide that from the relay needs to route through a proxy or a network they trust.

## What is stored

Wake tokens and delete secrets are stored only as hashes. A copy of the storage cannot wake a device or delete a registration. Push endpoints are stored as given, because the relay must send to them. Anyone who reads the snapshot file can push to those endpoints, so keep the file private.

## Running it

Build with `cargo build --release -p dissent-push-relay`. It listens on `127.0.0.1:8787` by default, so put a TLS proxy such as Caddy or nginx in front of it. Only the proxy should reach the relay.

```sh
dissent-push-relay --bind 127.0.0.1:8787 --snapshot /var/lib/dissent-push/registrations.json --ups-allow-host push.example.org
```

- `--ups-allow-host` lists the UnifiedPush hosts devices may register. Repeat it for more hosts. Without it, any public host is allowed.
- `--snapshot` keeps registrations across restarts. The file is written after each change.
- Private, loopback and link-local addresses are refused. `--dev` lifts that rule and allows plain HTTP. Use it only for local tests.

### APNs (not tested against Apple)

APNs needs a `.p8` key from the Apple developer portal:

```sh
dissent-push-relay --apns-key AuthKey_ABC1234567.p8 --apns-key-id ABC1234567 --apns-team-id TEAM123456 --apns-topic org.example.dissent
```

Add `--apns-sandbox` for the sandbox host. The request format is unit-tested, but nothing here has been sent to Apple yet. Background pushes must use priority 5, so this relay does not send high-priority alerts.

## API

- `POST /register` with `{"endpoint": {"kind": "unifiedpush", "url": "..."}}` or `{"endpoint": {"kind": "apns", "device_token": "..."}}`. Returns `wake_token` and `secret`. Keep the secret, because it is the only way to unregister.
- `POST /wake/<wake_token>` with an empty body. Returns `202` when the push was accepted. Returns `429` when rate-limited: 1 wake per token every 10 seconds, bursts of 5, and 2 requests per second per source address.
- `DELETE /register/<wake_token>` with `Authorization: Bearer <secret>`. Returns `204`.

## Limits

- The rate limit is per relay, so each relay limits its own traffic.
- The per-address limit counts the proxy's address if you run behind one. Configure the proxy to pass the real address on, or the limit applies to everyone at once.
- Registrations are not authenticated. Anyone who can reach the relay can register a device. The only check is that the endpoint is public.
