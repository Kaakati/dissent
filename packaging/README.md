# Running the seeder and the relay as services

The seeder keeps a community's history available when its members are offline. The relay carries encrypted
traffic between devices that cannot reach each other directly. Both are meant to run all the time on a home
server, a NAS or a VPS. This folder has what you need to install them as system services.

| Platform | Seeder | Relay |
|---|---|---|
| Linux (systemd) | `systemd/dissent-seeder.service` | `systemd/dissent-relay.service` |
| macOS (launchd) | `launchd/com.dissent.seeder.plist` | `launchd/com.dissent.relay.plist` |
| Windows | `dissent-seeder.exe service install --system` | `dissent-relay.exe service install --system` |

Each binary is also a plain command-line program. Run it by hand to try it first, then install it as a service.

## Before you start

1. Get the binaries. Download the `dissent-tools-<version>-<platform>` archive from the release page. It holds
   `dissent-seeder`, `dissent-relay` and this folder. Or build them with
   `cargo build --release -p dissent-seeder -p dissent-relay`.
2. Choose the seeder's passphrase. It must be at least 8 characters. The passphrase seals the seeder's identity on
   disk. If you lose it, the seeder loses its identity. Keep it with your backups of the data directory.

## Ports

| Service | Port | Protocol | Direction | Needed for |
|---|---|---|---|---|
| Relay | 443 | TCP | inbound | encrypted relay traffic (TLS) |
| Relay | 3340 | TCP | inbound | plain HTTP captive-portal probe; optional, but clients use it to check their network |
| Seeder | none | | | it needs outbound access only |

The relay uses no UDP. The seeder binds a random UDP port on all interfaces and has no setting for it. Open inbound
UDP to it only if you want direct connections to the seeder. Without that, traffic still reaches it through relays.

## Linux (systemd)

The release binaries need Ubuntu 24.04 or newer, or another distribution of the same age (glibc 2.39). On older
systems, such as Ubuntu 22.04 or Debian 12, build from source there instead.

The units run under a dedicated account that systemd creates on start (`DynamicUser=yes`). Its state lives in
`/var/lib/dissent-seeder` or `/var/lib/dissent-relay`, which systemd keeps across updates. Each unit is hardened:
a read-only system, no home directories, a private `/tmp`, no privilege escalation, and a short list of allowed
system calls. The relay keeps one capability, `CAP_NET_BIND_SERVICE`, so it can bind port 443 without root. The
seeder keeps none.

### Install the seeder

```sh
sudo install -m 0755 dissent-seeder /usr/local/bin/dissent-seeder
sudo install -m 0644 systemd/dissent-seeder.service /etc/systemd/system/
sudo install -d -m 0700 /etc/dissent
sudo sh -c 'umask 077; printf "%s\n" "your passphrase here" > /etc/dissent/seeder.passphrase'
sudo install -m 0644 systemd/seeder.env.example /etc/dissent/seeder.env
sudo nano /etc/dissent/seeder.env              # set DISSENT_SEEDER_JOIN to your invite link
sudo systemctl daemon-reload
sudo systemctl enable --now dissent-seeder
```

The passphrase file holds the passphrase on its only line. systemd reads it as a credential, so only this service
can read it. `/etc/dissent/seeder.env` is optional. It sets the invite link and the other settings.

### Point the seeder at a server

Ask a member of the server for an invite link: Server settings, then Invites. Put the link in
`/etc/dissent/seeder.env`:

```
DISSENT_SEEDER_JOIN=<invite link>
```

Several links go on one line, separated by commas. Restart the service to apply the change:
`sudo systemctl restart dissent-seeder`.

### Install the relay

The relay needs exactly one TLS mode. For a public server with a DNS name, it gets a certificate from Let's
Encrypt. Point the DNS name at the server first, then:

```sh
sudo install -m 0755 dissent-relay /usr/local/bin/dissent-relay
sudo install -m 0644 systemd/dissent-relay.service /etc/systemd/system/
sudo install -d -m 0755 /etc/dissent
sudo install -m 0644 systemd/relay.env.example /etc/dissent/relay.env
sudo nano /etc/dissent/relay.env               # set the hostname and the email
sudo systemctl daemon-reload
sudo systemctl enable --now dissent-relay
```

With your own certificate instead, set `DISSENT_RELAY_CERT` and `DISSENT_RELAY_KEY` in `relay.env`, and remove the
ACME lines. For a test on one machine, set `DISSENT_RELAY_DEV=true`. Only one mode can be set at a time.

### Check it

```sh
systemctl status dissent-seeder          # or dissent-relay
journalctl -u dissent-seeder -f          # live log
```

The seeder logs `network status` on its first status line, which means it has started. The relay logs the address
it listens on.

### Update

Replace the binary and restart. The data directory and the passphrase are kept:

```sh
sudo install -m 0755 dissent-seeder /usr/local/bin/dissent-seeder
sudo systemctl restart dissent-seeder
```

### Stop and uninstall

```sh
sudo systemctl disable --now dissent-seeder
sudo rm /etc/systemd/system/dissent-seeder.service /usr/local/bin/dissent-seeder
sudo systemctl daemon-reload
```

To remove the state as well, delete the data directory. This deletes the seeder's identity for good. systemd keeps
the state under `/var/lib/private/`, and `/var/lib/dissent-seeder` is a link to it:
`sudo rm -rf /var/lib/dissent-seeder /var/lib/private/dissent-seeder`. Do the same with `dissent-relay` for the relay.

Use one install method per machine. `dissent-seeder service install --system` writes a unit with the same name on
Linux, so it would overwrite these files.

## macOS (launchd)

The plists run the binaries as a hidden account, `_dissent`, which you create once. The seeder keeps its identity
in its data directory rather than in a keychain, because a system daemon has no user keychain.

```sh
# Create the group and the account once. The account has no login and no home directory.
# 401 is an example ID. Pick one that nothing else on the machine uses.
sudo dscl . -create /Groups/_dissent
sudo dscl . -create /Groups/_dissent PrimaryGroupID 401
sudo dscl . -create /Users/_dissent
sudo dscl . -create /Users/_dissent UniqueID 401
sudo dscl . -create /Users/_dissent PrimaryGroupID 401
sudo dscl . -create /Users/_dissent UserShell /usr/bin/false
sudo dscl . -create /Users/_dissent NFSHomeDirectory /var/empty
```

Then, for the seeder:

```sh
sudo install -m 0755 dissent-seeder /usr/local/bin/dissent-seeder
sudo install -d -o _dissent -g _dissent /var/db/dissent-seeder
sudo install -d -m 0700 /etc/dissent
sudo sh -c 'umask 077; printf "%s\n" "your passphrase here" > /etc/dissent/seeder.passphrase'
sudo install -m 0644 launchd/com.dissent.seeder.plist /Library/LaunchDaemons/
sudo launchctl bootstrap system /Library/LaunchDaemons/com.dissent.seeder.plist
```

Before you bootstrap, edit the plist to add the invite link: an `EnvironmentVariables` dictionary with
`DISSENT_SEEDER_JOIN` set to the link.

The relay is the same, with `/var/db/dissent-relay`, `com.dissent.relay.plist`, and its hostname and email set in
`ProgramArguments` before you bootstrap.

Check with `sudo launchctl print system/com.dissent.seeder`. Logs go to `/var/log/dissent-seeder.log`.

To update, replace the binary, then run `sudo launchctl kickstart -k system/com.dissent.seeder`. To stop and
uninstall:

```sh
sudo launchctl bootout system/com.dissent.seeder
sudo rm /Library/LaunchDaemons/com.dissent.seeder.plist /usr/local/bin/dissent-seeder
```

## Windows

Both programs install as Windows services with their own command. This needs an administrator. It uses the
service support built into the programs, so there is nothing extra to install. Paths must be absolute, because a
service does not run from your shell's folder.

```bat
dissent-seeder.exe service install --system -- --data-dir "C:\ProgramData\Dissent\seeder" --passphrase-file "C:\ProgramData\Dissent\seeder.pass" --join <invite>
dissent-seeder.exe service start --system
dissent-seeder.exe service status --system --data-dir "C:\ProgramData\Dissent\seeder"
dissent-seeder.exe service stop --system
dissent-seeder.exe service uninstall --system
```

Write the passphrase into `C:\ProgramData\Dissent\seeder.pass` on its only line, and restrict that file to
Administrators and SYSTEM. The arguments after `--` are saved exactly as written, and they are what the service runs.

The relay works the same way:

```bat
dissent-relay.exe service install --system -- --data-dir "C:\ProgramData\Dissent\relay" --hostname relay.example.org --acme-email you@example.org
```

Stopping the service asks the program to shut down cleanly, as the Linux and macOS services do. Logs go to the
`--log-dir` you give, or to the `logs` folder inside the data directory.

## Notes

- Both programs shut down cleanly on SIGTERM (Linux and macOS) and on a Windows service stop. The seeder sends its
  offline notice and saves its state first. The relay stops accepting connections and finishes what it is doing.
  The systemd and launchd units allow 20 seconds before they force a stop.
- Configuration can also come from environment variables. Each command-line flag has one, named `DISSENT_SEEDER_*`
  or `DISSENT_RELAY_*`. The example files list them.
- A seeder with no passphrase and no keychain stops with an error, and writes nothing unencrypted.
