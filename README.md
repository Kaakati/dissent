<p align="center">
  <img src="assets/icon/app_icon.svg" alt="Dissent logo" width="140">
</p>

<h1 align="center">Dissent</h1>

<p align="center"><strong>A place to talk with your communities that no company owns and nobody can switch off.</strong></p>

<p align="center">

[![Join on Dissent][join-badge]][join]

Have the app? Open **Join server** and paste this link: `dissent://invite/dss1pwq48a5vtxxfqgp8q23ztvvyx4wcnc7qewvpf7z3zex00sg66dnq0hf6cs.cmyq6evdjk6kl64a73qtdvqdkxhl646nu2mp3gggjhsxjbc56giq.0.gl4mkxwx4mqhyhjqhakel2j47u`

</p>

Dissent is a free, open-source chat app in the spirit of Discord. You can create servers, organise them into channels, invite friends, chat in real time and send private direct messages. The difference is underneath. Dissent has no central company server holding everyone's conversations. Your servers live on the computers of the people who use them, and the app connects those people to each other directly.

> **Status: early but usable (v0.1.19).** Servers, encrypted chat, direct messages, voice and video calls and screen sharing all work today between people who have Dissent installed. Dissent has not had a security review, so please don't rely on it yet for anything sensitive. See [Honest limits](#honest-limits).

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

Out of the box, Dissent also uses number0's public relays, and number0's directory to find devices faster. Both are optional: you can run on your own relays alone and switch the directory off in Settings › Network. The app always keeps at least one relay on, yours or number0's, because it can't go online without one.

### Private conversations, directly

**Direct messages** go straight from your device to the other person's device over an encrypted connection. If they're offline, your app holds the message and keeps trying until it gets through. You can message anyone you share a server with. You can also swap **contact links** with someone you've never shared a server with.

## Features

Everything below is in the app today, and all of it is free. There's no paid tier, and nothing is held back for subscribers.

### Free here, paid on Discord

Some things Discord keeps behind Nitro or server boosts are simply part of Dissent. This compares Discord as it was at the time of writing; its plans change, so check theirs for the current details.

| Feature | Discord | Dissent |
|---|---|---|
| File size per upload | 10 MB free, 50 MB with Nitro Basic, 500 MB with Nitro | Up to 10 GB, encrypted, for everyone |
| Profile banner | Nitro only | Free: a preset colour, an image, or a looping GIF |
| Animated profile banner | Nitro only | Free |
| Server banner | Needs server boost level 2 | Free for every server |
| Animated server banner | Needs server boost level 3 | Free (looping GIF) |
| Invite card with the server's banner | Needs server boosts | Free: every invite link shows the server's banner, icon, counts and description |
| Animated server icon | Needs server boosts | Free: a looping GIF, cropped to a square |
| Server boost levels | Bought, for perks | Free and earned: members who host a relay raise the level, and nothing basic is locked |
| How many servers you can join | 100, or 200 with Nitro | No limit set by Dissent |
| Downloads from several sources at once | No | Free: big files come from every member who has them |
| Message history kept for you | On Discord's servers | On your own devices and your friends' |

Dissent doesn't have everything Discord sells: there are no custom emoji, stickers, soundboards, vanity invite links or profile themes yet.

### Servers

- **Create and join servers** from invite links, and switch between them from the side rail. Drag servers to reorder them; the order is yours alone.
- **Invite cards.** An invite link pasted in chat shows a card with the server's banner, icon, name, member and online counts, and a Join button. Short invite links fit in a message, and a full link is offered for older versions.
- **Server Profile.** Owners set the name, an icon (a picture, or a looping GIF cropped to a square), a banner (a preset colour, an image or a looping GIF), up to five traits and a description, with a live preview. Admins can change everything except the name. The banner shows at the top of the channel list for every member.
- **Text and voice channels**, with slow mode and per-channel settings. Unread channels are marked, and a "New messages" line shows where you left off. Owners and admins drag channels into order, or move the selected one with Alt+Shift+Up and Down, and everyone sees the same order.
- **Encrypted servers.** Server messages are sealed so that only members can read them. A banned or removed member can't read anything new, and someone who has left can't post until they rejoin. Owners can switch on member-only mode, which also hides settings and moderation from people who have left.
- **Private channels**, seen only by the members and roles you choose. Their messages, replies, edits, reactions and files are sealed with the channel's own key. Removing someone cuts them off from new messages, and a reader added "since joining" can't read older ones. Seeders never see or keep a private channel's files. Who is typing in a private channel is shown only to the people who can read it.
- **Message history for new members**, following the server's history setting: new members either see only what was sent after they joined (the default for new servers) or the whole history. A newcomer can read new messages right away, from every member, not just the one who let them in.
- **Seeders, kept apart.** Server Settings › Members lists seeders separately from people. A seeder asks for a storage grant when it joins, the owner and admins are told, and they can grant or withdraw it.
- **Moderation:** owner, admin and moderator roles, banning and unbanning, and an audit trail every member's app checks for itself.
- **Right-click a server** to mark it read, invite people, mute it for a while or for good, choose its notification level (all messages, mentions only or nothing), hide muted channels, open its settings, create a channel or leave.

### Boosts, earned rather than bought

A server's boost level rises with the number of **relay hosts** it has: members whose relay your own app has checked and found working within the last 72 hours. Each level is something your app verifies from your own network, so nobody can buy or fake one. Nothing is paid for, and nothing is ever locked.

| Level | Verified hosts | What it raises |
|---|---|---|
| 0 | none | Nothing extra |
| 1 | 1 | Camera at 1080p 30 fps, and screen sharing at 1080p 30 fps |
| 2 | 3 | Voice up to 128 kbps, and calls stay direct for up to 8 people |
| 3 | 5 or more | Voice up to 256 kbps, camera and screen sharing at 1080p 60 fps, and calls stay direct for up to 10 people |

Level 0 already gives 720p camera video, screen sharing at 1080p 15 fps, voice at 96 kbps and direct calls for up to 6 people, so no level takes anything away. Chat, files, calls and screen sharing work the same at level 0. A level only raises what your app sends: two members may briefly see different levels, and a call never breaks over it.

The level shows beside the server name and on its invite cards, as "Level 2 · 3 relay hosts". Server Settings › Boosts lists the hosts, the level and what the next level needs. To become a host, run a relay for the server (see [Relays](#relays)).

### Messages

- **Mentions.** Type `@` to pick a member, filtered as you type, in channels, direct messages and group messages. Owners and admins can use `@everyone` in a server. A message that mentions you is highlighted, and channels, servers and conversations show red mention counts until you read them.
- **Formatting.** Discord-style markdown: bold, italic, underline, strikethrough, headers, lists, quotes, spoilers, links with your own text, and code blocks with syntax highlighting. Ctrl+B, Ctrl+I and Ctrl+U format the selected text.
- **Right-click a message** to react, reply, edit, forward, copy, pin, bookmark it to Saved Messages, set a reminder, mark it unread, copy a link to it, have it read aloud, or delete it.
- **Replies and reactions**, with quick reactions from your most-used emoji.
- **Files of up to 10 GB**, encrypted, downloaded from several members at once, and resumed after a restart. Pictures get a small preview, and videos show their first frame. Paste a picture into the message box to attach it. A file you send shows for you at once; the people you send it to get a Download card with progress, and if a download fails it says why and offers a Retry. Downloads go to your Downloads folder unless you choose another.
- **Remove identifying details, per file.** Each photo or video you attach shows a "Remove details" switch. On, Dissent sends a copy without where, when and with what it was made, and without names, under a generic name like `photo.jpg`; the picture itself isn't touched, and the file on your computer stays as it is. It starts on for photos and phone videos and off for music, other videos and documents, and you can change it for each file. Settings › Contacts & privacy sets the starting state for photos and videos.
- **Files go only to the people who can read them.** A file's encrypted data is handed out only to the server's members, a private channel's readers or a conversation's members. Once a message is deleted and nothing else refers to its files, members' apps stop sharing them.
- **Link previews** with the page's title, description and picture. They're fetched by the sender, after asking once, so the people reading never contact the website.
- **Saved Messages** shows each bookmarked message as it looks in the chat, with its formatting, link previews, invite cards and files, and a Jump back to it. A message from a spoiler channel stays blurred until you click it.
- **Scroll back** through a conversation's whole history. Older messages load a page at a time as you reach the top, and the message you were reading stays where it was.
- **Notifications** for channels, private channels, direct and group messages, once per message. An edit, a reaction, or the same message arriving again doesn't alert a second time. By default a notification names the sender but not what they wrote, since it can show on your lock screen; Settings › Notifications & sounds lets you show the text too, or nothing at all. Reminders follow the same setting.

### People and direct messages

- **Friends.** Add people by their address (`dsu1…`) or a contact link, accept or decline requests, and block people. Friends who are in a call or online show in Active Now.
- **Profiles** with your avatar, a banner and a status (online, idle, do not disturb or invisible). Profile cards show mutual servers and, if you allow it, mutual friends.
- **Direct and group messages**, encrypted, and delivered to every device of both people. A group needs you and two other people. Messages to someone offline are held, or left at their inbox, and sent when they're back. A message an inbox refuses shows as failed, with a Retry.
- **Unread conversations** appear on the side rail with a count, like Discord.
- **Close DM** takes a conversation out of the list. It comes back when you open it again or a new message arrives; old history syncing in doesn't bring it back.
- **`dissent://` links open the app.** An invite or a contact link opens a dialog that shows what it would do, and nothing happens until you confirm. A message link jumps to the message, and a device link opens Settings › Devices with its code filled in.

### Calls

- **Voice calls** one to one, in groups, and in voice channels. Small calls connect everyone directly. Bigger ones elect the strongest computer to forward everyone's sealed streams, and a backup takes over if it drops.
- **Video and screen sharing** during a call, with a camera preview before you join. Audio always keeps its share of the connection, so video can't crowd it out.
- **Video quality.** When everyone in a call decodes AV1, a camera can be sent at 1080p 30 fps. If anyone in the call can't decode AV1, the call uses H.264, which tops out at 720p 30 fps. That's the best case: on a typical computer the encoder often can't keep up at 1080p, so the sender steps down to 720p. When the connection is short of room, video drops to 360p at 15 fps. After a slowdown, quality climbs back once there's room again.
- **Video at the size you see it.** Each person's video is drawn at the size of their tile on your screen, so a large tile stays sharp and a small one costs little.
- **Call chat** works like any other chat: formatting, mentions, link previews, invite cards, files, replies and reactions. Its messages go to the call's conversation, so everyone who can read that DM, group or channel can read them, and the call chat says so.
- **Clean audio:** noise suppression, echo cancellation, automatic gain, push-to-talk, and a mic test.
- **A seeder can forward a server's calls.** The owner turns on "Seeder may forward calls", and the server's always-on seeder can then carry a call that's too big to connect directly. A forwarder that stops passing audio is reported and dropped for the rest of the call.
- Calls are encrypted end to end, and forwarders can't open what they pass along.

### You and your devices

- **Your identity** is backed up with a 24-word recovery phrase, and its keys are kept in your system's keychain. If you copy the phrase, it's kept out of Windows clipboard history and cloud clipboard, marked as private for clipboard managers on macOS and Linux, and cleared from the clipboard after 60 seconds.
- **Several devices.** Link a laptop or a USB stick to the same identity, and cut off a lost one.
- **Recovery with friends.** Choose guardians, and if you lose your recovery phrase, enough of them can vouch for your new identity.
- **Safety numbers** to compare with a friend, with a warning if their key changes.
- **Website-verified handles.** If you own a website, prove a handle like `@example.com` in Settings › My identity: add the TXT record it shows to your domain's DNS, press Check now, and use it as your handle in your servers. Every viewer's app checks the record for itself and shows a small mark, "Checked by your app", in the member list, message headers and profiles. A check asks your DNS resolver about the domain, so it runs only when you open someone's profile or point at their handle, never just because a handle is on screen, and a result is reused for hours. Only your system's resolver is asked unless you turn on the public resolver in Settings › Contacts & privacy. A handle whose record is missing or wrong never gets the mark, and one not checked recently says "not checked yet".
- **Quick switcher:** press Ctrl+K to jump anywhere. The app is fully usable from the keyboard, and every icon button has a tooltip that names it and shows its shortcut.
- **Relays.** Your app uses up to 30 relays, fastest first, measured by latency. They come from three places: relays you add, relays your servers offer, and the public defaults. Settings › Network shows each one's latency. **Pin** keeps a relay in the list when the list is cut down, and **Turn off** drops it on this device. Pins and turn-offs are never sent to anyone.
- **Hide my IP address.** A switch in Settings › Network. On, everything goes through a relay, so the people you talk to never see your IP address; the page names the relay that carries your traffic. It costs speed, and the relay sees your IP and when and with whom you connect, but not what you send. It also turns off finding people on your local network and the public phone book, so short invite and address links don't open; full links still work. It asks before it applies, since your connection restarts.
- **Use relays offered by my servers.** A switch in Settings › Network. Off, your app neither measures nor uses the relays your servers offer, and it stops asking them for anything. Each relay you use can see your IP address, and the settings page says so next to the switch.
- **Network health.** The title bar shows your connection at a glance: the size of the public phone book, how long a lookup takes, whether others can reach you, your peers, and your relay with its round trip. When the window is narrow it leaves out the least important parts rather than cutting words off, and hovering shows every number. The probe in [Things anyone can run](#things-anyone-can-run) shows how two people connect.
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

- **Push relay** for waking phones, as a prototype.

## Things anyone can run

Dissent works with no servers at all, but these optional helpers make it better, and anyone can run them:

- **Seeder.** An always-on member that keeps a server's history available, even when everyone else is offline. If the owner allows it, it can also forward the server's calls. To run one as a service on a home server, NAS or VPS, see [`packaging/README.md`](packaging/README.md).
- **Relay.** A helper that passes sealed traffic between people who can't connect directly. Running one near your community, for example in your country, makes relayed connections faster. A guide is in [`apps/relay/README.md`](apps/relay/README.md), and the service install is in [`packaging/README.md`](packaging/README.md).
- **Relay for your own servers.** Run `dissent-relay --join <invite> --data-dir <folder>` to host a relay for a server you're in. It joins the server as storage and admits only its members. Your relay counts toward the server's boost level once members' apps can reach it, which needs a domain name with a valid TLS certificate. Members can see the relay's address.
- **Probe.** A small tool that two people on different networks can run to see whether they connect directly or through a relay, and how fast. See [`apps/probe/README.md`](apps/probe/README.md).
- **Push relay.** A prototype that wakes sleeping phones with an empty notification, without ever learning who is talking to whom. It's in progress.

## Honest limits

Dissent is young, and we would rather tell you what it doesn't do yet than have you find out the hard way.

- **No security audit has been done.** The design notes in [`docs/design/06-threat-model.md`](docs/design/06-threat-model.md) list what each part protects against and what it doesn't.
- **History needs someone online.** You can only catch up on a server when at least one other member, or a seeder, is online.
- **New members can't read messages from before 0.1.7**, even when the server shares its full history. Those were sealed before history keys existed.
- **Older versions miss newer features.** Someone on an older Dissent sees plain `@names` instead of highlighted mentions, no banners or animated icons, no boost levels, and server changes such as a rename show for them only once they update.
- **Video is often 720p.** 1080p is the best case, not the usual one.
- **Boost levels need hosts with a domain and TLS.** A relay counts only when members' apps can reach it over a valid HTTPS certificate, so a home machine without a domain can't count as a host.
- **Bigger direct calls need everyone on this version.** A level's limit of 8 or 10 people for a direct call applies only while everyone in the call runs Dissent 0.1.12 or newer and has the upload for it. Otherwise the call is forwarded from 7 people, as at level 0. Upload is judged by what each computer sends now, not by what it could send.
- **Relay hosts see member devices connect.** A host's relay learns when each member's device connects to it, and its address is visible to the server's members. Non-members see nothing.
- **When both people talk at once, your voice comes through about 6 dB quieter.** The echo canceller can't yet tell your voice from the other person's speaker. Headphones avoid the problem, since there is no speaker to echo.
- **Someone who leaves a server keeps some access for a while.** They can't post. They stop being able to read new messages once their devices are removed from the server's encryption group, which happens when another member's app is online. The server's shared secret changes when someone leaves, but only once every staying member's devices run Dissent 0.1.16 or newer. Until then, its settings, roles and member changes stay readable to them. Member-only mode avoids that. The details are in [Leaving and the secret](docs/design/05-encrypted-servers.md#leaving-and-the-secret).
- **Until the secret changes, someone who left or was banned can come back under a new identity**, as a member or as a storage helper like a seeder. Dissent refuses a storage join from anyone who has been a person in the server, but not from a new identity. Storage grants now show admins every seeder and let them grant or withdraw it, but a seeder without a grant is not refused yet. Changing the secret on every ban and leave ends the window.
- **A sudden power cut can lose your last quarter-second of sent messages.** Other members usually already have them, so they come back when you sync.
- **Mutual friends shares your whole friends list with your friends**, not just the people you have in common. You can turn this off in Settings › Privacy.
- **Members see who else is there and when.** Everyone in a server sees its member list, roles and handles, who is online, and who is typing in the channels they can read. An invite link works much like a key here: someone who holds it can see the same without ever showing up in the member list, so share invite links with care.
- **People you connect to directly see your IP address.** That's how direct connections work. Long invite links and contact links carry your public address, so anyone you share one with learns your public IP. Addresses on your home network or VPN stay out of them. A relay sees the IP of everyone who uses it. To keep your IP from the people you talk to, turn on **Hide my IP address**: then only your relay sees it.
- **Invisible hides most, not all, of your activity.** While you're invisible, your app sends no online status or typing, holds back "delivered" receipts until you're visible again, stops announcing its address, and doesn't answer other devices unless it contacted them in the last 10 minutes. What still shows that you're online: anything you do yourself, such as sending a message, reacting, joining a call or opening a server; the devices your app is linked with for each server's live updates; and the fact that your device accepts a connection at all, which someone who knows its id can still test. Receipts held while you're invisible are lost if you restart the app before going visible, so those messages never show as delivered to their senders, although you still have them.
- **Some things are stored unencrypted on your computer.** Your identity, server messages and encryption state are sealed on disk, but direct and group messages, saved messages and reminders, each server's shared secret, contacts, notes and downloaded previews are not yet. Anyone who can read your files can read those. Portable mode keeps them in its folder unprotected too.
- **Some things in a photo or video can't be removed.** "Remove details" covers JPEG, PNG, WebP, GIF, TIFF and DNG, HEIC and AVIF, MP4 and MOV, and Matroska and WebM. A file it can't clean (a BMP or SVG picture, an AVI, an MP3, a document) makes sending ask first: Send as is, or Cancel. Nothing can remove what is in the picture itself, or the camera sensor's own noise pattern, which can link a photo to the camera that took it.
- **Linux builds need Ubuntu 24.04 or newer**, or another distribution of the same age (glibc 2.39 and PipeWire 1.0). That covers the desktop app, the seeder and the relay. Older systems such as Ubuntu 22.04 and Debian 12 can't run them.
- **No mobile app yet.**
- **Most testing has happened on one machine and over the internet between a few copies of the app.** Real-world measurements across many networks and mobile carriers are still to come.

## What's next

1. Retire older apps from servers. The server's secret now changes when someone leaves, not only on a ban, but only once every member's devices run an app that accepts it. A server with one older app keeps the old behaviour until that app updates.
2. Close the privacy gaps listed above: seal direct messages and saved messages on disk, keep your public IP out of invite and contact links, and hide an invisible device from connection tests. Refuse seeders without an admin's grant.
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

## Disclaimer

Dissent is open-source, peer-to-peer software. Nobody owns, runs or moderates the network it forms, and the developers and contributors have no access to, or control over, what people share on it. Each person is responsible for their own content and conduct. Read the full [Disclaimer and Limitation of Liability](DISCLAIMER.md) before using Dissent. The app shows it when you first start it, and again whenever it changes, and you can reread it in Settings › About.

## License

Dissent is dual-licensed under the [MIT License](LICENSE-MIT) and the [Apache License 2.0](LICENSE-APACHE), at your choice. The interface icons come from [Lucide](https://lucide.dev) and are used under the ISC License. The Google Sans Flex typeface is bundled under the SIL Open Font License in `apps/desktop/ui-next/public/fonts`. Voice calls use the [Opus](https://opus-codec.org) codec, included under its BSD license in `crates/dissent-audio/vendor/libopus`.

<!-- The join badge's landing page. This is a placeholder until GitHub Pages is enabled for the repository: change this line. -->
[join]: https://kaakati.github.io/dissent/invite/#dss1pwq48a5vtxxfqgp8q23ztvvyx4wcnc7qewvpf7z3zex00sg66dnq0hf6cs.cmyq6evdjk6kl64a73qtdvqdkxhl646nu2mp3gggjhsxjbc56giq.0.gl4mkxwx4mqhyhjqhakel2j47u
[join-badge]: assets/badges/join.svg
