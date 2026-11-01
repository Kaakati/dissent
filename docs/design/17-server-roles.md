# Design 17: server roles

Status: design, not built. Builds on custom roles and bots in `14-file-tracker.md` (Phase 0), which ship with the tracker,
and on `11-private-channels.md`, `05-encrypted-servers.md` and `15-webhooks.md`.

Product references: [own3d: Discord roles](https://www.own3d.tv/en/blog/discord/discord-roles/) and
[Discrofy: server roles explained](https://discrofy.com/blog/discord-server-roles-explained).

## Goal

An owner can set up any server shape those guides describe without a bot: staff, member tiers, interest and region tags,
cosmetic roles, verification gates, read-only announcements and staff-only channels.

## What exists and what is missing

| Capability | Today (tracker branch) | This design |
| --- | --- | --- |
| Custom roles with name, colour, permission bits, position | Yes, below Moderator | Keep |
| Assign one role to one member | Yes, Server Settings | Add the member right-click menu and bulk add |
| Ordering | `position` number only | Drag to reorder (`RoleOrder`) |
| Baseline for all members (`@everyone`) | None; member rights are implicit | `ServerBaseline` |
| Hoist, mentionable, self-assign, auto-assign | None | `RoleTraits` |
| Mention a role | None; `users` and `everyone` only | Appended field in the mention body |
| Channel overrides | None; `ChannelAccess` lists built-in roles and members only | `ChannelOverride` |
| Member permissions (send, react, attach, voice) | None | New bits |
| Kick, timeout | None; ban only | `Kick`, `Timeout` |
| Audit log | None | Derived from replay |
| Bots hold roles | Yes | Keep; add placement warnings |

## Decisions

1. **Admin and Moderator stay as fixed ranks.** They are the top of the list, cannot be dragged below custom roles and
   released clients keep agreeing on them. There is no Administrator bit: full bypass is the Admin rank, granted only by
   the owner. This is the guides' "one or two people" rule enforced by the model.
2. **Permissions are additive with one exception.** A member's server permissions are the baseline plus the union of their
   roles. In a channel, overrides apply in this order, each replacing what came before for the bits it names: baseline
   override, then role overrides (any allow beats a role deny at this level), then member override. Owner and Admin
   ignore overrides. The order is Discord's, so people's expectations and copied setups carry over.
3. **No bots needed for auto-roles.** A role marked auto-assign is held by every member through replay. No event per
   member and no online bot.
4. **New settings are new events, not fields on `RoleSpec`.** Postcard is positional, so `RoleSpec` stays as it is.
5. **The UI states what each permission really guarantees** (next section). A decentralized server cannot promise what a
   central server can, and the settings must not imply it does.

## How each permission is enforced

There is no server to refuse a request, so a permission holds only as strongly as its mechanism.

| Level | Mechanism | Applies to | Against a modified client |
| --- | --- | --- | --- |
| **Cryptographic** | Sealed under keys only readers hold | View channel, read history | Holds, except messages from before removal that the person already had keys for |
| **Replay** | Every peer rejects the event | Send, react, attach, edit, delete, ban, roles, channel changes | Holds: nobody else accepts the event |
| **Peer refusal** | Call participants refuse to exchange keys or forward media | Join a voice channel, and speak, video and screen share | Holds if the other participants run honest clients |
| **Courtesy** | The honest app applies it | Mentions notifying, viewing the audit log | Does not hold; it is shown in the UI as a convention |

`VIEW_AUDIT_LOG` is a courtesy on purpose: every member already holds the events, and the log only presents them. Hiding
it would claim secrecy it does not have. Moving the log's events into a staff-only sealed scope is out of scope.

## Permission bits

Existing bits 0 to 10 are unchanged. `Perms` keeps unknown bits, so new ones need no wire change.

| Bit | Name | Default baseline | Level | Risk label |
| --- | --- | --- | --- | --- |
| 11 | `VIEW_CHANNEL` | on | Cryptographic | |
| 12 | `SEND_MESSAGES` | on | Replay | |
| 13 | `ADD_REACTIONS` | on | Replay | |
| 14 | `ATTACH_FILES` | on | Replay | |
| 15 | `CREATE_INVITES` | on | Replay | |
| 16 | `CONNECT` | on | Peer refusal | |
| 17 | `SPEAK` | on | Peer refusal | |
| 18 | `VIDEO` | on | Peer refusal | |
| 19 | `SCREEN_SHARE` | on | Peer refusal | |
| 20 | `KICK_MEMBERS` | off | Replay | Elevated |
| 21 | `TIMEOUT_MEMBERS` | off | Replay | Elevated |
| 22 | `MUTE_IN_CALLS` | off | Peer refusal | Elevated |
| 23 | `VIEW_AUDIT_LOG` | off | Courtesy | |
| 24 | `READ_HISTORY` | on | Cryptographic | |

Existing bits get labels: `MANAGE_ROLES` and `MANAGE_SERVER` are **High risk**, since they allow escalation or control of
the server. `MANAGE_CHANNELS`, `BAN_MEMBERS` and `MENTION_EVERYONE` are **Elevated**. The editor groups bits as General,
Membership, Text, Voice and Advanced, in plain language with the risk label beside the toggle.

`READ_HISTORY` works only as a channel-level choice about keys for newcomers. The server's `HistoryVisibility` remains the
ceiling and nothing here can give someone history that setting withholds.

The moderator rank gains `KICK_MEMBERS`, `TIMEOUT_MEMBERS`, `MUTE_IN_CALLS` and `VIEW_AUDIT_LOG` under the new feature gate.

## Events

All appended and carried through `SealedControl`, so released clients show them as locked. Each is latest by HLC per key,
except the moderation actions.

| Payload | Key | Who | Notes |
| --- | --- | --- | --- |
| `RoleOrder { order: Vec<RoleId> }` | server | `MANAGE_ROLES` | Replaces `position` as the ranking once present. Roles at or above the actor must keep their relative order and stay above the rest. Roles missing from the list keep their old position under the listed ones |
| `RoleTraits { role, hoist, mentionable, self_assign, auto_assign }` | role | `MANAGE_ROLES`, role below actor | See the rules below |
| `ServerBaseline { perms }` | server | `MANAGE_ROLES` and `MANAGE_SERVER` | The actor may only change bits they hold |
| `ChannelOverride { channel, target, allow, deny }` | channel + target | `MANAGE_CHANNELS` and `MANAGE_ROLES` | `target` is `Everyone`, `Role(id)` or `Member(root)`. Empty allow and deny removes it. Applies to a category through `LayoutEntry`'s category, with channels inheriting unless they have their own |
| `Kick { user, reason }` | none | `KICK_MEMBERS`, target below actor | Removes membership; the person may rejoin with a valid invite |
| `Timeout { user, until_ms, reason }` | user | `TIMEOUT_MEMBERS`, target below actor | At most 28 days. `until_ms: 0` ends it |
| `ModerationReason { event, reason }` | event | Author of that event | Adds a reason for `Ban`, `RoleAssign` and others that have none. Up to 512 characters |

### Rules for traits

- **Self-assign:** a member may send `RoleAssign` for themselves, on or off. The role may carry only a safe set of bits:
  `ADD_REACTIONS`, `ATTACH_FILES`, `SEND_MESSAGES`, `VIEW_CHANNEL`, `READ_HISTORY`, `CONNECT`, `SPEAK`, `VIDEO`,
  `SCREEN_SHARE`, `CREATE_INVITES`. No `Manage`, moderation or `MENTION_EVERYONE` bits. Setting self-assign on a role with
  other bits is rejected, and so is adding such a bit to a self-assign role. This closes the guides' warning about
  self-assignment unlocking sensitive access.
- **Auto-assign:** the same safe set. Replay treats every current member as holding it. A `RoleAssign { on: false }` from
  someone with `MANAGE_ROLES` still removes it from that person, so a moderator can pull the role from one member.
- **Hoist:** display only. The member list groups by the highest hoisted role. The editor warns past 5 hoisted roles.
- **Mentionable:** anyone may mention the role; otherwise only holders of `MENTION_EVERYONE`. The role id goes in an
  appended mentions field and only the receiver's app decides to notify, so this is Courtesy level.

### Replay determinism

- **Time-based rules use the event's HLC, never a device's clock.** A timed-out member's `Message` is rejected if its HLC is
  before `until_ms`. Otherwise two peers would disagree on the same event.
- **Backdating remains the known limit.** As with bans, an author can claim an old HLC, and replay's ordering rules apply
  unchanged. Roles add no new power here, and this design does not solve it.
- Every check takes the state at the event's position, as Phase 0 does.

## Compatibility gate

Enforcing `SEND_MESSAGES` and the like on released kinds means a new client rejects an event an old client accepts, and the
two would replay different servers. So enforcement on released kinds is behind a switch, like `CustomRolePerms`:

- **`RolesV2 { on }`**, owner only, carried. The owner may turn it on only when every member device answers
  `Request::Features` with the new `FEATURE_ROLES_V2` bit.
- While off, new roles, display traits, self-assign and audit log all work, since they harm nobody. Baseline and override
  denials on released kinds show as "Waiting for N members to update", and the settings name who.
- New joiners with an old client after the switch is on are the same residual as member-only control. The invite page and
  join path refuse a client without the feature when the server has it on.

## View channel and keys

Today `ChannelAccess.roles` holds built-in `Role` values, so a custom role cannot open a private channel. Overrides fix that
and have to obey doc 11:

- A channel is **private** when its effective `VIEW_CHANNEL` for `Everyone` is denied. Its readers are computed by replay
  from overrides and assignments. `ChannelAccess` keeps working for channels with no overrides.
- **Any change that removes a reader starts a new key generation:** an override edit, `RoleAssign { on: false }`,
  `RoleDelete`, a kick, or `RoleOrder` if it changes an outcome. The core already rotates on `ChannelAccess`; it must rotate
  on these too. The author of the change creates and offers the key. If that author may not hold the key themselves (a
  moderator removing a role from someone in a channel the moderator can't read), the **next remaining reader to see the
  change** does it, with the lowest root id winning a tie.
- Turning a public channel private seals new messages only. The dialog says old ones stay readable to everyone who has them.
- **Self-assigned or auto-assigned roles that open a private channel get a warning**, since anyone can then read it.

This is the riskiest part of the design and has the most tests.

## Audit log

Derived entirely from replay: role changes, assignments, channel and override changes, bans, kicks, timeouts, invites,
server settings. Each row shows who, what, target, when and reason. It is filterable by person, action and target. No new
storage: it reads the event log.

## UI

**Server Settings, Roles** (replaces the current section):

- Left: the list in rank order, with Owner, Admin and Moderator fixed at the top, `@everyone` fixed at the bottom,
  and a search. Drag handles appear only on roles below your own, with a line showing how far you can drag.
- Right: the editor with four tabs.
  - **Display:** name, colour swatches and picker, hoist, mentionable, and a live preview of a chat message and member row.
  - **Permissions:** the grouped toggles, a search and Clear all. A bit you do not hold is disabled with a reason.
  - **Assignment:** self-assign and auto-assign, disabled with a reason when the role has unsafe bits.
  - **Members:** the list, search, Add members (bulk) and remove.
- **Inspect a member:** choose someone and see their effective permissions, per channel, with which role or override
  decided each. This replaces the guides' permissions calculator.
- **View as role:** shows the channel list and composer as that role would see them, without changing anything. It is what
  the guides mean by testing with a non-admin account. It can't show message contents you have no key for.

**Channel Settings, Permissions:** targets list (`@everyone`, roles, members) and, for each bit, Deny, Neutral or Allow.
It shows which category it inherits from and offers Sync to category.
Presets: **Read-only announcements**, **Staff only** and **Help channel** (post and attach, 10 s slowmode).

**Member menu and profile:** right-click a member, Roles, tick roles. Timeout and Kick appear when permitted. The profile
shows role chips.

**Self-assign:** a Channels & Roles page lists self-assign roles as toggles, and a **role panel** can be posted in a channel
as a card with buttons. Each button just sends the `RoleAssign`, so it needs no bot.

**Verification gate:** a preset for the guides' pattern. It sets the baseline to deny `SEND_MESSAGES`, `ADD_REACTIONS` and
`ATTACH_FILES`, and creates a self-assign **Verified** role that restores them. New members see the rules and an Accept
button. Account age is **not** offered: a key's age cannot be proven, so it could only be time since joining this server,
which replay can check and the gate offers as "Member for at least N minutes".

**Offboard staff:** one action on a member: remove every role with non-safe bits, list invites they created with a Revoke
all, and open the audit log filtered to their last 30 days.

**Templates**, offered when a server has no custom roles. Each creates roles and overrides the owner can edit:

| Template | Roles, top to bottom |
| --- | --- |
| Small community | Member (auto) |
| Community | Support, Contributor, Verified (self), interest tags (self), gate on |
| Gaming | Event Host, LFG (self), region tags EU, NA, OCE (self), per-game tags (self) |
| Creator or product | Support, Subscriber (manual), Beta Tester, Verified (self) |

Export and import of a role setup as JSON supports the guides' migration checklist, and carries no members.
Import creates roles beside the existing ones and deletes nothing.

Warnings in the editor: a bot role above staff, a role below the roles its holders must manage, more than 5 hoisted roles,
and a cosmetic role with permissions.

## Phases

1. **Display and ordering:** `RoleOrder`, `RoleTraits` (hoist, mentionable), the new editor, member right-click, role
   mentions. No enforcement change.
2. **Assignment:** self-assign, auto-assign, Channels & Roles, role panels, templates.
3. **Audit and moderation:** audit log, `Kick`, `Timeout`, `ModerationReason`, Offboard staff.
4. **Enforcement:** `RolesV2`, `ServerBaseline`, `ChannelOverride`, text and voice bits, Inspect, View as role, presets, gate.
5. **Private channels by role:** `VIEW_CHANNEL` and key rotation on role changes. Ships after phase 4 has run in a release.

## Tests

- Pins: `wire_order.rs` for each new payload, and an old-client decode test showing them as locked.
- Replay: every rule above from both sides, including a concurrent demote and message, self-assign with unsafe bits, role
  order that tries to lift a role above the actor, and override precedence table tests.
- Two peers converge after out-of-order delivery of every new event.
- Networked (`scripts/fwtest.sh`): a removed role holder cannot read new messages in a private channel and still reads old
  ones. A moderator who is not a reader triggers rotation through a remaining reader. A modified sender's denied message is
  rejected by everyone. A timed-out member can't post.
- Gate: `RolesV2` is refused while one device lacks the feature.
- UI driver: drag limits, disabled bits with reasons, inspect explanations, presets, templates.
- `06-threat-model.md` gets an entry for the enforcement levels.

## Role limit

`MAX_CUSTOM_ROLES` is 250, set before custom roles first shipped. Replay enforces it, so raising it later needs a feature
gate. Tags by region, game and interest use roles up quickly, and 250 matches Discord.
