# Design 03: key recovery and safety numbers

Your identity is a root key. Losing it means losing your account unless you set up recovery. Recovery does not copy the old key anywhere. It replaces the old key with a new one, approved by people you chose.

## Setting guardians

1. Open settings and choose **Recovery**.
2. Pick 1 to 16 friends. Each one must already have their own Dissent identity.
3. Pick a threshold, such as 2 of 3. The threshold is how many guardians must agree.
4. Your root key signs the list. The list is shared with your devices and kept in your event history.

You cannot make yourself a guardian. Changing the list later means signing a new list with your root key.

## Recovering a lost key

1. On a new device, finish first run with its fresh key (save its recovery phrase: this key becomes the account), then choose **Continue and recover with guardians**, or **Settings › Recovery › Recover with guardians** later.
2. Enter the lost account's id, which a friend can read off your profile, or its `dissent://guardians/...` link.
3. The app shows a `dissent://recover/...` link and QR code naming the old root and the new one. The link also carries this device's address after `?via=`. It shows a nine-digit code made from the request.
4. Send the link to each guardian, then call them or meet them and read the code out loud. Never send the code with the link.
5. The guardian opens the link (or pastes it in **Settings › Recovery › Help a friend recover**). Their app shows whose account it is, whether they are on its guardian list, and the servers they share. They type the code they heard and confirm they heard it from you directly. A wrong code is refused, so a link passed on by someone else cannot be approved unseen. They can also decline; a decline is signed, so you see who said no.
6. Their app signs the approval and sends it straight to your new device, with the owner's signed guardian list and, if they choose, invites to the servers they share with you. When the new device cannot be reached, the guardian gets a `dissent://approve/...` link to send back by hand, and you paste it into the recovery dialog.
7. The dialog shows each guardian as waiting, approved or declined, and "k of n approvals needed" once the list is known. When the threshold of different guardians on the list has approved, **Finish recovery** syncs the servers from the invites, publishes the rotation in each, and joins as the new identity.

Nothing secret moves between devices: approvals, the guardian list and invites are all things the person could paste.

Every guardian keeps the owner's signed list from the moment it was named (`Request::GuardianOf`), so the new device gets it from the first approval and never needs the owner's old device. The recovery in progress survives restarts (`recovery.bin`) until it finishes or is cancelled. See `dissent-core/src/recovery_flow.rs`.

If you still have the old root key, skip the guardians and rotate directly. Your old key signs the request.

## How other peers treat a rotation

When a peer receives a valid rotation from an old root to a new root:

- Record that the new root replaces the old root. Look up the new root wherever the old one was used.
- Show a clear key-change warning in any conversation with that person, so they can compare safety numbers again.
- Carry roles, memberships, handles, moderation and ownership across to the new root. Replay does this (`CommunityState::migrate` in `dissent-events`).
- Refuse any rotation that does not verify. Do not show a warning for a rotation that fails.

## Safety numbers

Two people can compare a safety number to confirm they hold each other's real root keys. The number is the same on both phones, whatever order the two keys are given in. It has 60 digits in 12 groups of 5. A matching number means nobody has swapped in a different key for either of you. A changed number means a key has changed and should be checked before you trust it.

## Linking and revoking devices

Each identity can have several devices. Each device has its own key. Only a device that holds the root key can add or remove devices.

1. **Start on the new device.** Choose **Add device**. The new device makes its own key and a `dissent://link/...` request with a short name, such as "Laptop". The request is signed by the new device's key, so the device proves it holds that key.
2. **Approve on a root device.** Open the request on a device that holds the root key and confirm. That device signs a cert binding the new device to your root. The reply carries the cert and the request's nonce.
3. **Finish on the new device.** It checks that the nonce matches its own request, then saves the cert. It now chats as your user, with no root key.

A device without the root key can chat but cannot add or revoke devices. Only root-holding devices can do that. The root key is never sent to a linked device. Sending it would make the linked device as powerful as the root. A future option may seal the root key to the new device, but that is not built yet.

**Revoking a device.** A root device signs a `DeviceRevocation` naming the lost device and a time. Peers must treat that device as untrusted from that time onward. Revocation does not reach into history. Events the device signed before the revocation time stay valid. The time is the millisecond of the revocation's HLC, and an event stamped in that millisecond or later is refused, whatever order it sorts in. A device that has not heard of the revocation can stamp a post in the same millisecond, and that post is refused too. Peers settle the set of revocations before they apply any event, so every peer refuses the same events. Limit: the device stamps its own events with its own clock. A device whose clock runs behind the revocation can still sign events stamped before it, for as far as its clock lags. Peers cannot tell those events from honest ones, so a lagging clock widens the window in which a revoked device's events are accepted.

**How peers must treat a revocation.** When a peer receives a valid `DeviceRevocation`:

- Reject every event from that device whose hybrid logical clock timestamp is after the revocation time.
- Keep events from that device with earlier timestamps.
- Refuse a revocation that does not verify against the root the device belongs to.

Replay applies this rule with `Payload::DeviceRevoke` and the hybrid logical clock (`dissent-events`).

## DNS-verified handles (optional)

A handle like `@example.com` lets people recognise you by a domain you own. It is optional. Your normal name still works without it.

**How to set one up:**

1. Pick a domain you control, such as `example.com`, and ask the app for its verification text.
2. The text starts with `dissent-root=`. It is tied to your root key, so keep it exactly as shown.
3. In your domain's DNS settings, add a **TXT** record. Name it `_dissent`, so the full name is `_dissent.example.com`. Put the text in as the content.
4. DNS changes can take a while to spread. Ask the app to check again once the record exists.

**How checking works:** each person's app looks up the TXT record itself, using the system DNS. A public resolver (Cloudflare) is asked as well only if the user turns that on in Settings › Contacts & privacy. It accepts the handle only if the value matches your root key exactly. A handle is never trusted because a peer says it is verified. If a peer shows you a handle, your own app checks it again before you see the verified mark.

**How others learn it:** the website handle is your server handle. After a pass, Settings › My identity offers to claim it in every server you are in, as an ordinary handle claim, so nothing new goes on the wire. Any server handle that is a domain is checked this way. The first claim in a server still wins, so someone else can hold `@example.com` in a server, but it never gets the mark.

**When it is checked:** a lookup tells the resolver and the domain's owner that someone is looking at the handle, so the desktop app never checks a handle just because it is shown. It checks when you open that person's profile, or point at the handle in the member list, and keeps the result for this run only. The member list and message headers show the mark only from a result the app already has. A pass shows the mark for 24 hours and is checked again, when next asked, after 6 hours, so a removed record stops verifying within a day. A failure is checked again after 1 hour, and a lookup that could not finish after 10 minutes. Until a fresh check passes, the handle says "not checked yet", never verified. See "Website handles" in `06-threat-model.md`.

**Limits:** the check proves who controls the domain, not who the person is. Look-alike domains that mix scripts are refused. Losing control of the domain, or the TXT record being removed, makes the handle stop verifying.

## At-rest protection

Your identity file is encrypted on disk. Its data key is kept by your operating system: Windows Credential Manager, the macOS Keychain, or the Secret Service on Linux. Where no such store exists, the key comes from a passphrase, stretched with Argon2id.

**What it protects against:**

- **A copied data folder.** Copying the app's data folder gives an attacker only ciphertext. The identity stays locked to your OS account.
- **A lost or stolen backup.** A backup of that folder, or a disk image taken while you are logged out, can't be read without the key.

**What it does not protect against:**

- **Malware running as you.** Any program running under your account can ask the OS store for the same key.
- **Someone at your unlocked computer.** The key is available while you are logged in.
- **A weak passphrase.** Argon2id slows guessing, but a short passphrase can still be guessed.
- **Leftover plaintext.** After upgrading an old plaintext file, the old bytes are overwritten with zeros. SSD wear-levelling and filesystem snapshots can keep old copies, so this is best effort.
