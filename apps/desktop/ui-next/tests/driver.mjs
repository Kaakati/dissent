// The ui-next driver: every check of the React UI's key and mouse driver, run against the Vite dev server with the mock in
// headless Edge through the DevTools Input domain (real key events, not synthetic DOM events). One section per area, each
// with its own browser page. Run it with `npm run test:ui` (it starts Vite), or against a running server:
//   UI_PORT=1420 SCRATCH=<dir> node tests/driver.mjs
// Sections: core (the shell, chat, transfers, dialogs, menus, Settings), friends (Friends, DM panel, profile card,
// addresses), calls (lobby, incoming call, voice channels, voice popovers, Settings > Voice), startup (gate, onboarding,
// Settings). Exits 1 when any check fails.
import { mkdirSync } from 'node:fs';
import { tmpdir } from 'node:os';
import { join } from 'node:path';
import { launch } from './cdp.mjs';
import { addressOf } from '../src/mock/address.js';
import { typingParts } from '../src/lib/typing-words.ts';

const UI_PORT = process.env.UI_PORT ?? '1420';
const BASE = `http://127.0.0.1:${UI_PORT}`;
const SCRATCH = process.env.SCRATCH ?? join(tmpdir(), 'dissent-ui-driver');
mkdirSync(`${SCRATCH}/shots`, { recursive: true });
const sleep = (ms) => new Promise((r) => setTimeout(r, ms));

const results = [];
const check = (name, ok, detail = '') => {
  results.push({ name, ok: Boolean(ok), detail });
  console.log(`${ok ? 'PASS' : 'FAIL'} ${name}${detail ? ` :: ${detail}` : ''}`);
};

// Every section's page reports its console errors here; one check per section, so the list is not repeated.
// The wording past three names is a pure function of the names, so it is read here, not through the page (the mock has
// five typers at most).
const wordingOf = (names) => typingParts(names).map((p) => (typeof p === 'string' ? p : p.name)).join('');
check('the wording counts the people past three', wordingOf([]) === '' && wordingOf(['A', 'B', 'C', 'D', 'E', 'F', 'G']) === 'A, B, C and 4 more are typing…', JSON.stringify([wordingOf([]), wordingOf(['A', 'B', 'C', 'D', 'E', 'F', 'G'])]));

const IGNORED_CONSOLE = /ERR_NO_BUFFER_SPACE|Failed to load resource/;
const noConsoleErrors = (section, errors) => {
  const real = errors.filter((e) => !IGNORED_CONSOLE.test(e));
  check(`${section}: no console errors`, real.length === 0, real.slice(0, 3).join(' | '));
};

// The main shell: channels, messages, the transfers, the dialogs, the menus and Settings (batches A and D, and the rest).
async function section_core() {
const b = await launch({ url: `${BASE}/`, width: 1600, height: 1000, port: 9778, profile: `${SCRATCH}/kb-next-profile` });
const { send, ev, consoleErrors } = b;
await sleep(800);

// Real key presses. modifiers: Alt=1, Ctrl=2, Shift=8.
const KEYS = {
  Enter: { code: 'Enter', vk: 13, text: '\r' },
  Escape: { code: 'Escape', vk: 27 },
  ArrowDown: { code: 'ArrowDown', vk: 40 },
  ArrowUp: { code: 'ArrowUp', vk: 38 },
  ArrowLeft: { code: 'ArrowLeft', vk: 37 },
  ArrowRight: { code: 'ArrowRight', vk: 39 },
  Tab: { code: 'Tab', vk: 9 },
  F10: { code: 'F10', vk: 121 },
  k: { code: 'KeyK', vk: 75 },
  ',': { code: 'Comma', vk: 188, text: ',' },
};
async function press(key, modifiers = 0) {
  const k = KEYS[key] ?? { code: `Key${key.toUpperCase()}`, vk: key.toUpperCase().charCodeAt(0) };
  const base = { key, code: k.code, windowsVirtualKeyCode: k.vk, nativeVirtualKeyCode: k.vk, modifiers };
  await send('Input.dispatchKeyEvent', { type: 'rawKeyDown', ...base, text: k.text });
  if (k.text) await send('Input.dispatchKeyEvent', { type: 'char', ...base, text: k.text });
  await send('Input.dispatchKeyEvent', { type: 'keyUp', ...base });
  await sleep(250);
}
const typeText = async (text) => {
  await send('Input.insertText', { text });
  await sleep(250);
};
const clickRail = (label) => ev(`(() => {
  const n = document.querySelector('nav[aria-label="Servers and direct messages"] button[aria-label=${JSON.stringify(label)}]');
  if (!n) throw new Error('no rail button named ${label}');
  n.click();
})()`);
// The sidebar rows are the buttons inside the channels column.
const SIDEBAR = 'aside[aria-label="Channels and conversations"]';
const rowByText = (text) => `[...document.querySelectorAll('${SIDEBAR} button')].find((b) => b.textContent.includes(${JSON.stringify(text)}))`;
const activeRow = () => ev(`document.querySelector('${SIDEBAR} button[aria-current="true"]')?.textContent ?? null`);
const waitForRow = async (name) => {
  for (let i = 0; i < 25; i++) {
    if (await ev(`!!${rowByText(name)}`)) return true;
    await sleep(200);
  }
  return false;
};
// Radix menu triggers open on pointerdown, so a menu button is pressed with real mouse events, not .click().
const pointerClick = async (expr) => {
  const pos = await ev(`(() => { const n = ${expr}; if (!n) throw new Error('no element: ' + ${JSON.stringify(expr)}); n.scrollIntoView({ block: 'nearest' }); const r = n.getBoundingClientRect(); return { x: r.left + r.width / 2, y: r.top + r.height / 2 }; })()`);
  await send('Input.dispatchMouseEvent', { type: 'mousePressed', x: pos.x, y: pos.y, button: 'left', clickCount: 1 });
  await send('Input.dispatchMouseEvent', { type: 'mouseReleased', x: pos.x, y: pos.y, button: 'left', clickCount: 1 });
  await sleep(250);
};
const menuItems = () => ev(`[...document.querySelectorAll('[role="menu"] [role^="menuitem"]')].map((n) => n.textContent.trim())`);
// The newest dialog's title: the dialogs are appended, and the channel settings overlay is not one of them.
const dialogTitle = () => ev(`[...document.querySelectorAll('[role="dialog"]:not([aria-label="Channel settings"]) h2, [role="alertdialog"] h2')].at(-1)?.textContent ?? null`);
const openDialogs = () => ev(`document.querySelectorAll('[role="dialog"]:not([aria-label="Channel settings"]), [role="alertdialog"]').length`);

// ---- 0. Esc in a DM returns to Home ----
await clickRail('Direct messages');
await sleep(400);
await waitForRow('Maya');
await ev(`${rowByText('Maya')}.click()`);
await sleep(500);
check('a DM is open', (await ev(`!!document.querySelector('${SIDEBAR} button[aria-current="true"]')`)) === true);
await press('Escape');
await sleep(400);
check('Esc in a DM leaves it for Home', (await ev(`!document.querySelector('${SIDEBAR} button[aria-current="true"]')`)) === true);

// ---- 1. Ctrl+K switcher: open, filter, ArrowDown, Enter ----
await ev(`document.querySelector('button[aria-label="Direct messages"]').focus()`);
await press('k', 2);
await sleep(250);
const opened = await ev(`!!document.querySelector('[role="dialog"] [cmdk-input]') && document.activeElement === document.querySelector('[role="dialog"] [cmdk-input]')`);
check('Ctrl+K opens the switcher with the field focused', opened === true);
await typeText('bo');
await sleep(200);
const labels = await ev(`[...document.querySelectorAll('[role="dialog"] [cmdk-item]')].map((n) => n.textContent.trim())`);
const GROUP = 'Book club organisers';
check('typing "bo" lists the Book Club server', labels.some((l) => l.includes('Book Club')), JSON.stringify(labels));
check('typing "bo" lists the group', labels.some((l) => l.includes(GROUP)), JSON.stringify(labels));
let steps = 0;
const selectedText = () => ev(`document.querySelector('[role="dialog"] [cmdk-item][data-selected="true"]')?.textContent?.trim() ?? null`);
while (!((await selectedText()) ?? '').includes(GROUP) && steps < labels.length) {
  await press('ArrowDown');
  steps++;
}
check('ArrowDown moves the selection onto the group', ((await selectedText()) ?? '').includes(GROUP), `after ${steps} presses`);
await press('Enter');
await sleep(400);
check('Enter closes the switcher', (await ev(`!document.querySelector('[role="dialog"] [cmdk-input]')`)) === true);
check('Enter opens the selected group', (await ev(`document.querySelector('header h1, header h2')?.textContent.startsWith('${GROUP}')`)) === true, await ev(`document.querySelector('header h1, header h2')?.textContent ?? null`));

// ---- 2. Esc closes the switcher and returns focus to the element that opened it ----
await clickRail('Riverside Makers');
await sleep(700);
await ev(`document.querySelector('button[aria-label="Riverside Makers menu"]').focus()`);
await press('k', 2);
await sleep(250);
await press('Escape');
await sleep(300);
const focusBack = await ev(`document.activeElement?.getAttribute('aria-label') ?? ''`);
check('Esc closes the switcher', (await ev(`!document.querySelector('[role="dialog"] [cmdk-input]')`)) === true);
check('Esc returns focus to the element that opened it', focusBack === 'Find or start a conversation' || focusBack === 'Riverside Makers menu', `active: ${focusBack} ${await ev(`document.activeElement?.outerHTML?.slice(0, 160) ?? null`)}`);

// ---- 3. Alt+Down / Alt+Shift+Down ----
await clickRail('Riverside Makers');
await sleep(700);
const first = await activeRow();
check('opening Riverside Makers selects #general', first === '#general' || (first ?? '').includes('general'), `active: ${first}`);
await press('ArrowDown', 1);
const next = await activeRow();
check('Alt+Down moves to the next text channel', next !== first && next !== null, `active: ${next}`);

// ---- 4. Rail ArrowDown moves focus to the next server ----
await ev(`document.querySelector('nav[aria-label="Servers and direct messages"] button[aria-label="Direct messages"]').focus()`);
await press('ArrowDown');
const r1 = await ev(`document.activeElement?.getAttribute('aria-label')`);
check('rail ArrowDown moves focus to the next item', r1 !== 'Direct messages' && r1 !== null, `focused: ${r1}`);

// ---- 5. Server menu: opens from the server name and lists the invite, handle and leave actions ----
await clickRail('Riverside Makers');
await sleep(600);
await pointerClick(`document.querySelector('button[aria-label="Riverside Makers menu"]')`);
await sleep(250);
const serverMenu = await menuItems();
check('the server menu offers Create invite, Claim handle, Copy server address and Leave server', ['Create invite', 'Claim handle', 'Copy server address', 'Leave server'].every((t) => serverMenu.some((l) => l.startsWith(t))), JSON.stringify(serverMenu));
await press('Escape');
await sleep(250);
check('Esc closes the server menu', (await ev(`!document.querySelector('[role="menu"]')`)) === true);

// ---- 6. New conversation menu, then the group dialog ----
await clickRail('Direct messages');
await sleep(400);
await pointerClick(`document.querySelector('button[aria-label="New conversation"]')`);
await sleep(250);
await ev(`[...document.querySelectorAll('[role="menuitem"]')].find((m) => m.textContent.includes('Create a group')).click()`);
await sleep(400);
check('New conversation offers "Create a group"', (await dialogTitle()) === 'Create a group', `title: ${await dialogTitle()}`);
check('the group dialog asks for three more people at the start', (await ev(`[...document.querySelectorAll('[role="dialog"] span')].some((s) => s.textContent === 'Pick 3 more')`)) === true);
for (let i = 0; i < 3; i++) await ev(`document.querySelectorAll('[role="dialog"] ul[aria-label="Contacts"] input[type="checkbox"]')[${i}].click()`);
await sleep(200);
check('three picks say "group of 4 with you"', (await ev(`[...document.querySelectorAll('[role="dialog"] span')].some((s) => s.textContent.includes('group of 4'))`)) === true);
check('the create button names the group size', (await ev(`[...document.querySelectorAll('[role="dialog"] button')].some((b) => b.textContent === 'Create group of 4')`)) === true);
await press('Escape');
await sleep(300);
check('Escape closes the group dialog', (await openDialogs()) === 0, `open: ${await openDialogs()}`);

// ---- 7. Context menu on a DM row, opened by right click and by Shift+F10 ----
await ev(`(() => { const row = ${rowByText('Maya')}; const r = row.getBoundingClientRect(); row.dispatchEvent(new MouseEvent('contextmenu', { bubbles: true, cancelable: true, clientX: r.left + 20, clientY: r.top + 10 })); })()`);
await sleep(300);
const dmMenu = await menuItems();
check('the DM context menu offers the safety number and contact actions', ['Compare safety number', 'Remove contact', 'Block'].every((t) => dmMenu.some((l) => l.startsWith(t))), JSON.stringify(dmMenu));
check('the first item takes the focus when the menu opens', (await ev(`document.activeElement?.getAttribute('role')?.startsWith('menuitem') === true`)) === true);
await press('Escape');
await sleep(200);
check('Escape closes the context menu', (await ev(`!document.querySelector('[role="menu"]')`)) === true);
await ev(`${rowByText('Maya')}.focus()`);
await press('F10', 8);
await sleep(300);
check('Shift+F10 on a focused row opens the same menu', (await ev(`!!document.querySelector('[role="menu"]')`)) === true);
await press('Escape');
await sleep(250);

// ---- 8. Submenus: Invite to server opens beside its row on hover and on ArrowRight ----
const mayaRowShown = await ev(`!!${rowByText('Maya')}`);
check('the Maya row is in the sidebar before the submenu checks', mayaRowShown, await ev(`location.href + ' | ' + [...document.querySelectorAll('aside[aria-label="Channels and conversations"] button')].map((n) => n.textContent.slice(0, 18)).join(' | ')`));
await ev(`(() => { const row = ${rowByText('Maya')}; const r = row.getBoundingClientRect(); row.dispatchEvent(new MouseEvent('contextmenu', { bubbles: true, cancelable: true, clientX: r.left + 20, clientY: r.top + 10 })); })()`);
await sleep(300);
const inviteBox = await ev(`(() => { const r = [...document.querySelectorAll('[role="menuitem"]')].find((m) => m.textContent.startsWith('Invite to server')); if (!r) return null; r.focus(); const b = r.getBoundingClientRect(); return { x: b.left + b.width / 2, y: b.top + b.height / 2 }; })()`);
check('Invite to server is in the DM menu', inviteBox !== null);
await press('ArrowRight');
await sleep(300);
const subOpen = await ev(`(() => { const s = document.querySelector('[role="menu"][data-state="open"] [role="menu"]') ?? [...document.querySelectorAll('[role="menu"]')].at(-1); return s ? [...s.querySelectorAll('[role^="menuitem"]')].map((n) => n.textContent.trim()) : null; })()`);
check('ArrowRight opens the Invite to server submenu', Array.isArray(subOpen) && subOpen.length > 0, JSON.stringify(subOpen));
await press('Escape');
await sleep(250);
await press('Escape');
await sleep(250);

// ---- 9. Mute from the conversation menu: a muted mark appears on the row until it is unmuted ----
await press('Escape');
await sleep(200);
await ev(`(() => { const row = ${rowByText('Maya')}; const r = row.getBoundingClientRect(); row.dispatchEvent(new MouseEvent('contextmenu', { bubbles: true, cancelable: true, clientX: r.left + 20, clientY: r.top + 10 })); })()`);
await sleep(300);
await ev(`(() => { const m = [...document.querySelectorAll('[role="menuitem"]')].find((x) => x.textContent.startsWith('Mute @')); m?.focus(); m?.click(); })()`);
await sleep(250);
// Opening the submenu, then choosing 15 minutes by keyboard.
await ev(`(() => { const m = [...document.querySelectorAll('[role="menuitem"]')].find((x) => x.textContent.startsWith('Mute @')); m?.focus(); })()`);
await press('ArrowRight');
await sleep(250);
await press('Enter');
await sleep(300);
check('a muted conversation shows the muted mark', (await ev(`!!${rowByText('Maya')}?.querySelector('[aria-label="Muted"]')`)) === true);
await ev(`(() => { const row = ${rowByText('Maya')}; const r = row.getBoundingClientRect(); row.dispatchEvent(new MouseEvent('contextmenu', { bubbles: true, cancelable: true, clientX: r.left + 20, clientY: r.top + 10 })); })()`);
await sleep(250);
await ev(`[...document.querySelectorAll('[role="menuitem"]')].find((x) => x.textContent.startsWith('Unmute @')).click()`);
await sleep(300);
check('unmuting removes the muted mark', (await ev(`!${rowByText('Maya')}?.querySelector('[aria-label="Muted"]')`)) === true);

// ---- 10. Create Channel: type radios, name cleaning, disabled until a name, Enter creates ----
// A server this user owns, so the owner-only controls show: created here, as the old driver does.
await clickRail('Create or join a server');
await sleep(300);
await ev(`document.querySelector('[role="dialog"] input[aria-label="Server name"]').focus()`);
await typeText('Settings test');
await ev(`[...document.querySelectorAll('[role="dialog"] button:not([role="tab"])')].find((b) => b.textContent === 'Create server').click()`);
await sleep(900);
await clickRail('Settings test');
await sleep(500);
await ev(`document.querySelector('button[aria-label="Create channel"]').click()`);
await sleep(300);
check('Create Channel opens with the title and the server name', (await dialogTitle()) === 'Create Channel' && (await ev(`[...document.querySelectorAll('[role="dialog"] p, [role="dialog"] div')].some((n) => n.textContent === 'in Settings test')`)) === true);
const radios = await ev(`[...document.querySelectorAll('[role="dialog"] input[name="channel-kind"]')].map((n) => n.value + ':' + n.checked)`);
check('the Text radio is checked by default', JSON.stringify(radios) === '["text:true","voice:false"]', JSON.stringify(radios));
const createDisabled = await ev(`[...document.querySelectorAll('[role="dialog"] button')].find((b) => b.textContent === 'Create Channel').disabled`);
check('Create Channel is disabled while the name is empty', createDisabled === true);
await ev(`document.querySelector('[role="dialog"] input[aria-label="Channel name"]').focus()`);
await typeText('Hello World!');
check('the text channel name is normalised as typed: hello-world', (await ev(`document.querySelector('[role="dialog"] input[aria-label="Channel name"]').value === 'hello-world'`)) === true);
check('the Private Channel switch is disabled', (await ev(`document.querySelector('[role="dialog"] input[role="switch"]').disabled === true`)) === true);
await ev(`document.querySelectorAll('[role="dialog"] input[name="channel-kind"]')[1].click()`);
await sleep(200);
await ev(`document.querySelector('[role="dialog"] input[aria-label="Channel name"]').focus()`);
await typeText(' Lounge');
check('a voice channel keeps the name as typed', (await ev(`document.querySelector('[role="dialog"] input[aria-label="Channel name"]').value === 'hello-worldLounge' || document.querySelector('[role="dialog"] input[aria-label="Channel name"]').value.endsWith('Lounge')`)) === true, await ev(`document.querySelector('[role="dialog"] input[aria-label="Channel name"]').value`));
await press('Escape');
await sleep(300);
check('Escape closes Create Channel', (await openDialogs()) === 0);

// ---- 11. Channel settings: hover actions, the overlay, save, invites and delete ----
await clickRail('Settings test');
await sleep(500);
const generalPos = await ev(`(() => { const b = ${rowByText('general')}; const r = b.getBoundingClientRect(); return { x: r.left + r.width / 2, y: r.top + r.height / 2 }; })()`);
await send('Input.dispatchMouseEvent', { type: 'mouseMoved', x: generalPos.x, y: generalPos.y });
// The hover style applies on a later frame, which a slower headless browser can take longer to reach: poll briefly.
let actionsShown = false;
for (let i = 0; i < 10 && !actionsShown; i++) {
  await sleep(200);
  actionsShown = await ev(`(() => { const a = document.querySelector('button[aria-label="Edit Channel"]'); return !!a && getComputedStyle(a.parentElement).display === 'flex'; })()`);
}
check('hovering a text channel shows the owner actions', actionsShown === true, await ev(`JSON.stringify({ media: matchMedia('(hover: hover)').matches, rowHover: [...document.querySelectorAll('aside[aria-label="Channels and conversations"] button')].filter((x) => x.textContent.includes('general')).map((x) => x.matches(':hover')), edit: [...document.querySelectorAll('button[aria-label="Edit Channel"]')].map((x) => getComputedStyle(x.parentElement).display), hit: (() => { const h = document.elementFromPoint(${generalPos.x}, ${generalPos.y}); return h ? h.tagName + ' ' + (h.getAttribute('aria-label') || h.textContent.slice(0, 20)) : null; })() })`));
// Keyboard and pointer-less users: Tab from "Create channel" lands on the first channel row, and its owner actions show
// there too (focus-within, not hover). The same actions are a menu on right click.
await ev(`document.querySelector('button[aria-label="Create channel"]').focus()`);
await press('Tab');
await sleep(300);
check('Tab from Create channel lands on the general channel row', (await ev(`document.activeElement?.textContent.includes('general') === true`)) === true, await ev(`document.activeElement?.textContent ?? null`));
check('the owner actions show on keyboard focus of a channel row', (await ev(`(() => { const a = document.querySelector('button[aria-label="Edit Channel"]'); return !!a && getComputedStyle(a.parentElement).display === 'flex'; })()`)) === true);
await ev(`(() => { const row = [...document.querySelectorAll('aside[aria-label="Channels and conversations"] button')].find((x) => x.textContent.includes('general')); const r = row.getBoundingClientRect(); row.dispatchEvent(new MouseEvent('contextmenu', { bubbles: true, cancelable: true, clientX: r.left + 20, clientY: r.top + 10 })); })()`);
await sleep(300);
const channelMenu = await menuItems();
check('right click on a channel row offers Create invite and Edit Channel', channelMenu.some((l) => l.startsWith('Create invite')) && channelMenu.some((l) => l.startsWith('Edit Channel')), JSON.stringify(channelMenu));
await press('Escape');
await sleep(200);
check('the actions are Create invite and Edit Channel', (await ev(`!!document.querySelector('button[aria-label="Create invite"]') && !!document.querySelector('button[aria-label="Edit Channel"]')`)) === true);
await ev(`document.querySelector('button[aria-label="Edit Channel"]').click()`);
await sleep(300);
check('Edit Channel opens the channel settings overlay', (await ev(`document.querySelector('[role="dialog"][aria-label="Channel settings"] h2')?.textContent.includes('general · Text Channels')`)) === true);
check('the settings nav lists Overview, Permissions, Invites and Delete Channel', (await ev(`JSON.stringify([...document.querySelectorAll('[aria-label="Channel settings sections"] button')].map((n) => n.textContent)) === JSON.stringify(['Overview', 'Permissions', 'Invites', 'Delete Channel'])`)) === true);
check('a text channel offers 14 slowmode choices, starting at Off', (await ev(`(() => { const o = document.querySelectorAll('select[aria-label="Slowmode"] option'); return o.length === 14 && o[0].textContent === 'Off' && o[13].textContent === '6h'; })()`)) === true);
check('the save bar is hidden with no changes', (await ev(`!document.querySelector('[aria-label="Unsaved changes"]')`)) === true);
await ev(`(() => { const i = document.querySelector('input[aria-label="Channel Name"]'); i.focus(); i.select(); })()`);
await typeText('Hello World');
check('the name is normalised as typed: hello-world', (await ev(`document.querySelector('input[aria-label="Channel Name"]').value === 'hello-world'`)) === true);
check('an edit shows the unsaved changes bar', (await ev(`document.querySelector('[aria-label="Unsaved changes"]')?.textContent.includes('Careful — you have unsaved changes!')`)) === true);
await ev(`[...document.querySelectorAll('[aria-label="Unsaved changes"] button')].find((b) => b.textContent === 'Reset').click()`);
await sleep(200);
check('Reset discards the edits and hides the bar', (await ev(`!document.querySelector('[aria-label="Unsaved changes"]') && document.querySelector('input[aria-label="Channel Name"]').value === 'general'`)) === true);
await ev(`(() => { const i = document.querySelector('input[aria-label="Channel Name"]'); i.focus(); i.select(); })()`);
await typeText('hello world');
await ev(`[...document.querySelectorAll('[aria-label="Unsaved changes"] button')].find((b) => b.textContent === 'Save').click()`);
await sleep(700);
check('Save renames the channel in the sidebar and hides the bar', (await ev(`!document.querySelector('[aria-label="Unsaved changes"]') && !!${rowByText('hello-world')}`)) === true);
await ev(`[...document.querySelectorAll('[aria-label="Channel settings sections"] button')].find((b) => b.textContent === 'Invites').click()`);
await sleep(300);
check('Invites offers Create invite', (await ev(`[...document.querySelectorAll('[role="dialog"][aria-label="Channel settings"] button')].some((b) => b.textContent === 'Create invite')`)) === true);
await ev(`[...document.querySelectorAll('[role="dialog"][aria-label="Channel settings"] button')].find((b) => b.textContent === 'Create invite').click()`);
await sleep(400);
check('Create invite opens the invite dialog', (await dialogTitle()) === 'Invite people');
await press('Escape');
await sleep(300);
check('Escape closes the invite dialog and keeps the overlay open', (await openDialogs()) === 0 && (await ev(`!!document.querySelector('[aria-label="Channel settings"]')`)) === true);
await ev(`(() => { const s = document.querySelector('select[aria-label="Slowmode"]') ?? null; s?.focus(); })()`);
await ev(`[...document.querySelectorAll('[aria-label="Channel settings sections"] button')].find((b) => b.textContent === 'Overview').click()`);
await sleep(150);
await ev(`(() => { const i = document.querySelector('input[aria-label="Channel Name"]'); i.focus(); i.select(); })()`);
await typeText('x');
await press('Escape');
await sleep(400);
check('Escape with unsaved edits asks before discarding', (await dialogTitle()) === 'Please confirm');
await press('Escape');
await sleep(300);
check('a declined discard keeps the overlay open', (await ev(`!!document.querySelector('[aria-label="Channel settings"]') && document.querySelectorAll('[role="alertdialog"]').length === 0`)) === true);
await ev(`[...document.querySelectorAll('[aria-label="Unsaved changes"] button')].find((b) => b.textContent === 'Reset').click()`);
await press('Escape');
await sleep(300);
check('Escape closes the overlay once nothing is unsaved', (await ev(`!document.querySelector('[aria-label="Channel settings"]')`)) === true);

// A voice channel: bitrate, user limit and relay, and no slowmode.
await ev(`document.querySelector('button[aria-label="Create voice channel"]').click()`);
await sleep(300);
await ev(`document.querySelector('[role="dialog"] input[aria-label="Channel name"]').focus()`);
await typeText('Lounge');
await press('Enter');
await sleep(700);
await ev(`(() => { const row = [...document.querySelectorAll('.group')].find((n) => n.textContent.includes('Lounge')); row?.querySelector('button[aria-label="Edit Channel"]')?.click(); })()`);
await sleep(300);
check('a voice channel offers bitrate, user limit and relay, not slowmode', (await ev(`(() => { const r = document.querySelector('input[aria-label="Bitrate"]'); return !!r && r.min === '8' && r.max === '96' && r.value === '64' && document.querySelector('input[aria-label="User Limit"]').max === '99' && !document.querySelector('select[aria-label="Slowmode"]'); })()`)) === true);
check('the relay choice starts at Automatic and offers the server relays', (await ev(`(() => { const o = [...document.querySelectorAll('select[aria-label="Relay"] option')].map((x) => x.textContent); return o[0] === 'Automatic' && o.includes('relay-eu.riverside.example'); })()`)) === true);
await press('Escape');
await sleep(300);

// Delete the renamed text channel after the confirmation.
await ev(`(() => { const row = [...document.querySelectorAll('.group')].find((n) => n.textContent.includes('hello-world')); row?.querySelector('button[aria-label="Edit Channel"]')?.click(); })()`);
await sleep(300);
await ev(`[...document.querySelectorAll('[aria-label="Channel settings sections"] button')].find((b) => b.textContent === 'Delete Channel').click()`);
await sleep(300);
check('Delete asks for confirmation first', (await dialogTitle()) === 'Please confirm');
await ev(`[...document.querySelectorAll('[role="alertdialog"] button')].find((b) => b.textContent === 'Delete Channel').click()`);
await sleep(700);
check('the deleted channel leaves the sidebar and the overlay closes', (await ev(`!${rowByText('hello-world')} && !document.querySelector('[aria-label="Channel settings"]')`)) === true);

// ---- F. Transfers page: tabs and row actions by key, auto-accept, the network line, the sidebar badge ----
await ev(`[...document.querySelectorAll('button')].find((x) => x.textContent.trim().startsWith('Transfers'))?.click()`);
for (let i = 0; i < 40 && !(await ev(`!!document.querySelector('[role="table"][aria-label="Transfers"]')`)); i++) await sleep(150);
await sleep(200);
const focusTab = (label) => ev(`(() => { const n = [...document.querySelectorAll('[role="tab"]')].find((x) => x.textContent.trim().startsWith('${label}')); if (!n) return false; n.focus(); return document.activeElement === n; })()`);
const focusLabelF = (label) => ev(`(() => { const n = document.querySelector('[aria-label="${label}"]'); if (!n) return false; n.focus(); return document.activeElement === n; })()`);
const selectedTabF = () => ev(`document.querySelector('[role="tab"][aria-selected="true"]')?.textContent.trim() ?? ''`);
const tabRowsF = () => ev(`document.querySelectorAll('[role="table"] [role="row"]').length - 1`);

const doneTabF = await focusTab('Completed');
await press('Enter');
check('F1 Completed tab opens by Enter', doneTabF && (await selectedTabF()).startsWith('Completed') && (await tabRowsF()) === 1, `rows: ${await tabRowsF()}`);
await focusTab('Active');
await press('Enter');
check('F2 Active tab opens by Enter', (await selectedTabF()).startsWith('Active'));

// Pause the first active row from the keyboard: the mock moves it to paused, so its button becomes Resume.
const pauseFocusedF = await focusLabelF('Pause');
await press('Enter');
await sleep(500);
check('F3 Pause by Enter turns the row into Resume', pauseFocusedF && (await ev(`document.querySelectorAll('[aria-label="Resume"]').length`)) >= 1);
await focusLabelF('Resume');
await press('Enter');
await sleep(500);

// Auto-accept: a 50 MB option takes focus and Enter like any button.
const acceptFocusedF = await ev(`(() => { const n = [...document.querySelectorAll('[role="group"] button')].find((x) => x.textContent.trim() === '50 MB'); if (!n) return false; n.focus(); return document.activeElement === n; })()`);
await press('Enter');
check('F4 auto-accept option takes focus and Enter', acceptFocusedF === true);

// Network line: hovering shows the tooltip with the DHT numbers, and Escape dismisses it.
const netCentre = await ev(`(() => { const r = document.querySelector('span.truncate[data-tauri-drag-region]')?.getBoundingClientRect(); return r ? { x: r.left + r.width / 2, y: r.top + r.height / 2 } : null; })()`);
if (netCentre) {
  await send('Input.dispatchMouseEvent', { type: 'mouseMoved', x: netCentre.x, y: netCentre.y });
  await sleep(400);
}
const tipF = await ev(`[...document.querySelectorAll('[data-slot="tooltip-content"]')].map((n) => n.textContent).join('|')`);
check('F5 network line tooltip lists the DHT size estimate', tipF.includes('DHT size estimate'), tipF.slice(0, 100));
await press('Escape');
check('F6 Escape closes the network tooltip', (await ev(`document.querySelectorAll('[data-slot="tooltip-content"]').length === 0`)) === true);

// The sidebar badge counts the active, paused and waiting transfers.
const transfersLinkF = await ev(`[...document.querySelectorAll('button')].find((x) => x.textContent.trim().startsWith('Transfers'))?.textContent ?? ''`);
check('F7 sidebar Transfers link shows the active count', /\d+ active/.test(transfersLinkF), transfersLinkF.trim());

// ---- 12. Settings shortcut: Ctrl+, opens Settings ----
await press(',', 2);
await sleep(400);
check('Ctrl+, opens Settings', (await ev(`!!document.querySelector('nav[aria-label="Settings sections"], [aria-label="Settings"]') || document.body.textContent.includes('My identity')`)) === true);

// ==== chat sections (batch A) begin ====
// ---- C1. DM header and search: the tools are there, typing filters and marks the matches, Esc clears the search ----
// (ported from keyboard-driver.mjs section 0b; the profile panel checks are the profile batch's)
await send('Page.navigate', { url: `${BASE}/` });
await sleep(2200);
await clickRail('Direct messages');
await sleep(400);
await ev(`${rowByText('Maya')}.click()`);
await sleep(600);
check('the DM header has Start voice call, Add friends to group and the profile toggle', (await ev(`['Start voice call with Maya', 'Add friends to group', 'Hide profile panel'].every(l => !!document.querySelector('button[aria-label="' + l + '"]'))`)) === true);
check('the DM search says "Search Maya"', (await ev(`document.querySelector('.search input')?.placeholder === 'Search Maya'`)) === true);
const dmTotal = await ev(`document.querySelectorAll('.msg').length`);
const dmFirst = await ev(`document.querySelector('.msg .body')?.textContent ?? ''`);
const dmWord = (dmFirst.trim().split(/\s+/).find((w) => w.length >= 4) ?? '').slice(0, 4);
await ev(`document.querySelector('.search input').focus()`);
await typeText(dmWord);
await sleep(250);
const dmShown = await ev(`document.querySelectorAll('.msg').length`);
const dmMarks = await ev(`document.querySelectorAll('mark.search-hit').length`);
check('typing in the DM search filters the messages and highlights the matches', dmShown >= 1 && dmShown < dmTotal && dmMarks >= 1, `word "${dmWord}": ${dmShown} of ${dmTotal} shown, ${dmMarks} marks`);
await press('Escape');
await sleep(300);
check('Esc clears the search, restores the messages and keeps the DM open', (await ev(`document.querySelector('.search input').value === '' && document.querySelectorAll('.msg').length === ${dmTotal} && document.querySelectorAll('mark.search-hit').length === 0 && document.querySelector('.chat-title h2')?.textContent === 'Maya'`)) === true);

// ---- C2. Replies and reactions in a channel: focus moves by keys, R replies, the picker takes Enter and Escape ----
// (ported from keyboard-driver.mjs section 6)
await send('Page.navigate', { url: `${BASE}/` });
await sleep(2200);
await clickRail('Riverside Makers');
await sleep(900);
await ev(`document.querySelector('.messages').focus()`);
await press('ArrowUp');
check('ArrowUp in the message list focuses the last message', (await ev(`document.activeElement?.classList.contains('msg')`)) === true);
await press('Tab');
const tabbed = await ev(`document.activeElement?.closest('.msg') !== null && document.activeElement?.tagName === 'BUTTON'`);
check('Tab from a focused message reaches a button inside it', tabbed === true, await ev(`document.activeElement?.getAttribute('aria-label') ?? document.activeElement?.className`));
const viaTab = await ev(`(() => { const m = document.activeElement?.closest('.msg'); return m ? [...m.querySelectorAll('.msg-actions button')].map(b => b.getAttribute('aria-label')) : []; })()`);
check('the focused message exposes Reply and Add reaction actions', viaTab.includes('Reply') && viaTab.includes('Add reaction'), JSON.stringify(viaTab));
await ev(`document.activeElement?.closest('.msg')?.focus()`);
await press('r');
check('R on a focused message opens the reply bar and focuses the composer', (await ev(`!document.querySelector('.reply-bar').hidden && document.activeElement === document.querySelector('.composer textarea')`)) === true);
await press('Escape');
check('Esc in the composer cancels the reply', (await ev(`document.querySelector('.reply-bar').hidden`)) === true);

await ev(`(() => { const m = [...document.querySelectorAll('.msg')].at(-1); m.focus(); m.querySelector('[aria-label="Add reaction"]').focus(); })()`);
await press('Enter');
await sleep(300);
check('Enter on Add reaction opens the picker with the search focused', (await ev(`document.activeElement?.classList.contains('emoji-search')`)) === true);
await press('ArrowDown');
await press('ArrowRight');
const cell = await ev(`document.activeElement?.classList.contains('emoji-cell') ? document.activeElement.textContent : null`);
check('ArrowDown then ArrowRight moves to the second emoji', cell === '👎', `cell ${cell}`);
await press('Enter');
await sleep(500);
const pill = await ev(`[...[...document.querySelectorAll('.msg')].at(-1)?.querySelectorAll('.reaction.mine') ?? []].map(p => p.textContent).find(t => t.startsWith('👎')) ?? null`);
check('Enter reacts: a highlighted pill appears on the message', pill === '👎1', `pill ${pill}`);
check('the picker closed after picking', (await ev(`!document.querySelector('.emoji-picker')`)) === true);
await ev(`[...document.querySelectorAll('.msg')].at(-1).querySelector('.reaction.add').focus()`);
await press('Enter');
await sleep(300);
await press('Escape');
check('Esc closes the picker and returns focus to the opener', (await ev(`!document.querySelector('.emoji-picker') && document.activeElement?.classList.contains('add')`)) === true);

// Unsupported backend: no affordances, R does nothing.
await ev(`window.__mockNoReactions = true`);
await clickRail('Direct messages');
await sleep(400);
await clickRail('Riverside Makers');
await sleep(900);
check('without a reactions array nothing social renders', (await ev(`document.querySelectorAll('.reply-ref, .reactions, [aria-label="Reply"], [aria-label="Add reaction"]').length`)) === 0);
await ev(`document.querySelector('.messages').focus()`);
await press('ArrowUp');
await press('r');
check('R does nothing when the backend lacks replies', (await ev(`document.querySelector('.reply-bar').hidden`)) === true);
await ev(`window.__mockNoReactions = false`);

// ---- C3. Typing: the row sits above the composer, names the typers in each wording, and keeps its height ----
// (ported from keyboard-driver.mjs section 8)
await send('Page.navigate', { url: `${BASE}/` });
await sleep(2200);
await clickRail('Direct messages');
await sleep(400);
await ev(`${rowByText('Maya')}.click()`);
await sleep(400);
const typingRow = () => ev(`document.querySelector('.typing-row')?.textContent ?? null`);
const rowHeight = () => ev(`document.querySelector('.typing-row')?.getBoundingClientRect().height ?? null`);
check('the typing row sits right above the composer and is polite-live', (await ev(`(() => {
  const r = document.querySelector('.typing-row');
  return !!r && r.getAttribute('aria-live') === 'polite' && r.nextElementSibling?.classList.contains('composer-host') === true;
})()`)) === true);
await ev(`window.__mockTypers(0)`);
await sleep(200);
const emptyHeight = await rowHeight();
check('the typing row is empty with nobody typing', (await typingRow()) === '', `text: ${JSON.stringify(await typingRow())}`);
const WORDINGS = [
  [1, ['Maya'], 'Maya is typing…'],
  [2, ['Maya', 'Jonas'], 'Maya and Jonas are typing…'],
  [3, ['Maya', 'Jonas', 'Leila'], 'Maya, Jonas and Leila are typing…'],
  [5, ['Maya', 'Jonas', 'Leila', 'Omar', 'Rana'], 'Maya, Jonas, Leila and 2 more are typing…'],
];
for (const [count, names, sentence] of WORDINGS) {
  await ev(`window.__mockTypers(${count})`);
  await sleep(250);
  const text = await typingRow();
  const bold = await ev(`[...document.querySelectorAll('.typing-row strong')].map(n => n.textContent)`);
  check(`${count} typer${count === 1 ? '' : 's'} reads "${sentence}"`, text === sentence, `text: ${JSON.stringify(text)}`);
  check(`${count} typer${count === 1 ? '' : 's'}: the names are bold`, JSON.stringify(bold) === JSON.stringify(names.slice(0, 3)), JSON.stringify(bold));
  check(`${count} typer${count === 1 ? '' : 's'}: the row keeps its height`, (await rowHeight()) === emptyHeight, `${await rowHeight()} vs ${emptyHeight}`);
}
await ev(`window.__mockTypers(0)`);

// ---- C4. Message menu: right click, Shift+F10, own-only Delete, the confirm, DM reactions and replies ----
// (ported from keyboard-driver.mjs section 8b; the menu is the shared one, so its items are read by role)
await send('Page.navigate', { url: `${BASE}/` });
await sleep(2200);
await clickRail('Direct messages');
await sleep(400);
await ev(`${rowByText('Maya')}.click()`);
await sleep(500);
const rightClickMsg = (text) => ev(`(() => {
  const row = [...document.querySelectorAll('.msg')].find(m => m.querySelector('.body')?.textContent.includes(${JSON.stringify(text)}));
  if (!row) throw new Error('no message: ' + ${JSON.stringify(text)});
  const r = row.getBoundingClientRect();
  row.dispatchEvent(new MouseEvent('contextmenu', { bubbles: true, cancelable: true, clientX: r.left + 40, clientY: r.top + 10 }));
})()`);
const shownMenuItems = () => ev(`[...document.querySelectorAll('[role="menu"] [role^="menuitem"]')].map(n => n.textContent.trim())`);
const clickMenuItem = (prefix) => ev(`[...document.querySelectorAll('[role="menu"] [role^="menuitem"]')].find(m => m.textContent.trim().startsWith(${JSON.stringify(prefix)})).click()`);

await rightClickMsg('Bring the ply on Saturday?');
await sleep(300);
const theirMenu = await shownMenuItems();
check('right-click on a message opens the message menu', theirMenu.length > 0 && theirMenu.some(l => l.startsWith('Reply')), JSON.stringify(theirMenu));
check('the menu on their message offers reaction, reply, copy text and copy ID', ['Add reaction', 'Reply', 'Copy text', 'Copy message ID'].every(p => theirMenu.some(l => l.startsWith(p))), JSON.stringify(theirMenu));
check('Delete is not offered on a message that is not yours', !theirMenu.some(l => l.startsWith('Delete')) && !theirMenu.some(l => l.startsWith('Edit')), JSON.stringify(theirMenu));
await clickMenuItem('Add reaction');
await sleep(300);
await ev(`[...document.querySelectorAll('.emoji-cell')].find(c => c.textContent === '👍').click()`);
await sleep(500);
const dmPill = await ev(`[...document.querySelectorAll('.msg')].find(m => m.querySelector('.body')?.textContent.includes('Bring the ply'))?.querySelector('.reaction.mine')?.textContent ?? null`);
check('a DM message takes a reaction from the menu', dmPill === '👍1', `pill ${dmPill}`);
await ev(`[...document.querySelectorAll('.msg')].find(m => m.querySelector('.body')?.textContent.includes('Bring the ply'))?.querySelector('.reaction.mine').click()`);
await sleep(500);
const unreacted = await ev(`![...document.querySelectorAll('.msg')].find(m => m.querySelector('.body')?.textContent.includes('Bring the ply'))?.querySelector('.reaction.mine')`);
check('clicking the reaction again takes it back off in a DM', unreacted === true);
await rightClickMsg('Bring the ply on Saturday?');
await sleep(300);
await clickMenuItem('Reply');
await sleep(250);
check('Reply from the menu shows the reply bar in a DM', (await ev(`!document.querySelector('.reply-bar').hidden && document.querySelector('.reply-bar').textContent.includes('Maya')`)) === true);
await typeText('Got it, thanks');
await press('Enter');
await sleep(600);
check('a DM reply shows the preview of the message it answers', (await ev(`[...document.querySelectorAll('.msg .reply-ref')].some(r => r.textContent.includes('Bring the ply on Saturday?'))`)) === true);

await rightClickMsg('Yes, reading it now.');
await sleep(300);
const ownMenu = await shownMenuItems();
check('Delete and Edit are offered on your own message', ownMenu.some(l => l.startsWith('Delete message')) && ownMenu.some(l => l.startsWith('Edit message')), JSON.stringify(ownMenu));
await clickMenuItem('Delete message');
await sleep(300);
const confirmText = await ev(`document.querySelector('[role="alertdialog"]')?.textContent ?? ''`);
check('Delete asks "Delete message? This can\'t be undone."', confirmText.includes("Delete message? This can't be undone."), confirmText);
await ev(`[...document.querySelectorAll('[role="alertdialog"] button')].find(b => b.textContent === 'Cancel').click()`);
await sleep(300);
check('Cancel keeps the message', (await ev(`[...document.querySelectorAll('.msg .body')].some(b => b.textContent.includes('Yes, reading it now.'))`)) === true);
await rightClickMsg('Yes, reading it now.');
await sleep(300);
await clickMenuItem('Delete message');
await sleep(300);
await ev(`[...document.querySelectorAll('[role="alertdialog"] button')].find(b => b.textContent === 'Delete').click()`);
await sleep(600);
check('confirming Delete replaces the text with "message deleted"', (await ev(`[...document.querySelectorAll('.msg .body.deleted')].some(b => b.textContent === 'message deleted')`)) === true);

// Shift+F10 on a focused message opens its menu, and the first item takes the focus.
await ev(`[...document.querySelectorAll('.msg')].find(m => m.querySelector('.body')?.textContent.includes('Bring the ply')).focus()`);
await press('F10', 8);
await sleep(300);
check('Shift+F10 on a focused message opens the menu', (await ev(`!!document.querySelector('[role="menu"]') && document.activeElement?.getAttribute('role')?.startsWith('menuitem') === true`)) === true);
await press('Escape');
await sleep(200);

// Shift+click on Delete message skips the confirm.
await rightClickMsg('All the photos from Saturday.');
await sleep(300);
const shiftAt = await ev(`(() => { const r = [...document.querySelectorAll('[role="menu"] [role^="menuitem"]')].find(m => m.textContent.trim().startsWith('Delete message')).getBoundingClientRect(); return { x: r.left + r.width / 2, y: r.top + r.height / 2 }; })()`);
await send('Input.dispatchMouseEvent', { type: 'mousePressed', x: shiftAt.x, y: shiftAt.y, button: 'left', clickCount: 1, modifiers: 8 });
await send('Input.dispatchMouseEvent', { type: 'mouseReleased', x: shiftAt.x, y: shiftAt.y, button: 'left', clickCount: 1, modifiers: 8 });
await sleep(600);
check('Shift+click on Delete message skips the confirm', (await ev(`!document.querySelector('[role="alertdialog"]') && [...document.querySelectorAll('.msg .body.deleted')].length >= 2`)) === true);

// ---- C5. Spoiler and age-restricted channels: spoiler text and media blur until clicked; an age gate asks once ----
// (ported from keyboard-driver.mjs section 7k; the mock sets the visibility, as the old driver does)
await send('Page.navigate', { url: `${BASE}/` });
await sleep(2200);
await ev(`window.__mockVisibility('c1', 'ch1', 'spoiler')`);
await clickRail('Riverside Makers');
await sleep(900);
const SPOILER_TEXT = 'Thanks all. The walkthrough video is in the attachments.';
check('a spoiler channel blurs its messages until they are clicked', (await ev(`(() => { const b = [...document.querySelectorAll('.body.spoiler-hidden')].find(n => n.textContent === ${JSON.stringify(SPOILER_TEXT)}); return !!b && b.getAttribute('role') === 'button' && b.tabIndex === 0; })()`)) === true);
const spoilerBefore = await ev(`document.querySelectorAll('.body.spoiler-hidden').length`);
await ev(`[...document.querySelectorAll('.body.spoiler-hidden')].find(n => n.textContent === ${JSON.stringify(SPOILER_TEXT)}).click()`);
await sleep(300);
check('a click reveals the spoiler message', (await ev(`document.querySelectorAll('.body.spoiler-hidden').length === ${spoilerBefore} - 1 && [...document.querySelectorAll('.body')].some(n => n.textContent === ${JSON.stringify(SPOILER_TEXT)} && !n.classList.contains('spoiler-hidden'))`)) === true);

// Attachments in a spoiler channel are blurred with the message text, and one click reveals them. A fresh load first: the
// text click above reveals its message's media too, as the old UI does.
await send('Page.navigate', { url: `${BASE}/` });
await sleep(2200);
await ev(`window.__mockVisibility('c1', 'ch1', 'spoiler')`);
await clickRail('Riverside Makers');
await sleep(900);
const mediaState = () => ev(`JSON.stringify({ att: document.querySelectorAll('.msg-content .attachments').length, blurred: document.querySelectorAll('.msg-content .attachments.spoiler-hidden').length })`);
const mediaBefore = JSON.parse(await mediaState());
check('a spoiler channel blurs attachments with their message', mediaBefore.att > 0 && mediaBefore.blurred === mediaBefore.att, JSON.stringify(mediaBefore));
const revealText = await ev(`(() => { const media = document.querySelector('.msg-content .attachments.spoiler-hidden'); const text = media.closest('.msg-content').querySelector('.body')?.textContent ?? ''; media.click(); return text; })()`);
await sleep(400);
const mediaAfter = JSON.parse(await mediaState());
check('a click reveals the clicked message media and its text', mediaAfter.blurred === mediaBefore.blurred - 1 && await ev(`(() => { const body = [...document.querySelectorAll('.body')].find(n => n.textContent === ${JSON.stringify(revealText)}); return !!body && !body.classList.contains('spoiler-hidden') && !body.closest('.msg-content').querySelector('.spoiler-hidden'); })()`) === true, JSON.stringify(mediaAfter));

// An age-restricted channel asks once, then stays open for this device.
await ev(`window.__mockVisibility('c1', 'ch2', 'age')`);
await clickRail('Riverside Makers');
await sleep(600);
await ev(`${rowByText('hardware')}.click()`);
await sleep(500);
check('an age-restricted channel shows the confirmation instead of messages', (await ev(`!!document.querySelector('.age-gate') && !document.querySelector('textarea[aria-label="Message"]')`)) === true);
await ev(`document.querySelector('.age-gate button').click()`);
await sleep(400);
check('confirming opens the channel and shows the composer', (await ev(`!document.querySelector('.age-gate') && !!document.querySelector('textarea[aria-label="Message"]')`)) === true);
check('the confirmation is remembered for this channel on this device', (await ev(`localStorage.getItem('age-ok:c1:ch2') === 'yes'`)) === true);
await ev(`${rowByText('general')}.click()`);
await sleep(300);
await ev(`${rowByText('hardware')}.click()`);
await sleep(400);
check('coming back to the channel does not ask again', (await ev(`!document.querySelector('.age-gate') && !!document.querySelector('textarea[aria-label="Message"]')`)) === true);
await ev(`window.__mockVisibility('c1', 'ch2', 'default')`);

// ---- C6. A downloading file: its name opens the transfer details in the right column; Close gives the column back ----
await send('Page.navigate', { url: `${BASE}/` });
await sleep(2200);
await clickRail('Riverside Makers');
await sleep(900);
// Starting a download of an available file makes it a downloading card (the mock reports its progress).
await ev(`(() => { const b = [...document.querySelectorAll('.att-card button[aria-label^="Download "]')][0]; if (!b) throw new Error('no available file'); b.click(); })()`);
await sleep(700);
const hasDownloading = await ev(`!!document.querySelector('.att-card.downloading .att-name-text')`);
if (hasDownloading) {
  await ev(`document.querySelector('.att-card.downloading .att-name-text').click()`);
  await sleep(400);
  check('a downloading file name opens its transfer details in the right column', (await ev(`!!document.querySelector('button[aria-label="Close transfer details"]')`)) === true);
  await ev(`document.querySelector('button[aria-label="Close transfer details"]').click()`);
  await sleep(300);
  check('closing the transfer details gives the right column back to the members', (await ev(`!document.querySelector('button[aria-label="Close transfer details"]') && /Members/.test(document.querySelector('aside[aria-label="Right column"], aside:last-of-type')?.textContent ?? document.body.textContent)`)) === true);
} else {
  check('a downloading file is in the Riverside channel to test the details', false, 'no .att-card.downloading in the mock');
}

// ==== chat sections (batch A) end ====
// ---- 13. Console ----
// ---- Composer emoji: the button opens the picker, and the pick lands at the caret ----
await ev(`document.querySelector('button[aria-label="Direct messages"]').click()`);
await sleep(400);
await ev(`[...document.querySelectorAll('aside[aria-label="Channels and conversations"] button')].find((x) => x.textContent.includes('Maya')).click()`);
await sleep(500);
await ev(`(() => { const t = document.querySelector('textarea[aria-label="Message"]'); t.focus(); t.value = 'ab'; t.setSelectionRange(1, 1); t.dispatchEvent(new Event('input', { bubbles: true })); })()`);
await sleep(200);
await ev(`document.querySelector('button[aria-label="Insert emoji"]').click()`);
await sleep(400);
check('the emoji button opens the picker, named for the composer', (await ev(`!!document.querySelector('[role="dialog"][aria-label="Insert emoji"] .emoji-grid button')`)) === true);
const beforeEmoji = await ev(`document.querySelector('textarea[aria-label="Message"]').value`);
await ev(`document.querySelector('[role="dialog"][aria-label="Insert emoji"] .emoji-grid button').click()`);
await sleep(300);
const afterEmoji = await ev(`document.querySelector('textarea[aria-label="Message"]').value`);
check('a picked emoji lands between the text before and after the caret', afterEmoji.length > beforeEmoji.length && afterEmoji.startsWith('a') && afterEmoji.endsWith('b'), JSON.stringify(afterEmoji));
await ev(`(() => { const t = document.querySelector('textarea[aria-label="Message"]'); t.value = ''; t.dispatchEvent(new Event('input', { bubbles: true })); })()`);

noConsoleErrors('core', consoleErrors);

await b.close();
}

// Friends, the DM profile panel, the profile card and addresses (batch B).
async function section_friends() {
// the DM profile panel, the full profile, the profile card, the conversation menu's Profile item, and the address checks.
// Ported from the Friends, DM panel, profile and address sections of keyboard-driver.mjs, on the ui-next DOM.


const b = await launch({ url: `${BASE}/`, width: 1600, height: 1000, port: 9851, profile: `${SCRATCH}/kb-b-profile` });
await sleep(2600);
// The copy buttons write to the clipboard; the headless browser needs the permission to answer.
await b.send('Browser.grantPermissions', { permissions: ['clipboardReadWrite', 'clipboardSanitizedWrite'], origin: BASE });

// The main column, the right column ("Details") and the sidebar are the regions the checks read.
const MAIN = 'main';
const RIGHT = 'aside[aria-label="Details"]';
const clickByText = (selector, text) => b.ev(`(() => { const n = [...document.querySelectorAll(${JSON.stringify(selector)})].find(x => x.textContent.trim().includes(${JSON.stringify(text)})); if (!n) return false; n.click(); return true; })()`);
const activeText = () => b.ev(`document.activeElement?.textContent?.trim() ?? ''`);
const bodyText = () => b.ev(`document.body.innerText`);
const toastShown = async (needle, tries = 12) => {
  for (let i = 0; i < tries; i++) {
    if ((await bodyText()).includes(needle)) return true;
    await sleep(200);
  }
  return false;
};

// ---- 0. Friends home ----
check('Home opens on Friends (the title and the quick link)', (await b.ev(`document.querySelector('h1')?.textContent === 'Friends'`)) === true);
const tabs = await b.ev(`[...document.querySelectorAll('[role="tablist"][aria-label="Friends"] [role="tab"]')].map(n => n.textContent.trim()).join('|')`);
check('the Friends tabs are Online, All and Pending with the count (no Blocked: nobody is blocked)', tabs === 'Online|All|Pending2', tabs);
check('the Online tab is selected first', (await b.ev(`document.querySelector('[role="tablist"] [role="tab"][aria-selected="true"]')?.textContent.trim() === 'Online'`)) === true);

await b.ev(`[...document.querySelectorAll('[role="tablist"] [role="tab"]')].find(n => n.textContent.trim() === 'Online').focus()`);
await b.press('Tab');
check('the tabs are keyboard reachable: Tab moves from Online to All', (await activeText()) === 'All', await activeText());
await b.press('Enter');
await sleep(200);
check('Enter on All selects it and lists "All friends"', (await b.ev(`document.querySelector('[role="tablist"] [role="tab"][aria-selected="true"]')?.textContent.trim() === 'All' && document.querySelector('main')?.textContent.includes('All friends — ')`)) === true);

const glyphs = await b.ev(`['Online', 'Idle', 'Do Not Disturb', 'Offline'].map(l => !!document.querySelector('main [role="img"][aria-label="' + l + '"]'))`);
check('friend rows show the four presence glyphs (online, idle, dnd, offline)', JSON.stringify(glyphs) === '[true,true,true,true]', JSON.stringify(glyphs));
check('the DND glyph is red (its colour token resolves)', (await b.ev(`getComputedStyle(document.querySelector('main [role="img"][aria-label="Do Not Disturb"]')).backgroundColor`)) === 'rgb(211, 54, 57)', await b.ev(`getComputedStyle(document.querySelector('main [role="img"][aria-label="Do Not Disturb"]')).backgroundColor`));
check('a friend in a call shows the call line under the name', (await b.ev(`[...document.querySelectorAll('main span')].some(n => n.textContent.startsWith('In a call · '))`)) === true);
check('the friend avatar is 32 px', (await b.ev(`document.querySelector('main [role="img"][aria-label="Online"]').parentElement.getBoundingClientRect().width`)) === 32);

await b.ev(`[...document.querySelectorAll('button')].find(n => n.textContent.trim() === 'Add Friend').focus()`);
await b.press('Enter');
await sleep(250);
check('Add Friend switches the body to the add form', (await b.ev(`!!document.querySelector('input[aria-label="Contact link"]')`)) === true);
check('the add form shares your contact link', (await bodyText()).includes('dissent://contact/'));
check('Add Friend is pressed while the form is open', (await b.ev(`document.querySelector('button[aria-pressed="true"]')?.textContent.trim() === 'Add Friend'`)) === true);

// A pasted address with a typo is refused before it is sent. Real key presses.
await b.ev(`document.querySelector('input[aria-label="Contact link"]').focus()`);
await b.typeText('dsu1qqqqqqqqqqqqqqqqqqqq');
await b.press('Enter');
check('a typo in a pasted address is refused with a friendly message', await toastShown('That address has a typo'));
await b.ev(`(() => { const i = document.querySelector('input[aria-label="Contact link"]'); i.focus(); i.select(); })()`);
await b.press('Backspace');
const serverAddress = addressOf('server', 'a'.repeat(52));
await b.typeText(serverAddress);
await b.press('Enter');
check('a server address pasted as a person is refused', await toastShown('That is a server address'));
check('the form keeps what was pasted when the request is refused', (await b.ev(`document.querySelector('input[aria-label="Contact link"]').value`)) === serverAddress);

await b.ev(`(() => { const i = document.querySelector('input[aria-label="Contact link"]'); i.focus(); i.select(); })()`);
await b.press('Backspace');
await b.typeText('dissent://contact/mock-pending');
await b.press('Enter');
await sleep(500);
check('a contact link sent from the form switches to Pending and clears the input', (await b.ev(`document.querySelector('[role="tablist"] [role="tab"][aria-selected="true"]')?.textContent.trim().startsWith('Pending') === true && !document.querySelector('input[aria-label="Contact link"]')`)) === true);

// ---- Pending ----
check('Pending shows Accept and Ignore on incoming and Cancel on outgoing', (await b.ev(`['Accept Nadia', 'Ignore Nadia', 'Accept Yusuf', 'Ignore Yusuf', 'Cancel request to Karim'].every(l => !!document.querySelector('button[aria-label="' + l + '"]'))`)) === true);
await b.ev(`document.querySelector('button[aria-label="Accept Nadia"]').click()`);
await sleep(700);
check('Accept takes the request out of Pending into friends', (await b.ev(`!document.querySelector('button[aria-label="Accept Nadia"]')`)) === true);
check('the Pending tab count drops to match (Yusuf is left)', (await b.ev(`[...document.querySelectorAll('[role="tablist"] [role="tab"]')].find(n => n.textContent.startsWith('Pending'))?.textContent === 'Pending1'`)) === true);
check('the Active Now column is the right column on Friends', (await b.ev(`!!document.querySelector('aside[aria-label="Active now"] h3') && document.querySelector('aside[aria-label="Active now"] h3').textContent === 'Active Now'`)) === true);
check('Active Now lists the friend in a call', (await b.ev(`[...document.querySelectorAll('aside[aria-label="Active now"] button')].some(n => n.textContent.includes('In a call'))`)) === true);

// Back to All, then a friend's More button opens the conversation menu with the Profile item.
await clickByText('[role="tablist"] [role="tab"]', 'All');
await sleep(250);
await b.ev(`document.querySelector('button[aria-label="More actions for Jonas"]')?.click()`);
await sleep(300);
const menuText = await b.ev(`document.querySelector('[role="menu"]')?.textContent ?? ''`);
check('More on a friend opens the conversation menu with Profile', menuText.includes('Profile') && menuText.includes('Mark as read'), menuText.slice(0, 80));
await b.press('Escape');
await sleep(250);

// Message opens the conversation; Esc in a DM returns to Friends.
await b.ev(`document.querySelector('button[aria-label="Message Jonas"]')?.click()`);
await sleep(600);
check('Message opens the direct conversation', (await b.ev(`!!document.querySelector('${RIGHT}') && !document.querySelector('[aria-label="Active now"]')`)) === true);
await b.press('Escape');
await sleep(400);
check('Esc in a DM returns to Friends', (await b.ev(`document.querySelector('h1')?.textContent === 'Friends'`)) === true);

// ---- 0b. DM profile panel ----
await clickByText('button', 'Maya');
await sleep(700);
check('the DM profile panel is open in the right column', (await b.ev(`document.querySelector('${RIGHT}')?.hidden === false && document.querySelector('${RIGHT}').textContent.includes('View full profile')`)) === true);
check('the DM panel avatar is 80 px with a presence glyph', (await b.ev(`(() => { const r = document.querySelector('${RIGHT} [role="img"][aria-label="Online"]'); return !!r && Math.round(r.parentElement.getBoundingClientRect().width) === 80; })()`)) === true);
const mutualText = await b.ev(`[...document.querySelectorAll('${RIGHT} button')].find(n => /Mutual Servers?$/.test(n.textContent.trim()))?.textContent.trim() ?? null`);
check('the mutual servers line reads "N Mutual Servers"', /^\d+ Mutual Servers?$/.test(mutualText ?? ''), String(mutualText));
check('the panel shows no mutual friends or contacts line', (await b.ev(`!/mutual (friend|contact)/i.test(document.querySelector('${RIGHT}').textContent)`)) === true);
const mutualCount = Number((mutualText ?? '0').split(' ')[0]);
await b.ev(`[...document.querySelectorAll('${RIGHT} button')].find(n => /Mutual Servers?$/.test(n.textContent.trim())).click()`);
await sleep(200);
check('clicking the mutual servers count lists that many servers', (await b.ev(`document.querySelectorAll('${RIGHT} ul li').length === ${mutualCount}`)) === true, `count ${mutualCount}`);
check('the panel shows Contact since with the date and year', (await b.ev(`(() => { const h = [...document.querySelectorAll('${RIGHT} h4')].find(n => n.textContent === 'Contact since'); return !!h && /[A-Z][a-z]{2} \\d{1,2}, \\d{4}/.test(h.nextElementSibling?.textContent ?? ''); })()`)) === true);

// Hide and show the panel from the chat header's toggle.
await b.ev(`document.querySelector('button[aria-label="Hide profile panel"]')?.click()`);
await sleep(300);
check('the header toggle hides the DM panel', (await b.ev(`document.querySelector('${RIGHT}')?.hidden === true`)) === true);
await b.ev(`document.querySelector('button[aria-label="Show profile panel"]')?.click()`);
await sleep(300);

// ---- Full profile ----
await clickByText('button', 'View full profile');
await sleep(600);
const dialogText = await b.ev(`document.querySelector('[role="dialog"]')?.textContent ?? ''`);
check('View full profile opens the full profile dialog', dialogText.includes('Mutual servers') && dialogText.includes('Safety number') && dialogText.includes('Handles'), dialogText.slice(0, 60));
check('the full profile shows the address with a QR code', (await b.ev(`!!document.querySelector('[role="dialog"] [role="img"][aria-label="QR code of the user address"]')`)) === true);
await b.ev(`document.querySelector('[role="dialog"] button[aria-label="Copy user address"]').click()`);
const copied = await toastShown('User address copied');
check('the copy button copies the address and says so', copied, copied ? '' : String((await bodyText()).length));
await b.press('Escape');
await sleep(400);
check('Esc closes the full profile', (await b.ev(`!document.querySelector('[role="dialog"]')`)) === true);

// ---- 0c. Profile card from a server member, and from the conversation menu ----
await b.ev(`document.querySelector('button[aria-label="Riverside Makers"]')?.click()`);
await sleep(900);
const memberButton = await b.ev(`[...document.querySelectorAll('${RIGHT} button')].find(n => n.textContent.includes('Maya'))?.textContent ?? null`);
check('a server member is a button in the member list', memberButton !== null, String(memberButton));
await b.click(`${RIGHT} button`, await b.ev(`[...document.querySelectorAll('${RIGHT} button')].findIndex(n => n.textContent.includes('Maya'))`));
await sleep(500);
const card = await b.ev(`(() => { const c = document.querySelector('[role="dialog"][aria-label="User profile"]'); if (!c) return null; const r = c.getBoundingClientRect(); return { text: c.textContent, inside: r.right <= innerWidth && r.bottom <= innerHeight && r.left >= 0 && r.top >= 0, width: Math.round(r.width) }; })()`);
check('clicking a member opens the profile card', !!card && card.text.includes('Maya'), card?.text?.slice(0, 60) ?? 'no card');
check('the card is 300 px and inside the window', !!card && card.width === 300 && card.inside, JSON.stringify(card && { width: card.width, inside: card.inside }));
check('the card shows the address tag and the mutual servers line', !!card && card.text.includes('·') && /mutual server|No mutual servers/.test(card.text));
check('the card has Message, Verify safety number and the role line for Maya', !!card && card.text.includes('Message') && card.text.includes('Verify safety number'));
await b.press('Escape');
await sleep(300);
check('Esc closes the card', (await b.ev(`!document.querySelector('[role="dialog"][aria-label="User profile"]')`)) === true);
check('closing the card returns the focus to the member row', (await activeText()).includes('Maya'), await activeText());

await b.click(`${RIGHT} button`, await b.ev(`[...document.querySelectorAll('${RIGHT} button')].findIndex(n => n.textContent.includes('Maya'))`));
await sleep(500);
await b.click(MAIN, 0);
await sleep(300);
check('a click outside closes the card', (await b.ev(`!document.querySelector('[role="dialog"][aria-label="User profile"]')`)) === true);

// Conversation menu: Profile opens the card next to the menu's point. The server's channel list is open, so Home comes first.
await b.ev(`document.querySelector('button[aria-label="Direct messages"]')?.click()`);
await sleep(500);
await b.ev(`(() => { const n = [...document.querySelectorAll('button')].find(x => x.textContent.includes('Jonas') && !x.closest('main') && !x.closest('[role="dialog"]') && !x.closest('[aria-label="Active now"]')); if (!n) throw new Error('no Jonas row, h1=' + document.querySelector('h1')?.textContent + ' rows=' + [...document.querySelectorAll('aside button, nav button')].map(x => x.textContent.trim().slice(0, 12)).join(' | ')); const r = n.getBoundingClientRect(); n.dispatchEvent(new MouseEvent('contextmenu', { bubbles: true, cancelable: true, clientX: r.left + 20, clientY: r.top + 10 })); })()`);
await sleep(300);
check('right-click on a DM row opens the conversation menu with Profile', (await b.ev(`[...document.querySelectorAll('[role="menu"] [role="menuitem"]')].some(n => n.textContent.trim() === 'Profile')`)) === true);
await clickByText('[role="menuitem"]', 'Profile');
await sleep(500);
check('Profile in the conversation menu opens the card for that person', (await b.ev(`document.querySelector('[role="dialog"][aria-label="User profile"]')?.textContent.includes('Jonas') === true`)) === true);
await b.press('Escape');
await sleep(300);

// ---- Address checks on the card ----
await b.ev(`(() => { const n = [...document.querySelectorAll('button')].find(x => x.textContent.includes('Jonas') && !x.closest('main') && !x.closest('[aria-label="Active now"]')); const r = n.getBoundingClientRect(); n.dispatchEvent(new MouseEvent('contextmenu', { bubbles: true, cancelable: true, clientX: r.left + 20, clientY: r.top + 10 })); })()`);
await sleep(300);
await clickByText('[role="menuitem"]', 'Profile');
await sleep(500);
check('the card suffix is the short address (dsu1…)', (await b.ev(`document.querySelector('[role="dialog"][aria-label="User profile"]')?.textContent.includes('dsu1')`)) === true);
await b.press('Escape');
await sleep(300);

// ---- Contacts & privacy: the card as others see it ----
await b.press(',', 2);
await sleep(400);
await clickByText('button', 'Contacts & privacy');
await sleep(400);
check('Contacts & privacy shows the card preview', (await b.ev(`document.querySelector('[role="dialog"]')?.textContent.includes('This is your card as other people see it.') || document.body.innerText.includes('This is your card as other people see it.')`)) === true);
check('the preview shows your name and no actions', (await b.ev(`(() => { const t = [...document.querySelectorAll('div')].find(d => d.textContent.trim() === 'Sami' && d.className.includes('text-head')); return !!t && ![...document.querySelectorAll('button')].some(n => n.textContent.trim() === 'Verify safety number' && n.closest('.bg-ink')); })()`)) === true);
const toggleSel = `document.querySelector('input[aria-label="Let people add me by address"]')`;
check('the address switch is on by default', (await b.ev(`${toggleSel}.checked`)) === true);
await b.ev(`${toggleSel}.click()`);
await sleep(300);
check('switching it off sticks', (await b.ev(`${toggleSel}.checked`)) === false);
await b.ev(`${toggleSel}.click()`);
await sleep(300);
check('switching it back on sticks', (await b.ev(`${toggleSel}.checked`)) === true);
await b.press('Escape');
await sleep(300);

// ---- Console ----
const errors = b.consoleErrors.filter((e) => !/ERR_NO_BUFFER_SPACE|Failed to load resource/.test(e));
noConsoleErrors('friends', errors);
await b.screenshot(`${SCRATCH}/shots/B-driver-last.png`);
await b.close();

}

// Calls: the lobby, the incoming call, voice channels, the voice popovers and Settings > Voice (batch C).
async function section_calls() {
// Drives the ui-next app (Vite dev server with the mock) through the calls batch: the lobby, the incoming call dialog, the
// voice channels in the sidebar, the call pane and its controls, the voice popovers on the user bar, and Settings > Voice.
// Screenshots go to SCRATCH/shots as C-next-*.png, for the comparison with the current UI's C-*.png.


const b = await launch({ url: `${BASE}/`, width: 2000, height: 1333, port: 9991, profile: `${SCRATCH}/kb-profile-next-c` });
await sleep(2500);

// Clicks the first element matching the selector whose text contains `text` (or any, when text is empty), with a DOM click.
const clickBy = (selector, text = '') => b.ev(`(() => {
  const n = [...document.querySelectorAll(${JSON.stringify(selector)})].find((x) => ${JSON.stringify(text)} === '' || x.textContent.includes(${JSON.stringify(text)}) || x.getAttribute('aria-label') === ${JSON.stringify(text)});
  if (!n) return false;
  n.click();
  return true;
})()`);
const keyPress = async (key, code, vk, text = undefined) => {
  const base = { key, code, windowsVirtualKeyCode: vk, nativeVirtualKeyCode: vk, modifiers: 0 };
  await b.send('Input.dispatchKeyEvent', { type: 'rawKeyDown', ...base, text });
  if (text) await b.send('Input.dispatchKeyEvent', { type: 'char', ...base, text });
  await b.send('Input.dispatchKeyEvent', { type: 'keyUp', ...base });
  await sleep(200);
};
const pressEsc = () => keyPress('Escape', 'Escape', 27);
const pressEnter = () => keyPress('Enter', 'Enter', 13, '\r');

// The conversation menu of a direct conversation offers Start a call; a group's conversation menu is not checked here.
await b.ev(`(() => { const n = [...document.querySelectorAll('aside[aria-label="Channels and conversations"] button')].find((x) => x.textContent.includes('Maya')); n?.dispatchEvent(new MouseEvent('contextmenu', { bubbles: true, cancelable: true, clientX: 400, clientY: 300 })); })()`);
await sleep(400);
check('C0 the conversation menu of a DM offers Start a call', (await b.ev(`[...document.querySelectorAll('[role="menuitem"]')].some((n) => n.textContent.includes('Start a call'))`)) === true);
await pressEsc();
await sleep(200);

// ---- C1. A live group call: the lobby replaces the chat, with the pill, Join and the roster on the right ----
await clickBy('aside[aria-label="Channels and conversations"] button', 'Saturday workshop crew');
await sleep(500);
check('C1 a live group call opens the lobby in place of the chat', (await b.ev(`!!document.querySelector('section[aria-label="Get ready to join"]')`)) === true);
check('C1 the lobby header has the Call in progress pill and Join', (await b.ev(`[...document.querySelectorAll('[role="status"]')].some((n) => n.textContent.startsWith('Call in progress')) && [...document.querySelectorAll('button')].some((n) => n.textContent === 'Join')`)) === true);
check('C1 the lobby names the people in the call and who is not', (await b.ev(`[...document.querySelectorAll('ul[aria-label]')].map((n) => n.getAttribute('aria-label')).join('|')`)) === 'In the call|Not in the call', await b.ev(`[...document.querySelectorAll('ul[aria-label]')].map((n) => n.getAttribute('aria-label')).join('|')`));
check('C1 the route note says the call is end-to-end encrypted and forwarded', (await b.ev(`document.body.textContent.includes('End-to-end encrypted. You’ll connect through Jonas')`)) === true);
check('C1 the mic meter has 16 bars', (await b.ev(`document.querySelectorAll('[role="meter"] span').length === 16`)) === true);
await b.screenshot(`${SCRATCH}/shots/C-next-lobby.png`);

// The mic button in the lobby toggles, and Not now returns to the chat.
await clickBy('button', 'Mute microphone');
await sleep(200);
check('C2 the lobby mic button mutes the mic in the preview', (await b.ev(`[...document.querySelectorAll('button')].some((n) => n.getAttribute('aria-label') === 'Unmute microphone' && n.getAttribute('aria-pressed') === 'true')`)) === true);
await clickBy('button', 'Unmute microphone');
await clickBy('button', 'Not now');
await sleep(300);
check('C2 Not now returns to the chat, not the lobby', (await b.ev(`!document.querySelector('section[aria-label="Get ready to join"]')`)) === true);
await clickBy('aside[aria-label="Channels and conversations"] button', 'Saturday workshop crew');
await sleep(300);
check('C2 the dismissed lobby stays dismissed while the conversation is open', (await b.ev(`!document.querySelector('section[aria-label="Get ready to join"]')`)) === true);

// ---- C3. Join from the lobby: the call pane, its tiles, and the controls with the disabled camera and share ----
await clickBy('button', 'Not now');
// Another conversation first: a dismissed lobby comes back only once the user has gone somewhere else.
await clickBy('aside[aria-label="Channels and conversations"] button', 'Maya');
await sleep(300);
await clickBy('aside[aria-label="Channels and conversations"] button', 'Saturday workshop crew');
await sleep(300);
await clickBy('section[aria-label="Get ready to join"] button', 'Join call');
await sleep(600);
check('C3 Join opens the call pane in place of the lobby', (await b.ev(`!!document.querySelector('section[aria-label="Call"]') && !document.querySelector('section[aria-label="Get ready to join"]')`)) === true);
check('C3 the call pane has the call chat', (await b.ev(`!!document.querySelector('section[aria-label="Call chat"]')`)) === true);
check('C3 the group call chat shows the messages of the group the call is in', (await b.ev(`document.querySelector('section[aria-label="Call chat"]')?.querySelectorAll('time').length > 0`)) === true);
check('C3 the camera and screen share slots are disabled and say Coming soon', (await b.ev(`['Camera, coming soon', 'Share screen, coming soon'].every((l) => { const n = document.querySelector('button[aria-label="' + l + '"]'); return n && n.disabled; })`)) === true);
check('C3 the header says how many are in the call', (await b.ev(`document.body.textContent.includes('in call') && document.body.textContent.includes('End-to-end encrypted')`)) === true);
check('C3 the camera and share slots are aria-disabled with the Coming soon tooltip', (await b.ev(`['Camera, coming soon', 'Share screen, coming soon'].every((l) => { const n = document.querySelector('button[aria-label="' + l + '"]'); return n && n.getAttribute('aria-disabled') === 'true' && n.title === 'Coming soon'; })`)) === true);
check('C3 the call header has the overflow menu and the search box', (await b.ev(`!!document.querySelector('button[aria-label="More group actions"]') && !!document.querySelector('input[aria-label="Search messages"]')`)) === true);
await b.screenshot(`${SCRATCH}/shots/C-next-call-group.png`);

// Mute and deafen in the call pane.
await clickBy('section[aria-label="Call"] button', 'Mute microphone');
await sleep(200);
check('C4 Mute in the call pane sets the pressed state', (await b.ev(`document.querySelector('button[aria-label="Unmute microphone"]')?.getAttribute('aria-pressed') === 'true'`)) === true);
await clickBy('section[aria-label="Call"] button', 'Unmute microphone');
await sleep(200);
await clickBy('section[aria-label="Call"] button', 'Deafen');
await sleep(200);
check('C4 Deafen in the call pane sets the pressed state', (await b.ev(`document.querySelector('button[aria-label="Undeafen"]')?.getAttribute('aria-pressed') === 'true'`)) === true);
await clickBy('section[aria-label="Call"] button', 'Undeafen');
await sleep(200);

// The call details column is for channel calls only: a group call's right column is the lobby roster's absence.
check('C4 the user bar shows the call under Voice connected', (await b.ev(`document.body.textContent.includes('Voice connected')`)) === true);

// Leave the call: the live call is still there, so the lobby comes back.
await clickBy('section[aria-label="Call"] button', 'Leave call');
await sleep(500);
check('C4 Leave call returns to the lobby while the call is still live', (await b.ev(`!!document.querySelector('section[aria-label="Get ready to join"]')`)) === true);
check('C4 the user bar voice strip is gone after leaving', (await b.ev(`!document.body.textContent.includes('Voice connected')`)) === true);

// ---- C5. The incoming call: a group call rings; Esc declines it ----
await b.ev(`window.__mockRing && window.__mockRing('group')`);
await sleep(500);
check('C5 a ringing group call is an alertdialog', (await b.ev(`document.querySelector('[role="alertdialog"]')?.textContent.includes('Group call')`)) === true);
check('C5 the ringing call offers Ignore and Join call', (await b.ev(`!!document.querySelector('[role="alertdialog"] button[aria-label="Ignore"]') && !!document.querySelector('[role="alertdialog"] button[aria-label="Join call"]')`)) === true);
check('C5 the quick replies and the start-muted choice are there', (await b.ev(`document.body.textContent.includes('Joining in 5 minutes') && document.body.textContent.includes('Start with my microphone muted')`)) === true);
check('C5 the rings animate unless reduced motion is set', (await b.ev(`document.querySelectorAll('[role="alertdialog"] .ring-pulse').length === 3`)) === true);
await b.screenshot(`${SCRATCH}/shots/C-next-incoming-group.png`);
await pressEsc();
await sleep(400);
check('C5 Esc declines the incoming call and closes the dialog', (await b.ev(`!document.querySelector('[role="alertdialog"]')`)) === true);

// A direct call rings; Enter answers it.
await b.ev(`window.__mockRing && window.__mockRing('dm')`);
await sleep(500);
check('C6 a ringing direct call offers Decline and Answer', (await b.ev(`!!document.querySelector('[role="alertdialog"] button[aria-label="Decline"]') && !!document.querySelector('[role="alertdialog"] button[aria-label="Answer"]')`)) === true);
check('C6 the direct call is labelled Incoming call', (await b.ev(`document.querySelector('[role="alertdialog"]')?.textContent.includes('Incoming call')`)) === true);
await b.screenshot(`${SCRATCH}/shots/C-next-incoming-dm.png`);
await pressEsc();
await sleep(400);
check('C6 Esc declines the direct call', (await b.ev(`!document.querySelector('[role="alertdialog"]')`)) === true);

// A group call rings again; Enter answers it. The mock creates the call for the group ring only.
await b.ev(`window.__mockRing && window.__mockRing('group')`);
await sleep(500);
await pressEnter();
await sleep(700);
check('C6 Enter answers the group call and closes the dialog', (await b.ev(`!document.querySelector('[role="alertdialog"]')`)) === true);
check('C6 the answered call shows the call pane', (await b.ev(`!!document.querySelector('section[aria-label="Call"]')`)) === true);
await clickBy('section[aria-label="Call"] button', 'Leave call');
await sleep(400);

// A quick reply sends a message and declines.
await b.ev(`window.__mockRing && window.__mockRing('dm')`);
await sleep(500);
await clickBy('[role="alertdialog"] button', 'Text me instead');
await sleep(500);
check('C6 a quick reply closes the dialog', (await b.ev(`!document.querySelector('[role="alertdialog"]')`)) === true);

// ---- C7. Voice channels: joined by a click, their people, the sidebar state, and the user bar with Disconnect ----
await clickBy('button[aria-label="Riverside Makers"]');
await sleep(600);
check('C7 the server opens with its voice channels listed under their own header', (await b.ev(`[...document.querySelectorAll('aside[aria-label="Channels and conversations"] *')].some((n) => n.textContent === 'Voice channels')`)) === true);
const voiceRows = await b.ev(`[...document.querySelectorAll('aside[aria-label="Channels and conversations"] button')].filter((n) => n.title.startsWith('Join ') || n.title.startsWith('Connected to ')).map((n) => n.textContent.trim())`);
check('C7 the voice rows are live (not disabled)', (await b.ev(`[...document.querySelectorAll('aside[aria-label="Channels and conversations"] button')].filter((n) => n.title.startsWith('Join ')).every((n) => !n.disabled)`)) === true, JSON.stringify(voiceRows));
check('C7 people in a voice channel are listed under it', (await b.ev(`document.querySelectorAll('aside [role="listitem"]').length >= 4`)) === true);
await clickBy('aside[aria-label="Channels and conversations"] button', 'workshop');
await sleep(700);
check('C7 clicking a voice channel joins it and shows the call pane', (await b.ev(`!!document.querySelector('section[aria-label="Call"]')`)) === true);
check('C7 the joined voice channel is the active row', (await b.ev(`[...document.querySelectorAll('aside[aria-label="Channels and conversations"] button')].some((n) => n.textContent.includes('workshop') && n.getAttribute('aria-current') === 'true')`)) === true);
check('C7 the user bar says Voice connected with the channel and server', (await b.ev(`document.body.textContent.includes('Voice connected') && document.body.textContent.includes('#workshop · Riverside Makers')`)) === true);
check('C7 the right column shows the call details', (await b.ev(`document.querySelector('aside[aria-label="Details"]')?.textContent.includes('Call details') && document.querySelector('aside[aria-label="Details"]')?.textContent.includes('Encryption')`)) === true);
check('C7 the call header names the channel and the count', (await b.ev(`[...document.querySelectorAll('h2')].some((n) => n.textContent === 'workshop')`)) === true);
check('C7 the voice roster lists you first in the channel you joined', (await b.ev(`[...document.querySelectorAll('[role="list"][aria-label="In workshop"] [role="listitem"]')][0]?.textContent.includes('Sami') === true`)) === true);
check('C7 the call details show the route sections the call has (no route block)', (await b.ev(`!document.querySelector('aside[aria-label="Details"]')?.textContent.includes('ROUTE') && !document.querySelector('aside[aria-label="Details"]')?.textContent.includes('Route')`)) === true);
check('C7 the call details show Forwarded by and Backup for a forwarded call', (await b.ev(`document.querySelector('aside[aria-label="Details"]')?.textContent.includes('Forwarded by') && document.querySelector('aside[aria-label="Details"]')?.textContent.includes('Backup')`)) === true);
await b.screenshot(`${SCRATCH}/shots/C-next-voice-channel.png`);

// The voice rows' tooltips follow the connection.
check('C7 the connected row says so in its title', (await b.ev(`[...document.querySelectorAll('aside[aria-label="Channels and conversations"] button')].some((n) => n.title === 'Connected to workshop')`)) === true);

// ---- C8. Voice popovers on the user bar: the mic chevron and the headphone chevron ----
await clickBy('button', 'Microphone options');
await sleep(400);
const popLabels = () => b.ev(`[...document.querySelectorAll('[role="menu"] [role="menuitem"], [role="menu"] [role="menuitemradio"]')].slice(0, 12).map((n) => n.textContent.trim()).join('|')`);
const inputMenu = await popLabels();
check('C8 the mic chevron opens the input menu with device, profile, volume and settings', inputMenu.includes('Input Device') && inputMenu.includes('Input Profile') && inputMenu.includes('Voice Settings'), inputMenu);
await b.screenshot(`${SCRATCH}/shots/C-next-input-menu.png`);
await pressEsc();
await sleep(300);
await clickBy('button', 'Output options');
await sleep(400);
const outputMenu = await popLabels();
check('C8 the headphone chevron opens the output menu', outputMenu.includes('Output Device') && outputMenu.includes('Voice Settings'), outputMenu);
check('C8 the output menu has a volume slider', (await b.ev(`document.querySelector('[role="menu"] [role="slider"], [role="menu"] input[type="range"]') !== null`)) === true);
await pressEsc();
await sleep(300);
check('C8 Escape closes the popover', (await b.ev(`!document.querySelector('[role="menu"]')`)) === true);

// Voice Settings in the popover opens Settings on the Voice page.
await clickBy('button', 'Output options');
await sleep(400);
await clickBy('[role="menu"] [role="menuitem"]', 'Voice Settings');
await sleep(500);
check('C8 Voice Settings in the popover opens Settings on the Voice page', (await b.ev(`!!document.querySelector('h1') && [...document.querySelectorAll('h1')].some((n) => n.textContent === 'Voice')`)) === true);

// ---- C9. Settings > Voice: devices, volumes, the mic test, the profiles, sensitivity, push to talk ----
check('C9 the Voice page has its blocks', (await b.ev(`['Devices', 'Volume', 'Mic Test', 'Input Profile', 'Input Sensitivity', 'Push to Talk'].every((t) => [...document.querySelectorAll('h2, legend')].some((n) => n.textContent === t))`)) === true);
check('C9 the mic test meter has 40 bars', (await b.ev(`[...document.querySelectorAll('[aria-hidden="true"]')].some((n) => n.children.length === 40)`)) === true);
check('C9 the input profile offers the three profiles', (await b.ev(`['Voice Isolation', 'Studio', 'Custom'].every((t) => document.body.textContent.includes(t))`)) === true);
await b.screenshot(`${SCRATCH}/shots/C-next-settings-voice.png`);

await clickBy('button', 'Mic Test');
await sleep(300);
check('C9 Mic Test toggles to Stop Mic Test with aria-pressed', (await b.ev(`[...document.querySelectorAll('button')].some((n) => n.textContent === 'Stop Mic Test' && n.getAttribute('aria-pressed') === 'true')`)) === true);
await clickBy('button', 'Stop Mic Test');
await sleep(200);

// Custom shows the processing and advanced blocks; the others do not.
await clickBy('input[name="input-profile"][value="custom"]');
await sleep(300);
check('C9 the Custom profile shows Processing and Advanced', (await b.ev(`['Processing', 'Advanced'].every((t) => [...document.querySelectorAll('h2')].some((n) => n.textContent === t))`)) === true);
check('C9 Custom offers the noise suppression choice and echo cancellation', (await b.ev(`!!document.querySelector('select[aria-label="Noise suppression"]') && document.body.textContent.includes('Echo Cancellation')`)) === true);
await clickBy('input[name="input-profile"][value="studio"]');
await sleep(300);
check('C9 Studio hides the custom processing', (await b.ev(`![...document.querySelectorAll('h2')].some((n) => n.textContent === 'Processing')`)) === true);
check('C9 Studio explains that the mic is unprocessed', (await b.ev(`document.body.textContent.includes('Studio sends the microphone unprocessed')`)) === true);

// Automatic sensitivity: the handle is disabled while it is on, and the note says which mode is in use.
const handleDisabled = () => b.ev(`document.querySelector('input[aria-label="Input sensitivity threshold"]')?.disabled`);
const autoOn = await b.ev(`[...document.querySelectorAll('label')].find((l) => l.textContent === 'Automatically Adjust Input Sensitivity')?.querySelector('input').checked`);
check('C9 the handle follows the automatic switch', (await handleDisabled()) === autoOn, `auto ${autoOn}, disabled ${await handleDisabled()}`);
await clickBy('label', 'Automatically Adjust Input Sensitivity');
await sleep(300);
check('C9 switching automatic sensitivity changes whether the handle is live', (await handleDisabled()) === !autoOn, `disabled: ${await handleDisabled()}`);
check('C9 the sensitivity note matches the mode', (await b.ev(`document.body.textContent.includes(${autoOn ? "'Drag the handle to set the level'" : "'Following the room noise'"})`)) === true);
await clickBy('label', 'Automatically Adjust Input Sensitivity');
await sleep(300);
check('C9 switching back restores the handle', (await handleDisabled()) === autoOn);

// Push to talk: the key button shows only while the switch is on, and the binder takes the next key.
check('C9 the push key is hidden while push to talk is off', (await b.ev(`!document.querySelector('button[aria-label="Push to talk key"]')`)) === true);
await clickBy('label', 'Push to Talk');
await sleep(300);
check('C9 the push key shows Set a key or the saved key', (await b.ev(`!!document.querySelector('button[aria-label="Push to talk key"]')`)) === true);
await clickBy('button[aria-label="Push to talk key"]');
await sleep(200);
check('C9 the binder asks for a key and says Esc cancels', (await b.ev(`document.querySelector('button[aria-label="Push to talk key"]')?.textContent === 'Press a key (Esc cancels)'`)) === true);
await b.send('Input.dispatchKeyEvent', { type: 'rawKeyDown', key: 'Control', code: 'ControlLeft', windowsVirtualKeyCode: 17, nativeVirtualKeyCode: 17, modifiers: 2 });
await sleep(100);
await b.send('Input.dispatchKeyEvent', { type: 'keyUp', key: 'Control', code: 'ControlLeft', windowsVirtualKeyCode: 17, nativeVirtualKeyCode: 17, modifiers: 2 });
await sleep(100);
check('C9 a modifier on its own keeps the binder waiting', (await b.ev(`document.querySelector('button[aria-label="Push to talk key"]')?.textContent === 'Press a key (Esc cancels)'`)) === true);
await keyPress('v', 'KeyV', 86, 'v');
await sleep(400);
check('C9 the next key is saved as the push key', (await b.ev(`document.querySelector('button[aria-label="Push to talk key"]')?.textContent === 'V'`)) === true, await b.ev(`document.querySelector('button[aria-label="Push to talk key"]')?.textContent`));
await b.screenshot(`${SCRATCH}/shots/C-next-settings-voice-ptt.png`);

// Escape closes Settings after the Voice page.
await pressEsc();
await sleep(300);
check('C9 Esc closes Settings after the Voice page', (await b.ev(`!document.querySelector('[data-settings-page]') && ![...document.querySelectorAll('h1')].some((n) => n.textContent === 'Voice')`)) === true);

// ---- C10. Leaving the voice channel from the user bar ----
await clickBy('button[aria-label="Disconnect"]');
await sleep(500);
check('C10 Disconnect ends the voice connection', (await b.ev(`!document.body.textContent.includes('Voice connected')`)) === true);
check('C10 the text channel comes back after Disconnect', (await b.ev(`!document.querySelector('section[aria-label="Call"]')`)) === true);

// ---- C11. The call button in a conversation header: a DM rings the other person (start_call); a group starts or joins
// its call (join_group_call). Relay operators has no live call, so its header shows the chat's call button. ----
await clickBy('button[aria-label="Direct messages"]');
await sleep(500);
await clickBy('aside[aria-label="Channels and conversations"] button', 'Maya');
await sleep(500);
check('C11 the DM header has the Start voice call button', (await b.ev(`!!document.querySelector('button[aria-label="Start voice call with Maya"]')`)) === true);
await clickBy('button[aria-label="Start voice call with Maya"]');
await sleep(700);
check('C11 the DM call button shows the call pane with the person it rings', (await b.ev(`!!document.querySelector('section[aria-label="Call"]') && document.body.textContent.includes('Calling Maya')`)) === true);
check('C11 the DM call chat shows the messages of the conversation', (await b.ev(`document.querySelector('section[aria-label="Call chat"]')?.querySelectorAll('time').length > 0`)) === true);
await clickBy('section[aria-label="Call"] button', 'Leave call');
await sleep(500);

await clickBy('aside[aria-label="Channels and conversations"] button', 'Relay operators');
await sleep(500);
check('C11 the group header has the Start a voice call in button', (await b.ev(`!!document.querySelector('button[aria-label="Start a voice call in Relay operators"]')`)) === true);
await clickBy('button[aria-label="Start a voice call in Relay operators"]');
await sleep(700);
check('C11 the group call button joins the group call and shows the call pane', (await b.ev(`!!document.querySelector('section[aria-label="Call"]')`)) === true);
await b.screenshot(`${SCRATCH}/shots/C-next-group-call-button.png`);
await clickBy('section[aria-label="Call"] button', 'Leave call');
await sleep(500);

await b.close();
}

// Startup, onboarding and Settings (batch E).
async function section_startup() {
// Drives the ui-next app (Vite dev server with the mock) through the startup gate, first run and Settings, with real key
// events (DevTools Input domain). Each part launches its own page, as each one starts from a different URL.


const b = await launch({ url: `${BASE}/`, width: 2000, height: 1333, port: 9991, profile: `${SCRATCH}/kb-profile-next-e` });
await sleep(2500);

// ---- E. Startup and Settings (ui-next-e): the gear and Ctrl+, open Settings; Esc closes it and returns focus; the search
// narrows the sections; Edit Image takes keys and applies a 256 px WebP; first run; the portable gate. ----
const keyPress = async (page, key, code, vk, modifiers = 0, text = undefined) => {
  const base = { key, code, windowsVirtualKeyCode: vk, nativeVirtualKeyCode: vk, modifiers };
  await page.send('Input.dispatchKeyEvent', { type: 'rawKeyDown', ...base, text });
  if (text) await page.send('Input.dispatchKeyEvent', { type: 'char', ...base, text });
  await page.send('Input.dispatchKeyEvent', { type: 'keyUp', ...base });
  await sleep(200);
};
const pressEnter = (page) => keyPress(page, 'Enter', 'Enter', 13, 0, '\r');
const pressEsc = (page) => keyPress(page, 'Escape', 'Escape', 27);
const pressArrowRight = (page) => keyPress(page, 'ArrowRight', 'ArrowRight', 39);
const pressPlus = (page) => keyPress(page, '+', 'Equal', 187, 0, '+');
const pressR = (page) => keyPress(page, 'r', 'KeyR', 82, 0, 'r');
const activeLabel = () => b.ev(`document.activeElement?.getAttribute('aria-label') ?? null`);

await b.ev(`document.querySelector('button[aria-label="Settings"]').focus()`);
await pressEnter(b);
check('E1 Enter on the gear opens Settings', (await b.ev(`!!document.querySelector('[data-settings-page]')`)) === true);
await pressEsc(b);
check('E2 Esc closes Settings', (await b.ev(`!document.querySelector('[data-settings-page]')`)) === true);
check('E3 closing Settings returns focus to the gear', (await activeLabel()) === 'Settings', `focused: ${await activeLabel()}`);

await b.ev(`document.querySelector('button[aria-label="Direct messages"]').focus()`);
await b.press(',', 2);
await sleep(400);
check('E4 Ctrl+, opens Settings', (await b.ev(`!!document.querySelector('[data-settings-page]')`)) === true);
await pressEsc(b);
check('E5 Esc after Ctrl+, returns focus to the element focused before', (await activeLabel()) === 'Direct messages', `focused: ${await activeLabel()}`);

await b.press(',', 2);
await sleep(400);
await b.ev(`document.querySelector('[data-settings-page] input[type="search"]').focus()`);
await b.typeText('net');
const navAfterSearch = await b.ev(`[...document.querySelectorAll('[data-settings-page] nav button')].map(n => n.textContent).join('|')`);
check('E6 the search narrows the sections to Network', navAfterSearch === 'Network', navAfterSearch);
await pressEsc(b);
check('E7 Esc from the search field still closes Settings', (await b.ev(`!document.querySelector('[data-settings-page]')`)) === true);

// Edit Image from My identity. The file picker is stubbed: the click is recorded and a PNG is handed to that input.
await b.press(',', 2);
await sleep(400);
await b.ev(`(() => { const native = HTMLInputElement.prototype.click; HTMLInputElement.prototype.click = function () { if (this.type === 'file') window.__pickerInput = this; else native.call(this); }; })()`);
const pickPicture = async () => {
  await b.ev(`[...document.querySelectorAll('[data-settings-page] button')].find(n => n.textContent.includes('Change avatar')).click()`);
  await sleep(200);
  await b.ev(`(async () => {
    const blob = await (await fetch('app-icon-96.png')).blob();
    const dt = new DataTransfer();
    dt.items.add(new File([blob], 'face.png', { type: 'image/png' }));
    window.__pickerInput.files = dt.files;
    window.__pickerInput.dispatchEvent(new Event('change'));
  })()`);
  await sleep(700);
};
await pickPicture();
check('E8 picking a picture opens Edit Image', (await b.ev(`[...document.querySelectorAll('[data-slot="dialog-content"] h2')].map(n => n.textContent).includes('Edit Image')`)) === true);
check('E9 the crop has a mask with a 3 px white ring', (await b.ev(`getComputedStyle(document.querySelector('[data-crop-mask]')).borderTopWidth`)) === '3px');
check('E10 the footer is Reset, Cancel, Apply', (await b.ev(`[...document.querySelectorAll('[data-slot="dialog-content"] button')].map(n => n.textContent.trim()).filter(t => ['Reset', 'Cancel', 'Apply'].includes(t)).join('|')`)) === 'Reset|Cancel|Apply');
const transformOf = () => b.ev(`document.querySelector('[data-crop-image]')?.style.transform ?? ''`);
const scaleOf = (t) => Number(t.match(/scale\(([-\d.e]+)\)/)?.[1]);
const xOf = (t) => Number(t.match(/translate\((-?[\d.e-]+)px/)?.[1]);
const fitT = await transformOf();
await b.ev(`document.querySelector('[data-crop-stage]').focus()`);
await pressPlus(b);
const zoomedT = await transformOf();
check('E11 + zooms in', scaleOf(zoomedT) > scaleOf(fitT), `${scaleOf(fitT)} -> ${scaleOf(zoomedT)}`);
check('E12 the zoom slider follows the zoom', (await b.ev(`Number(document.querySelector('[data-slot="dialog-content"] input[type="range"][aria-label="Zoom"]').value) > 0`)) === true);
await pressArrowRight(b);
check('E13 ArrowRight pans the picture', xOf(await transformOf()) > xOf(zoomedT));
await pressR(b);
check('E14 R rotates the picture a quarter turn', (await transformOf()).includes('rotate(90deg)'));
check('E15 the rotate control is an icon-only button named Rotate 90 degrees', (await b.ev(`document.querySelector('[aria-label="Rotate 90°"]')?.textContent === ''`)) === true);
await pressEsc(b);
check('E16 Escape cancels Edit Image, keeps Settings open, and sets no picture', (await b.ev(`!document.querySelector('[data-slot="dialog-content"]') && !!document.querySelector('[data-settings-page]') && !document.querySelector('[data-settings-page] [data-slot="avatar"] img')`)) === true);

await pickPicture();
await pressEnter(b);
await sleep(900);
check('E17 Enter applies and closes Edit Image', (await b.ev(`!document.querySelector('[data-slot="dialog-content"]')`)) === true);
const avatarSize = await b.ev(`(async () => { const img = document.querySelector('[data-settings-page] [data-slot="avatar"] img'); if (!img) return null; await img.decode(); return [img.naturalWidth, img.naturalHeight]; })()`);
check('E18 Apply produces a 256 by 256 picture', avatarSize?.join('x') === '256x256', String(avatarSize));
check('E19 the picture is encoded as WebP', (await b.ev(`atob(document.querySelector('[data-settings-page] [data-slot="avatar"] img').src.split(',')[1]).slice(8, 12) === 'WEBP'`)) === true);
await b.ev(`[...document.querySelectorAll('[data-settings-page] button')].find(n => n.textContent.includes('Remove avatar')).click()`);
await sleep(400);
check('E20 Remove avatar falls back to the initials', (await b.ev(`!document.querySelector('[data-settings-page] [data-slot="avatar"] img') && document.querySelector('[data-settings-page] [data-slot="avatar"]')?.textContent === 'SA'`)) === true);
await pressEsc(b);
check('E21 Esc closes Settings after the avatar work', (await b.ev(`!document.querySelector('[data-settings-page]')`)) === true);

// Settings pages: Network shows the status table; Updates checks the mock release; Sounds disables its controls.
await b.press(',', 2);
await sleep(400);
await b.ev(`[...document.querySelectorAll('[data-settings-page] nav button')].find(n => n.textContent === 'Network').click()`);
await sleep(500);
check('E22 Network shows Resolved via in the status table', (await b.ev(`[...document.querySelectorAll('[data-settings-page] dt')].some(n => n.textContent === 'Resolved via')`)) === true);
// The search keeps its text between openings, as the current UI's does, so clear it before the next section.
await b.ev(`document.querySelector('[data-settings-page] input[type="search"]').focus()`);
await keyPress(b, 'a', 'KeyA', 65, 2);
await keyPress(b, 'Backspace', 'Backspace', 8);
await b.ev(`[...document.querySelectorAll('[data-settings-page] nav button')].find(n => n.textContent === 'Updates').click()`);
await sleep(300);
await b.ev(`[...document.querySelectorAll('[data-settings-page] button')].find(n => n.textContent.includes('Check for updates')).click()`);
await sleep(600);
check('E23 Check for updates reports the release in the mock', (await b.ev(`[...document.querySelectorAll('[data-settings-page] [role="status"]')].some(n => n.textContent.startsWith('Update available: v'))`)) === true);
await b.ev(`[...document.querySelectorAll('[data-settings-page] nav button')].find(n => n.textContent === 'Notifications & sounds').click()`);
await sleep(300);
await b.ev(`document.querySelector('[data-settings-page] input[type="checkbox"]').click()`);
await sleep(300);
const sounds24 = await b.ev(`JSON.stringify([...document.querySelectorAll('[data-settings-page] input')].map(n => [n.type, n.checked, n.disabled]))`);
check('E24 Play sounds off disables the per-kind toggles and the volume', (await b.ev(`[...document.querySelectorAll('[data-settings-page] input[type="checkbox"]')].slice(1).every(n => n.disabled) && document.querySelector('[data-settings-page] input[type="range"]').disabled`)) === true, sounds24);
await pressEsc(b);
noConsoleErrors('startup', b.consoleErrors);

// First run: the identity screen covers the shell until the phrase is confirmed. Continue waits for the checkbox.
const onboard = await launch({ url: `${BASE}/?firstrun`, width: 2000, height: 1333, port: 9992, profile: `${SCRATCH}/kb-profile-next-first` });
await sleep(2500);
const continueDisabled = () => onboard.ev(`[...document.querySelectorAll('button')].find(n => n.textContent.trim() === 'Continue')?.disabled ?? null`);
check('E26 first run shows the identity screen', (await onboard.ev(`!!document.querySelector('[aria-label="Welcome to Dissent"]')`)) === true);
check('E27 the recovery phrase lists 24 words', (await onboard.ev(`document.querySelectorAll('[aria-label="Recovery phrase"] li').length`)) === 24);
check('E28 Continue waits for the checkbox', (await continueDisabled()) === true);
await onboard.ev(`document.getElementById('onboard-saved').focus()`);
await keyPress(onboard, ' ', 'Space', 32, 0, ' ');
check('E29 the checkbox enables Continue', (await continueDisabled()) === false);
await onboard.ev(`[...document.querySelectorAll('button')].find(n => n.textContent.trim() === 'Continue').focus()`);
await pressEnter(onboard);
await sleep(500);
check('E30 Continue closes the identity screen', (await onboard.ev(`!document.querySelector('[aria-label="Welcome to Dissent"]')`)) === true);
noConsoleErrors('onboarding', onboard.consoleErrors);
await onboard.close();

// Portable profile: a new one asks for a passphrase and refuses a short or mismatched one before it opens the shell.
const portable = await launch({ url: `${BASE}/?unlock=new`, width: 2000, height: 1333, port: 9993, profile: `${SCRATCH}/kb-profile-next-unlock` });
await sleep(2500);
check('E32 a new portable profile asks for a passphrase', (await portable.ev(`document.querySelector('h1')?.textContent === 'Create a passphrase'`)) === true);
await portable.typeText('short');
await pressEnter(portable);
await sleep(300);
check('E33 a short passphrase is refused', (await portable.ev(`document.querySelector('[role="alert"]')?.textContent`)) === 'Use at least 8 characters.');
await portable.ev(`document.querySelector('[aria-label="Passphrase"]').focus()`);
await portable.press('a', 2);
await portable.typeText('correct horse 1');
await portable.ev(`document.querySelector('[aria-label="Repeat the passphrase"]').focus()`);
await portable.typeText('correct horse 2');
await pressEnter(portable);
await sleep(300);
check('E34 mismatched passphrases are refused', (await portable.ev(`document.querySelector('[role="alert"]')?.textContent`)) === 'The two passphrases don’t match.');
await portable.ev(`document.querySelector('[aria-label="Repeat the passphrase"]').focus()`);
await portable.press('a', 2);
await portable.typeText('correct horse 1');
await pressEnter(portable);
await sleep(900);
check('E35 a matching passphrase opens the shell', (await portable.ev(`!document.querySelector('[aria-label="Unlock Dissent"]') && !!document.querySelector('button[aria-label="Direct messages"]')`)) === true);
await portable.close();

const locked = await launch({ url: `${BASE}/?unlock=1`, width: 2000, height: 1333, port: 9994, profile: `${SCRATCH}/kb-profile-next-locked` });
await sleep(2500);
check('E36 a locked profile asks to unlock', (await locked.ev(`document.querySelector('h1')?.textContent === 'Unlock Dissent'`)) === true);
await locked.typeText('wrong');
await pressEnter(locked);
await sleep(300);
check('E37 a wrong passphrase is refused and the shell stays shut', (await locked.ev(`document.querySelector('[role="alert"]')?.textContent === 'That passphrase is wrong.' && !document.querySelector('button[aria-label="Direct messages"]')`)) === true);
await locked.close();

const chooser = await launch({ url: `${BASE}/?chooser=1`, width: 2000, height: 1333, port: 9995, profile: `${SCRATCH}/kb-profile-next-chooser` });
await sleep(2500);
check('E38 the chooser offers four options and focuses the first', (await chooser.ev(`document.querySelectorAll('[data-choice]').length === 4 && document.activeElement?.getAttribute('data-choice') === 'installed'`)) === true);
await chooser.close();


await b.close();
}

const SECTIONS = [
  ['core', section_core],
  ['friends', section_friends],
  ['calls', section_calls],
  ['startup', section_startup],
];
// SECTIONS=core,calls runs only those sections (comma-separated names); all of them by default.
const only = process.env.SECTIONS?.split(',').map((x) => x.trim()).filter(Boolean);
for (const [name, run] of SECTIONS) {
  if (only && !only.includes(name)) continue;
  try {
    await run();
  } catch (err) {
    check(`${name}: the section ran to the end`, false, String(err?.message ?? err).slice(0, 300));
  }
}

const failed = results.filter((r) => !r.ok);
console.log(`\n${results.length - failed.length}/${results.length} checks passed`);
process.exit(failed.length ? 1 : 0);
