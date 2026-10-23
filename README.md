# Dissent

**A place to talk with your communities that no company owns and nobody can switch off.**

Dissent is a free, open-source chat app in the spirit of Discord. You can create servers, organise them into channels, invite friends, chat in real time and send private direct messages. The difference is underneath. Dissent has no central company server holding everyone's conversations. Your servers live on the computers of the people who use them, and the app connects those people to each other directly.

> **Status: early prototype.** Dissent works today for text chat between people who have it installed, but it is not finished and has not had a security review. Please don't rely on it yet for anything sensitive. See [What works today](#what-works-today) and [Honest limits](#honest-limits).

---

## Why Dissent exists

Most chat apps work the same way. You and your friends send messages to a company's computers, and the company passes them along. That's convenient, but it also means:

- The company can read, sell, lose or delete your conversations.
- The company decides the rules, and can ban you or your whole community.
- If the company shuts down, changes its terms or gets blocked in your country, your community disappears with it.
- Your account belongs to the company, not to you.

Dissent is built on a simple idea: **the people in a community should be the ones who keep it running.** No one has to rent a server, and no one is in charge of the whole network. If one person goes offline, everyone else keeps talking.

## The big ideas, in plain words

### You are your key, not an account

When you first open Dissent, it creates a unique digital identity for you on your own device. There is no sign-up form, email or phone number, and no company keeps a list of users.

Your identity comes with a **recovery phrase** of 24 ordinary words. Those words *are* your identity. Write them down and keep them somewhere safe. With them you can restore yourself on a new computer. Without them, nobody can recover your identity for you, because there is no company to call.

### Names are for people, keys are for trust

Everyone picks a display name, and names can repeat. So next to each name, Dissent shows a short tag made from that person's identity, like `maya·k3f9q2xa`. Two people can both be called "Maya", but they can't both have the same tag. Inside each server you can also claim a handle like `@maya`. The first person to claim a handle in that server gets it.

### Servers belong to the people in them

When you create a server, you become its **owner**. The server's history isn't stored in some data centre. Each member's app keeps a copy, and when members come online, their apps quietly compare notes and fill in anything they missed.

The rules travel with the server. Who is an admin, who is a moderator and who is banned are all recorded as signed entries in the server's history. Every member's app checks those entries itself. Nobody can fake a ban, or pretend to be a moderator, because a signature can't be forged.

### Invites are links you share however you like

To bring someone in, you create an **invite link** and send it any way you want: in a text, an email or a QR code. The link contains what their app needs to find the server and to confirm that it really is *your* server, not an imposter.

### Finding each other without a central directory

How do two computers on opposite sides of the world find each other without a company in the middle? Dissent uses a giant public "phone book" that already exists. It is shared by millions of computers worldwide and run by nobody in particular, so no one can take it down. Your app looks up where to reach a server or a friend, then connects to them directly.

When two people can't connect directly, often because of home routers or mobile networks, their apps can hand messages to a **relay**. A relay is a helper computer that passes along sealed envelopes it can't open. Anyone can run one, and the app works with any relay you choose.

### Private conversations, directly

**Direct messages** go straight from your device to the other person's device over an encrypted connection. If they're offline, your app holds the message and keeps trying until it gets through. You can message anyone you share a server with. You can also swap **contact links** with someone you've never shared a server with.

## What works today

- Create your own servers and become their owner.
- Join as many servers as you like from invite links, and switch between them from the side rail.
- Create channels, and send, edit and delete messages.
- Use roles: owner, admins and moderators, with banning and unbanning.
- See member lists, and open anyone's profile to see their name, their handles and the servers you share.
- Send direct messages, with messages to offline people sent automatically later.
- Add contacts by link, even without a shared server.
- Back up and restore your identity with a recovery phrase.
- Keep a separate space for each identity, so restoring a different phrase never mixes up two people's data.
- See live network health in the corner of the window: how big the public network is, whether you're reachable, and how many people you're connected to.

## Honest limits

Dissent is young, and we would rather tell you what it doesn't do yet than have you find out the hard way.

- **Server messages aren't end-to-end encrypted yet.** Connections between devices are encrypted, so outsiders and relays can't read along. Every member of a server can read that server's messages, as you'd expect, but there's no extra encryption layer yet. That layer is planned.
- **Your identity file on disk isn't password-protected yet.** Anyone with access to your computer account could copy it. Protecting it with your system's keychain or a passphrase is planned.
- **History needs someone online.** Because there is no central server, you can only catch up on a server's history when at least one other member is online. Optional always-on helpers, called seeders, are planned to fix this for small communities.
- **One device per identity, for now.** Using the same identity on your laptop and your phone at the same time isn't supported yet.
- **No voice, video or mobile app yet.** These come after text chat is solid.
- **No security audit has been done.**

## What's coming

Roughly in this order:

1. **Resilience.** Optional always-on helpers that keep history available, an easy-to-run relay, using one identity on several devices, and recovery with help from trusted friends.
2. **End-to-end encryption for servers**, so only members can ever read a server's messages, even in theory.
3. **Voice and video.** One-to-one calls first, then group calls where the strongest computer in the call helps relay the streams. Calls stay encrypted end to end.
4. **Polish and safety.** Protection against look-alike names, safety numbers to verify a friend, warnings when someone's identity changes, notifications on phones, and optional verified handles tied to a website you own.

## Questions people ask

**Is there a company behind this? Who runs the servers?**
No company runs any servers. Every part of the system, including relays and helpers, can be run by anyone, and none of them is required.

**What happens if the owner of a server disappears?**
The server keeps working for everyone else, because every member's app holds a copy. Admins they appointed can keep moderating.

**Can someone delete my community?**
No single party can. There's no central switch to flip.

**Can moderators be abused?**
Roles are recorded in the server's history, signed by whoever granted them, and every app checks them. A moderator can only do what their role allows. An admin can't remove the owner.

**I lost my recovery phrase. Can you help?**
Unfortunately not. Nobody holds a copy, which is exactly what keeps your identity yours. Planned social recovery will let a few trusted friends help you restore access.

**Why does Windows ask to allow Dissent through the firewall?**
Dissent talks directly to other people's computers, so it needs permission to accept connections. That's normal for peer-to-peer apps. Choosing "Allow" lets friends reach you directly instead of always going through a relay.

**Is it really free?**
Yes. It's open source, so anyone can read the code, check what it does, and improve it.

## For developers

Dissent is written in Rust, with a desktop app built on Tauri. If you want to build it or contribute:

- [`docs/Decentralized Discord Alternative — Architecture Spec.md`](docs/Decentralized%20Discord%20Alternative%20%E2%80%94%20Architecture%20Spec.md) describes the overall design and roadmap.
- [`docs/design/02-communities-and-dms.md`](docs/design/02-communities-and-dms.md) describes how servers, channels and direct messages work.
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

Dissent is dual-licensed under the [MIT License](LICENSE-MIT) and the [Apache License 2.0](LICENSE-APACHE), at your choice. The icons come from [Lucide](https://lucide.dev) and are used under the ISC License.
