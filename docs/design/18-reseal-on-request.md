# Reseal on request (design note, not built)

A community message is sealed under the sender's current MLS epoch. A member who joined at a later epoch never holds
that epoch's keys, so a message sealed just before the add commit reached the sender stays locked for the joiner for
good. `unseal_pass` catches up only for epochs later than the reader's.

The sender side holds a send while a member it already lists is missing from its group
(`membership::join::wait_for_pending_adds`, bounded by `Timings::pending_add_wait`). That covers a sender who knows of
the joiner. It does not cover one that has not seen the join at all when it sends. This note is the backstop for that
case.

## Proposal

- **Who asks.** A device holding a sealed message it cannot open, whose HLC is after its own `join_hlc` and whose epoch is
  older than the epoch it joined at. Older messages, sent before it joined, stay locked by design, as today.
- **Whom it asks.** The message's author device, over RPC (`Request::Reseal { community, event }`), rate limited per
  author, and at most once per event. Without the author online, any member that has opened the message may answer
  instead; the answer is checked as below, so a member cannot put words in the author's mouth.
- **The answer.** The answering device re-encrypts the stored text under its current epoch and returns it point to
  point. The receiver accepts it only if it matches the author's commitment. Today a sealed message has no plaintext
  commitment, so the author adds one when it seals, carried through `SealedControl` (generation
  `CARRIER_GENERATION`), so older clients show it as locked and do not fail the batch (see `05-encrypted-servers.md`,
  "Compatibility rule").
- **The commitment is keyed, never a bare hash of the text.** Anyone who holds the event, a member of an older epoch or
  anyone reading the DAG at rest, could otherwise try every short message ("yes", "ok") against a plain hash. It is a
  salted commitment: the author draws a random 32-byte salt, puts it inside the sealed payload next to the text, and
  publishes `blake3(b"dissent/reseal-commit/v1" || salt || text)` in the carrier. The salt reaches only those who can
  open the message, so the commitment says nothing to anyone else. The reseal answer carries the salt with the text, and
  the receiver recomputes the commitment. (An HMAC under a key derived from the original epoch's exporter secret would
  also do, but the joiner never held that epoch, so it could not check it.)
- **Storage.** The text is stored under the original event id, as `store_content` does for a message opened normally.
  Nothing new enters the DAG.
- **Limits.** Older authors do not answer. A reseal reveals nothing new: it goes to a current member, and only for a
  message sent after that member joined.

## Tests

Hold the add commit on the sender until well after it sends (`test_support::hold_mls_commits`), past the send-side bound,
so the message goes out at the old epoch. The joiner must ask for a reseal and read the message. A request for a message
sent before the joiner joined must be refused, and so must an answer whose text does not match the author's commitment.
