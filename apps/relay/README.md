# Dissent relay

A relay helps two Dissent devices talk when neither can reach the other directly, for example behind home routers or mobile networks. The relay passes along encrypted packets it cannot open. Anyone can run one. This is the open-source relay binary, `dissent-relay`.

## What you need

- A small Linux server with a public IP address (a VPS is fine).
- A DNS name pointing at it, such as `relay.example.org`. Create an `A` (and `AAAA` if you have IPv6) record first, because the TLS certificate needs it.
- The binary. Build it with `cargo build --release -p dissent-relay` and copy `target/release/dissent-relay` to `/usr/local/bin/`.

## Ports

Open these in the server firewall and in your provider's control panel.

| Port | Protocol | Why |
| --- | --- | --- |
| 443 | TCP | Relay traffic over HTTPS. Also used to issue the Let's Encrypt certificate. |
| 3340 | TCP | Plain HTTP. Serves the captive-portal probe. Optional, but clients use it to check their network. |

The relay does not use UDP, so you do not need to open any UDP ports.

## Run it with Let's Encrypt

```sh
dissent-relay --hostname relay.example.org --acme-email you@example.org --acme-cache /var/lib/dissent-relay/acme
```

On the first start it gets a certificate and caches it under the cache directory. It then prints the URL to use:

```text
Relay listening. Put this URL in a Dissent client: https://relay.example.org
```

To test without hitting Let's Encrypt rate limits, add `--acme-staging`. Staging certificates are not trusted by browsers, so use them only for testing.

## Run it with your own certificate

```sh
dissent-relay --hostname relay.example.org --cert /etc/dissent-relay/fullchain.pem --key /etc/dissent-relay/privkey.pem
```

The certificate and key must be PEM files. The relay reloads them once a day, so a renewal tool can replace them in place.

## Systemd unit

Save this as `/etc/systemd/system/dissent-relay.service`:

```ini
[Unit]
Description=Dissent relay
After=network-online.target
Wants=network-online.target

[Service]
ExecStart=/usr/local/bin/dissent-relay --hostname relay.example.org --acme-email you@example.org --acme-cache /var/lib/dissent-relay/acme
User=dissent-relay
Group=dissent-relay
StateDirectory=dissent-relay
WorkingDirectory=/var/lib/dissent-relay
# Lets a non-root user bind port 443.
AmbientCapabilities=CAP_NET_BIND_SERVICE
NoNewPrivileges=true
Restart=on-failure
RestartSec=5

[Install]
WantedBy=multi-user.target
```

Then create the user and start the service:

```sh
sudo useradd --system --home /var/lib/dissent-relay --shell /usr/sbin/nologin dissent-relay
sudo systemctl daemon-reload
sudo systemctl enable --now dissent-relay
sudo journalctl -u dissent-relay -f
```

## Pointing a Dissent client at your relay

The relay URL is the address you printed, for example `https://relay.example.org`. The Dissent engine takes it as `RelayChoice::Custom`, and that is the only way to set it today. The desktop app does not expose a relay setting yet, so it uses iroh's default relays unless it runs in loopback mode. Community records do not advertise relays yet either, so a relay helps only clients you configure to use it.

## Local testing

`--dev` serves plain HTTP with no TLS on `--bind`, which defaults to `0.0.0.0:3340`. Use it on one machine for testing only:

```sh
dissent-relay --dev --bind 127.0.0.1:3340
```

Clients then use `http://127.0.0.1:3340`.

## Flags

| Flag | Default | Meaning |
| --- | --- | --- |
| `--bind` | `0.0.0.0:3340` | HTTP address. With TLS, it serves only the captive-portal probe. |
| `--https-bind` | `0.0.0.0:443` | HTTPS address for relay traffic. TLS modes only. |
| `--dev` | off | Plain HTTP, no TLS. Local testing only. |
| `--hostname` | none | Public DNS name. Required with `--acme-email`. Used in the printed URL. |
| `--acme-email` | none | Let's Encrypt account email. Needs `--hostname`. |
| `--acme-staging` | off | Use Let's Encrypt staging. |
| `--acme-cache` | `acme-cache` | Directory for the ACME account and certificates. |
| `--cert` / `--key` | none | PEM certificate chain and private key. Use together. |

Set `RUST_LOG=debug` for more logging.
