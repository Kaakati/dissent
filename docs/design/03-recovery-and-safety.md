# Design 03: key recovery and safety numbers

Your identity is a root key. Losing it means losing your account unless you set up recovery. Recovery does not copy the old key anywhere. It replaces the old key with a new one, approved by people you chose.

## Setting guardians

1. Open settings and choose **Recovery**.
2. Pick 1 to 16 friends. Each one must already have their own Dissent identity.
3. Pick a threshold, such as 2 of 3. The threshold is how many guardians must agree.
4. Your root key signs the list. The list is shared with your devices and kept in your event history.

You cannot make yourself a guardian. Changing the list later means signing a new list with your root key.

## Recovering a lost key

1. On a new device, choose **Recover account** and enter a fresh root key. The app creates it on the device.
2. The app shows a `dissent://recover/...` link or QR code. It names your old root and the new one.
3. Send that link to your guardians. Each one opens it and approves with their own app. Their approval is a `dissent://approve/...` link that comes back to you.
4. Once the threshold of different guardians has approved, the new device assembles a rotation and signs it with the new root key. The rotation proves you hold the new key.
5. Anyone can check the rotation. Nothing secret moves between devices.

If you still have the old root key, skip the guardians and rotate directly. Your old key signs the request.

## How other peers treat a rotation

When a peer receives a valid rotation from an old root to a new root:

- Record that the new root replaces the old root. Look up the new root wherever the old one was used.
- Show a clear key-change warning in any conversation with that person, so they can compare safety numbers again.
- Carry roles and memberships across to the new root. This is future core work. Until it lands, a rotated member appears as a new member.
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

The rule above depends on events and the hybrid logical clock, which are future core work.

## DNS-verified handles (optional)

A handle like `@example.com` lets people recognise you by a domain you own. It is optional. Your normal name still works without it.

**How to set one up:**

1. Pick a domain you control, such as `example.com`, and ask the app for its verification text.
2. The text starts with `dissent-root=`. It is tied to your root key, so keep it exactly as shown.
3. In your domain's DNS settings, add a **TXT** record. Name it `_dissent`, so the full name is `_dissent.example.com`. Put the text in as the content.
4. DNS changes can take a while to spread. Ask the app to check again once the record exists.

**How checking works:** each person's app looks up the TXT record itself, using the system DNS and a public fallback. It accepts the handle only if the value matches your root key exactly. A handle is never trusted because a peer says it is verified. If a peer shows you a handle, your own app checks it again before you see the verified mark.

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
