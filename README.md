<p align="center">
  <img src="assets/icon/app_icon.png" alt="Dissent logo" width="140">
</p>

<h1 align="center">Dissent</h1>

<p align="center"><strong>A place to talk with your communities that no company owns and nobody can switch off.</strong></p>

Dissent is a free, open-source chat app in the spirit of Discord. You can create servers, organise them into channels, invite friends, chat in real time and send private direct messages. The difference is underneath. Dissent has no central company server holding everyone's conversations. Your servers live on the computers of the people who use them, and the app connects those people to each other directly.

> **Status: early prototype.** Text chat works today between people who have Dissent installed. Much more is built underneath and is being brought into the app step by step. Dissent has not had a security review, so please don't rely on it yet for anything sensitive. See [Honest limits](#honest-limits).

---

## Why Dissent exists

Most chat apps work the same way. You and your friends send messages to a company's computers, and the company passes them along. That's convenient, but it also means:

- The company can read, sell, lose or delete your conversations.
- The company decides the rules, and can ban you or your whole community.
- If the company shuts down, changes its terms or gets blocked in your country, your community disappears with it.
- Your account belongs to the company, not to you.

Dissent is built on a simple idea: **the people in a community should be the ones who keep it running.** No one has to rent a server, and no one is in charge of the whole network. If one person goes offline, everyone else keeps talking.

## How it works, in plain words

### You are your key, not an account

When you first open Dissent, it creates a unique digital identity for you on your own device. There is no sign-up form, email or phone number, and no company keeps a list of users.

Your identity comes with a **recovery phrase** of 24 ordinary words. Write them down and keep them somewhere safe. With them, you can restore yourself on a new computer.

### Names are for people, keys are for trust

Everyone picks a display name, and names can repeat. So next to each name, Dissent shows a short tag made from that person's identity, like `maya·k3f9q2xa`. Inside each server you can also claim a handle like `@maya`. The first person to claim it in that server gets it.

Handles are protected against look-alikes. Someone can't take `@paypal` by swapping in a Cyrillic letter that looks the same, and hidden or direction-flipping characters are refused.

### Servers belong to the people in them

When you create a server, you become its **owner**. The server's history isn't stored in a data centre. Each member's app keeps a copy, and when members come online, their apps quietly compare notes and fill in anything they missed. They exchange only what's missing, so catching up stays quick even for big servers.

The rules travel with the server. Who is an admin, who is a moderator and who is banned are all recorded as signed entries in the server's history. Every member's app checks those entries itself, so nobody can fake a ban or pretend to be a moderator. Even when two people act at the same moment, for example a ban and the banned person's last message, every app reaches exactly the same result.

### Invites are links you share however you like

To bring someone in, you create an **invite link** and send it any way you want: in a text, an email or a QR code. The link proves the server really is *yours*, not an imposter.

### Finding each other without a central directory

Dissent uses a giant public "phone book" that already exists. It is shared by millions of computers worldwide and run by nobody in particular, so no one can take it down. Your app looks up where to reach a server or a friend, then connects to them directly. Only connection details are published there, never your home IP address.

When two people can't connect directly, often because of home routers or mobile networks, their apps hand messages to a **relay**. A relay is a helper computer that passes along sealed envelopes it can't open. Anyone can run one, and you can choose your own relays in the app's settings.

### Private conversations, directly

**Direct messages** go straight from your device to the other person's device over an encrypted connection. If they're offline, your app holds the message and keeps trying until it gets through. You can message anyone you share a server with. You can also swap **contact links** with someone you've never shared a server with.

## What you can do in the app today

- **Servers:** create servers, join as many as you like from invite links, and switch between them from the side rail. Unread servers and channels are marked, and a "New messages" line shows where you left off.
- **Channels and messages:** create channels, and send, edit and delete messages. Messages are grouped by person and day, and links are clickable.
- **Moderation:** use roles (owner, admins and moderators), with banning and unbanning.
- **People:** see member lists grouped by role. Click anyone to see their profile card, the handles they hold and the servers you share.
- **Direct messages and contacts:** messages to offline people are sent automatically later, and contact links let you reach someone without a shared server.
- **Quick switcher:** press Ctrl+K to jump to any server, channel or conversation. The app is fully usable from the keyboard.
- **Your identity:** back up and restore it with a recovery phrase. Each identity keeps its own separate data.
- **Network:** use the public relays or your own, and see live network health in the corner of the window. That shows how big the public network is, whether you're reachable, and how many people you're connected to.

## Portable version

Dissent also comes as a portable zip for Windows. You don't install it. Unzip the folder anywhere, for example on a USB stick, and run `Dissent.exe` from inside it.

The first time you run it, Dissent asks how you want to use it here. Each choice is saved in a small file called `portable-choice` next to `Dissent.exe`, so it only asks once:

- **Use this computer's Dissent profile.** Opens the profile the installed Dissent already uses on this computer. Nothing is copied, and the installed app and this copy can't run at the same time.
- **Link as a new device (recommended for a USB stick).** Gives this stick its own device key. Open the installed Dissent, go to Settings, then Devices, paste the code shown here under "Approve a new device", and paste the grant it gives you back. You can remove this device from the Devices list at any time.
- **Copy my profile into this portable folder.** Makes a copy of the installed profile, protected by a passphrase. Both copies then share one device identity, so linking is the safer choice for a stick. Close the installed Dissent before you copy.
- **Start fresh / restore from recovery phrase.** Starts a new identity protected by a passphrase, or restores one from your recovery phrase.

Whichever you choose:

- **Your data stays in the folder.** Dissent keeps its data in a `DissentData` folder next to `Dissent.exe`, so the whole folder is self-contained. To move Dissent to another computer, copy the whole folder. The "installed profile" option keeps its data in the installed app's folder instead.
- **A passphrase protects it.** Options that keep data in `DissentData` ask for a passphrase every time Dissent starts. Keep it safe: without it, nobody can open that profile, and nobody can recover it for you.
- **Only one copy runs at a time.** If Dissent is already open with the same profile, a second copy shows a message and closes.
- **Updates are manual.** When a new version is out, Dissent tells you. Download the new portable zip from the release page and unpack it over the old folder. Your data stays in `DissentData`.

## Built, and coming to the app next

These pieces are built and tested underneath, and are being connected to the app:

- **Encrypted servers:** messages that only current members can read. When someone is banned, they lose the ability to read new messages. This is being wired in now.
- **Voice and video calls**, encrypted end to end:
  - one-to-one calls
  - small group calls, where everyone sends to everyone
  - larger calls, where the strongest computer in the call forwards everyone's sealed streams without being able to open them, and a ready backup takes over within moments if it drops
  - calls ride the same connections as chat, so they work through home routers without extra servers
- **Recovery with friends:** choose a few trusted people as guardians. If you lose your recovery phrase, enough of them can vouch for your new identity, and your roles and handles in every server carry over.
- **Several devices:** link your phone and laptop to the same identity, and cut off a lost device.
- **Safety numbers:** a number you and a friend can compare to be sure nobody is impersonating either of you.
- **Replies and reactions** to messages.
- **Website-verified handles:** if you own a website, prove a handle like `@example.com`. Every viewer's app checks it for itself.

## Things anyone can run

Dissent works with no servers at all, but these optional helpers make it better, and anyone can run them:

- **Seeder.** An always-on member that keeps a server's history available, even when everyone else is offline.
- **Relay.** A helper that passes sealed traffic between people who can't connect directly. Running one near your community, for example in your country, makes relayed connections faster. A guide is in [`apps/relay/README.md`](apps/relay/README.md).
- **Probe.** A small tool that two people on different networks can run to see whether they connect directly or through a relay, and how fast. See [`apps/probe/README.md`](apps/probe/README.md).
- **Push relay.** A prototype that wakes sleeping phones with an empty notification, without ever learning who is talking to whom. It's in progress.

## Honest limits

Dissent is young, and we would rather tell you what it doesn't do yet than have you find out the hard way.

- **Server messages aren't end-to-end encrypted in the app yet.** Connections between devices are encrypted, so outsiders and relays can't read along. The extra layer that keeps messages readable only to current members is built and being connected now.
- **Your identity file on disk isn't password-protected yet.** Anyone with access to your computer account could copy it.
- **History needs someone online.** You can only catch up on a server when at least one other member, or a seeder, is online.
- **Calls, guardians, multiple devices and safety numbers aren't in the app yet.** They're built underneath and listed above.
- **No mobile app yet.**
- **Most testing has happened on one machine and over the internet between two copies of the app.** Real-world measurements across many networks and mobile carriers are still to come.
- **No security audit has been done.**

## What's next

1. Bring encrypted servers, calls, guardians, multiple devices and safety numbers into the app.
2. Protect your identity file with your system's keychain or a passphrase.
3. Measure direct connections across real home and mobile networks, and run relays where they help most.
4. Phone notifications through push relays, and a mobile app.
5. A security review.

## Questions people ask

**Is there a company behind this? Who runs the servers?**
No company runs any servers. Every part of the system, including relays, seeders and push relays, can be run by anyone, and none of them is required.

**What happens if the owner of a server disappears?**
The server keeps working for everyone else, because every member's app holds a copy. Admins they appointed can keep moderating.

**Can someone delete my community?**
No single party can. There's no central switch to flip.

**Can moderators be abused?**
A moderator can only do what their role allows, and every member's app checks this. An admin can't remove the owner.

**I lost my recovery phrase. Can you help?**
Nobody holds a copy, which is exactly what keeps your identity yours. Guardian recovery, described above, will let a few trusted friends help you move to a new identity without losing your place in your servers.

**Why does Windows ask to allow Dissent through the firewall?**
Dissent talks directly to other people's computers, so it needs permission to accept connections. Choosing "Allow" lets friends reach you directly instead of always going through a relay.

**Is it really free?**
Yes. It's open source, so anyone can read the code, check what it does, and improve it.

## For developers

Dissent is written in Rust, with a desktop app built on Tauri. If you want to build it or contribute:

- [`docs/Decentralized Discord Alternative — Architecture Spec.md`](docs/Decentralized%20Discord%20Alternative%20%E2%80%94%20Architecture%20Spec.md) covers the overall design and roadmap.
- [`docs/design/`](docs/design/) holds the design notes for servers and DMs, recovery and safety, and calls.
- [`CLAUDE.md`](CLAUDE.md) explains the project layout, coding conventions and how to run the tests.

To run the desktop app from source:

```sh
cargo install tauri-cli --version "^2"
cd apps/desktop/src-tauri
cargo tauri dev
```

To run the tests:

```sh
cargo test --workspace
```

## Contributing

Ideas, bug reports and code are all welcome. Please open an issue to talk about larger changes before starting on them. If you find a security problem, please report it privately rather than in a public issue.

## License

Dissent is dual-licensed under the [MIT License](LICENSE-MIT) and the [Apache License 2.0](LICENSE-APACHE), at your choice. The interface icons come from [Lucide](https://lucide.dev) and are used under the ISC License. The Poppins typeface is bundled under the SIL Open Font License in `apps/desktop/ui-next/public/fonts`. Voice calls use the [Opus](https://opus-codec.org) codec, included under its BSD license in `crates/dissent-audio/vendor/libopus`.
