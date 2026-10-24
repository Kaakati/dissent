# Dissent seeder

A seeder is a Dissent member that never sleeps. It runs on a server or an always-on PC and keeps a community's history. When everyone's phone and laptop is off, a new member can still join from the seeder and receive the messages.

A seeder has no screen and no user. It joins the communities you give it, syncs their messages, and stays online.

## What it can read

A seeder joins as a member, so it can see what the members can see. Messages are encrypted for the community's group, and a seeder can read a message only from the time it joined the group. Older messages reach it as locked, which means it stores them without being able to read them. New members who join later also receive the locked history.

## Running it

Build it with `cargo build --release -p dissent-seeder`. Then run it with an invite link from a member:

```sh
dissent-seeder --data-dir /var/lib/dissent-seeder --passphrase-file /etc/dissent/seeder.pass --join <invite>
```

The seeder's identity is stored encrypted in the data directory. Its key comes from the operating system's keychain when there is one, which is the normal case on Windows and macOS. Headless Linux servers usually have no keychain, so give a passphrase instead:

- `--passphrase-file <path>`: a file whose first line is the passphrase. Keep it readable only by the account that runs the seeder.
- `DISSENT_SEEDER_PASSPHRASE`: the same passphrase, as an environment variable.

The passphrase must be at least 8 characters. Keep the data directory and the passphrase together in your backups. The `vault.salt` file in the data directory is not secret, but without it the identity cannot be opened. If you lose the passphrase, the seeder loses its identity. If neither a keychain nor a passphrase is available, the seeder exits with an error and writes nothing unencrypted.

Other useful flags:

- `--name <name>`: the name members see. Defaults to `seeder`.
- `--loopback`: binds only to this machine, with no relays and no DHT. Only for tests on one computer.
- `--status-every <seconds>`: how often the seeder logs and updates its status file. Defaults to 60.
- `--log-dir <dir>`: also writes log files there.

## Running it in the background

Install it as a background service, and give the run arguments after `--`. The arguments are saved exactly as you write them.

```sh
dissent-seeder service install --user -- --data-dir /home/me/dissent-seeder --passphrase-file /home/me/seeder.pass --join <invite>
dissent-seeder service start --user
dissent-seeder service status --user --data-dir /home/me/dissent-seeder
dissent-seeder service stop --user --data-dir /home/me/dissent-seeder
dissent-seeder service uninstall --user
```

The seeder installs as a per-user service by default, because the operating system keychain belongs to the user:

- **Windows:** a Task Scheduler task called "Dissent Seeder". It starts at logon, runs hidden, and restarts after a failure. No administrator rights are needed.
- **Linux:** a `systemd --user` unit. The service stops when you log out unless you enable lingering with `loginctl enable-linger <your user>`. The install command prints this hint.
- **macOS:** a LaunchAgent, which starts when you log in.

A machine-wide service with `--system` has no user keychain, so it needs `--passphrase-file`. On Windows it is a real Windows service and needs an administrator. On Linux it is a hardened systemd unit, and `--run-as <user>` runs it as an account other than root.

Paths in the run arguments must be absolute, because a service does not run from your shell's folder.

`service status` shows whether the service is installed and running, and when it last started. It also reads the seeder's `status.json` in the data directory, and prints the number of communities, their members, and their message counts. Logs go to `<data-dir>/logs/dissent-seeder.log`. The file rolls over at 10 MB, and rolled files are deleted after 7 days.

`service stop` asks the seeder to shut down cleanly, and gives it 5 seconds before it ends the task. On Windows the task runs in your session, so signing out stops the seeder. It starts again at your next logon.

## What it can and cannot see

A seeder can see the community's structure: its channels, its members, and the times messages were posted. It can read only the messages it was a group member for. It cannot read messages from before it joined. It holds the encrypted history for everyone, so it is only as private as the community's own membership. Anyone who can read a seeder's data directory and its passphrase can read what the seeder can read.
