# Dissent UI (React, next)

The React + shadcn/ui desktop UI, and the only frontend. It keeps the design of the plain-module UI it replaced (palette,
type, spacing, layout); that UI is removed and lives in git history.

**Status: phase 2.** The foundation and the app shell are done: theme, backend layer, store, title bar, server rail,
sidebar, user panel with the status menu, and the main and right slots. The chat pane is ported: the message list, rows,
composer, attachments, header, typing and new-message alerts. Friends, the DM profile panel and the profile card are
ported too. The call and voice boards are not yet. The [port checklist](#port-checklist) lists every current view.

## Commands

Run from `apps/desktop/ui-next`:

| Command | What it does |
|---|---|
| `npm install` | Installs the exact versions in `package.json` and `package-lock.json`. |
| `npm run dev` | Browser preview on `http://127.0.0.1:1420` against the mock backend (`src/mock/mock.js`). |
| `npm run build` | Type-checks (`tsc`, strict) and builds to `dist/`. |
| `npm run typecheck` | Type-checks the app and `vite.config.ts` only. |
| `npm run lint` | ESLint (typescript-eslint, react-hooks). |
| `npm run test:ui` | Builds the app into a scratch folder and serves it with `vite preview` on port 1420 (or `UI_PORT`), against the mock. Runs `tests/driver.mjs` (every key and mouse check, through headless Chrome or Edge; `CHROME_PATH` names the browser, and on Windows Edge is the default). Exits non-zero on a failure. No dev server, so no hot reloads during a run. Needs Node 24 (the driver imports two `.ts` modules directly). `SECTIONS=core,calls` runs some sections. See `PARITY.md` for what is covered. |

From the repo root:

| Command | What it does |
|---|---|
| `scripts/dev-next.sh` | Runs the Tauri desktop app on this UI. Nothing in `tauri.conf.json` changes; the script overrides `build.devUrl`, `build.beforeDevCommand` and `build.features` for that run. Starts Vite on 1420 itself. |

Environment for `scripts/dev-next.sh`: `DISSENT_DATA_DIR` (a separate profile), `DISSENT_LOOPBACK=1` (loopback only,
see `AGENTS.md`), `DISSENT_NEXT_PORT` (another Vite port).

## Architecture

```
src/
  main.tsx              entry: loads the mock in a browser, reads capabilities, renders, starts listeners, boots
  App.tsx               the shell layout: title bar, grid (rail | sidebar | main | right), user panel, toasts
  index.css             theme: shadcn variables, Dissent tokens, @theme mapping, Poppins
  shell.css             the few shell rules utilities cannot express: app grid, presence glyphs, rail pill
  lib/
    api.ts              every Tauri command, typed, with the names and args of the old UI's api.js
    tauri.ts            the window.__TAURI__ shape and the current window
    store.ts            the shell store (zustand): state.js, the navigation actions and their Back/Forward history,
                        status, calls and the engine-event actions
    events.ts           backend events into the store; auto-idle activity reporting
    features.ts         capability gating, the same list as features.js
    images.ts           avatar and server-icon cache, fed by every response
    unread.ts           last-seen markers in localStorage, the same keys as unread.js
    toasts.ts           transient errors
    calls.ts            call actions (voice channels, live DM and group calls, ringing, the call chat) and the lobby state
    voice.ts            voice settings and audio devices: the saved copy, the debounced save queue, the input profiles
    identity.ts         hues, initials, plurals
    profiles.ts         a counter the profile views re-read user_profile on, after a friend, safety or note change
    mock.ts             loads src/mock/mock.js in a browser (no-op inside the desktop app)
  mock/
    mock.js             the browser mock backend
    address.js          the address helpers the mock needs (lib/address.ts is the typed copy)
    types.ts            wire types
    chat.ts             the open conversation's messages, the composer's state, loads on navigation, engine message
                        events, attachment progress (views/chat.js state, actions.js loadMessages and loadDm)
    chat-actions.ts     sends, edits, deletes, reactions, file sends and attachment actions, then a reload
    alerts.ts           new-message sound and desktop notification (shouldAlert, alertFor), notification click
    target.ts           the chat target that is open, or null: one key for the chat, the alerts and the header
    typing.ts           typing signals (3 s throttle); typing-words.ts the wording; the names live in the shell store
    attachments.ts      size limit, extensions, icons, transfer text; text.ts linkify; time.ts day and clock labels
    emoji.ts            the reaction picker's emoji and search words
    osnotify.ts         desktop notifications while the window is in the background
  components/
    ui/                 shadcn primitives (see "Components"), restyled to the tokens
    shell/              title bar, server rail, sidebar, main area, right panel, user panel
    call/               the call pane, the lobby, the incoming call, call-host.tsx, and call-mode.ts (which board shows)
    voice/              the user bar's mic and output popovers, and the Settings > Voice helpers (mic monitor, keys)
    chat/               the chat pane: header, message list and rows, composer, attachments, embeds, reactions, emoji
                        picker, typing row, notices, the message menu; chat.css holds its rules
    transfers/          the Transfers page, its storage panel, the file details panel, the progress bar
    friends/            the Friends page: header tabs and Add Friend, the lists and rows, Active Now (`friends-state.ts`)
    dm/                 the DM profile panel in the right column
    profile/            the profile card (`openProfileCard`, `ProfileCardHost`), the full profile dialog, the mutual
                        servers list, `use-user-profile.ts`
    network/            the title bar's network and DHT line
    user-avatar.tsx     avatar with picture or initials, and the presence glyph
    presence.tsx        presence glyph
    dissent-mark.tsx    the Dissent mark (from the old UI's icons.svg)
    toaster.tsx         the toast stack
public/fonts/           Poppins (SIL OFL 1.1, OFL.txt alongside)
```

Data flow: components read the store and call its actions. Actions call `api`, then `set` the store. Backend events go
through `events.ts` into the same actions. Views never call `invoke` directly. A failed action shows a toast and does
not throw.

The browser preview and the desktop app share one backend contract. `lib/mock.ts` imports `src/mock/mock.js`,
which installs `window.__TAURI__` when there is no Tauri. So both UIs run against the same fixtures and the mock is
maintained in one place.

Ported from the current UI so far: `state.js` (partly), `api.js`, `features.js`, `unread.js`, `avatars.js`, `main.js`
(startup and listeners), `views/titlebar.js`, `views/rail.js`, `views/sidebar.js` (lists and user panel),
`views/statusmenu.js`. Not yet: `address.js` (the address tag in the status menu), `appearance.js`, `keyboard.js`.

## Components

`components/ui/` holds the shadcn primitives: `button`, `tooltip`, `scroll-area`, `separator`, `avatar`,
`dropdown-menu`, `context-menu`. They came from the shadcn CLI (`shadcn@4.22.1`, style `new-york`, lucide icons) and
were then restyled to the Dissent tokens:

- `button`: Dissent variants. `default` is the red primary, `secondary` the elevated surface, `ghost` the hover
  surface, `icon` is 32 px. The 40 px default height and 13 px semibold label follow `style.css`.
- `tooltip`: ink surface, 13 px semibold, no arrow (the current rail tips have none).
- `dropdown-menu` and `context-menu`: ink surface with the menu border and radius; the highlighted item is the red
  accent, as `.menu-item` is.
- `avatar`: used through `UserAvatar`, which adds the hue background and the presence glyph.
- `scroll-area`: not used. The current UI scrolls natively with thin scrollbars (`.dissent-scroll`); Radix's
  scrollbars would look different. It stays in the tree for views that need it.
- `separator`: unchanged apart from the class at the call site.

Do not re-run `shadcn add` over these files without re-applying the restyling.

## Dialogs and menus

- `openDialog({ title, sub, className, wide, content, footer, onClose })` (`lib/dialog.ts`) opens a dialog. `content` is
  a function of `close`, so a form closes itself after its action. `DialogHost` (mounted in `App.tsx`) renders the
  stack on shadcn `Dialog`. The dialog's own layout (the Create Channel panel) passes its classes as `className`.
- `confirm({ message, okLabel, danger })` resolves `true` or `false` on shadcn `AlertDialog`. Escape and the scrim are
  Cancel. A new confirmation answers the open one with `false`.
- `openContextMenu(x, y, items, { placeAbove })` (`lib/menu.ts`) opens a menu at a point: a right click, Shift+F10 (the
  browser sends `contextmenu` to the focused row) or a button's position. `ContextMenuHost` (mounted in `App.tsx`)
  renders it. Items are plain data (`MenuEntry`): `{ label, icon, sub, danger, disabled, onPick }`, `{ submenu }` (with `scroll` for a long one), `{ quick: [{ emoji, label, onPick }] }` (a row of emoji),
  `{ radio, checked }`, `{ slider }`, `{ header }` and `'sep'`. A menu tied to a button is a `DropdownMenu` that renders
  the same `MenuItems`.
- Keys: arrows, Home and End, Enter, Escape, submenus on ArrowRight or hover, and the first row takes the focus. A pick
  runs after the menu has closed, so a dialog it opens keeps the focus. A slider row changes with Left and Right.
- `lib/actions.ts` holds the engine calls the dialogs and menus make. Each refreshes the store and returns a falsy value
  on failure, with a toast.

## Theme mapping

Dark only, as the current UI is. `index.css` `:root` holds shadcn's variables with the Dissent values, and the raw
Dissent tokens under their current names. `@theme inline` exposes both to Tailwind.

| shadcn variable | Dissent token | Value |
|---|---|---|
| `--background` | `--chat` | `#1a1a1e` |
| `--foreground` | `--text` | `#dbdee1` |
| `--card` | `--panel` | `#202024` |
| `--card-foreground` | `--text-strong` | `#f2f3f5` |
| `--popover` | `--ink` | `#111214` (menus, tooltips) |
| `--primary` | `--accent` | `#d2452f` |
| `--secondary` | `--elevated` | `#2a2b30` |
| `--muted` | `--hover` | `rgb(255 255 255 / .05)` |
| `--muted-foreground` | `--muted` (text) | `#a0a6b0` |
| `--accent` | `--accent` (menu highlight) | `#d2452f` |
| `--destructive` | `--danger` | `#d33639` |
| `--border` | `--border` | `#252529` |
| `--input` | field background | `#222327` |
| `--ring` | `--focus` | `#ff8266` |
| `--radius` | `--r-2` | `8px` (`rounded-sm` 4, `rounded-md` 6, `rounded-lg` 8) |
| `--sidebar` | `--sidebar` | `#121214` |
| `--sidebar-border` | `--divider` | `#222225` |

Raw tokens with no shadcn equivalent keep their names: `--rail`, `--chat`, `--panel`, `--elevated`, `--elevated-hover`,
`--input-border` (`#27282b`, the field edge), `--divider`, `--ink`, `--hover`, `--selected`, `--border-strong`,
`--border-menu`, `--text`, `--text-strong`, `--faint`, `--offline`, `--accent-hover`, `--accent-text`,
`--accent-tint`, `--danger-text`, `--danger-hover`, `--danger-tint`, `--warn`, `--success`, `--success-text`, `--link`,
`--link-hover`, `--role-*`, `--focus`, `--motion`, the `--s-*` spacing, `--fs-*` and `--lh-*` type sizes, the fixed
sizes (`--rail-w`, `--sidebar-w`, `--row-channel`, ...), `--r-*` radii and `--shadow-*`.

Tailwind names, from `@theme inline`:

- colours: `bg-chat`, `bg-panel`, `bg-rail`, `bg-sidebar`, `bg-ink`, `bg-elevated`, `bg-hover`, `bg-selected`,
  `text-text`, `text-text-strong`, `text-faint`, `text-muted-foreground`, `text-accent-text`, `text-danger-text`,
  `text-success-text`, `text-warn`, `border-divider`, `border-border-menu`, and so on.
- type: `text-body` (15/22), `text-ui` (14/20), `text-sm` (13/18), `text-meta` (11/16), `text-tiny` (10/16),
  `text-head` (19/24), `text-title` (23/30).
- spacing: `gap-s-2`, `pl-s-4`, `h-…` (the spacing scale is `s-1` … `s-12` as px tokens).
- radius: `rounded-sm`, `rounded-md`, `rounded-lg` (not `rounded-r-*`, which is Tailwind's directional radius).

Font: Poppins 400, 500, 600 and 700 (latin and latin-ext, the same unicode ranges as `style.css`), from `public/fonts`.

Known difference from the current UI: `style.css` puts `* { font-size-adjust: 0.49 }` inside the last `@font-face`
block, where the browser ignores it. The shipped UI therefore renders Poppins without the adjustment (computed
`font-size-adjust: none`). This UI applies it at top level, as the rule's comment describes, so text is slightly
narrower than in the shipped UI. To match the shipped look, delete that rule from `src/index.css`.

## Port checklist

Status: **done** (ported and checked against the current UI), **partial** (the shell part is in; the rest is listed),
**todo**. "Batch" is the suggested parallel group in the next phase.

### Shell and startup

| Current file | Status | Here | Batch |
|---|---|---|---|
| `main.js` | partial | `main.tsx`, `lib/events.ts`. The portable gate runs before boot (`components/startup/gate.tsx`), first run and the mock settings and onboarding views are in `main.tsx`. The incoming-call and call-ending listeners and the call sounds are in `components/call/call-host.tsx`. Missing: the other mock views. The notification-click and attachment-progress listeners are in `lib/events.ts`. | C |
| `views/titlebar.js` | done | `components/shell/titlebar.tsx`; Back and Forward are in `lib/store.ts`. The network line is in `components/network/net-status.tsx`. | F |
| `views/status.js` | done | `components/network/net-status.tsx` (polls every 15 s, tooltip with the full numbers) and `components/network/format.ts`. "Resolved via" is `lib/dials.ts`. | F |
| `views/rail.js` | done | `components/shell/server-rail.tsx`; Add a Server opens `components/dialogs/server-dialog.tsx`. | D |
| `views/sidebar.js` | partial | `components/shell/sidebar.tsx`: the find box (quick switcher), Invite links, Invite people, Create channel, Create voice channel, the channel row actions (Create invite, Edit Channel), the server menu (`components/menus/server-menu.tsx`), the DM rows (presence, the second line, the muted mark, the right-click conversation menu with Profile). A voice channel row joins on a click (`openVoiceChannel` in `lib/calls.ts`). Missing: Start a call in the conversation menu. | B |
| `views/statusmenu.js` | done | `components/shell/user-bar.tsx`, `StatusMenu`. Missing: the address line (`address.js`). | B |
| `views/right.js` | partial | `components/right/right-column.tsx`: the Friends page's Active Now, the server member list, a group's members and the DM profile panel, in the shell's right slot (`components/shell/right-panel.tsx`). The call details (`components/call/call-body.tsx`) and the lobby roster (`components/call/lobby.tsx`) are here too. | C |
| `state.js` | partial | `lib/store.ts`. Message, composer and dialog slices come with their batches. | all |

### Main pane and messages

| Current file | Status | Here | Batch |
|---|---|---|---|
| `views/chat.js` | done | `components/chat/`: `chat-header.tsx`, `chat-body.tsx`, `message-list.tsx` (day dividers, new-messages line, grouping, search, keys), `message-row.tsx` (body, links, reply reference, spoiler, edit, hover actions), `composer.tsx`, `chat-notices.tsx` (welcome, key change, age gate, empty pane), `message-menu.ts`. State and loading in `lib/chat.ts`, sends in `lib/chat-actions.ts`. Missing: the profile card on a name click (batch B) and the call pane and lobby in the channel slot (batch C). | A |
| `views/attachments.js` | done | `components/chat/attachments.tsx` (cards with progress, download, pause, cancel, retry, open, the picture of a downloaded image, staged chips), `lib/attachments.ts`. The drop overlay is `ChatOverlay` in `chat-body.tsx`, fed by the drag events in `lib/chat.ts`. | A |
| `views/filedetail.js` | done | `components/right/file-detail-panel.tsx` opens `FileDetail` or `DirectTransferDetail` (`components/transfers/file-detail.tsx`) in the right column, from a downloading file's name. | A |
| `views/emoji.js` | done | `components/chat/emoji-picker.tsx` (arrows, Enter, Escape, returns focus to the opener), `lib/emoji.ts`. | A |
| `typing.js`, `typing-words.js` | done | `lib/typing.ts` (the signals, 3 s throttle), `lib/typing-words.ts` (the wording), `components/chat/typing-row.tsx`. The names come from the shell store. | A |
| `views/lobby.js` | done | `components/call/lobby.tsx`: the header's pill and Join, the self preview with the mic meter, the devices, the route note, Not now and Join call, and the roster in the right column. | C |
| `views/call.js` | done | `components/call/call-body.tsx`: the header, the route banner with its quality, the tiles with speaking rings, the controls (mute, deafen, leave) and the disabled camera and share slots, the call chat, and the call details. | C |
| `views/incoming.js` | done | `components/call/incoming.tsx`: an alertdialog (Esc declines, Enter answers, the rings stop under reduced motion), mounted by `components/call/call-host.tsx`. | C |
| `keyboard.js` | done | `components/keyboard-shortcuts.tsx`: Ctrl+K, Ctrl+,, Alt+Up/Down, Alt+Shift+Up/Down, Esc in a DM, and the rail's arrow keys. | D |
| `views/members.js` | done | `components/right/right-column.tsx`. A member's click opens the profile card (`components/profile/profile-card.tsx`). | D, B |
| `views/dmpanel.js` | done | `components/dm/dm-panel.tsx`: banner with the contact status and More, the 80 px avatar, name and tag, mutual servers, Contact since, View full profile. Shown for direct conversations while the header toggle is on. | B |
| `views/profile.js` | done | `components/profile/profile-card.tsx` (the card, placed by the click, Escape and outside click close it; the moderation buttons follow the engine's rules), `full-profile.tsx` (the dialog, the safety line, Copy contact link), `use-user-profile.ts`. The conversation menu's Profile item and the member click open it. | B |
| `views/address-card.js` | done | `components/address-card.tsx`, with the QR code and the copy button (`copyText`, as before). | B |
| `address.js` | done | `lib/address.ts` is the typed copy; `src/mock/address.js` is what the mock imports. The profile and Friends use its helpers. | B, D |
| `qr.js` | done | `lib/qr.ts`, drawn by `components/address-card.tsx` and `components/dialogs/qr-code.tsx`. | B |

### Pages

| Current file | Status | Here | Batch |
|---|---|---|---|
| `views/pages.js` | done | Transfers: `components/transfers/transfers-page.tsx` (tabs, rows and actions, storage panel), `store-meter.tsx`, `progress.tsx`, `format.ts`, actions in `lib/transfers.ts`. Friends: `friends.js` (see its row). | F, B |
| `views/friends.js` | done | `components/friends/`: the header tabs (Online, All, Pending with the count, Blocked when someone is blocked), Add Friend (paste an address or contact link, "Looking them up…", the friendly errors), the search, the grouped rows (presence, name, tag on hover, the call or status line, Message and More), Accept, Ignore and Cancel, and Active Now. The tab, search and form stay between visits. | B |
| `views/switcher.js` | done | `components/dialogs/quick-switcher.tsx`, on the shadcn Command (cmdk). Ranking is the current UI's: prefix, word start, substring. | D |

### Dialogs, menus and settings

| Current file | Status | Here | Batch |
|---|---|---|---|
| `views/dialogs.js` | done | `components/dialogs/`: server (create and join), invite and contact links, add contact, create channel, claim handle, note or nickname, safety number, new group, share a file, and `confirm()`. The contact link's QR code and address card wait for batch B. | D |
| `views/contextmenu.js` | done | `lib/menu.ts` (the item model and `openContextMenu`), `components/menus/menu-items.tsx` and `context-menu-host.tsx`. Built on shadcn `DropdownMenu` (see "Dialogs and menus"). | D |
| `views/channel-settings.js` | done | `components/channel-settings/channel-settings.tsx`: Overview, Permissions, Invites, the save bar and Delete. | D |
| `views/settings.js` | partial | `components/settings/settings-page.tsx` and one file per page in `components/settings/`. The Voice page is `components/settings/voice-page.tsx` (batch C). | E |
| `views/settings-voice.js` | done | `components/settings/voice-page.tsx`: devices, volumes, the mic test with its 40-bar meter (`components/voice/mic-monitor.ts`), the input profiles, the Custom processing, sensitivity with the live level, and the push-to-talk binder (`components/voice/accelerator.ts`). | C |
| `views/voice-menus.js`, `views/voice-prefs.js` | done | `components/voice/voice-menus.tsx` (the chevron popovers, built on `openContextMenu`) and `lib/voice.ts` (the saved settings and the debounced save queue). The user bar's chevrons are live. | C |
| `views/guardians.js` | done | `components/settings/recovery-page.tsx` and `devices-page.tsx`, with device linking. | E |
| `views/avatar.js` | done | `components/settings/avatar-editor.tsx`: `chooseImage()` and the Edit Image crop. The server icon dialog (batch D) calls `chooseImage({ shape: 'rounded', onApply })`. | E |
| `views/onboarding.js` | done | `components/startup/onboarding.tsx`, with Restore from a recovery phrase. | E |
| `views/unlock.js` | done | `components/startup/gate.tsx`: the chooser, the create, copy and unlock screens, and the link screen. `lib/passphrase.ts` holds the strength hint. | E |
| `views/updater.js` | done | `components/settings/update-dialog.tsx`: `UpdateButton` (in the main header, for batch A to keep) and the dialog. The Updates page is in `updates-page.tsx`. | E |
| `appearance.js` | done | `lib/appearance.ts`, with the zoom and compact rules in `shell.css`. The Appearance page is in `appearance-page.tsx`. | E |
| `sounds.js`, `sounds/` | done | The settings are in `lib/sound-settings.ts` (the Notifications & sounds page). The players are `lib/sounds.ts`: the join and leave sounds run on every call change (`watchCallSounds` in `lib/calls.ts`), the ringtone runs while an incoming call is shown and while an outgoing one rings, and message notifications go through `lib/alerts.ts`. | C |
| `osnotify.js` | done | `lib/osnotify.ts`. The sound and the choice of message to alert are `lib/alerts.ts` (shouldAlert, alertFor, openNotifiedConversation). | A |

### Libraries and assets

| Current file | Status | Here |
|---|---|---|
| `api.js` | done | `lib/api.ts`. All 137 commands, checked against the file. |
| `features.js` | done | `lib/features.ts`. |
| `unread.js` | done | `lib/unread.ts`. |
| `avatars.js` | done | `lib/images.ts`. |
| `ui.js` | partial | `lib/identity.ts`, `components/user-avatar.tsx`, `lib/toasts.ts`, `lib/dialog.ts` (`openDialog`, `confirm`), `lib/clipboard.ts` (`copyText`). `skeletonRows` is `SkeletonRows` in `components/chat/parts.tsx`, `linkify` is `lib/text.ts`, the time labels are `lib/time.ts`. |
| `mock.js` | shared | `src/mock/mock.js`, the only copy. |
| `icons.svg` | partial | Lucide icons (`lucide-react`) replace the sprite. The Dissent mark is in `dissent-mark.tsx`. |
| `fonts/` | done | `public/fonts/`. |
| `style.css` | partial | Tokens in `index.css`, shell rules in `shell.css`. The chat rules are in `components/chat/chat.css`, under the current UI's class names. Still to port: the dialog, settings and call rules. |

## Decisions and deviations

- **Transfer progress fills.** The current UI's `.progress-fill` is an inline span with no width, so its bars never show the
  accent fill (the track is all you see). `ProgressBar` draws the fill, as the stylesheet intends. Matching the old look
  here would mean drawing no fill at all.
- **Controls without a view are disabled**, not left live. The camera and screen share in the call pane are disabled
  slots labelled "Coming soon" until the video batch lands. Every other control is live.
- **Context menus** are shadcn `DropdownMenu`s anchored to a point, not the shadcn `ContextMenu`. The imperative
  `openContextMenu(x, y)` needs a point, and a Radix ContextMenu opens only from a `contextmenu` event on its own
  trigger. The look and the keys are the same. The shadcn `context-menu.tsx` stays in the tree, unused.
- **Presence and the status menu** use `shadcn` dropdowns with a duration submenu. Dissent's own status glyphs are in
  `shell.css`.
- **The user panel's status** reads and writes `get_status` and `set_status`, as before.
- **Calls**: join, leave, answer, decline, quick replies and the lobby are live (`lib/calls.ts`). The engine's `CallView`
  has no `messages`, so the call chat shows what the engine sends in `messages` and is otherwise empty. A sent line goes
  into the call's conversation (`send_call_message`), as in the current UI. A group call is matched to the open group
  through the group this device joined (`useCallRoom`), because the room carries no group id.
- **The lobby and the chat** share the main column: `components/call/call-mode.ts` picks the call pane, the lobby or the
  chat. Not now hides the lobby until the user goes to another place.
- **Dark only.** There is no light theme, as in the current UI.
- **Chat styles are the current UI's rules.** `components/chat/chat.css` keeps the current UI's class names (`.msg`,
  `.composer`, `.att-card`, ...), so a driver and the screenshots read the same markup in both UIs. Text colour is
  `--muted-foreground`: `--muted` is shadcn's surface, so `var(--muted)` would be invisible text.
- **A group's header shows the direct-message tools** (call, add to group, profile), as the current UI does: a group
  reports `dm: true`, so the current UI's "More" branch never runs. The call button starts a call with the group's id, as
  the current UI does. Check that with the calls batch.
- **No older history.** A conversation loads its newest 200 messages, as the current UI does. There is no "load older"
  action; the list stays at its end while it was there, and a conversation opens at its end.
- **The "New messages" line is for server channels only**, as in the current UI. Conversations have none.
- **The member-only banner is not ported.** The current UI gates it on `member_only`, a feature the list lacks, so it
  never shows.
- **Attachment cards keep the current layout**: a column, with the Pause and Cancel buttons placed over a downloading
  card. The progress fill is drawn (see the transfer progress note above).
- **The mock:** `lib/mock.ts` imports `src/mock/mock.js`. The mock also needs the `address` helpers, in `src/mock/address.js`.

- **Settings and the portable gate keep the current UI's look**, with three deliberate differences. The chooser's option
  buttons lay out their text in a grid that the button centres, so the current UI's text runs past the card; here the
  card lays out as designed. Removing a device refreshes the list (the current UI keeps the row until the page reopens).
- **Settings search keeps its text between openings**, as the current UI does (`navQuery` there, a component state here
  that survives because the page stays mounted).
- **The DM panel shows for direct conversations only.** A group keeps its member list in the same column; the current UI
  asks for a profile for the group's id there and never gets one.
- **The profile's Copy contact link copies your own contact link**, as the current UI does (`contactLink()`), not the
  person's. Kept on purpose; a person's own link is the conversation menu's Copy contact link.
- **`--danger` is defined** (it was missing, so the Do Not Disturb glyph was drawn dark). `shell.css` reads it.

## Verification

- `npm run build` and `npm run lint` pass.
- In a headless Edge, against the mock: Home and a server render without console errors; Back and Forward move between
  the places opened (Friends, a channel, a conversation), and Alt+Left goes back. The first channel of a server opens
  as in the current UI.
- Chat, checked against the current UI with the same mock at 1600 x 1000: a server channel, a DM, a group, a spoiler
  channel, an age gate, and the typing row with 1, 2, 3 and 5 people match, within a few pixels. A keyboard driver
  (kept outside the repo) passes 55 checks: the DM search, reply and reaction keys, the typing wordings, the message
  menu with the confirm and Shift-skip, spoiler and age gate, and the transfer details. Not checked yet: the drop
  overlay, staged files, editing in place, and the desktop notifications (they need the window, not the browser).
- Friends, the DM panel and the profile card, checked against the current UI with the same mock at 1600 x 1000: the
  Friends page, the DM panel and the full profile match within a few pixels. A keyboard driver (kept outside the repo)
  passes 52 checks: the tabs and their keys, presence and call lines, Add Friend's errors, Accept, the More menu, the DM
  panel and its toggle, the full profile, the card from a member (placed, focus back, outside click), the conversation
  menu's Profile, the card's address, and the privacy card preview. Not checked: the Blocked tab (the mock has nobody
  blocked), Start a call, and the moderation buttons (the mock's role does not show them).
- The desktop dev path is `scripts/dev-next.sh`. Tauri serves the dev URL only when the build has no
  `custom-protocol` feature, so the script clears `build.features` for that run.
