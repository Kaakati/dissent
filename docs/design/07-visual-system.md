# 07. Visual system and spacing audit

The desktop UI (`apps/desktop/ui`) was measured in headless Edge at 1600×1000 (plus 900, 1280 and 2560 wide) with a CDP driver that reads `getBoundingClientRect` and computed styles for every surface. Reference values come from the two layout references in `docs/` (a DM view and a server view of a mainstream chat client), normalised to the 48 px rail pitch, and from the fixed sizes requested for this redesign: channel row 32, DM row 42, member row 42, rail item 48 + 8 gap, 4 px grid, hit targets ≥ 32.

Part 1 is the audit of the UI before the redesign. Part 2 is the visual system that replaces it, implemented as custom properties at the top of `style.css`. Part 3 is the before/after record.

## Part 1. Audit (before)

Measurements are at 100 % font scale, Cozy density, 1600 px wide unless noted. "ref" is the target value.

### Global

| # | Issue | Measured | Target |
|---|---|---|---|
| G1 | Line heights are unitless (`font: 16px/1.375`), so every smaller size inherits a fractional line box: 12 px text gets 16.5 px, 14 px gets 19.25 px, 15 px gets 20.625 px, 13 px gets 17.875 px, 20 px gets 27.5 px. Half-pixel positions then propagate down every column (section head at y 36.5, rows at 113.5 / 149.5 / 185.5, member rows at 52.5 / 131, group titles at 110.5). | 16.5 / 19.25 / 20.625 / 27.5 px | explicit 16 / 18 / 20 / 24 px line heights |
| G2 | Icons are `1em`, so their size follows the surrounding font: 14 px in sidebar rows and menus, 15 px in Settings nav, 16 px in headers and the composer, 12 px in the search box, 18 px in the update button. Six sizes, none on the 16/20/24 scale. | 12, 14, 15, 16, 18 px | 16 / 20 / 24 px tokens |
| G3 | Icon buttons (`.icon-btn`) are 24×24 (4 px padding + 16 px icon) in the chat header, user panel, dialog close and ID rows, and 20×20 in section heads. All below the 32 px hit target. | 20–24 px | 32 px |
| G4 | Two button heights: 34 px for filled buttons (6 + 22 + 6) and 36 px for `.ghost` and `.danger-outline` because their 1 px border is added on top. In the confirm dialog Cancel is 36 px at y 529 and Delete is 34 px at y 530, so the two labels sit 1 px apart. | 34 vs 36 px | one 32 px button with a transparent border reserved |
| G5 | `button:hover { filter: brightness(1.12) }` applies to every `<button>`, including transparent rows, icon buttons and author names, so hover brightens text instead of a surface. | global | hover only on filled buttons; rows use `--hover` |
| G6 | `button.danger` (filled red) also matches `.menu-item.danger`, so the "Leave server" menu item renders red text on a red background and is unreadable. | red on red | red text on ink, red fill only on hover |
| G7 | Accent `#e5533d` is used as text on `#313338` (send icon, "New messages", home and add rail glyphs): 3.39:1. White on the accent fill (primary buttons, menu hover): 3.73:1. Danger `#ed4245` as text on chat: 3.29:1; on elevated (hover toolbar delete icon): 2.96:1. Author colours `hsl(h 80% 74%)` fall to 4.12:1 for blue hues (h 240). Avatar initials, white on `hsl(h 45% 40%)`, fall to 3.21:1 for yellow hues (h 60). | 2.96–4.12:1 | ≥ 4.5:1 text, ≥ 3:1 icons |
| G8 | No `font-variant-numeric: tabular-nums` anywhere: timestamps, the rail badge, member counts, DHT numbers and the safety grid use proportional figures. | proportional | tabular |
| G9 | Native radio and checkbox controls in Settings use the browser default blue (`accent-color` unset) next to an orange accent. | blue | `accent-color: var(--accent)` |
| G10 | Ellipsis truncation exists on channel names, DM names, the chat title and the server name, but none of them carries a `title`, so the full text is unreachable. Member rows do. | partial | `title` on every truncated label |

### Server rail

| # | Issue | Measured | Target |
|---|---|---|---|
| R1 | Rail item 48 px with 8 px gap: pitch 56 px. Correct. The separator adds 4 + 2 + 4 px of its own margin on top of two 8 px gaps, so home-to-first-server is 26 px (home bottom y 60, first server y 86). | 26 px | 18 px (8 + 2 + 8); first server at y 78 |
| R2 | Unread badge sits at top-right (`top: 0; right: 2px`, 16×16, 11 px text) over the home button's edge. The references place the count at the bottom-right with a 2 px rail-coloured ring so it reads as attached to the icon. | top-right, no ring | bottom-right, 16 px min, 2 px ring |
| R3 | Rail glyph colours: home logo tinted `#e5533d` on `#313338` (3.39:1), plus glyph the same. | 3.39:1 | ≥ 3:1 for icons via `--accent-text` |
| R4 | Home button holds a 34 px logo image; the hover morph (24 → 16 px radius) is right. Tooltip at `left: 64px`, 14 px/500, on ink. Fine. | | keep |
| R5 | Pill indicator is a 4 px white bar: 8 px unread, 20 px hover, 40 px active. Matches the reference. The pill is a navigation indicator, not an accent border, and is kept. | | keep |

### Sidebar: header, section heads, rows

| # | Issue | Measured | Target |
|---|---|---|---|
| S1 | Server header (`.sidebar-top`) is 67 px tall (12 + 46 + 8 + 1 border) while the chat header next to it is 48 px, so the two top borders do not meet (y 67 vs y 48). In Home the find box top is 48 px, so the header height changes between Home and a server. | 67 / 48 px | 48 px in both modes |
| S2 | Server name button padding 12/8 inside an 8 px inset puts the name at x 88 but the caret at x 280–296, 8 px short of the 304 right edge used by rows. Caret icon 16 px. | | name x 88, caret 20 px ending at x 296 with 8 px padding, same as section-head buttons |
| S3 | Section head ("CHANNELS", "DIRECT MESSAGES"): padding 16/8/4, height 36.5 px, 12 px/16.5. Its action button is 20×20 at x 276–296. | 36.5 px | 40 px (24 top + 16 line), 32 px button whose icon ends at x 296 |
| S4 | Channel row height 34 px vs 32 px reference. Hash icon 14 px at x 88; `button:has(> .icon)` wins over `.row-item` and forces a 6 px gap, so the label starts at x 108. Font 14 px/19.25. | 34 px, 14 px icon | 32 px, 20 px icon, 14/18, label at x 114 |
| S5 | DM (`.convo`) row is 34 px tall with a 32 px avatar: 1 px of vertical padding, and the two-line text block (name 14/16.8 + preview 12/14.4 = 31.2 px) overflows the row by almost 0. Reference row is 42 px. The `.convo { height: 42px }` rule is overridden by `.row-item { height: 34px }` which is declared later. | 34 px | 42 px, 14/18 + 12/16 |
| S6 | Unread dot is 6 px at `left: -6px`, outside the row and inside the 8 px scroll padding; it is clipped to 2 px when the list scrolls. Unread channel name is bold 600 and `--text-strong`. Fine. | 6 px outside row | 8 px dot at x 74 (centred in the 8 px gutter) |
| S7 | Sidebar row left edges: section-head text x 88, channel icon x 88, DM avatar x 88, but the user panel avatar is at x 80 (`.me` has 8 px padding and is outside the 8 px scroll inset). | 80 vs 88 | 88 everywhere |
| S8 | Channel skeleton rows are 42 px (`.sidebar-scroll .skel-row`) while real channel rows are 34 (→ 32). | 42 px | 32 px for channels, 42 px for DMs and members |

### User panel

| # | Issue | Measured | Target |
|---|---|---|---|
| U1 | Panel 52 px: correct. Name 14 px/16.8 and suffix 12 px/14.4 stack to 31.2 px, so text is positioned at y 936.5 / 953 (half pixels). | 16.8 / 14.4 | 14/18 + 12/16 = 34, centred (9 px top) |
| U2 | Gear button 24×24 with a 16 px icon, 8 px from the right edge. | 24 px | 32 px button, 20 px icon, right edge at x 296 like the rows |
| U3 | Avatar at x 80 (see S7). | 80 | 88 |

### Chat header

| # | Issue | Measured | Target |
|---|---|---|---|
| H1 | Height 48 px, padding 0 16: correct. Title hash icon is 16 px; the reference uses a 24 px glyph. The DM avatar in the title is 32 px, 8 px from the top (y 7.5, half pixel). | 16 px | 24 px icon; 24 px avatar in DMs |
| H2 | Title 16 px/22 weight 700. Reference is 16/20 weight 600. | 700 / 22 | 600 / 20 |
| H3 | Member and panel toggles are 24×24 with 16 px icons. | 24 px | 32 px, 24 px icon |
| H4 | Search box 200×24 with a 12 px icon and 13 px text, `--rail` background. The header gap is 12 px. | 12 px icon, 13 px text | 16 px icon, 14 px text, 24 px tall, 144 px wide growing to 240 on focus is not needed; keep 200 |
| H5 | The update button is `position: fixed; top: 10px; right: 14px`. With no right column (Home before a DM is opened) it sits on top of the search box (x 1554–1586 vs search right edge 1584). With the member list open it floats over the member column. | overlaps | lives in the header tools, left of the search box |

### Messages

| # | Issue | Measured | Target |
|---|---|---|---|
| M1 | Group spacing: `.msg.start { margin-top: 16px }` vs 17 px reference. Within a group 2 + 2 px padding gives 26.5 px continuation rows (22 line + 4 + 0.5 from the hidden 12/16.5 gutter time). | 16 px, 26.5 px rows | 17 px, 26 px rows |
| M2 | Avatar 40 px at x 328 (16 px from the pane edge): correct. It starts at row y + 4 (2 px row padding + 2 px gutter padding) while the name line box starts at y + 2, so the avatar top is 2 px below the header top. | +4 | +2 (gutter padding 0) |
| M3 | Content column starts at x 384 (72 px): correct. Right padding is 16 px, so the hover toolbar (62×32 at `top: -14px; right: 16px`) overlaps the end of long lines. Reference reserves 48 px on the right. | 16 px | 48 px right padding; toolbar at right 16 |
| M4 | Hover toolbar buttons 28×28 with 16 px icons, bar background `--elevated` with no border. | 28 px | 32 px buttons, 20 px icons, 1 px border, 4 px radius |
| M5 | Author 16 px/22 weight 500 in `hsl(h 80% 74%)`; timestamp 12 px/16.5 with an 8 px gap. Baselines align (text bottoms 113 and 113), so only the fractional line height and the 8 px gap (reference 4 px, `margin-left: .25rem`) are off. | gap 8, 16.5 lh | gap 4 (6 is fine), 12/16, tabular |
| M6 | Gutter time on hover: 12 px, centred in the 72 px column, `padding-top: 4px`, text at y + 8 vs body text at y + 2. Reference: 11 px, right-aligned at 56 px, same line box as the body. | 12 px centred, 4 px low | 11 px/22, right-aligned to x 368, no offset |
| M7 | Message hover is `rgb(255 255 255 / .04)` (lighter). Reference darkens the row (`rgb(0 0 0 / .06)`), which keeps avatars and links from losing contrast. | lighter | darker |
| M8 | Date divider: 12 px/16.5 weight 600, margin 16/16/8, 1 px lines at `--border`. Reference: margin-top 24, bottom 8, 12 px/13 weight 600. | 16 top | 24 top, 8 bottom, 12/16 |
| M9 | "New messages" line: accent text 12 px/16.5 on chat, 3.39:1. Margin 8/16. | 3.39:1 | `--accent-text` (5.2:1), 12/16, keep the 8 px margins |
| M10 | Edited marker, pending note, deleted body all 12 px/16.5 muted italic. Fine; only line height. | 16.5 | 16 |
| M11 | Edit form: textarea 62 px (min 44) and a 36 px button row 8 px below; Save 34, Cancel 36 (G4). | | 32 px buttons, 8 px gap |

### Welcome block, empty states, skeletons

| # | Issue | Measured | Target |
|---|---|---|---|
| W1 | Channel welcome: 68 px mark with a 36 px glyph, h2 32 px/44 weight 700, p 16/22 muted, padding 16/16/24. The requested scale is 24/30 for welcome titles. | 32/44 | 24/30; mark 68 px with a 36 px glyph (kept; it is a mark, not a UI icon) |
| W2 | DM welcome: 80 px avatar, h2 32 px, suffix mono 14 px, two muted lines. Same fix. The first date divider then sits 16 px below (24 px after the fix). | 32/44 | 24/30 |
| W3 | Empty states ("Pick a conversation…", "No messages yet. Say hello.") are a single muted 16 px line centred with 24 px padding. No glyph, no secondary action. Sidebar empties ("No channels.", "No conversations yet.") are `.muted` paragraphs at 16 px inside a 14 px list. | plain text | glyph + title + one muted line; sidebar empties at 14/18 with 8 px padding |
| W4 | Message skeleton rows are 56 px (padding 8) with 12 px lines 8 px apart; a real group is 17 + 48. The circle at x 328 and lines at x 384 line up with real rows. Sidebar skeletons are 42 px in both channel and DM lists (S8). | 56 px | 65 px pitch (17 + 48), name line 16 px and body line 16 px with 6 px gap; channel skeleton 32 px |

### Composer

| # | Issue | Measured | Target |
|---|---|---|---|
| C1 | Host padding 0 16 24, composer 44 px min height, 8 px radius, `--elevated`: all match the reference. Textarea padding 11/8 centres 16/22 text exactly. | | keep |
| C2 | Plus and send buttons are 32×32 with 16 px icons; the reference uses 24 px glyphs in a 40 px wide slot (icon at x 16 from the composer edge). Plus icon at x 340 (12 px in). | 16 px icon | 24 px icon, button 32×32 at x 332 → icon x 336 |
| C3 | Plus menu width 220, `bottom: 52px`, items 35.5 px tall (8 px padding + 19.25), 14 px icon, 2 px item radius. Reference menu items are 32 px with 14/18 text and 16 px icons. | 35.5 px | 32 px, 14/18, 16 px icon |
| C4 | Server menu (same `.menu`): items 35.5 px; third item unreadable (G6). | | same as C3 |

### Member list

| # | Issue | Measured | Target |
|---|---|---|---|
| L1 | Column 240 px, padding 16/8. Group title 12 px/16.5 weight 600 with margin 16/4, so the first title sits at y 32 and later ones 16 px under the previous row (reference: 24 px above, 4 below, 40 px head like the channel list). | 16.5 lh, 16 top | 12/16, 24 top, 4 bottom |
| L2 | Member row 42 px with a 32 px avatar at x 1376 (16 px in): correct. Name 16 px/22 weight 400; reference is 16/20 weight 500. Handle 12 px/16.5 muted. | 22 lh, 400 | 16/20, 500; handle 12/16 |
| L3 | Member hover `rgb(255 255 255 / .04)`: consistent with rows. Keep. | | keep |
| L4 | Role colours: owner `#f0b232` (7.3:1), admin `#ff8a72` (6.0:1), moderator `#7cb7ff` (6.6:1). All pass. | | keep, as tokens |

### DM profile panel

| # | Issue | Measured | Target |
|---|---|---|---|
| P1 | Column 340, banner 120, 80 px avatar at −40 with a 6 px ring, body padding 0 16: all match the reference. | | keep |
| P2 | Name h3 20 px/27.5 weight 700; suffix mono 13 px/17.875; h4 labels 12 px/16.5 weight 700 margin 16/4. | fractional | 20/24, 12/16 mono, 12/16 weight 600 |
| P3 | Mutual-server rows 30 px with 20 px mini icons and 9 px initials; handle rows 27.5 px. | 30 / 27.5 px | 32 px rows, 24 px mini icons with 10 px initials |
| P4 | The two action buttons are stacked with no gap (y 423–457 and 457–491) because `.profile-actions button { width: 100% }` makes them block-level without a row gap. They read as one two-row block. | 0 px gap | 8 px gap, 32 px buttons |

### Profile card

| # | Issue | Measured | Target |
|---|---|---|---|
| K1 | Card 300 px, banner 64, 64 px avatar at −32 with a 5 px ring, body padding 0 14 14. Reference card: 300 wide, 60 px banner, 80 px avatar, 16 px padding. | 64 px avatar, 14 px padding | 80 px avatar with a 6 px ring, 16 px padding |
| K2 | Name 18 px/24.75 weight 700, suffix 13 px, handle 14 px/19.25, meta 13 px/17.875, chip 20.5 px tall. | fractional | 20/24, 12/16 mono, 14/18, 12/16, chip 20 px |
| K3 | Action buttons 34 px full-width with an 8 px gap: fine apart from the height. "Message" primary is white on `#e5533d` (3.73:1). | 3.73:1 | darker fill `#d2452f` (4.53:1) |

### Quick switcher

| # | Issue | Measured | Target |
|---|---|---|---|
| Q1 | Dialog 560 px at 15 vh, ink surface, 8 px radius. Field 47 px (12 + 22 + 12 + 1). Items 42 px (8 + 20.625 + 8 + … ) with a 15 px label and 13 px meta; the DM rows use a 32 px avatar so they are 48 px and the list rhythm changes between channel and DM rows (42 vs 48). | 47 / 42 / 48 | field 48, every row 40 with a 24 px lead (avatar, server mark or 20 px hash), 16/20 label, 12/16 meta |
| Q2 | Hint 12 px/16.5 with 8/14 padding. | | 12/16, padding 8 16 |

### Settings (full page)

| # | Issue | Measured | Target |
|---|---|---|---|
| T1 | Nav items 34.5 px (7 + 20.625 + 7), 15 px text, 15 px icons at x 354.5; titles 12 px/16.5 with margin 14/6. | fractional | 32 px rows (6 + 20 + 6), 16/20 text, 20 px icons, titles 12/16 with margin 16/4 |
| T2 | Page title 20 px/27.5 weight 700 inside a 98.5 px sticky head (56 + 27.5 + 14 + 1). | 27.5 | 20/24; head 60 top + 24 + 16 bottom = 100 → 96 on the grid (56 + 24 + 16) |
| T3 | Blocks: padding 14/0, h2 12 px/16.5 margin 0 0 8. ID rows 36 px with a 90 px label at 14 px and 12 px mono value. Radio rows 32.5 px at 15 px. Net table 14 px/19.25. | 14 px padding, fractional | 16/0 padding, 12/16 heads, 14/20 body, rows on 4 px |
| T4 | Close button 36 px circle with `--border` outline; "ESC" label 11 px small-caps. | 11 px | 12/16 weight 600 |
| T5 | Inputs 40 px (8 + 22 + 8 + 2 border), 16 px text, `--rail` background: correct. Textareas resize vertically; fine. | | keep |
| T6 | At 900 px the nav column is 180 px and content padding 24 px; the layout holds. | | keep |

### Dialogs, toasts, status bar

| # | Issue | Measured | Target |
|---|---|---|---|
| D1 | Dialog 460 px, 8 px radius, head padding 16 16 0, body 12 16 16, title 20 px/27.5. Reference modal is 440 px with 16 px padding, a 20/24 title and a footer band in `--sidebar`. | 460, 27.5 | 440, 20/24, 16 px padding, footer band |
| D2 | Tabs 38 px with 8/4 padding and a 2 px inset underline; the tab list has a 12 px bottom margin. | 38 | 40 px tabs, padding 8 0 10, 16 px gap |
| D3 | Close button 24×24 (G3). | | 32 |
| D4 | Safety grid cells 38 px, 16 px mono, 8 px gap. | 38 | 40 px, tabular |
| D5 | Update dialog progress bar 8 px with a green fill (`#23a55a`, 3.97:1 on chat, fine for a bar); the "Later" ghost button is 36 px next to a 34 px primary (G4). | | 32 px both |
| D6 | Toasts 356 px, 14 px/19.25, 10/12 padding, 1 px danger or accent border, ink background. Error text uses `--danger` on ink (4.88:1, passes). | 19.25 | 14/20, 12/16 padding, 8 px radius |
| D7 | Status bar 22 px, 12 px/16.5, padding 2 12. | 22 px | 24 px, 12/16, padding 4 12, tabular figures |

### Density and widths

| # | Issue | Measured | Target |
|---|---|---|---|
| X1 | Compact: message gutter 48 px with a 28 px avatar at x 322 (10 px inset vs 16 in Cozy), so avatars and the left edge jump when density changes. The hover time ("10:03 AM", 44–50 px at 12 px) does not fit the 48 px gutter and wraps, making that row 37 px. | wraps | gutter 56 (16 + 28 + 12), 11 px time, no wrap |
| X2 | Compact rows: channel 28 px, group margin 10 px. Both fine; expressed as token overrides. | | tokens |
| X3 | 900 px: right column hides under 1000 px, search shrinks to 140 px, composer 556 px. Holds. 2560 px: messages run 1920 px wide; same as the reference, which also has no measure cap. Holds. | | keep; cap nothing |

## Part 2. Visual system

Everything below is a custom property on `:root` in `style.css`. Compact density overrides a handful of them under `html[data-density="compact"]`; nothing else about density lives outside the tokens.

### Grid and spacing

4 px base grid. `--s-1: 4px`, `--s-2: 8px`, `--s-3: 12px`, `--s-4: 16px`, `--s-5: 20px`, `--s-6: 24px`, `--s-8: 32px`, `--s-10: 40px`, `--s-12: 48px`. Column insets: rail 12, sidebar and member list 8 (plus 8 inside rows, so content starts at 16), chat 16, settings 60 (24 under 900 px).

### Type scale

| Token | Size / line | Use |
|---|---|---|
| `--t-body` | 16 / 22 | message bodies, inputs, dialog text, member names (20 line) |
| `--t-ui` | 14 / 18 | sidebar rows, menus, buttons, toasts, settings body (20 line) |
| `--t-meta` | 12 / 16 | timestamps, section heads, hints, status bar, suffixes |
| `--t-tiny` | 11 / 16 | gutter time, rail badge |
| `--t-head` | 20 / 24 | chat and page titles, card names, dialog titles |
| `--t-title` | 24 / 30 | welcome titles |

Weights: 400 body, 500 names and menu labels, 600 headings, section heads and buttons, 700 welcome titles. Times, counts and IDs use `font-variant-numeric: tabular-nums`.

### Rows, icons, hit targets

| Token | Value |
|---|---|
| `--row-channel` | 32 px (28 compact) |
| `--row-dm`, `--row-member` | 42 px |
| `--row-menu`, `--row-nav`, `--row-switcher` | 32 px |
| `--rail-item`, `--rail-gap` | 48 px, 8 px |
| `--header-h` | 48 px |
| `--panel-h` | 52 px (user panel) |
| `--icon-sm`, `--icon`, `--icon-lg` | 16, 20, 24 px |
| `--hit` | 32 px (every icon button) |
| `--avatar-sm`, `--avatar`, `--avatar-msg`, `--avatar-card`, `--avatar-xl` | 24, 32, 40 (28 compact), 80, 80 px |
| `--msg-gutter` | 72 px (56 compact) |
| `--msg-group-gap`, `--msg-pad` | 17 px (10 compact), 2 px |

### Radius, elevation

`--r-1: 4px` (rows, buttons, inputs, menus), `--r-2: 8px` (composer, dialogs, cards, toasts), `--r-pill: 999px`, `--r-rail: 24px → 16px` on hover/active. Shadows: `--shadow-1` (toolbars, menus) `0 2px 8px rgb(0 0 0 / .3)`, `--shadow-2` (popovers, cards) `0 8px 24px rgb(0 0 0 / .4)`, `--shadow-3` (dialogs, switcher) `0 16px 48px rgb(0 0 0 / .5)`.

### Surfaces and colour

| Token | Value | Contrast notes |
|---|---|---|
| `--rail` | `#1e1f22` | |
| `--sidebar` | `#2b2d31` | |
| `--user-bar` | `#232428` | |
| `--chat` | `#313338` | |
| `--elevated` | `#383a40` | composer, toolbars, secondary buttons |
| `--ink` | `#111214` | menus, tooltips, cards, toasts |
| `--text` | `#dbdee1` | 9.4:1 on chat |
| `--text-strong` | `#f2f3f5` | 11.4:1 on chat |
| `--muted` | `#a0a6b0` | 5.2:1 on chat, 4.6:1 on elevated |
| `--accent` | `#d2452f` | fill; white on it 4.5:1 |
| `--accent-hover` | `#c23f2a` | fill hover; white 5.2:1 |
| `--accent-text` | `#ff8266` | accent as text or icon on dark: 5.2:1 on chat, 4.7:1 on elevated |
| `--danger` | `#d33639` | fill; white 4.8:1 |
| `--danger-text` | `#fa7679` | 4.8:1 on chat, 4.3:1 on elevated (icons) |
| `--warn` | `#f0b232` | 6.7:1 on chat |
| `--success` | `#23a55a` | icons and bars only (4.0:1) |
| `--link` | `#7cb7ff` | 6.1:1 |
| `--role-owner / admin / mod` | `#f0b232`, `#ff8a72`, `#7cb7ff` | 7.3 / 5.5 / 6.1:1 |
| `--hover` | `rgb(255 255 255 / .05)` | rows |
| `--selected` | `rgb(255 255 255 / .09)` | rows |
| `--hover-msg` | `rgb(0 0 0 / .06)` | message rows darken |
| `--border` | `rgb(255 255 255 / .06)` | |
| `--focus` | `--accent-text` | 2 px ring, 2 px offset |

Per-person colour keeps the hashed hue from `hueOf` but maps it through OKLCH so lightness is the same for every hue: author names `oklch(80% 0.12 h)` (worst hue 6.4:1 on chat, was 4.1:1), avatar fills `oklch(48% 0.14 h)` with white initials (worst 5.6:1, was 3.2:1), banners `oklch(42% 0.10 h)`. The orange accent is the brand mark's colour family; the UI uses it for fills and focus, and `--accent-text` for anything that is text or a glyph on a dark surface.

### Motion

Only `transform` and `opacity` animate, plus `border-radius` and `background-color` on the rail morph. 150 ms ease-out. `prefers-reduced-motion` removes the rail morph transition and the skeleton shimmer.

## Part 3. Before and after

Measured again after the redesign with the same driver. Every value below is at 1600×1000, Cozy, 100 %.

| Surface | Issue (before) | Fix (after) |
|---|---|---|
| Global type | 12/16.5, 14/19.25, 15/20.625, 20/27.5 line boxes; half-pixel rows everywhere | explicit 16/22, 14/18, 12/16, 11/16, 20/24, 24/30; every measured row and text box lands on a whole pixel |
| Icons | 1em: 12–18 px depending on context | 16 (next to a label), 20 (rows, icon buttons), 24 (chat header, composer) |
| Icon buttons | 20–24 px | 32 px hit target everywhere (`--hit`) |
| Buttons | 34 px filled vs 36 px outlined; Cancel/Delete 1 px apart | 32 px with a reserved 1 px border; dialog footer buttons share a baseline |
| Colour | accent text 3.39:1, white on accent 3.73:1, danger text 3.29:1, blue author names 4.12:1, yellow avatars 3.21:1 | fill `#d2452f` (white 4.5:1), text `#ff8266` (5.2:1), danger split the same way, `oklch(80% .12 h)` names (worst 6.4:1), `oklch(48% .14 h)` avatars (worst 5.6:1) |
| Server menu | "Leave server" red on red | red text on ink; red fill with white text on hover only |
| Rail | separator added 26 px between home and the first server; badge top-right without a ring | 18 px (8 + 2 + 8), first server at y 78; badge bottom-right with a 2 px rail ring; "+" glyph 24 px |
| Sidebar header | 67 px in a server, 48 px in Home, top border 19 px below the chat header's | 48 px in both, borders drawn as inset shadows so content centres on whole pixels (server name 8–40, caret 20 px ending at x 298) |
| Section heads | 36.5 px, 20 px button | 40 px (text 16–32 within), 32 px button whose icon ends at x 298 |
| Channel rows | 34 px, 14 px hash at x 88 with a 6 px gap forced by `:has()` | 32 px, 20 px hash at x 88, label at x 114, 14/18 weight 500 |
| DM rows | 34 px with a 31.2 px text block; 42 px rule overridden | 42 px with 14/18 + 12/16, avatar at x 88, text at x 132 |
| Unread marker | 6 px dot outside the row | 4×8 pill at x 72, the same shape as the rail pill |
| User panel | avatar at x 80, gear 24 px, 16.8/14.4 text | avatar at x 88 (same column as the rows), gear 32 px at x 272–304, 14/18 + 12/16 centred in 52 px |
| Chat header | 16 px hash, 700 weight title, 24 px tool buttons, 12 px search glyph | 24 px hash at y 12, 16/20 weight 600 title, 32 px tools with 24 px icons at y 8, 16 px search glyph, 14 px search text |
| Update button | fixed at top-right; covered the search box with no right column | in the header tools between the toggle and the search box, 32 px round, 20 px icon |
| Messages | 16 px group gap, 26.5 px continuation rows, avatar at row + 4, 16 px right padding under the toolbar, 28 px toolbar buttons, 12 px centred gutter time 4 px low | 17 px gap, 26 px rows, avatar at row + 2 (same top as the name line), 48 px right padding, 32 px toolbar buttons with 20 px icons and a 1 px border, 11 px gutter time right-aligned to x 368 on the body line; hover darkens |
| Name and time | 8 px gap, 16.5 px time box | 6 px gap, 12/16 tabular time on the same baseline |
| Dividers | date 16 px above, 8 below; "New messages" at 3.39:1 | 24 above, 8 below, 12/16; "New messages" in `--accent-text` |
| Welcome | 32/44 title, 16/16/24 padding | 24/30 title, 24 px top padding, 12 px under the mark, 4 px between lines |
| Empty states | a single muted line | glyph (40 px) + title + one muted line, centred; sidebar empties 14/18 |
| Composer | 16 px glyphs 12 px in | 24 px glyphs 16 px in, text starts at x 384 like message bodies, 44 px, 8 px radius |
| Menus | 35.5 px items, 2 px radius, 14 px icons | 32 px items, 14/18 weight 500, 16 px icons, 6/8 menu padding |
| Member list | first title at y 32, 16/22 weight 400 names | first title text 16–32 (level with the chat title), others 40 px with 24 above; 16/20 weight 500 names, 12/16 handles |
| DM profile panel | 27.5 px name, two stacked buttons with no gap, 30 px server rows | 20/24 name, 12/16 mono suffix, 12/16 section labels, 32 px server rows with 24 px marks, actions pinned to the bottom with an 8 px gap |
| Profile card | 64 px avatar, 14 px padding, 18/24.75 name | 80 px avatar with a 6 px ring, 60 px banner, 16 px padding, 20/24 name, 20 px chip |
| Quick switcher | 47 px field, 42/48 px rows | 48 px field, 40 px rows with 24 px leads, 16/20 label, 12/16 meta |
| Settings | 34.5 px nav rows, 15 px text, 27.5 px title, 98.5 px head, 14 px blocks, blue native radios | 32 px rows with 20 px icons and 16/20 text, 20/24 title in a 96 px head, 16 px blocks with 12/16 labels, 32 px radio and checkbox rows in the accent colour; nav column rounded to a whole pixel |
| Dialogs | 460 px, 20/27.5 title, 12/16 body padding, no footer | 440 px, 20/24 title, 16 px padding, the action row becomes a `--sidebar` footer band; tabs 40 px with a 2 px `--accent-text` underline; safety cells 40 px tabular |
| Update dialog | 36 px "Later" next to a 34 px primary | both 32 px in the footer band; 8 px bar on `--success` |
| Toasts, status bar | 19.25 px toast text, 22 px status bar | 14/20 toasts with 12/16 padding and 8 px radius; 24 px status bar with tabular figures |
| Skeletons | 56 px message rows, 42 px channel rows | 48 px rows on the 17 px group gap, circle at x 328 and lines at x 384; channel 32, member and DM 42 |
| Compact | avatar 10 px in, hover time wrapped to two lines | avatar at x 328 like Cozy, gutter 56, 11 px time on one line, 28 px channel rows, 10 px group gap, all as token overrides |
| Truncation | no tooltips | `title` on channel names, DM names, the server name, the chat title, card names and mutual-server rows |

### Deliberately different from the references

- Chat title is 16/20 weight 600 (as in the references) rather than the 20/24 header size; 20/24 is used for page and dialog titles.
- Welcome titles are 24/30 as specified, where the references use 32 px.
- DM rows are 42 px as specified; the DM reference measures 44 px at 100 %.
- The user panel stays inside the sidebar column. The newer reference layout spans it under the rail, but the rail here carries the home and add buttons and the panel holds only a gear.
- The first member-list heading sits level with the chat title (text 16–32 px) instead of 24 px down, so the two columns share a top line.
- The rail pill is kept: it is a white navigation indicator, not an accent border on a row.
- Toasts stay bottom-right above the status bar; the references pop notices at the top centre.
- Header features that do not exist here (topic, pins, threads, voice) are not drawn.

## Part 4. Replies and reactions

Built against the engine contract that core2 will implement: `MessageItem.reply_to: { id, author_name, text | null }`, `MessageItem.reactions: [{ emoji, count, me }]`, and the commands `send_reply`, `react` and `unreact`. The UI feature-detects: reply and reaction affordances appear only when the channel's message items carry a `reactions` array (`view.social` in `views/chat.js`). Without it the channel renders exactly as before, and R in the message list does nothing. DMs never show them (the contract is channel-only).

| Piece | Size and placement |
|---|---|
| Hover bar | Reply (`reply`) and Add reaction (`smile-plus`) lead the bar, then Edit and Delete; 32 px buttons, 20 px icons |
| Reply reference | 22 px row above the message spanning both grid columns, 2 px below it; 16 px avatar (`.avatar.xs`), author in `--text`, one-line muted snippet, 14/18 weight 500; connector 2 px `--border-strong` from x 36 (avatar centre) across to x 68, curving down to the avatar top with a 6 px radius (x 30 to 52 in Compact); a reply always starts a new group |
| Missing original | "Original message was deleted" or "Original message is encrypted" in italic muted 14/18 (the optional `locked: true` on `reply_to` picks the second) |
| Jump | click scrolls the original to the centre, focuses it and fades a `--accent-text` overlay from 18 % to 0 over 1.2 s (opacity only, off under reduced motion) |
| Reply bar | 32 px band above the composer on `--elevated` with the top radii, "Replying to **Name**" 14/18 and a 32 px ✕; the composer loses its top radii while the bar shows; Esc in the composer cancels |
| Reaction pills | 24 px tall, 8 px radius, 6/8 padding, 16 px emoji in the system emoji font, tabular count 14/18; mine: `--accent-text` border and a 12 % tint; hover tooltip "Grace, Ken and 2 others" from the optional `names` (up to three) or the count and `me` |
| Add pill | 32 px wide `smile-plus` pill at the end of the row, shown on hover and focus-within |
| Picker | 304 px on `--ink`, 8 px padding, 32 px search, 8 × 36 px cells with 22 px emoji; 32 curated emoji with a name list; Down from the search enters the grid, arrows and Home/End move, Enter picks, Esc closes and returns focus to the opener; no emoji font or list is downloaded |
| Keyboard | the message list is focusable; Up/Down move focus between messages (a focused message shows its bar, so Tab reaches the actions); R replies to the focused message |

Contract notes for core2, all optional and already handled when absent: `reactions[].names` (up to three display names, for the tooltip), `reply_to.author` (root id, so the 16 px avatar takes the author's colour), `reply_to.locked` (true when the original is sealed, to say "encrypted" instead of "deleted"). The mock implements all three, and `window.__mockNoReactions = true` simulates a backend without the feature.

Screenshots: `social-1600/` (`server-top`, `server`, `hover-reactions`, `pill-hover`, `reply-bar`, `reply-typing`, `reply-sent`, `jump-flash`, `picker`, `picker-filtered`, `reacted`, `picker-from-pill`, `missing-originals`, `keyboard-reply`, `compact`, `unsupported-hover`).

## Part 5. File attachments

Built against the file-sharing contract: `MessageItem.attachments: [{ id, name, size, mime, state, progress, local_path, needed? }]` with `state` one of available, downloading, paused, downloaded, failed, disk_full, too_large; the cap is one constant, `MAX_FILE_BYTES = 10 * 1024 ** 3` in `views/attachments.js`, and every limit message derives from it; the commands `pick_files()` (returns `[{ path, name, size, mime }]`, `[]` on cancel), `send_files(communityId, channelId, paths, text?)`, `send_dm_files(peer, paths, text?)`, `download_attachment(communityId | null, messageId, attachmentId)`, `open_attachment(path)` and `reveal_attachment(path)` (the path must be inside the app's downloads directory); the events `dissent://attachment-progress { message_id, attachment_id, progress }`, `dissent://drag-over` (on enter and over), `dissent://drag-leave` (on leave and after a drop) and `dissent://files-dropped` with `[{ path, name, size, mime }]`, directories skipped. The desktop side of this (dialog, drag events, asset protocol for images, with a CSP `img-src` of `'self' asset: http://asset.localhost`) is implemented; the engine side is not yet. Feature detection: attach affordances (the "+" menu item, the drop overlay, chips) appear only when the open conversation's items carry an `attachments` array (`view.files` in `views/chat.js`); channels and DMs are detected separately. Nothing is downloaded automatically.

| Piece | Size and placement |
|---|---|
| "+" menu | "Upload a file" (`paperclip`) first; opens the backend's native dialog through `pick_files`; picked files are staged, not sent |
| Drop overlay | covers the chat pane at 92 % `--chat`, 150 ms opacity transition only; 520 px dashed `--accent-text` box, 40 px paperclip, "Drop to send to #channel" at 20/24 and "Up to 10 GB, end-to-end encrypted" at 14/18; driven only by the backend's `dissent://drag-over`, `dissent://drag-leave` and `dissent://files-dropped` events (the mock makes the same events from the browser's drag events) |
| Staged chips | a band above the composer on `--elevated` sharing the composer's radii (stacked under the reply bar when both show); chips 32 px tall, 4 px radius, 16 px type icon, name truncated with the full name in the tooltip, size 12/16, 32 px ✕; over-limit chips use the danger border and tint and read "over 10 GB", and the send button is disabled while one is staged |
| Image | `<img>` inside a 400 × 300 max box, 8 px radius, only when `local_path` exists; the source is `convertFileSrc(local_path)` from the backend, never a URL from the message; a 20 px lock badge sits bottom-right; SVG is never shown as an image |
| File card | 400 px max, 12 px padding, `--sidebar` surface with a 1 px `--border-strong` border, 8 px radius; 24 px type icon (image, audio, video, archive, code, text, file); name 14/18 weight 500 with the real extension in a 16 px badge (danger-coloured for programs); size 12/16, 12 px lock with "End-to-end encrypted", state text; actions as 32 px icon buttons: Download (available), Resume (paused), Retry (failed, disk_full), Open + Show in folder (downloaded); too_large shows "Too large to download (over 10 GB)", disk_full shows "Not enough disk space (needs 19.6 GB)" from the `needed` field |
| Progress | the shared `.progress` bar at 4 px, `--success` fill scaled with `transform: scaleX` only, `aria-valuenow`; the meta line shows transferred and total with tabular figures, "1.2 GB of 9.8 GB · 12%", and "Paused · …" while paused; sizes format as B, KB, MB or GB with one decimal |
| Opening programs | .exe .msi .bat .cmd .ps1 .js .vbs .scr .jar .app .dmg .sh (by the real last extension) open only after the confirm dialog "… is a program (.exe). Opening it runs it on this computer." |
| Messages | a file-only message has no empty text line; attachments stack under the body with 8 px gaps |

Mock: `#general` carries every state (downloaded image and PDF, available CSV and TXT, a ZIP that downloads in five progress steps, a 12.4 GB too-large ZIP, a 9.8 GB MKV paused at 12 % that resumes from there, a 9.6 GB TAR whose retry ends in disk_full again, an undownloaded PNG placeholder), `#off-topic` has `invoice.pdf.exe` and an SVG, the Grace DM has a downloaded image and a `notes.md` whose download fails (Retry). `pick_files` returns two files, three with `window.__mockPickBig = true` (one of 11.2 GB, over the cap). `window.__mockNoFiles = true` simulates a backend without file sharing.

Screenshots: `files-1600/` (`cards-top`, `cards-bottom`, `downloading`, `downloaded`, `large-states`, `resumed`, `exe-card`, `exe-warning`, `plus-menu`, `staged-over-limit`, `staged`, `sent-files`, `drop-overlay`, `dropped-chip`, `dm-files`, `dm-retry`, `compact`, `unsupported`).

### Screenshots

Kept outside the repo, in `C:/Users/Kaaka/AppData/Local/Temp/claude/H--Rust-Projects-Dissent/dcb79fca-de63-4c46-81d0-05654136d058/scratchpad/design/`:

- `before-1600/` and `after-1600/`: 47 surfaces each (`home-empty`, `dm`, `dm-top`, `dm-hover`, `dm-unread`, `dm-no-panel`, `server`, `server-top`, `server-hover-cont`, `server-hover-start`, `rail-hover`, `row-hover`, `section-head-hover`, `server-unread`, `new-messages`, `search-filter`, `composer-typing`, `composer-menu`, `server-menu`, `member-hover`, `profile-card`, `profile-card-author`, `msg-actions-own`, `msg-edit`, `dialog-confirm`, `server-no-members`, `channel-empty`, `switcher`, `switcher-filtered`, `dialog-server`, `dialog-server-join`, `dialog-invite`, `dialog-handle`, `dialog-safety`, `dialog-update`, `dialog-update-progress`, `dialog-add-contact`, `toasts`, `focus-ring`, `skeleton-server`, `server-compact`, `dm-compact`, `settings-account`, `settings-profile`, `settings-network`, `settings-appearance`, `settings-about`), each with a `.json` of measurements.
- `before-900/`, `after-900/`, `before-1280/`, `after-1280/`, `before-2560/`, `after-2560/`: `home-empty`, `dm`, `dm-top`, `server`, `server-top`, `settings-account`.
- `tauri-*.png`: the real app in loopback mode.
