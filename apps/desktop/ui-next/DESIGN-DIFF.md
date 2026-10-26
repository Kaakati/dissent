# DM view: design diff against the reference screenshot

Reference: `docs/Screenshot 2026-10-24 022128.png` (a Discord-style DM with a profile panel), 2089 x 1392. The values
in the reference column are read from the screenshot (about ±2 px). Before and after are measured on the mock, at the same
window size, with Maya's DM open and the profile panel shown. Dissent's own tokens (`index.css`) and the current UI's
sizes stay where they are unless a row says "fixed". Discord branding, Nitro, gifts and quests are not copied.

| Region | Reference | Before (ui-next) | After | Decision |
|---|---|---|---|---|
| DM list row | 44 px high, avatar 32 px, name 15 px semibold | 42 px, avatar 32 px, name 15 px medium | unchanged | Kept: 42 px is the current UI's `--row-dm`. |
| DM list name weight | semibold | medium (500) | unchanged | Kept: the current UI's row weight. |
| Sidebar width | ~253 px | 240 px | unchanged | Kept: `--sidebar-w`. |
| Header height | ~50 px | 56 px | unchanged | Kept: `--header-h` (56). |
| Header title | Name, bold, with avatar | Avatar 20 px + name 15 px semibold | unchanged | Kept. |
| Header tools | call, video, pin, members, profile toggle, then search | call, members, profile toggle, update, then search | unchanged | Kept: Dissent has no video call button yet and no pin tool here. |
| Header search width | ~230 px | 232 px | unchanged | Matches. |
| Message avatar | ~36 px | 32 px | unchanged | Kept: the current UI's `--avatar-msg` (40) is larger; matches the message layout. |
| Message name and time | Name bold, time muted, same line | Name in the role colour, time muted, same line | unchanged | Matches. |
| Link card | Title link, description, image | Title link, description, image | unchanged | Matches. |
| Rich card | Tile and label | Small tile beside the title (32 px) | unchanged | Matches the shared fixture change. |
| Composer height | ~46 px | 47 px | unchanged | Matches. |
| Composer tools | emoji, GIF, sticker, gift | emoji only | unchanged | Kept: Dissent has no GIF, sticker or gift tools. |
| Profile banner | ~104 px | 120 px | unchanged | Kept: the current UI's `.banner` (120). |
| Profile avatar | ~90 px, thick ring | 80 px, ring 6 px | unchanged | Kept: `--avatar-xl` (80). |
| Profile name | ~18 px semibold | 23 px bold | 18 px semibold | Fixed: the name is subordinate to the banner, so the hierarchy reads better. |
| Profile handle line | regular 14 px | mono 12 px address tag | unchanged | Kept on purpose: the address tag is Dissent's identity display (`dsu1…`). |
| Mutual line | icon, "N Mutual Friend • N Mutual Servers" | link text "2 Mutual Servers", no icon | "N Mutual Friend(s) • N Mutual Servers", a stack of up to three pictures, 14 px medium | Fixed: the friends count, with its stacked pictures, sits beside the servers count, and clicking it lists the friends. Friends appear only for contacts who share their list, and the line is hidden when there are none (see `mutual-friends.tsx`). |
| Member since block | label + date | "Contact since" label (11 px semibold) + date (14 px) | unchanged | Kept: Dissent says "Contact since". |
| View Full Profile button | 36 px high, full width | 40 px high, full width | unchanged | Kept: the current UI's button height (`--btn-h`). |
| User bar | avatar, name, status, icons | avatar, name, status, mic, headphones, settings | unchanged | Matches. |

## Not done, and why

- **Banner, avatar and header heights, and the row height.** These are the current UI's own tokens, and Dissent keeps
  its design. The reference is a different product's layout.
