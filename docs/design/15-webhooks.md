# Design 15: webhooks

Status: design, not built. **Parked** until the `tracker` branch merges. Revised 2026-10-29 with the user's
decisions (below). Depends on custom roles and bots (`14-file-tracker.md`, "Phase 0"), which are built on the
unmerged `tracker` branch. Nothing here can land before that phase is on `main`.

A webhook lets an outside service (GitHub, CI, monitoring, anything that can send an HTTPS POST) post messages into
one channel of a server. The service posts to a URL. A headless **bridge** receives the POST, checks it, formats it,
and posts it as a **bot**: a member identity that is marked as a bot, is not in the server's MLS group, and holds a
posting key only for the channels its hooks target.

It builds on:
- `14-file-tracker.md`, "Phase 0": `Perms`, `RoleBook`, `BotRegister`, `create_bot`, `Request::Features`.
- `05-encrypted-servers.md`: the carrier, member-only control, the history key of each epoch ("History messages").
- `11-private-channels.md`: per-generation channel keys, `ChannelKeys`, the offer-on-new-generation and retry
  machinery. The tracker's restricted categories (`tracker::restricted`) use the same push-and-pull pattern.
- `apps/seeder`: the headless member, its vault and its service install. `apps/relay`'s Let's Encrypt flags.

## Decisions

Made by the user on 2026-10-29.

1. **A bot reads only the channels it posts in.** It is not in the MLS group. Its posts are sealed under a key for
   each channel it posts in (see "Bot posting keys"). It reads nothing else, its own posts excepted.
2. **No rotation grace.** A rotated URL stops working as soon as the bridge sees the rotation.
3. **Built-in Let's Encrypt** (HTTP-01 and TLS-ALPN-01). A bring-your-own certificate and a reverse proxy stay
   supported.
4. **One bridge runs bots for several servers.**
5. **Bot channel scope is enforced in replay at once.** It is not held behind an owner switch, because it only
   restricts bots. (The team lead's call.)
6. **Old devices do not block hooks. The owner confirms the risk** (see "Mixed versions" and "Server Settings").
7. **No general per-channel keys now.** Webhooks stay write-only. `CK` is kept as a named derivation only (see
   "Generalizing: channel keys").

## Goals and non-goals

Goals:
- incoming webhooks into one text channel each;
- GitHub events formatted well;
- a generic JSON body;
- Slack- and Discord-compatible bodies, so existing integrations only need the URL changed;
- hooks created, rotated and revoked in Server Settings;
- a bridge that holds the least possible: bot identities, posting keys for hooked channels, and hook secrets.

Non-goals:
- outgoing webhooks, where the server calls a URL;
- slash commands, and interactive bots that read and answer (see "Generalizing: channel keys");
- arbitrary message templates;
- attachments or images taken from a payload;
- a hosted bridge run by the project.

## Pieces

| Piece | What it is |
| --- | --- |
| Bot | A root made by `create_bot` and registered with `BotRegister { on: true }`. It joins as a person but is never in the MLS group. It holds no custom role bits by default. |
| Hook | A control event (`WebhookCreate`) that names a bot, one channel and a provider. Its event id is the hook id. It is live until `WebhookRevoke`. |
| Bot posting key | For each hooked channel and each key period, a key derived by members and given to the bot. The bot seals its posts with it. See "Bot posting keys". |
| Hook secret | 32 random bytes, made by the bridge and kept only by the bridge. The URL token and the provider signing secret are derived from it. It is never in the DAG. |
| Bridge | `apps/webhook-bridge`, a headless program modelled on `apps/seeder`. It runs one `Engine` per bot, possibly in several servers, behind one HTTPS listener. |
| Manager | A member whose bits include the new `MANAGE_WEBHOOKS`. The owner and admins hold it. |

## The bot

A bot is the phase 0 bot with these rules.

**Not in the MLS group.** A bot joins with `Join`, like a person, after its `BotRegister`; `create_bot` already
publishes the register first. Members do not admit it to the group:
- the engine refuses `MlsJoin` from a bot root;
- `CommitRules` refuses a commit that adds a device of a bot root;
- an add that was made before the device saw the `BotRegister` is undone: any member that finds a bot leaf in the
  group removes it, and a removal of a bot's device is always a valid commit;
- the bridge itself never asks for `MlsJoin`, and ignores a welcome.

So a bot holds no epoch secret, no history key and no MLS-exported control key. It cannot open MLS messages,
`HistoryMessage`s, member-sealed control events, or any private channel.

**What it still holds: the community secret.** It needs the secret for the gossip topic, for request tokens and for
sync, as a storage member does. With it, it can open:
- the control events sealed under the community secret: channel names, member display names, roles, as storage
  members can before member-only control;
- the webhook events, which are always under the community secret (see "Events").

Server Settings recommends turning member-only control on when the first bot is added. With it on, a bot reads no
server structure beyond member ids and channel ids. The rest of its replay stays locked, as a storage member's does.
The bot does not need it: members enforce what bots may do.

**Channel scope, in replay.** A bot root may author only:
- `BotPost` events, see below;
- `Delete` of its own posts;
- its own `Join` and `Leave`.

A `BotPost` is accepted only in a channel that at least one live hook naming that bot targets at the event's
position. Every other kind from a bot root is rejected, reactions and plain messages included. A stolen bot key can
therefore post only where its hooks point, and a revoke closes the channel to it.

**Display.** The bot's name is its `BotRegister` name, and the UI shows the bot badge on every post. A payload cannot
change the author or the badge. A Discord-style `username` becomes a bold first line, "via <username>", only when the
hook allows it (off by default).

**Slowmode.** A bot gets `BYPASS_SLOWMODE` only through a role that has it. Otherwise the bridge merges the events of
a slowmode window into one post.

## Bot posting keys

The base key of a channel for one key period:
- **Public channel:** the period is an MLS epoch `e`. The base is the epoch's history key
  `H_e = export_secret(group, "dissent/history/v1", 32)`, which every person device in that epoch already records
  (`05`, "History messages").
- **Private channel:** the period is a key generation `g`. The base is the channel's generation key `P_g`, which
  every reader holds (`11`).

The bot posting key for a bot `B`, a channel `c` and a period `p`:

```
K(B, c, p) = blake3::derive_key("dissent/bot-post/v1", base_p ‖ community ‖ c ‖ B)
```

- **Members derive it** from the base they already hold. No extra key is made or stored on their side.
- **The bot cannot derive** `base_p` from `K`, and cannot derive the key of another channel, another period or
  another bot.
- **What the bot reads is its own posts only.** Its key opens nothing that members write, since members' messages in
  that channel keep their MLS or history form. It is in effect write-only, which goes beyond decision 1's "read only
  the channels it posts in".

**The post.** A `BotPost` carries `BotBody { kind, period, ciphertext }` inside the carrier (`CARRIED_BOT`, like
`carry_private`, generation `CARRIER_GENERATION`), so released clients decode it as a locked control event. The
fields:
- `kind`: `Message` or `Edit { target }`, an appended enum.
- `period`: `Epoch(e)` or `Generation(g)`.
- `ciphertext`: XChaCha20-Poly1305 under `K`, with a random nonce and associated data
  `"dissent/bot-post-aad/v1" ‖ community ‖ channel ‖ period ‖ author`.

Replay checks the scope and never opens the text. The message view opens it with the `K` the reading device derives.

**Distribution, built on the private-channel machinery:**
- **Push.** When a commit moves the epoch, the committing device sends `Request::PushBotKeys { community, keys }` to
  each bot device with a live hook in the community. Each key is `(channel, period, K)`, for that bot's hooked
  channels only. A new private-channel generation is pushed the same way by the device that makes it, the author of
  the `ChannelAccess`.
- **Pull.** A bot with no key for the current period asks `Request::BotKeys { community }` of up to three member
  devices, with the same backoff as private-channel keys: 1 s, doubling to 60 s, cut short when a member comes
  online.
- **Who answers.** An active person device answers `BotKeys` only for a requester whose root is a registered bot with
  a live hook in the responder's replay. It gives only the hooked channels, and only the current period plus the one
  before it, so a post in flight across a commit still lands. Everyone else gets the uniform refusal.
- **What the bot keeps.** It keeps the newest two periods, sealed under the vault in `bot_keys.bin`, and drops older
  ones.

**History.** A bot's post is readable by whoever holds the base key of its period. In a public channel that is the
members of the epoch, and joiners under the history setting, because they get `H_e` through `HistoryKeys`. In a
private channel it is the readers of the generation. So a bot's posts follow the same history rules as members'
messages, with no extra setting.

**Hooks to private channels** need no access change. The bot is not made a reader. It gets only `K` for that channel,
derived from `P_g` by a reader. Revoking the last hook for the channel, or a `ChannelAccess` that starts a new
generation, leaves the bot with no key for the new period.

### What it costs

- **A key push on every epoch change.** Each hooked channel and bot costs one 32-byte key per commit, sent to every bot
  device with hooks in the community. Commits happen on joins, leaves, bans and device adds. For a server with
  `h` hooks it is `h` small messages per commit, to a few devices.
- **Latency after a commit.** Until a push or pull lands, the bot holds only the previous epoch's key. It may still
  post under it, and replay accepts the previous period. Beyond that, posts wait in the bridge's queue. With no member
  online for a whole period change, the bot cannot get the new key, and a later post goes out under the newest key it
  has. Members open it if they still hold that base, which they do unless they joined after it.
- **Departure window.** A member removed in epoch `e+1` still holds `H_e`, so it can read bot posts made under
  epoch `e` after its removal: by a bot that has not yet seen the new key, during the one-period grace. This is the
  same window as "The window before a removal commit lands" in `06`, a little longer, and bounded by the push.
- **Per-period secrecy.** As with `HistoryMessage`, a bot's posts are protected per epoch or generation, not per
  message.
- **Old clients see bot posts as locked** (see "Mixed versions").
- **Storage members** cannot read bot posts, as they cannot read messages.
- **Members' messages are unchanged:** no new form for persons, no change to files, replies or reactions in hooked
  channels.

### Generalizing: channel keys

The same derivation with no bot in it,

```
CK(c, p) = derive_key("dissent/channel-key/v1", base_p ‖ community ‖ c)
```

would give every channel its own key per period. If members also sealed their channel text under `CK`, a bot or
integration could be handed `CK` for chosen channels and read them: interactive bots, per-channel audiences for
tools, and a bot that answers. That is the general form of decision 1.

It is not built here, because the webhooks need only write access. What it would cost:

- **Every text kind needs a new form.** Replies, edits, reactions and file manifests are MLS application messages
  today, and only plain messages use the history key. Moving all of them to `CK` gives up per-message forward
  secrecy for every kind, not only plain messages. That is the cost `05` accepted for plain messages, now for
  everything.
- **A compatibility gate** like `HistorySupport`: a device sends the new forms only when every member device
  supports them.
- **Two forms in replay for every channel,** permanently.
- **Each read-capable bot is one more party holding a channel's text,** per channel and per period, with the same push
  cost as above.

Decided (decision 7): not built now. `CK` stays as the named derivation in the code, one function, so the bot key
and any later reading key share it. Read-capable bots are reconsidered when there is a use for them.

## Events

Appended to `Payload` after the tracker's payloads. The webhook events are carried (`carry_sealed`, generation
`CARRIER_GENERATION`). They are added to the accepted list in `open_sealed_carried`, and to the release list in
`wire_order.rs`.

They are always sealed under the community secret, even under member-only control, as invite events are. The bot
must open them to know its hooks, and it holds no epoch key. The cost is that storage members, and anyone else who
holds the secret, see hook names, channel ids, providers and filters. No secret is in them.

`BotPost` is a new carried form (`CARRIED_BOT`), not a `Payload` variant, so it needs no `Payload` change.

```rust
digest_id!(HookId, "A webhook: the id of the `WebhookCreate` event that made it.");

pub enum HookProvider { Generic, GitHub, Slack, Discord }   // appended-only enum

pub struct HookSpec {
    pub name: String,            // 1..=32 chars, trimmed, as a role name
    pub bot: RootId,             // a registered bot of this community
    pub channel: ChannelId,      // a live text channel, public or private (not voice, not control)
    pub provider: HookProvider,
    pub require_signature: bool, // default true for GitHub, false otherwise
    pub show_username: bool,     // let a payload's `username` show as "via <name>"
    pub events: Vec<String>,     // GitHub event names to post; empty = defaults. At most 32, each <= 64 chars
    pub epoch: u32,              // the secret epoch the bridge must hold; +1 on rotate
    pub enabled: bool,           // a paused hook keeps its secret but posts nothing
}

Payload::WebhookCreate { spec: HookSpec }
Payload::WebhookUpdate { hook: HookId, spec: HookSpec }
Payload::WebhookRevoke { hook: HookId }
```

`HookSpec` is postcard, which is not self-describing. A later field needs a new payload variant, never an added field.

Replay rules, all in the control channel:

- **All three kinds** need `MANAGE_WEBHOOKS`. New kinds always honor custom bits, as for roles.
- **`WebhookCreate` and `WebhookUpdate`:**
  - `spec.bot` must be a registered bot and a current member.
  - `spec.channel` must be a live text channel.
  - `spec.epoch` must be at least the hook's current one.
  - An update must keep `bot` and `channel`. To change them, revoke the hook and create another.
  - At most 16 live hooks per channel and 64 per community.
  - The latest HLC per hook wins, as for `RoleUpdate`.
- **`WebhookRevoke`** is permanent. A later update of that hook is refused, as for `RoleDelete`.
- **Ripple effects.** Deleting the channel, `BotRegister { on: false }`, or a ban or leave of the bot makes the hook
  dead in replay. Dead is not revoked: re-registering the bot does not bring the hook back, because replay keeps it
  dead after any such event. The manager creates a new one.
- **New permission bit:** `MANAGE_WEBHOOKS = 1 << 11`, appended to `Perms::NAMED` as `manage_webhooks`. `MANAGE_BOTS`
  does not grant it, because a hook opens a channel to the internet.

## How the hook config reaches the bridge

The decision is in two layers. The DAG holds what a hook is. A direct manager-to-bridge RPC carries only the secret,
once, from the bridge to the manager.

1. The manager's app publishes `WebhookCreate`. The hook now exists in every member's replay, the bot's included.
2. The manager's app calls one of the bot's devices: `Request::WebhookIssue { community, hook, epoch }`.
3. The bridge checks in its own replay that:
   - the hook is live and names this bot;
   - `epoch` equals `spec.epoch`;
   - the caller's root authored the hook's latest spec event, the create or the update that set this epoch.

   The bridge cannot check `MANAGE_WEBHOOKS` itself under member-only control, since role events are locked to it. It
   does not need to: an unauthorized spec event is rejected by every member, so the bot's posts for it are rejected
   too. Answering only that event's author means the bridge never hands a secret to anyone else.
4. The bridge makes a fresh 32-byte secret `S` for `(hook, epoch)`, seals it in `hooks.sealed` under the vault, and
   answers `Response::WebhookIssued { secret: S }` over the authenticated QUIC connection.
5. The manager's app derives the URL and, for providers that sign, the signing secret, and shows them once. It
   stores neither.

**Rotation, with no grace (decision 2):**
1. The manager publishes `WebhookUpdate` with `epoch + 1`, then calls `WebhookIssue` for the new epoch.
2. When the bridge replays the update, it deletes the old secret at once. The hook answers `404` until the new secret
   is issued, which the UI does straight after.
3. A sender that keeps using the old URL gets `404`. GitHub records the failures, and its Redeliver button can resend
   them once the URL is updated.

**Revocation** is `WebhookRevoke` alone. It needs no contact with the bridge, which drops the secret when it replays
the event, even if it was offline when the manager acted.

Why not only a sealed control event with the secret in it:
- Every holder of the community secret can open the webhook events, members and storage members alike. Any of them
  could then post as the hook.
- GitHub's HMAC needs the raw signing secret on the bridge, so a hash of the secret would not be enough.
- The secret would stay in every DAG for ever.

Why not only an RPC, with no event:
- Other admins could not see, audit or revoke a hook while its creator is offline.
- The bot's channel scope could not be checked in replay.
- A bridge restored from backup would have no authority to compare against.

**New requests and answers,** appended to `Request` and `Response`:
- `WebhookIssue`, answered with `WebhookIssued`.
- `WebhookStatus { community }`, answered with `WebhookStatus(Vec<HookStatus>)`.
- `BotKeys`, answered with `BotKeys(Vec<(ChannelId, Period, [u8; 32])>)`.
- `PushBotKeys`, answered with `Ack`.

All four are listed in `Request::community()`, so they carry the community token. Each refuses an unauthorized caller
with one uniform error (privacy audit P-09). `WebhookStatus` is answered to the same roots as `WebhookIssue`, and to
any root that is the author of a live spec event in the community.

`HookStatus` is `{ hook, last_ok_ms, last_error: Option<&'static str>, posted_24h, refused_24h }`. It is kept in memory
on the bridge. Error texts are fixed and carry no payload content.

**Feature bit:** `FEATURE_WEBHOOKS = 1 << 2` in `Request::Features`. A manager's app talks to a bot device only when
it answers with this bit.

## The endpoint

```
POST /hooks/<hook-id>/<token>
```

The parts:
- `hook-id` is the lowercase base32 of the `HookId`, 52 characters. Hook ids are globally unique, so one bridge
  routes hooks of many servers by id alone.
- `token` is the lowercase base32 of `blake3::derive_key("dissent/webhook-url/v1", S ‖ epoch_le)`, 52 characters.
- The signing secret, for providers that sign, is `blake3::derive_key("dissent/webhook-signing/v1", S ‖ epoch_le)`,
  shown as hex. The derivations are one-way, so a leaked URL does not reveal the signing secret, nor the reverse.
- `GET /` answers `204` for proxy health checks and says nothing about the bridge. With HTTP-01 on, port 80 also
  serves `/.well-known/acme-challenge/<token>`. Every other path is `404`.

### Token check

The check runs the same way whether or not the hook exists:

1. Parse both segments. A malformed one is treated as unknown.
2. Look up the hook id. An unknown hook uses a fixed dummy expected token.
3. Compare `blake3::keyed_hash(local_key, presented)` with the stored `blake3::keyed_hash(local_key, expected)` as
   `blake3::Hash` values, whose `==` is constant time. Raw bytes are never compared in a loop.
4. Unknown, wrong token, revoked, dead and paused all answer `404` with the same body. A more specific status comes
   only after the token matches.

### Provider signatures

Signatures are checked over the raw body, before parsing, with `ring::hmac` (`HMAC_SHA256`). Its `hmac::verify` is
constant time.

| Provider | Header | Checked |
| --- | --- | --- |
| GitHub | `X-Hub-Signature-256: sha256=<hex>` | HMAC-SHA256(signing secret, body) |
| Generic | `X-Dissent-Signature: t=<unix s>,v1=<hex>` | HMAC-SHA256(signing secret, `t` ‖ `.` ‖ body), and `t` within ±300 s |
| Slack / Discord | none | the URL token only, as on those platforms |

With `require_signature` on, a missing or bad signature is `401`. With it off, a signature that is present is still
checked, and a bad one is still `401`.

### Replay protection

- **TLS** comes first: a captured request needs a broken TLS connection or a leaked log.
- **GitHub:**
  - The bridge keeps the `X-GitHub-Delivery` ids it accepted per hook for 24 h, up to 10 000. A repeat answers `200`
    and posts nothing.
  - GitHub's signature does not cover that header, so for signed GitHub hooks the bridge also keeps the BLAKE3 of
    each accepted body for 24 h. Either match is a duplicate.
- **Generic signed:** the ±300 s window, plus the `v1` values seen within it.
- **Unsigned hooks** have only TLS. The UI says so.
- **Restarts:** the caches start empty after a restart (open question 3).

### Bodies

- **Content types:** `application/json`, and `application/x-www-form-urlencoded` with a `payload=` field. Anything
  else is `415`.
- **Size:** at most 1 MiB, enforced while reading and before any parsing. Over that is `413`. `--max-body` raises the
  limit to at most 5 MiB.
- **Parsing:** `serde_json` into typed structs that name only the fields used. Unknown fields are ignored.

### Responses

| Status | When |
| --- | --- |
| `202` | Accepted and queued |
| `200` | Duplicate delivery; nothing posted |
| `204` | Valid, but this hook does not post this event type |
| `400` | Not valid JSON for the provider, or nothing to post |
| `401` | Bad or missing signature, after the token matched |
| `404` | Unknown, revoked, dead or paused hook, or a wrong token |
| `413` | Body too large |
| `415` | Content type not supported |
| `429` | Rate limited, with `Retry-After` |
| `503` | The bot is not ready (not joined, or no posting key yet) or the queue is full, with `Retry-After` |

Error bodies are fixed short texts that never echo the payload.

## Formatting

Output is the chat's Discord-style markdown (`ui-next/src/lib/markdown.ts`).

**Escaping.** Every string taken from a payload is escaped:
- markdown punctuation gets a backslash;
- a line start that would form a header, list or quote is escaped;
- `@` is followed by U+2060 (word joiner).

A mention is never real in any case: mentions travel in a sealed list the bridge never fills, and a bot has no
`MENTION_EVERYONE`.

**Links** come only from known payload fields (`html_url`, `compare`), and only if they are `https`.

**Length.** A post is cut to `MAX_MESSAGE_CHARS` (8 000) with "…".

### GitHub

The event name comes from `X-GitHub-Event`. The default filter is `push`, `pull_request`, `issues`, `release` and
failed `workflow_run`. `events` in the spec replaces it.

| Event | Post |
| --- | --- |
| `ping` | "**repo**: webhook connected" |
| `push` | "**repo** `branch`: **pusher** pushed N commits", then up to 5 lines `` `abc1234` first line — author ``, "and N more", and a "[compare](…)" link. A tag push or branch delete is one line. An empty push posts nothing. |
| `pull_request` | opened, reopened, closed (as "merged" when merged), ready_for_review: "**repo**: **user** opened [#12 Title](url)". Other actions post nothing. |
| `issues` | opened, closed, reopened, in the same form. |
| `release` | published: "**repo**: release [v1.2.3](url) published by **user**". |
| `workflow_run` | completed: "**repo**: workflow **CI** on `branch` **failed** / succeeded / cancelled ([run](url))". Successes post only with `workflow_run:success` in `events`. |
| other | `204`, unless named in `events`, which posts "**repo**: <event> by **user**". |

### Generic JSON

`{ "text": "...", "title": "...", "url": "https://..." }`. `text` is required. `title` becomes a bold first line,
linked to `url` when `url` is `https`. `text` is posted as markdown, with `@` neutralised.

### Slack-compatible

- `text` is required.
- `blocks` and `attachments` are optional.
- mrkdwn is converted: `<url|label>` becomes `[label](url)`, and `*bold*` becomes `**bold**`.
- From `blocks`, only `section` and `header` text is used.
- From `attachments`, only `pretext`, `title`, `title_link`, `text` and `fields` are used.

### Discord-compatible

- `content` is the post's text.
- `embeds` (up to 10) become title (linked to `url`), description, and `**name**: value` fields.
- `username` shows only with `show_username`.
- `avatar_url`, `thumbnail`, `image` and `footer.icon_url` are ignored. The bridge never fetches a URL.

## Rate limits and queue

- **Before the token check:**
  - 60 requests per minute per source address, and 600 per minute for the whole bridge.
  - The source address is the TCP peer. It is the last hop of `X-Forwarded-For` only with `--trust-proxy` and a
    loopback peer.
  - Addresses are kept in memory only.
- **Per hook:** a token bucket of 30 posts per minute, burst 10. Over it is `429`.
- **Per bot:** 120 posts per minute across its hooks, so one server's traffic cannot starve the other bots of a
  shared bridge.
- **Merging:** posts to one channel within 2 s merge into one, up to the length cap. Under slowmode, a bot without
  `BYPASS_SLOWMODE` merges for the slowmode window.
- **Queue:** at most 100 pending posts per hook, in memory. A full queue answers `503`. A post waiting for a posting
  key waits up to 5 minutes. A failed send is retried three times over a minute, then counted in `refused_24h` and
  dropped.

## One bridge, several bots

**Layout.** One process, one vault (one keychain entry or passphrase), and per bot:
- `<data-dir>/bots/<bot-root>/identity.bin`, sealed;
- `profile/`, the engine data;
- `hooks.sealed`;
- `bot_keys.bin`.

One HTTPS listener routes `/hooks/<hook-id>/…` to the bot whose replay holds that hook.

**Commands:**
- `dissent-webhook-bridge bots add --token-file <path>` joins with a bot token and prints the bot's root. The token
  file is read once. The bridge then keeps only the sealed identity, and tells the operator to delete the file.
- `bots list` shows each bot's server, name, hooks and readiness.
- `bots remove <root>` leaves the server and deletes the bot's folder.
- `run` starts every bot.

**Engines.** Each bot runs its own `Engine` and iroh endpoint, since each bot is a different device key. This costs
one endpoint and one gossip instance per bot. That is fine for tens of bots; a cap of 64 per process is set.

**Isolation.** The bots share a process, so a compromise of the bridge host is a compromise of every bot on it. Each
bot can still post only in its own hooked channels and read only its own posts. Operators who want separation run
one bridge per server.

## TLS and deployment

Exactly one of these modes is chosen at start.

### `--acme-email <email> --hostname <name>`: built-in Let's Encrypt

**Crate:** `instant-acme`. It is a pure-Rust ACME (RFC 8555) client on rustls and hyper, maintained in the rustls
ecosystem by the maintainer of rustls and quinn. It supports every challenge type, because it leaves serving the
challenge to the caller. The version is pinned at implementation (read its `examples/` first). Certificate requests
use `rcgen`, which is already in the lockfile.

`tokio-rustls-acme` was not chosen, although it is in the lockfile through iroh-relay and `apps/relay` uses it. It
validates by TLS-ALPN-01 only, and decision 3 asks for HTTP-01 too.

- **`--acme-challenge tls-alpn-01`** (the default) needs only port 443. The listener's
  `rustls::server::ResolvesServerCert` returns the challenge certificate for a ClientHello offering ALPN
  `acme-tls/1`, and closes that connection after the handshake. It never reaches the HTTP router.
- **`--acme-challenge http-01`** also binds `--acme-http-bind` (default `0.0.0.0:80`). Port 80 serves
  `/.well-known/acme-challenge/<token>` while an order is open, and `404` for everything else. No redirect is
  served, so port 80 never handles webhook traffic.
- **Storage.**
  - The account credentials and the certificate's private key are sealed under the vault in `<data-dir>/acme/`.
  - The certificate chain is stored as PEM, since it is public.
  - `--acme-staging` uses the Let's Encrypt staging directory. `--acme-directory <url>` points elsewhere (tests use
    Pebble).
- **Renewal.**
  - The bridge checks every 12 h, with up to 1 h of jitter.
  - It renews when under 30 days remain, or when the ACME server's renewal info (ARI) says so, if `instant-acme`
    exposes it.
  - On failure it backs off from 1 h to 12 h.
  - The new certificate is swapped into the resolver with no restart, through an `ArcSwap` (or a lock) holding the
    current `CertifiedKey`.
  - A warning is logged when under 7 days remain, and `bots list` shows it.
- **Rate limits.** The bridge never orders more than once per 6 h per hostname, so a crash loop does not hit Let's
  Encrypt's limits.

### `--tls-cert <pem> --tls-key <pem>`: bring your own

rustls 0.23 with `ring`, TLS 1.2 and 1.3 only. The files are re-read on SIGHUP (Unix) and when they change (checked
every 10 minutes), so external renewals work.

### `--bind 127.0.0.1:8788` with no TLS flags: behind a reverse proxy

Plain HTTP, loopback only. A non-loopback bind without TLS is refused at start. `packaging/README.md` gets a Caddy
snippet that turns off access logs for `/hooks/*`, since the token is in the path.

### Next to the relay

`apps/relay` owns its port. Either:
- run the bridge on another port with its own ACME;
- or use one reverse proxy for both.

Two ACME clients cannot both use TLS-ALPN-01 on one port 443. Bot devices are member devices, so a members-only
hosted relay admits them with no extra step.

### Other run flags

`--data-dir`, `--passphrase-file` (or the keychain), `--trust-proxy`, `--max-body`, `--loopback`, and `service
install|start|stop|status|uninstall`, reusing the seeder's service code through a shared module. No secret ever goes
on a command line or into a service file (audit P-27).

### Dependencies

`axum` is already in the workspace. Add to `[workspace.dependencies]`:
- `instant-acme`;
- `rcgen` (already in the lockfile);
- `tokio-rustls`;
- `rustls-pemfile`;
- `arc-swap` (if not already present).

No HTTP client of the bridge's own. `instant-acme` brings hyper for talking to the ACME server, and that is its only
outgoing traffic.

## Server Settings

A "Webhooks" section, shown to holders of `MANAGE_WEBHOOKS`.

**List.** For each hook:
- name, channel, bot, provider;
- created by, and when;
- enabled or paused;
- last delivery and the 24 h counts, from `WebhookStatus` when the bot is reachable.

**Create:**
1. Pick a channel and a provider.
2. Pick a bot, or create one. Creating one (`create_bot`) shows its token once, with the `bots add` command for the
   bridge.
3. The app publishes `WebhookCreate`.
4. It waits for the bot to replay the event: it retries `WebhookIssue` while the answer is "unknown hook", for up to
   30 s.
5. It shows the URL, and for GitHub and signed generic hooks the signing secret.
   - Each has a copy button. The clipboard entry is excluded from history and cleared after 60 s (audit P-19).
   - The dialog says the URL is shown once.

**Before the first bot.** The dialog says:
- a bot reads only its own posts;
- it sees channel and member names unless member-only control is on, which it offers to turn on.

**Old devices (decision 6).** Before the community's first hook, the app counts member devices that do not answer
`FEATURE_WEBHOOKS`, using `devices_without`. A device that is offline or too old to answer counts.
- If any remain, the dialog shows:
  - the count;
  - the people they belong to, where the directory knows the device's root: display name, plus "N devices"
    per person; devices whose root is unknown are counted as "unknown devices";
  - the two risks: these devices show bot posts as locked, and they would admit a stolen bot key to the MLS group.
- Creating the hook needs an explicit confirm: a checkbox, "I understand the risk", then Create. Without it the
  button stays disabled.
- The confirm is not stored as a setting. **Every later hook re-warns** while old devices remain: the same list, with
  a shorter text and one confirm button.
- With no old devices left, there is no warning.
- The count is computed when the dialog opens. A device that comes online on an old version afterwards is caught by
  the next hook's check, not by this one.

**Other actions:**
- **Rotate** publishes the update and issues the new secret in one step. It warns that the old URL stops working at
  once.
- **Pause, Resume, Edit** update the spec. Edit covers the name, the filter and `show_username`.
- **Revoke** is permanent, after a confirm.

**Bridge offline.** Create and Rotate publish the event and then say the URL can be fetched when the bridge is back
("Show URL", a fresh `WebhookIssue`). Nothing stores the URL in the meantime.

## Threat model

### What a bot and its bridge host can read

- **Posts:** the bot's own posts only. It is not in the MLS group, holds no history key or epoch key, and its posting
  keys open nothing else (decision 1).
- **Server structure:**
  - with member-only control off: what the community secret opens, which is channel names, display names and roles,
    as for a storage member;
  - with it on: member ids and channel ids only.
- **Gossip:** as a holder of the community secret it receives the topic's presence, typing and signed addresses in
  clear, as every secret holder does today (audit P-02). Sealing these under an epoch key, the audit's fix, would
  exclude bots too.
- **Network:** a bot device sees what a member device sees, namely member device ids and their addresses.

### A stolen bot key or bridge host

The attacker:
- can post in the hooks' channels only (replay scope);
- can read the bot's own posts and what the community secret opens;
- can try `MlsJoin`, which every current member refuses. An older member may admit it (see "Mixed versions"), so the
  owner should not create hooks while old devices remain, and the dialog says so;
- cannot create hooks, change roles or read members' messages.

Revoke a bot with `BotRegister { on: false }` or a ban. A ban also rotates the community secret.

### A hook URL

- **It is a bearer credential for one channel.** Whoever holds it can post there as the bot, within the rate limits,
  until it is rotated or revoked. It reads nothing.
- **Where it leaks:** CI logs, screens, proxy access logs.
- **What limits the damage:**
  - signatures, required by default for GitHub;
  - one-step rotation with no grace;
  - per-hook limits;
  - the bot badge on every post;
  - the proxy snippet that turns off path logging.
- **What the URL reveals:** the bridge's hostname and the hook id. The hook id is an event id, which names neither
  the server nor its members without the DAG.

### Abuse

- The per-address and global limits apply before any hashing.
- The body is read only after the token matches, and only up to the cap.
- Signatures are checked before any parsing.

### No server-side request forgery

The bridge never fetches a URL from a payload:
- **Payload URLs:** avatar, image and thumbnail URLs are ignored. Link URLs are copied into the text, never fetched.
- **Link previews:** every bot engine runs with `link_previews = false`. The bridge refuses to start if a profile
  says otherwise, and a test pins this.
- **Dependencies:** the bridge has no HTTP client dependency of its own. A CI check on `cargo tree -p
  dissent-webhook-bridge -e normal` fails if a new HTTP client appears outside `dissent-core`'s preview path and
  `instant-acme`.
- **ACME:** the client talks only to the configured directory URL, which comes from the command line, never from a
  payload.

### Logging hygiene, at every level

- **Never logged:**
  - the token, or the path past `/hooks/<first 8 of hook-id>`;
  - `S`, the signing secret, signature headers, the ACME account key, the certificate key;
  - bodies or any payload field;
  - the source IP at info and above.
- **Logged at info:** the short hook id, the bot's short root, the provider, the status code, the size and the
  duration.
- **HTTP layer:** no `tower_http::trace` default span, which records the full URI. A custom span records the matched
  route and the short id. axum's rejection texts are replaced by the fixed ones.
- **Default filter:** `info,mainline=error,mainline::rpc=off,iroh_relay=error`.
- **Rotation:** 10 MB files kept 7 days, with an age limit on the current file too (audit P-24).
- **Pinned by a test:** it captures the tracing output of a full request and asserts that neither the token, nor the
  signing secret, nor a marker string from the body appears in it.

### What others see

- **Senders:** the bridge's hostname, IP and response codes.
- **Let's Encrypt:** the hostname. It also goes into Certificate Transparency logs, so the bridge's name is public.
- **Members:** the bot with its badge, and hook names, channels and authors in Server Settings.
- **Storage members and other secret holders:** the webhook events' fields (always under the community secret) and the
  existence of bot posts, with their author, channel and size.
- **The DAG** never holds a secret, a token or a token's hash.

## Mixed versions

- **Released clients** decode the webhook events and `BotPost` as carried events they cannot open. They show them as
  locked, and their replay is unchanged. Bot posts are therefore invisible to them as text: a member on an old
  version sees "locked message" where the bot posted.
- **The channel scope** is enforced by new replay at once (decision 5). A released client does not apply it, but a
  `BotPost` is locked to it anyway, and a bot's plain message is rejected by new clients. So the divergence is only
  in what an old client shows: it may show a stolen bot key's plain messages, which new clients reject.
- **Old members may admit a bot to the MLS group.** An old member does not know `is_bot`, so it answers a bot's
  `MlsJoin`. The honest bridge never asks. A stolen bot key could ask, and get every public channel's text from then
  on, until a new member removes the bot leaf. That happens on its next commit check, since new members remove bot
  leaves.
  - This does not block hooks (decision 6). Server Settings lists the old devices and their people, and requires an
    explicit confirm before the first hook. It warns again for every later hook while old devices remain.
- **The bridge** needs this version. `WebhookIssue`, `BotKeys` and `PushBotKeys` to an older device get the generic
  unknown-request error, which the app reads as "too old".
- **The bot-key answer** is computed by each answering member. A member too old to answer gives nothing, and the bot
  asks another.

## Build plan

Every phase ends green on `cargo test -p` and `cargo clippy -p -- -D warnings` for the crates it touches. Networked
tests are `#[ignore]` with `loopback_only`, run through `scripts/fwtest.sh`.

### Phase 0 (prerequisite)

Custom roles and bots from `tracker` on `main`.

### Phase 1: events (`dissent-events`)

Work:
- `HookId`, `HookProvider`, `HookSpec`, the three payloads, `MANAGE_WEBHOOKS`.
- `HookBook` in `CommunityState`: live, revoked, dead, per-channel index.
- The `CARRIED_BOT` form with `BotBody`, its seal and open helpers, and the AAD.
- The replay rules, including the bot authoring rules and channel scope.
- Carrier and `wire_order` entries.

Tests:
- create, update and revoke need the bit, including through a custom role;
- the spec checks: unregistered bot, voice channel, deleted channel, lower epoch, a changed bot or channel on update,
  the caps;
- a revoked hook is never updated;
- concurrent updates resolve to the highest HLC;
- a `BotPost` in a hook channel is accepted, and is rejected in another channel and after the revoke's HLC;
- a bot's plain message, reaction or control event is rejected;
- the ripple effects, and that re-registering does not revive a dead hook;
- a carrier round trip, and that a 0.1.x wire list decodes them as locked;
- `upgrade_compat` still loads.

### Phase 2: core (`dissent-core`)

Work:
- `bot_post_key` and `channel_key` derivations, with fixed test vectors.
- `MlsJoin` refused from bots, `CommitRules` refuse adding bots, bot leaves removed.
- The bot post path: seal under the newest key held, with the previous-period grace.
- `PushBotKeys` from the committer and the `ChannelAccess` author, and `BotKeys` with retries.
- `WebhookIssue` and `WebhookStatus` with uniform refusals.
- `FEATURE_WEBHOOKS`.
- Engine API: create, update, rotate, revoke, issue, status.
- The view opens bot posts.

Unit tests:
- the refusal paths;
- the derivations;
- a bot cannot open a `HistoryMessage`, an MLS message, a private channel or another bot's post with what it holds.

Integration tests (`it::webhooks`):
- an owner, a member and a bot engine;
- create, issue, post: the member sees the post with `bot: true`;
- an epoch change by the member's leave pushes a new key, and the next post is under it;
- the bot gets no history key, epoch key or private-channel key under `All`;
- `MlsJoin` from the bot is refused;
- a hook to a private channel: readers see the post, a non-reader member does not, and the bot is not a reader;
- a revoke made while the bot is offline takes effect when it syncs.

### Phase 3: `apps/webhook-bridge`

Work:
- CLI with `bots add/list/remove`, the vault, the service install, and one engine per bot.
- The axum server, the token check, signatures, replay caches, limits, formatters, queue, `hooks.sealed`.
- ACME with both challenges, BYO certificate reload, the loopback proxy mode.
- The logging setup.

Tests:
- formatters against synthetic fixtures in `apps/webhook-bridge/tests/fixtures/`;
- the escaping cases;
- GitHub's documented HMAC vector: secret `It's a Secret to Everybody`, body `Hello, World!`, signature
  `757107ea0eb2509fc211221cce984b8a37570b6d7586c22c46f4379c8b043e17`;
- the generic signature window;
- an identical `404` for unknown, wrong, revoked and paused;
- `413` before parsing, `415`, `429` with `Retry-After`;
- duplicate delivery id and duplicate body;
- the log redaction test;
- a non-loopback bind without TLS is refused;
- the bridge refuses to start with previews on;
- routing two bots in two servers;
- the per-bot limit;
- the ACME resolver answers `acme-tls/1` with the challenge certificate, and port 80 serves only challenge paths.

`#[ignore]` tests:
- ACME end to end against Pebble in Docker, with `--acme-directory`. Never stop Docker Desktop; other projects use
  it.
- Bridge plus member engine on loopback: a POST appears as a post.

### Phase 4: desktop

Work:
- Tauri commands (serde_json across the boundary).
- The Server Settings section, the show-once dialog, rotate, pause, revoke, status.
- The first-bot notices and the member-only offer.
- The old-device list (count, people, unknown devices), the required confirm, and the re-warning on later hooks.

Tests:
- the URL is not kept in state after the dialog closes;
- Create stays disabled until the confirm is ticked while old devices remain;
- a second hook warns again;
- no warning shows once every device answers the feature.

### Phase 5: docs and release

- `packaging/README.md`: ACME, Caddy, systemd, Docker.
- A "Webhooks" section in `06-threat-model.md`.
- The tools archive gets the binary.
- Linux clippy in Docker.

## Open questions for the user

Questions 1 (read-capable bots) and 2 (old devices) were decided: see decisions 7 and 6.

3. **Persistent replay caches.** Should delivery ids and body hashes survive a restart, in a small sealed file?
4. **Updating posts.** Should CI events edit the earlier post (for example "running" then "failed")? `BotBody.kind`
   allows `Edit`. The bridge would keep a map from run id to post id.
5. **Webhook events under member-only control.** They stay under the community secret so the bot can read them, which
   shows hook names and channels to storage members. Is that acceptable? The alternative is sealing the spec under
   the bot posting key, which needs the bot to hold older periods' keys.
6. **Built into the relay.** Should the bridge be an option of `apps/relay`, sharing its port and ACME, rather than a
   separate binary?
7. **GitHub defaults.** Are push, PR, issues, releases and failed workflow runs the right defaults?
