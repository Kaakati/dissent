<p align="center">
  <img src="assets/icon/app_icon.png" alt="Dissent logo" width="140">
</p>

<h1 align="center">Dissent</h1>

<p align="center"><strong>A place to talk with your communities that no company owns and nobody can switch off.</strong></p>

Dissent is a free, open-source chat app in the spirit of Discord. You can create servers, organise them into channels, invite friends, chat in real time and send private direct messages. The difference is underneath. Dissent has no central company server holding everyone's conversations. Your servers live on the computers of the people who use them, and the app connects those people to each other directly.

> **Status: early but usable (v0.1.8).** Servers, encrypted chat, direct messages, voice and video calls and screen sharing all work today between people who have Dissent installed. Dissent has not had a security review, so please don't rely on it yet for anything sensitive. See [Honest limits](#honest-limits).

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

### Servers

- **Create and join servers** from invite links, and switch between them from the side rail. Drag servers to reorder them; the order is yours alone.
- **Invite cards.** An invite link pasted in chat shows a card with the server's banner, icon, name, member and online counts, and a Join button. Short invite links fit in a message, and a full link is offered for older versions.
- **Server Profile.** Owners set the name, icon, a banner (a preset colour, an image or a looping GIF), up to five traits and a description, with a live preview. Admins can change everything except the name. The banner shows at the top of the channel list for every member.
- **Text and voice channels**, with slow mode and per-channel settings. Unread channels are marked, and a "New messages" line shows where you left off.
- **Encrypted servers.** Server messages are sealed so that only members can read them. A banned or removed member can't read anything new. Owners can switch on member-only mode, which also hides settings and moderation from people who have left.
- **Message history for new members**, following the server's history setting: new members either see only what was sent after they joined (the default for new servers) or the whole history.
- **Moderation:** owner, admin and moderator roles, banning and unbanning, and an audit trail every member's app checks for itself.
- **Right-click a server** to mark it read, invite people, mute it for a while or for good, choose its notification level (all messages, mentions only or nothing), hide muted channels, open its settings, create a channel or leave.

### Messages

- **Mentions.** Type `@` to pick a member, filtered as you type. Owners and admins can use `@everyone`. A message that mentions you is highlighted, and channels and servers show red mention counts until you read them.
- **Formatting.** Discord-style markdown: bold, italic, underline, strikethrough, headers, lists, quotes, spoilers, links with your own text, and code blocks with syntax highlighting. Ctrl+B, Ctrl+I and Ctrl+U format the selected text.
- **Right-click a message** to react, reply, edit, forward, copy, pin, bookmark it to Saved Messages, set a reminder, mark it unread, copy a link to it, have it read aloud, or delete it.
- **Replies and reactions**, with quick reactions from your most-used emoji.
- **Files of up to 10 GB**, encrypted, downloaded from several members at once, and resumed after a restart. Pictures get a small preview. Downloads go to your Downloads folder unless you choose another.
- **Link previews** with the page's title, description and picture. They're fetched by the sender, after asking once, so the people reading never contact the website.

### People and direct messages

- **Friends.** Add people by their address (`dsu1…`) or a contact link, accept or decline requests, and block people. Friends who are in a call or online show in Active Now.
- **Profiles** with your avatar, a banner and a status (online, idle, do not disturb or invisible). Profile cards show mutual servers and, if you allow it, mutual friends.
- **Direct and group messages**, encrypted, and delivered to every device of both people. Messages to someone offline are held, or left at their inbox, and sent when they're back. A message an inbox refuses shows as failed, with a Retry.
- **Unread conversations** appear on the side rail with a count, like Discord.

### Calls

- **Voice calls** one to one, in groups, and in voice channels. Small calls connect everyone directly. Bigger ones elect the strongest computer to forward everyone's sealed streams, and a backup takes over if it drops.
- **Video and screen sharing** during a call, with a camera preview before you join. Audio always keeps its share of the connection, so video can't crowd it out.
- **Clean audio:** noise suppression, echo cancellation, automatic gain, push-to-talk, and a mic test.
- Calls are encrypted end to end, and forwarders can't open what they pass along.

### You and your devices

- **Your identity** is backed up with a 24-word recovery phrase, and its keys are kept in your system's keychain.
- **Several devices.** Link a laptop or a USB stick to the same identity, and cut off a lost one.
- **Recovery with friends.** Choose guardians, and if you lose your recovery phrase, enough of them can vouch for your new identity.
- **Safety numbers** to compare with a friend, with a warning if their key changes.
- **Quick switcher:** press Ctrl+K to jump anywhere. The app is fully usable from the keyboard.
- **Network:** use the public relays or your own, and see live network health.
- **Updates** are offered in the app and installed with one click. Every update is checked against Dissent's signing key.

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

## Built underneath, not in the app yet

- **Private channels.** Channels only chosen members and roles can see, each with its own key. The rules are built and tested, but the keys, the Private Channel switch and the member picker aren't finished, so the app doesn't offer them yet.
- **AV1 video in the installers.** Calls can use AV1 when every computer in the call supports it, but the installers ship without the AV1 encoder for now and send H.264.
- **Website-verified handles:** if you own a website, prove a handle like `@example.com`. Every viewer's app checks it for itself.
- **Push relay** for waking phones, as a prototype.

## Things anyone can run

Dissent works with no servers at all, but these optional helpers make it better, and anyone can run them:

- **Seeder.** An always-on member that keeps a server's history available, even when everyone else is offline.
- **Relay.** A helper that passes sealed traffic between people who can't connect directly. Running one near your community, for example in your country, makes relayed connections faster. A guide is in [`apps/relay/README.md`](apps/relay/README.md).
- **Probe.** A small tool that two people on different networks can run to see whether they connect directly or through a relay, and how fast. See [`apps/probe/README.md`](apps/probe/README.md).
- **Push relay.** A prototype that wakes sleeping phones with an empty notification, without ever learning who is talking to whom. It's in progress.

## Honest limits

Dissent is young, and we would rather tell you what it doesn't do yet than have you find out the hard way.

- **No security audit has been done.** The design notes in [`docs/design/06-threat-model.md`](docs/design/06-threat-model.md) list what each part protects against and what it doesn't.
- **History needs someone online.** You can only catch up on a server when at least one other member, or a seeder, is online.
- **New members can't read messages from before 0.1.7**, even when the server shares its full history. Those were sealed before history keys existed.
- **Older versions miss newer features.** Someone on an older Dissent sees plain `@names` instead of highlighted mentions, no banners, and server changes such as a rename show for them only once they update.
- **A sudden power cut can lose your last quarter-second of sent messages.** Other members usually already have them, so they come back when you sync.
- **Mutual friends shares your whole friends list with your friends**, not just the people you have in common. You can turn this off in Settings › Privacy.
- **Screen sharing doesn't work on Linux yet**, and the Linux camera isn't supported either.
- **No mobile app yet.**
- **Most testing has happened on one machine and over the internet between a few copies of the app.** Real-world measurements across many networks and mobile carriers are still to come.

## What's next

1. Private channels in the app.
2. AV1 video in the installers, and screen sharing and camera on Linux.
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
Nobody holds a copy, which is exactly what keeps your identity yours. If you chose guardians in Settings › Recovery, enough of them can vouch for a new identity, and your roles and handles in every server carry over.

**Why does Windows ask to allow Dissent through the firewall?**
Dissent talks directly to other people's computers, so it needs permission to accept connections. Choosing "Allow" lets friends reach you directly instead of always going through a relay.

**Is it really free?**
Yes. It's open source, so anyone can read the code, check what it does, and improve it.

## For developers

Dissent is written in Rust, with a desktop app built on Tauri 2 and a React interface (`apps/desktop/ui-next`). If you want to build it or contribute:

- [`docs/Decentralized Discord Alternative — Architecture Spec.md`](docs/Decentralized%20Discord%20Alternative%20%E2%80%94%20Architecture%20Spec.md) covers the overall design and roadmap.
- [`docs/design/`](docs/design/) holds the design notes: servers and DMs, recovery and safety, calls, encrypted servers, the threat model, the visual system, files, audio, DM delivery and private channels.
- [`CLAUDE.md`](CLAUDE.md) explains the project layout, coding conventions and how to run the tests.

You need Rust and Node 24. To run the desktop app from source, against the React dev server:

```sh
cargo install tauri-cli --version "^2"
npm --prefix apps/desktop/ui-next ci
scripts/dev-next.sh
```

To try the interface alone in a browser, with a built-in mock backend: `npm --prefix apps/desktop/ui-next run dev`.

To run the tests:

```sh
cargo test --workspace                      # Rust unit and integration tests
scripts/fwtest.sh dissent-core it --ignored # networked tests (loopback)
npm --prefix apps/desktop/ui-next run test:ui   # the interface, in a headless browser
```

AV1 video is behind the `svt-av1` feature of `dissent-video`, which builds SVT-AV1 from the `third_party/SVT-AV1` submodule and needs NASM and libclang. Without it, video uses H.264.

## Contributing

Ideas, bug reports and code are all welcome. Please open an issue to talk about larger changes before starting on them. If you find a security problem, please report it privately rather than in a public issue.

## License

Dissent is dual-licensed under the [MIT License](LICENSE-MIT) and the [Apache License 2.0](LICENSE-APACHE), at your choice. The interface icons come from [Lucide](https://lucide.dev) and are used under the ISC License. The Poppins typeface is bundled under the SIL Open Font License in `apps/desktop/ui-next/public/fonts`. Voice calls use the [Opus](https://opus-codec.org) codec, included under its BSD license in `crates/dissent-audio/vendor/libopus`.
