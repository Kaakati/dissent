# Parity with the current UI

Read-only inventory of `apps/desktop/ui` (the shipped UI) against `apps/desktop/ui-next`, for the cut-over. Status:

- **done**: ported and in the tree. The keyboard driver (`tests/driver.mjs`) covers most of it; see "Not covered".
- **partial**: ported with a known gap, listed under it.
- **missing**: not ported.
- **mock only**: exists for the browser preview in the old UI, not for the app. Not ported on purpose.

Source of each row: the old file's exports and call sites, checked against the ui-next component named in the row.

## Views (`apps/desktop/ui/views/`)

| Old file | Status | ui-next | Gap |
|---|---|---|---|
| `address-card.js` | done | `components/address-card.tsx` | |
| `attachments.js` | done | `components/chat/attachments.tsx`, `lib/attachments.ts` | |
| `avatar.js` | done | `components/settings/avatar-editor.tsx` (Edit Image, `chooseImage`), server icon in `components/menus/server-menu.tsx` | |
| `call.js` | partial | `components/call/call-body.tsx` | The call chat stays empty: the engine's `CallView` has no messages (backend). Camera and screen share are disabled slots until the video work lands. |
| `channel-settings.js` | done | `components/channel-settings/channel-settings.tsx` | |
| `chat.js` | done | `components/chat/` (header, body, message list, composer, message menu) | |
| `contextmenu.js` | done | `lib/menu.ts`, `components/menus/menu-items.tsx`, `context-menu-host.tsx` | |
| `dialogs.js` | done | `components/dialogs/` (server, invite, contact link, add contact, create channel, claim handle, safety number, new group, note or nickname, share a file, confirm, QR) | |
| `dmpanel.js` | done | `components/dm/dm-panel.tsx` | |
| `emoji.js` | done | `components/chat/emoji-picker.tsx`, `lib/emoji.ts` | |
| `filedetail.js` | done | `components/right/file-detail-panel.tsx`, `components/transfers/file-detail.tsx` | |
| `friends.js` | done | `components/friends/` | |
| `guardians.js` | done | `components/settings/recovery-page.tsx`, `devices-page.tsx` | |
| `incoming.js` | done | `components/call/incoming.tsx`, `call-host.tsx` | |
| `lobby.js` | done | `components/call/lobby.tsx` | Group calls are matched through the group this device joined (`useCallRoom`), because the room has no group id. Check against a real group call. |
| `members.js` | done | `components/right/right-column.tsx` | |
| `onboarding.js` | done | `components/startup/onboarding.tsx` | |
| `pages.js` | done | `components/transfers/transfers-page.tsx`, friends in `components/friends/` | |
| `profile.js` | done | `components/profile/profile-card.tsx`, `full-profile.tsx` | |
| `right.js` | done | `components/right/right-column.tsx` | |
| `rail.js` | done | `components/shell/server-rail.tsx` | |
| `settings-voice.js` | done | `components/settings/voice-page.tsx` | |
| `settings.js` | done | `components/settings/settings-page.tsx` and one page per section | |
| `sidebar.js` | done | `components/shell/sidebar.tsx`, `components/menus/conversation-menu.tsx`, `server-menu.tsx` | |
| `statusmenu.js` | partial | `components/shell/user-bar.tsx` (`StatusMenu`) | The address tag in the menu's header (`shortOf` in the old file) is not shown. |
| `status.js` | done | `components/network/net-status.tsx`, `format.ts` | |
| `switcher.js` | done | `components/dialogs/quick-switcher.tsx` | |
| `titlebar.js` | done | `components/shell/titlebar.tsx` | |
| `typing.js`, `typing-words.js` | done | `lib/typing.ts`, `lib/typing-words.ts`, `components/chat/typing-row.tsx` | |
| `unlock.js` | done | `components/startup/gate.tsx`, `lib/passphrase.ts` | |
| `updater.js` | done | `components/settings/update-dialog.tsx` | |
| `voice-menus.js`, `voice-prefs.js` | done | `components/voice/voice-menus.tsx`, `lib/voice.ts` | |
| `state.js` | done | `lib/store.ts` and the slices in `lib/` | |
| `main.js` | partial | `main.tsx`, `lib/events.ts`, `components/call/call-host.tsx` | The `?view=` routes are mock only (below). |
| `keyboard.js` | done | `components/keyboard-shortcuts.tsx` | |
| `mock-only views` (`?view=call`, `dm`, `files`, `incoming`, `joincall`, `contacts`, `transfers`, `onboarding`, `settings`, `group`) | mock only | none | Preview shortcuts in the old mock. Not ported. The driver opens each state by clicking. |

## Dialogs and modal surfaces

| Old | Status | ui-next |
|---|---|---|
| `confirmDialog` | done | `lib/dialog.ts` `confirm()`, `components/dialogs/dialog-host.tsx` |
| `openServerDialog` (create and join) | done | `components/dialogs/server-dialog.tsx` |
| `openInviteDialog`, `openContactDialog` | done | `components/dialogs/invite-dialogs.tsx`, `link-dialog.tsx` |
| `openAddContactDialog` | done | `components/dialogs/small-dialogs.tsx` |
| `openCreateChannelDialog` | done | `components/dialogs/create-channel-dialog.tsx` |
| `openClaimHandleDialog` | done | `components/dialogs/small-dialogs.tsx` |
| `openSafetyDialog` | done | `components/dialogs/safety-dialog.tsx` |
| `openNewGroupDialog` | done | `components/dialogs/new-group-dialog.tsx` |
| `openNoteDialog` | done | `components/dialogs/small-dialogs.tsx` |
| `openFileSendDialog` | done | `components/dialogs/file-send-dialog.tsx` |
| `openSwitcher` | done | `components/dialogs/quick-switcher.tsx` |
| `openChannelSettings` | done | `components/channel-settings/channel-settings.tsx` |
| `chooseImage`, `openServerIconDialog` | done | `components/settings/avatar-editor.tsx`, `components/menus/server-menu.tsx` |
| `openUpdateDialog` | done | `components/settings/update-dialog.tsx` |
| `openFullProfile` | done | `components/profile/full-profile.tsx` |
| `showOnboarding` | done | `components/startup/onboarding.tsx` |
| Incoming call | done | `components/call/incoming.tsx` |

## Context menus

| Old call site | Status | ui-next |
|---|---|---|
| `chat.js`: the header's "..." menu | done | `MoreTools` in `components/chat/chat-header.tsx`, shared with the call and lobby headers |
| `chat.js`: message menu | done | `components/chat/message-menu.ts` |
| `sidebar.js`: server menu | done | `components/menus/server-menu.tsx` |
| `sidebar.js`: conversation menu | done | `components/menus/conversation-menu.tsx` (includes Start a call for DMs) |
| `statusmenu.js` | done | `components/shell/user-bar.tsx` |
| `voice-menus.js` | done | `components/voice/voice-menus.tsx` |

## Settings pages (`views/settings.js` NAV)

| Old section | Status | ui-next |
|---|---|---|
| My identity | done | `components/settings/account-page.tsx` |
| Recovery | done | `components/settings/recovery-page.tsx` |
| Devices | done | `components/settings/devices-page.tsx` |
| Contacts & privacy | done | `components/settings/privacy-page.tsx` (with the profile card preview) |
| Appearance | done | `components/settings/appearance-page.tsx` |
| Notifications & sounds | done | `components/settings/sounds-page.tsx` |
| Voice | done | `components/settings/voice-page.tsx` |
| Network | done | `components/settings/network-page.tsx` |
| Updates | done | `components/settings/updates-page.tsx` |
| Settings search | done | `components/settings/settings-page.tsx` |

## Startup

| Old | Status | ui-next |
|---|---|---|
| Portable gate and chooser | done | `components/startup/gate.tsx` |
| First run identity | done | `components/startup/onboarding.tsx` |

## Libraries and assets

| Old | Status | ui-next |
|---|---|---|
| `api.js` | done | `lib/api.ts` (every command) |
| `features.js` | done | `lib/features.ts` |
| `unread.js` | done | `lib/unread.ts` |
| `avatars.js` | done | `lib/images.ts` |
| `appearance.js` | done | `lib/appearance.ts` |
| `sounds.js` | done | `lib/sounds.ts`, `lib/sound-settings.ts`, `lib/calls.ts` (call sounds) |
| `osnotify.js` | done | `lib/osnotify.ts`, `lib/alerts.ts` |
| `qr.js` | done | `lib/qr.ts` |
| `address.js` | partial | `lib/address.ts` is a copy of `ui/address.js`. It moves with the cut-over (see below). |
| `ui.js` | done | `lib/identity.ts`, `components/user-avatar.tsx`, `lib/toasts.ts`, `lib/dialog.ts`, `lib/clipboard.ts`, `lib/text.ts`, `lib/time.ts` |
| `icons.svg` | done | `lucide-react` icons |
| `style.css` | partial | Tokens in `index.css`, shell rules in `shell.css`, chat in `components/chat/chat.css`, calls in `components/call/call.css`. Dialog rules are shadcn restyled, not the old class names. |
| `fonts/` | done | `public/fonts/` |
| `mock.js` | shared | `lib/mock.ts` loads `apps/desktop/ui/mock.js` in the browser preview and the driver |

## Known gaps across the tree

1. The call chat is empty in the real app, because `CallView` has no messages (backend).
2. Camera and screen share are disabled slots labelled "Coming soon" (video work).
3. Done: the status menu shows the address tag.
4. Group call matching uses `useCallRoom` (see the lobby row).
5. Done: `lib/mock.ts` loads `src/mock/mock.js`, and `src/mock/address.js` and `lib/address.ts` are copies of `apps/desktop/ui/address.js`, so nothing here imports the old UI. The copies stay in step until `apps/desktop/ui` is removed.
6. The old dialog and settings CSS classes are not all ported (the look is matched by shadcn restyling).
7. The app logs "Applying inline style violates the following Content Security Policy directive" once per modal open. `react-style-singleton` (via Radix Dialog's scroll lock) injects a `<style>` element, which the CSP blocks. The document doesn't scroll, so the blocked lock changes nothing visible. Left as is on purpose; the CSP is not widened.

## Cut-over prerequisites

- Done: the mock and address helpers are copied into `ui-next` (`src/mock/`), and `lib/mock.ts` points at them.
- Done: `tauri.conf.json` `frontendDist` is `ui-next/dist`, and the release workflow builds ui-next first.
- Not done, after v0.1.6 ships on ui-next: remove `apps/desktop/ui` once nothing imports it.

## Verification

- `npm run test:ui` runs `tests/driver.mjs` against the mock. Its check count is in the README.
- Not covered by the driver: the Tauri backend (real engine behaviour), the packaged build, and visual comparison beyond the screenshots in the scratchpad.
