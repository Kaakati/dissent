# Dissent relay

A relay helps two Dissent devices talk when neither can reach the other directly, for example behind home routers or mobile networks. The relay passes along encrypted packets it cannot open. Anyone can run one. This is the open-source relay binary, `dissent-relay`.

## What you need

- A small Linux server with a public IP address (a VPS is fine), or a Windows or macOS machine that stays on.
- A DNS name pointing at it, such as `relay.example.org`. Create an `A` (and `AAAA` if you have IPv6) record first, because the TLS certificate needs it.
- The binary. Build it with `cargo build --release -p dissent-relay` and copy `target/release/dissent-relay` to `/usr/local/bin/`.

## Ports

Open these in the server firewall and in your provider's control panel.

| Port | Protocol | Why |
| --- | --- | --- |
| 443 | TCP | Relay traffic over HTTPS. Also used to issue the Let's Encrypt certificate. |
| 3340 | TCP | Plain HTTP. Serves the captive-portal probe. Optional, but clients use it to check their network. |

The relay does not use UDP, so you do not need to open any UDP ports.

## Run it in the foreground

```sh
dissent-relay --data-dir /var/lib/dissent-relay --hostname relay.example.org --acme-email you@example.org
```

On the first start it gets a certificate and caches it under `<data-dir>/acme`. It then prints the URL to use:

```text
Relay listening. Put this URL in a Dissent client: https://relay.example.org
```

To test without hitting Let's Encrypt rate limits, add `--acme-staging`. Staging certificates are not trusted by browsers, so use them only for testing.

## Run it with your own certificate

```sh
dissent-relay --data-dir /var/lib/dissent-relay --hostname relay.example.org --cert /etc/dissent-relay/fullchain.pem --key /etc/dissent-relay/privkey.pem
```

The certificate and key must be PEM files. The relay reloads them once a day, so a renewal tool can replace them in place.

## Install it as a background service

The relay installs as a machine-wide service by default. Give the same run arguments as above, after `--`. The arguments are saved exactly as you write them. Paths must be absolute.

```sh
sudo dissent-relay service install -- --data-dir /var/lib/dissent-relay --hostname relay.example.org --acme-email you@example.org
sudo dissent-relay service start
dissent-relay service status
sudo dissent-relay service stop
sudo dissent-relay service uninstall
```

- **Linux:** a systemd unit at `/etc/systemd/system/dissent-relay.service`. It is hardened: no privilege escalation, a read-only system, no access to home directories, a private `/tmp`, and writes only under its data directory. It binds port 443 without root, through a single capability, when you use a port below 1024. Add `--run-as <user>` to `install` to run it as an account that is not root. Create that account first, with no login shell. Then give that account ownership of the data directory.
- **Windows:** a Windows service, started by the Service Control Manager. Run the commands from an administrator prompt. The service restarts after a failure. Stopping it shuts the relay down cleanly.
- **macOS:** a launchd daemon in `/Library/LaunchDaemons`, run with `sudo`.

The service writes rolling log files to `<data-dir>/logs`. They roll over at 10 MB and are kept for 7 days.

The unit that `install` writes on Linux looks like this:

```ini
[Unit]
Description=Dissent relay: carries encrypted traffic between devices that cannot connect directly
After=network-online.target
Wants=network-online.target

[Service]
ExecStart=/usr/local/bin/dissent-relay --data-dir /var/lib/dissent-relay --hostname relay.example.org --acme-email you@example.org
WorkingDirectory=/var/lib/dissent-relay
Restart=on-failure
RestartSec=5
KillSignal=SIGTERM
TimeoutStopSec=10
NoNewPrivileges=true
ProtectSystem=strict
ProtectHome=true
PrivateTmp=true
ReadWritePaths=/var/lib/dissent-relay
AmbientCapabilities=CAP_NET_BIND_SERVICE
CapabilityBoundingSet=CAP_NET_BIND_SERVICE

[Install]
WantedBy=multi-user.target
```

The `AmbientCapabilities` line appears only when the relay binds a port below 1024.

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
| `--acme-cache` | `<data-dir>/acme` | Directory for the ACME account and certificates. |
| `--cert` / `--key` | none | PEM certificate chain and private key. Use together. |
| `--data-dir` | none | State directory: ACME cache and logs. Required to install a service. |
| `--log-dir` | none | Also write rolling log files here. |

Service subcommands: `service install [--user|--system] [--run-as <user>] -- <run arguments>`, `service uninstall`, `service start`, `service stop`, and `service status`. Set `RUST_LOG=debug` for more logging.
