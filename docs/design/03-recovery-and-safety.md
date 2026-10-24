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
