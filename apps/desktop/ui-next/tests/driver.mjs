// The ui-next driver: every check of the React UI's key and mouse driver, run against the Vite dev server with the mock in
// headless Edge through the DevTools Input domain (real key events, not synthetic DOM events). One section per area, each
// with its own browser page. Run it with `npm run test:ui` (it starts Vite), or against a running server:
//   UI_PORT=1420 SCRATCH=<dir> node tests/driver.mjs
// Sections: core (the shell, chat, transfers, dialogs, menus, Settings), friends (Friends, DM panel, profile card,
// addresses), calls (lobby, incoming call, voice channels, voice popovers, Settings > Voice), startup (gate, onboarding,
// Settings). Exits 1 when any check fails.
import { mkdirSync, readdirSync, readFileSync, statSync, writeFileSync } from 'node:fs';
import { fileURLToPath } from 'node:url';
import { tmpdir } from 'node:os';
import { join } from 'node:path';
import { launch } from './cdp.mjs';
import { addressOf, base32Encode } from '../src/mock/address.js';
import { typingParts } from '../src/lib/typing-words.ts';

const UI_PORT = process.env.UI_PORT ?? '1420';
const BASE = `http://127.0.0.1:${UI_PORT}`;
const SCRATCH = process.env.SCRATCH ?? join(tmpdir(), 'dissent-ui-driver');
mkdirSync(`${SCRATCH}/shots`, { recursive: true });
const sleep = (ms) => new Promise((r) => setTimeout(r, ms));
// Polls `test` (an async function) until it is truthy, up to `ms`. Resolves to the last value, so a failure shows it.
const until = async (test, ms = 15000) => {
  const end = Date.now() + ms;
  let value = await test();
  while (!value && Date.now() < end) {
    await sleep(100);
    value = await test();
  }
  return value;
};

const results = [];
const check = (name, ok, detail = '') => {
  results.push({ name, ok: Boolean(ok), detail });
  console.log(`${ok ? 'PASS' : 'FAIL'} ${name}${detail ? ` :: ${detail}` : ''}`);
};

// Static guard for the packaged app's policy. A data: image is blocked unless img-src allows data:, and the policy is the
// csp in tauri.conf.json. The check reads that img-src and scans the app's own source (not the mock) for data:image URLs.
{
  const config = JSON.parse(readFileSync(fileURLToPath(new URL('../../src-tauri/tauri.conf.json', import.meta.url)), 'utf8'));
  const imgSrc = config.app.security.csp.split(';').map((d) => d.trim()).find((d) => d.startsWith('img-src')) ?? '';
  const dataImageFiles = [];
  const walk = (dir) => {
    for (const name of readdirSync(dir)) {
      const path = join(dir, name);
      if (statSync(path).isDirectory()) {
        if (name !== 'mock') walk(path);
      } else if (/\.(tsx?|jsx?)$/.test(name) && /data:image\//.test(readFileSync(path, 'utf8'))) {
        dataImageFiles.push(path.slice(path.indexOf('src')));
      }
    }
  };
  walk(fileURLToPath(new URL('../src', import.meta.url)));
  check(
    'the packaged app\'s img-src allows data: wherever a source file shows a data: image',
    dataImageFiles.length === 0 || /(^|\s)data:(\s|$)/.test(imgSrc),
    `${dataImageFiles.length} files use data:image (${dataImageFiles.join(', ')}); img-src is "${imgSrc}"`,
  );
}

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
await ev(`(() => { const f = document.querySelector('[role="dialog"] input[aria-label="Server Name"]'); f.focus(); f.select(); })()`);
await typeText('Settings test');
await ev(`[...document.querySelectorAll('[role="dialog"] button:not([role="tab"])')].find((b) => b.textContent.trim() === 'Create').click()`);
await sleep(900);
check('the new owned server is on the rail right after it is created', await ev(`[...document.querySelectorAll('button[data-server-id]')].some((b) => b.getAttribute('aria-label') === 'Settings test')`) === true, await ev(`[...document.querySelectorAll('button[data-server-id]')].map((b) => b.getAttribute('aria-label')).join(' | ') + ' || dialogs: ' + document.querySelectorAll('[role=dialog]').length + ' || toast: ' + [...document.querySelectorAll('[role=alert], [data-toast]')].map(n => n.textContent).join(' ')`));

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
check('the Private Channel switch is on for a text channel', (await ev(`document.querySelector('[role="dialog"] input[role="switch"]').disabled === false`)) === true);
await ev(`document.querySelectorAll('[role="dialog"] input[name="channel-kind"]')[1].click()`);
await sleep(200);
check('a voice channel cannot be private', (await ev(`document.querySelector('[role="dialog"] input[role="switch"]').disabled === true`)) === true);
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

// ---- 11b. Server Settings: the owner's entry, Server Profile with its live preview, the unsaved bar, the banner presets
// and upload, the traits and the description, the sidebar banner, and no entry for members ----
const SS = '[role="dialog"][aria-label="Server settings"]';
const ssButton = (text) => `[...document.querySelectorAll('${SS} button')].find((b) => b.textContent.trim() === ${JSON.stringify(text)})`;
const ssBar = () => ev(`!!document.querySelector('${SS} [aria-label="Unsaved changes"]')`);
const fill = async (selector, text) => {
  await ev(`(() => { const i = document.querySelector(${JSON.stringify(selector)}); i.focus(); i.select(); })()`);
  await typeText(text);
};
const saveChanges = async () => {
  await ev(`${ssButton('Save Changes')}.click()`);
  await sleep(800);
};

await clickRail('Settings test');
await sleep(500);
check('SS0 with no banner the plain header shows the server name', (await ev(`document.querySelector('${SIDEBAR} [data-server-top]')?.dataset.serverTop === 'plain' && document.querySelector('${SIDEBAR} [data-server-name]')?.textContent === 'Settings test'`)) === true);

await pointerClick(`document.querySelector('button[aria-label="Settings test menu"]')`);
await sleep(250);
check('SS1 the owner\'s server menu offers Server Settings first', (await menuItems())[0]?.startsWith('Server Settings'), JSON.stringify(await menuItems()));
await ev(`[...document.querySelectorAll('[role="menuitem"]')].find((m) => m.textContent.startsWith('Server Settings')).click()`);
await sleep(400);
check('SS2 Server Settings opens on Server Profile', (await ev(`document.querySelector('${SS} h1')?.textContent`)) === 'Server Profile');
check('SS3 the nav lists Server Profile and Boosts, then People: Members, Invites and Bans', (await ev(`JSON.stringify([...document.querySelectorAll('${SS} nav button')].map((b) => b.textContent))`)) === JSON.stringify(['Server Profile', 'Boosts', 'Members', 'Invites', 'Bans']));
check('SS4 the form holds Name, Server Icon, Banner, Traits, Message history and Description', (await ev(`JSON.stringify([...document.querySelectorAll('${SS} section h2')].map((n) => n.textContent))`)) === JSON.stringify(['Name', 'Server Icon', 'Banner', 'Traits', 'Message history', 'Description']));
check('SS5 the name field holds the name, stops at 64, and nothing is unsaved', (await ev(`(() => { const i = document.querySelector('${SS} input[aria-label="Server Name"]'); return i.value === 'Settings test' && i.maxLength === 64; })()`)) === true && !(await ssBar()));
check('SS6 the icon block offers Change Server Icon', (await ev(`!!${ssButton('Change Server Icon')}`)) === true);

// The unsaved bar, the live preview and Reset.
await fill(`${SS} input[aria-label="Server Name"]`, 'Scratch name');
check('SS7 an edit shows the unsaved bar with Reset and Save Changes', (await ev(`JSON.stringify([...document.querySelectorAll('${SS} [aria-label="Unsaved changes"] button')].map((b) => b.textContent.trim()))`)) === JSON.stringify(['Reset', 'Save Changes']) && (await ev(`document.querySelector('${SS} [aria-label="Unsaved changes"]').textContent.includes('Careful — you have unsaved changes!')`)) === true);
check('SS8 the preview card follows the name as it is typed', (await ev(`document.querySelector('${SS} [data-server-card] [data-card-name]')?.textContent`)) === 'Scratch name');
await press('Escape');
await sleep(300);
check('SS9 leaving with unsaved changes shakes the bar and keeps the page open', (await ev(`!!document.querySelector('${SS}') && !!document.querySelector('${SS} [aria-label="Unsaved changes"][data-shake]')`)) === true);
await ev(`${ssButton('Reset')}.click()`);
await sleep(200);
check('SS10 Reset puts the saved name back and hides the bar', (await ev(`document.querySelector('${SS} input[aria-label="Server Name"]').value === 'Settings test'`)) === true && !(await ssBar()));

// Banner presets: ten swatches and an upload tile; a pick rings the swatch and shows in the preview.
check('SS11 the banner grid has ten presets and an Upload Image tile', (await ev(`document.querySelectorAll('${SS} [data-banner-swatch]').length === 10 && !!document.querySelector('${SS} [data-banner-upload]')`)) === true);
await ev(`document.querySelector('${SS} [data-banner-swatch="6"]').click()`);
await sleep(200);
check('SS12 a picked preset is checked and fills the preview banner', (await ev(`(() => { const s = document.querySelector('${SS} [data-banner-swatch="6"]'); const p = document.querySelector('${SS} [data-card-banner]'); return s.getAttribute('aria-checked') === 'true' && p.dataset.cardBanner === 'preset' && p.style.background === s.style.background; })()`)) === true);

// Traits: five slots, no more. A slot takes an emoji from the picker and a label, and shows as a chip in the preview.
check('SS13 there are exactly five trait slots, with a smiley placeholder', (await ev(`document.querySelectorAll('${SS} [data-trait-slot]').length === 5 && !!document.querySelector('${SS} [data-trait-slot="0"] button svg')`)) === true);
await ev(`document.querySelector('${SS} [data-trait-slot="0"] button').click()`);
await sleep(300);
check('SS14 the trait emoji opens the emoji picker above the page', (await ev(`(() => { const p = document.querySelector('.emoji-picker[aria-label="Trait emoji"]'); if (!p) return false; const r = p.getBoundingClientRect(); const hit = document.elementFromPoint(r.left + r.width / 2, r.top + r.height / 2); return !!hit && p.contains(hit); })()`)) === true);
const pickedEmoji = await ev(`(() => { const b = document.querySelector('.emoji-picker .emoji-cell'); const e = b.textContent; b.click(); return e; })()`);
await sleep(200);
await fill(`${SS} input[aria-label="Trait 1 label"]`, 'Woodwork');
check('SS15 a trait shows as a chip in the preview', (await ev(`document.querySelector('${SS} [data-card-traits]')?.textContent`)) === `${pickedEmoji}Woodwork`, String(await ev(`document.querySelector('${SS} [data-card-traits]')?.textContent ?? null`)));
check('SS16 a trait label stops at 24 characters', (await ev(`document.querySelector('${SS} input[aria-label="Trait 1 label"]').maxLength`)) === 24);
// The emoji is optional: a label alone is a trait. An emoji with no label is not, and marks its slot.
await fill(`${SS} input[aria-label="Trait 2 label"]`, 'Label only');
check('SS16a a trait with a label and no emoji shows as a chip', (await ev(`JSON.stringify([...document.querySelectorAll('${SS} [data-card-traits] li')].map((n) => n.textContent))`)) === JSON.stringify([`${pickedEmoji}Woodwork`, 'Label only']));
await ev(`document.querySelector('${SS} [data-trait-slot="2"] button').click()`);
await sleep(300);
await ev(`document.querySelector('.emoji-picker .emoji-cell').click()`);
await sleep(200);
// Whether the element `selector` names is what a click at its centre would hit, so nothing covers it.
const topmost = (selector) => ev(`(() => { const n = [...document.querySelectorAll(${JSON.stringify(selector)})].at(-1); if (!n) return 'missing'; const r = n.getBoundingClientRect(); const hit = document.elementFromPoint(r.left + r.width / 2, r.top + r.height / 2); return !!hit && n.contains(hit) ? true : (hit ? hit.tagName + '.' + String(hit.className).slice(0, 60) : 'nothing'); })()`);
check('SS16b an emoji with no label marks its slot', (await ev(`document.querySelector('${SS} [data-trait-slot="2"]').getAttribute('aria-invalid')`)) === 'true');
// Save refuses the unlabelled emoji with a toast, which must show above the page.
await ev(`${ssButton('Save Changes')}.click()`);
await sleep(400);
const toastOnTop = await topmost('[role="status"]');
check('SS16d a toast shows above Server Settings', toastOnTop === true, String(toastOnTop));
await ev(`document.querySelector('${SS} [data-trait-slot="2"] button').dispatchEvent(new MouseEvent('contextmenu', { bubbles: true, cancelable: true }))`);
await sleep(200);
check('SS16c right-click clears a trait emoji', (await ev(`!document.querySelector('${SS} [data-trait-slot="2"]').hasAttribute('aria-invalid') && !!document.querySelector('${SS} [data-trait-slot="2"] button svg')`)) === true);

// The description counts its characters against 300.
check('SS17 the description counter starts at 0/300', (await ev(`document.querySelector('${SS} [data-description-count]').textContent`)) === '0/300');
await fill(`${SS} textarea[aria-label="Description"]`, 'We make things.');
check('SS18 the counter follows the text, and the field stops at 300', (await ev(`document.querySelector('${SS} [data-description-count]').textContent === '15/300' && document.querySelector('${SS} textarea[aria-label="Description"]').maxLength === 300`)) === true);
check('SS19 the preview shows the description', (await ev(`document.querySelector('${SS} [data-card-description]')?.textContent`)) === 'We make things.');
await saveChanges();
check('SS20 Save Changes publishes and hides the bar', !(await ssBar()) && (await ev(`document.querySelector('${SS} [data-banner-swatch="6"]').getAttribute('aria-checked')`)) === 'true');
check('SS20a the saved traits come back from the server view and fill the preview', (await ev(`JSON.stringify([...document.querySelectorAll('${SS} [data-card-traits] li')].map((n) => n.textContent))`)) === JSON.stringify([`${pickedEmoji}Woodwork`, 'Label only']) && (await ev(`document.querySelector('${SS} input[aria-label="Trait 2 label"]').value`)) === 'Label only');
await sleep(400);
await b.screenshot(`${SCRATCH}/shots/server-profile.png`);
check('SS21 the sidebar shows the banner with the name over it, in white', (await ev(`(() => { const top = document.querySelector('${SIDEBAR} [data-server-top="banner"]'); if (!top) return false; const name = top.querySelector('[data-server-name]'); const r = top.getBoundingClientRect(); return name.textContent === 'Settings test' && Math.abs(r.width / r.height - 16 / 9) < 0.05 && getComputedStyle(name.parentElement).color === 'rgb(255, 255, 255)'; })()`)) === true);

// An uploaded banner: the crop is 16:9 and the picture is 960 by 540.
await ev(`(() => { const native = HTMLInputElement.prototype.click; window.__nativeInputClick = native; HTMLInputElement.prototype.click = function () { if (this.type === 'file') window.__pickerInput = this; else native.call(this); }; })()`);
await ev(`document.querySelector('${SS} [data-banner-upload]').click()`);
await sleep(200);
await ev(`(async () => {
  const blob = await (await fetch('app-icon-128.png')).blob();
  const dt = new DataTransfer();
  dt.items.add(new File([blob], 'wide.png', { type: 'image/png' }));
  window.__pickerInput.files = dt.files;
  window.__pickerInput.dispatchEvent(new Event('change'));
})()`);
await sleep(700);
const maskShape = await ev(`(() => { const r = document.querySelector('[data-crop-mask]')?.getBoundingClientRect(); return r ? r.width / r.height : 0; })()`);
check('SS22 the banner crop is 16:9', Math.abs(maskShape - 16 / 9) < 0.05, String(maskShape));
const cropOnTop = await topmost('[data-slot="dialog-content"]');
check('SS22a the banner crop dialog opens above Server Settings', cropOnTop === true, String(cropOnTop));
await ev(`document.querySelector('[data-crop-stage]').focus()`);
await press('Enter');
await sleep(900);
// Change Server Icon opens the same crop dialog, also above the page. Escape cancels it and keeps the page.
await ev(`${ssButton('Change Server Icon')}.click()`);
await sleep(200);
await ev(`(async () => {
  const blob = await (await fetch('app-icon-128.png')).blob();
  const dt = new DataTransfer();
  dt.items.add(new File([blob], 'icon.png', { type: 'image/png' }));
  window.__pickerInput.files = dt.files;
  window.__pickerInput.dispatchEvent(new Event('change'));
})()`);
await sleep(700);
const iconCropOnTop = await topmost('[data-slot="dialog-content"]');
check('SS23a the icon crop dialog opens above Server Settings', iconCropOnTop === true, String(iconCropOnTop));
await press('Escape');
await sleep(300);
check('SS23b Escape cancels the icon crop and keeps Server Settings open', (await ev(`!document.querySelector('[data-slot="dialog-content"]') && !!document.querySelector('${SS}')`)) === true);
const uploadSize = await ev(`(async () => { const t = document.querySelector('${SS} [data-banner-upload]'); const url = t.style.backgroundImage.slice(5, -2); const img = new Image(); img.src = url; await img.decode(); return [img.naturalWidth, img.naturalHeight, t.getAttribute('aria-checked'), url.slice(0, 5)]; })()`);
// The draft is shown from a blob: URL. The app's CSP allows blob: and asset: images, not data: ones.
check('SS23 the upload is a 960 by 540 picture from a blob URL, selected and unsaved', uploadSize?.join('x') === '960x540xtruexblob:' && (await ssBar()), String(uploadSize));
// An animated GIF banner keeps its animation: the editor sends the GIF to the engine with the crop instead of drawing
// one frame on a canvas, and the draft shows the looping GIF.
const LOOP_GIF = 'R0lGODlhQAAkAIEAAP8AAAAAAAAAAAAAACH/C05FVFNDQVBFMi4wAwEAAAAh+QQACgAAACwAAAAAQAAkAAAITwABCBxIsKDBgwgTKlzIsKHDhxAjSpxIsaLFixgzatzIsaPHjyBDihxJsqTJkyhTqlzJsqXLlzBjypxJs6bNmzhz6tzJs6fPn0CDCh2qMSAAIfkEAQoAAQAsAAAAAEAAJACBAMgAAAAAAAAAAAAACE8AAQgcSLCgwYMIEypcyLChw4cQI0qcSLGixYsYM2rcyLGjx48gQ4ocSbKkyZMoU6pcybKly5cwY8qcSbOmzZs4c+rcybOnz59AgwodqjEgACH5BAEKAAEALAAAAABAACQAgQAA/wAAAAAAAAAAAAhPAAEIHEiwoMGDCBMqXMiwocOHECNKnEixosWLGDNq3Mixo8ePIEOKHEmypMmTKFOqXMmypcuXMGPKnEmzps2bOHPq3Mmzp8+fQIMKHaoxIAA7';
await ev(`document.querySelector('${SS} [data-banner-upload]').click()`);
await sleep(200);
await ev(`(() => {
  const bytes = Uint8Array.from(atob(${JSON.stringify(LOOP_GIF)}), (c) => c.charCodeAt(0));
  const dt = new DataTransfer();
  dt.items.add(new File([bytes], 'loop.gif', { type: 'image/gif' }));
  window.__pickerInput.files = dt.files;
  window.__pickerInput.dispatchEvent(new Event('change'));
})()`);
await sleep(700);
check('SS23c an animated GIF opens the 16:9 crop', Math.abs((await ev(`(() => { const r = document.querySelector('[data-crop-mask]').getBoundingClientRect(); return r.width / r.height; })()`)) - 16 / 9) < 0.05);
await ev(`document.querySelector('[data-crop-stage]').focus()`);
await press('Enter');
for (let i = 0; i < 30 && (await ev(`!!document.querySelector('[data-crop-stage]')`)); i++) await sleep(100);
const gifDraft = await ev(`(async () => { const t = document.querySelector('${SS} [data-banner-upload]'); const url = t.style.backgroundImage.slice(5, -2); const bytes = new Uint8Array(await (await fetch(url)).arrayBuffer()); let frames = 0; for (let i = 0; i + 2 < bytes.length; i++) if (bytes[i] === 0x21 && bytes[i + 1] === 0xf9 && bytes[i + 2] === 0x04) frames++; return [t.hasAttribute('data-banner-animated'), String.fromCharCode(...bytes.slice(0, 4)), frames].join(' '); })()`);
check('SS23d the draft is the animated GIF, all its frames kept', gifDraft === 'true GIF8 3', String(gifDraft));
await ev(`HTMLInputElement.prototype.click = window.__nativeInputClick`);
await saveChanges();
check('SS24a the saved animated banner loops in the sidebar', (await ev(`getComputedStyle(document.querySelector('${SIDEBAR} [data-server-banner]')).backgroundImage.includes('image/gif')`)) === true);
await send('Emulation.setEmulatedMedia', { features: [{ name: 'prefers-reduced-motion', value: 'reduce' }] });
await sleep(400);
check('SS24b under reduced motion the sidebar shows the still poster frame', (await ev(`getComputedStyle(document.querySelector('${SIDEBAR} [data-server-banner]')).backgroundImage.includes('image/jpeg')`)) === true, String(await ev(`getComputedStyle(document.querySelector('${SIDEBAR} [data-server-banner]')).backgroundImage.slice(0, 40)`)));
await send('Emulation.setEmulatedMedia', { features: [{ name: 'prefers-reduced-motion', value: 'no-preference' }] });
await sleep(300);
check('SS24 a saved upload shows in the preview and the sidebar', (await ev(`document.querySelector('${SS} [data-card-banner]').dataset.cardBanner === 'image' && !!document.querySelector('${SIDEBAR} [data-server-top="banner"]')`)) === true && !(await ssBar()));
// The banner as members see it, above the channel list. Then back into Server Settings to remove it.
await press('Escape');
await sleep(400);
await b.screenshot(`${SCRATCH}/shots/sidebar-banner.png`);

await pointerClick(`document.querySelector('button[aria-label="Settings test menu"]')`);
await sleep(250);
await ev(`[...document.querySelectorAll('[role="menuitem"]')].find((m) => m.textContent.startsWith('Server Settings')).click()`);
await sleep(400);
await ev(`${ssButton('Remove Banner')}.click()`);
await saveChanges();
check('SS25 Remove Banner, saved, brings the plain header back', (await ev(`document.querySelector('${SIDEBAR} [data-server-top]')?.dataset.serverTop`)) === 'plain');

// A rename reaches the rail and the sidebar, trimmed; then the name goes back.
await fill(`${SS} input[aria-label="Server Name"]`, '  Renamed test  ');
await saveChanges();
check('SS26 Save renames the server in the sidebar and the rail, trimmed', (await ev(`!!document.querySelector('button[aria-label="Renamed test menu"]') && !!document.querySelector('nav[aria-label="Servers and direct messages"] button[aria-label="Renamed test"]')`)) === true);
await fill(`${SS} input[aria-label="Server Name"]`, 'Settings test');
await saveChanges();
await press('Escape');
await sleep(300);

check('SS27 Escape closes Server Settings once nothing is unsaved', (await ev(`!document.querySelector('${SS}') && !!document.querySelector('button[aria-label="Settings test menu"]')`)) === true);

// An invite link in a message shows the same card, with the preview's banner, description and traits.
await ev(`document.querySelector('button[aria-label="Create channel"]').click()`);
await sleep(300);
await ev(`document.querySelector('[role="dialog"] input[aria-label="Channel name"]').focus()`);
await typeText('cards');
await press('Enter');
await sleep(700);
await ev(`${rowByText('cards')}?.click()`);
await sleep(500);
await ev(`document.querySelector('textarea[aria-label="Message"]').focus()`);
await typeText('Join us: dissent://invite/mock-rustaceans');
await press('Enter');
await sleep(1500);
check('SS27a the invite card shows the server\'s banner, description and traits', (await ev(`(() => { const c = [...document.querySelectorAll('main [data-server-card]')].at(-1); if (!c) return 'no card'; return [c.querySelector('[data-card-banner]').dataset.cardBanner, c.querySelector('[data-card-description]')?.textContent, [...c.querySelectorAll('[data-card-traits] li')].map((n) => n.textContent).join('|')].join(' / '); })()`)) === 'preset / Systems programmers sharing crates, questions and the occasional crab. / 🦀Rust|🛠️Tooling', String(await ev(`[...document.querySelectorAll('main [data-server-card]')].length`)));
// The invite card states the answering member's boost.
check('BS0 the invite card shows the boost level and hosts', (await ev(`[...document.querySelectorAll('main [data-server-card]')].at(-1)?.querySelector('[data-card-boost]')?.textContent`)) === 'Level 1 · 1 relay host');

// Boosts: a server with no host is level 0 and shows no level; publishing a relay makes this person a host.
check('BS1 a server with no relay host shows no level by its name', (await ev(`!document.querySelector('${SIDEBAR} [data-boost-level]')`)) === true);
await pointerClick(`document.querySelector('button[aria-label="Settings test menu"]')`);
await sleep(250);
await ev(`[...document.querySelectorAll('[role="menuitem"]')].find((m) => m.textContent.startsWith('Server Settings')).click()`);
await sleep(400);
await ev(`${ssButton('Boosts')}.click()`);
await sleep(300);
check('BS2 Boosts starts at level 0 and says nothing basic is locked', (await ev(`document.querySelector('${SS} [data-boost-summary]')?.textContent`)) === 'Level 0 · 0 relay hosts' && (await ev(`document.querySelector('${SS}').textContent.includes('never lock anything')`)) === true);
await fill(`${SS} input[aria-label="Relay address"]`, 'https://relay.me.example/');
await ev(`${ssButton('Publish')}.click()`);
await sleep(800);
check('BS3 a published relay makes a host, listed with a thank-you, and the level rises', (await ev(`document.querySelector('${SS} [data-boost-summary]')?.textContent`)) === 'Level 1 · 1 relay host' && (await ev(`document.querySelector('${SS} [data-boost-hosts]')?.textContent.includes('relay.me.example')`)) === true);
check('BS4 the sidebar shows the level by the server name', (await ev(`document.querySelector('${SIDEBAR} [data-boost-level]')?.getAttribute('aria-label')`)) === 'Level 1 · 1 relay host');
await ev(`${ssButton('Withdraw my relay')}.click()`);
await sleep(800);
check('BS5 withdrawing drops back to level 0', (await ev(`document.querySelector('${SS} [data-boost-summary]')?.textContent`)) === 'Level 0 · 0 relay hosts');
await press('Escape');
await sleep(300);

// A member of Riverside Makers gets no Server Settings.
await clickRail('Riverside Makers');
await sleep(600);
check('BS6 Riverside Makers, with three relay hosts, shows level 2 by its name', (await ev(`document.querySelector('${SIDEBAR} [data-boost-level]')?.getAttribute('aria-label')`)) === 'Level 2 · 3 relay hosts');
check('BS7 the member list marks each relay host', (await ev(`document.querySelectorAll('[data-relay-host]').length`)) === 3, String(await ev(`document.querySelectorAll('[data-relay-host]').length`)));
await pointerClick(`document.querySelector('button[aria-label="Riverside Makers menu"]')`);
await sleep(250);
check('SS28 a member\'s server menu has no Server Settings', !(await menuItems()).some((l) => l.startsWith('Server Settings')), JSON.stringify(await menuItems()));
await press('Escape');
await sleep(200);

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

// The download folder starts at the OS Downloads folder. Reset to default is offered only once the user picks a folder.
const storageAside = `document.querySelector('aside[aria-label="Storage"]')`;
check('the default download folder shows the OS Downloads path, with no reset', (await ev(`${storageAside}?.textContent.includes('C:/Users/ada/Downloads') && !${storageAside}.textContent.includes('Reset to default')`)) === true);
await ev(`[...${storageAside}.querySelectorAll('button')].find((x) => x.textContent.trim() === 'Change').click()`);
await sleep(300);
check('a picked folder shows its path and offers Reset to default', (await ev(`${storageAside}.textContent.includes('/Downloads/Dissent') && [...${storageAside}.querySelectorAll('button')].some((x) => x.textContent.trim() === 'Reset to default')`)) === true);
await ev(`[...${storageAside}.querySelectorAll('button')].find((x) => x.textContent.trim() === 'Reset to default').click()`);
await sleep(300);
check('Reset to default goes back to the OS Downloads folder', (await ev(`${storageAside}.textContent.includes('C:/Users/ada/Downloads') && !${storageAside}.textContent.includes('/Dissent') && ![...${storageAside}.querySelectorAll('button')].some((x) => x.textContent.trim() === 'Reset to default')`)) === true);

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
check('the DM header has a video call button, and the sidebar heading is title case', (await ev(`!!document.querySelector('button[aria-label="Start video call with Maya"]') && [...document.querySelectorAll('aside[aria-label="Channels and conversations"] span')].some(n => n.textContent === 'Direct Messages')`)) === true);
const fontFaces = await ev(`[...document.fonts].filter(f => f.family.replace(/"/g, '') === 'Google Sans Flex').length`);
check('the UI font, Google Sans Flex, is bundled and registered', fontFaces >= 2 && (await ev(`document.fonts.check('14px "Google Sans Flex"')`)) === true, `faces: ${fontFaces}`);
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
check('the menu on their message offers reaction, reply, copy text and copy ID', ['Add Reaction', 'Reply', 'Copy Text', 'Copy message ID'].every(p => theirMenu.some(l => l.startsWith(p))), JSON.stringify(theirMenu));
check('Delete is not offered on a message that is not yours', !theirMenu.some(l => l.startsWith('Delete')) && !theirMenu.some(l => l.startsWith('Edit')), JSON.stringify(theirMenu));
// The quick reactions lead the message menu: 👍 is one of them, so it is picked from there.
await ev(`[...document.querySelectorAll('[role="menu"] [role^="menuitem"]')].find(m => m.textContent.trim() === '👍').click()`);
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
check('Delete and Edit are offered on your own message', ownMenu.some(l => l.startsWith('Delete Message')) && ownMenu.some(l => l.startsWith('Edit Message')), JSON.stringify(ownMenu));
await clickMenuItem('Delete Message');
await sleep(300);
const confirmText = await ev(`document.querySelector('[role="alertdialog"]')?.textContent ?? ''`);
check('Delete asks "Delete message? This can\'t be undone."', confirmText.includes("Delete message? This can't be undone."), confirmText);
await ev(`[...document.querySelectorAll('[role="alertdialog"] button')].find(b => b.textContent === 'Cancel').click()`);
await sleep(300);
check('Cancel keeps the message', (await ev(`[...document.querySelectorAll('.msg .body')].some(b => b.textContent.includes('Yes, reading it now.'))`)) === true);
await rightClickMsg('Yes, reading it now.');
await sleep(300);
await clickMenuItem('Delete Message');
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

// Shift+click on Delete Message skips the confirm.
await rightClickMsg('All the photos from Saturday.');
await sleep(300);
const shiftAt = await ev(`(() => { const r = [...document.querySelectorAll('[role="menu"] [role^="menuitem"]')].find(m => m.textContent.trim().startsWith('Delete Message')).getBoundingClientRect(); return { x: r.left + r.width / 2, y: r.top + r.height / 2 }; })()`);
await send('Input.dispatchMouseEvent', { type: 'mousePressed', x: shiftAt.x, y: shiftAt.y, button: 'left', clickCount: 1, modifiers: 8 });
await send('Input.dispatchMouseEvent', { type: 'mouseReleased', x: shiftAt.x, y: shiftAt.y, button: 'left', clickCount: 1, modifiers: 8 });
await sleep(600);
check('Shift+click on Delete Message skips the confirm', (await ev(`!document.querySelector('[role="alertdialog"]') && [...document.querySelectorAll('.msg .body.deleted')].length >= 2`)) === true);

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
  check('a download from several providers says how many on the card', (await ev(`[...document.querySelectorAll('.att-card.downloading .att-sources')].some((n) => n.textContent === 'From 3 sources')`)) === true);
  await ev(`document.querySelector('.att-card.downloading .att-name-text').click()`);
  await sleep(400);
  check('a downloading file name opens its transfer details in the right column', (await ev(`!!document.querySelector('button[aria-label="Close transfer details"]')`)) === true);
  check('the transfer details name the sources and count the verified pieces', (await ev(`(() => { const t = document.body.textContent; return t.includes('Downloading from 3 sources') && /[0-9]+ of [0-9]+ pieces verified/.test(t); })()`)) === true);
  await ev(`document.querySelector('button[aria-label="Close transfer details"]').click()`);
  await sleep(300);
  check('closing the transfer details gives the right column back to the members', (await ev(`!document.querySelector('button[aria-label="Close transfer details"]') && /Members/.test(document.querySelector('aside[aria-label="Right column"], aside:last-of-type')?.textContent ?? document.body.textContent)`)) === true);
} else {
  check('a downloading file is in the Riverside channel to test the details', false, 'no .att-card.downloading in the mock');
}

// ---- C7. A picture's preview: the sender's thumbnail in a rounded box, sized like an embed; a click starts the full download ----
await send('Page.navigate', { url: `${BASE}/` });
await sleep(2200);
await clickRail('Riverside Makers');
await sleep(900);
const preview = JSON.parse(await ev(`JSON.stringify((() => { const b = document.querySelector('.att-preview-btn'); if (!b) return null; const r = b.getBoundingClientRect(); const img = b.querySelector('img'); return { label: b.getAttribute('aria-label'), w: r.width, h: r.height, loaded: !!img && img.complete && img.naturalWidth > 0 }; })())`) ?? 'null');
check('a picture shows the sender thumbnail, sized like an embed', preview !== null && preview.label === 'Download enclosure.png' && preview.loaded && preview.w <= 400 && preview.h <= 300 && Math.abs(preview.w / preview.h - 1.6) < 0.05, JSON.stringify(preview));
await ev(`document.querySelector('.att-preview-btn').click()`);
await sleep(700);
check('a click on the preview starts the full download', (await ev(`!!document.querySelector('button[aria-label="Downloading enclosure.png"]') && document.querySelector('.att-preview-btn')?.disabled === true`)) === true);

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

// ---- Mentions: the @ autocomplete and the pick, the highlight, the pills, the badges, and @everyone without the permission ----
await ev(`document.querySelector('button[aria-label="Riverside Makers"]').click()`);
await sleep(600);
check('the hardware channel row shows its mention count', (await ev(`!!document.querySelector('aside[aria-label="Channels and conversations"] [aria-label="1 mentions"]')`)) === true);
// The server opened on #general, which cleared its own mention; the rail counts the rest.
check('the server rail badge sums the unread mentions of the server', (await ev(`!!document.querySelector('button[aria-label="Riverside Makers"]').closest('.rail-item').querySelector('[aria-label="1 mentions"]')`)) === true);
await ev(`[...document.querySelectorAll('aside[aria-label="Channels and conversations"] button')].find((x) => x.textContent.includes('hardware')).click()`);
await sleep(900);
check('opening the channel clears its mention count', (await ev(`[...document.querySelectorAll('aside[aria-label="Channels and conversations"] button')].find((x) => x.textContent.includes('hardware')).querySelector('[aria-label$=" mentions"]') === null`)) === true);
check('reading every mentioned channel clears the server rail badge', (await ev(`document.querySelector('button[aria-label="Riverside Makers"]').closest('.rail-item').querySelector('[aria-label$=" mentions"]') === null`)) === true);
check('a message that mentions you is highlighted', (await ev(`[...document.querySelectorAll('article.msg.mention')].some((a) => a.textContent.includes('the 3 mm ply is in the rack'))`)) === true);
check('the mention is a pill that names you', (await ev(`[...document.querySelectorAll('article.msg .mention-pill')].some((p) => p.textContent === '@Sami')`)) === true);
await ev(`[...document.querySelectorAll('article.msg .mention-pill')].find((p) => p.textContent === '@Sami').click()`);
await sleep(300);
check('clicking a mention pill opens the profile card', (await ev(`!!document.querySelector('[role="dialog"][aria-label="User profile"]')`)) === true);
await press('Escape');
await sleep(300);

await ev(`[...document.querySelectorAll('aside[aria-label="Channels and conversations"] button')].find((x) => x.textContent.includes('general')).click()`);
await sleep(700);
check('an admin @everyone is highlighted for every member', (await ev(`[...document.querySelectorAll('article.msg.mention')].some((a) => a.textContent.includes('the laser is free from 5 pm'))`)) === true);
check('a plain member @everyone is highlighted for nobody', (await ev(`![...document.querySelectorAll('article.msg.mention')].some((a) => a.textContent.includes('who is in for Saturday'))`)) === true);
check('@everyone is a pill only where it is valid', (await ev(`[...document.querySelectorAll('article.msg .mention-pill')].filter((p) => p.textContent === '@everyone').length === 1`)) === true);

// The autocomplete: "@" opens the members, typing filters them, Enter picks, Escape closes. @everyone is not offered
// without the permission, which the viewer (a member here) does not hold.
// The composer is typed into with real input, so React sees the change (a value set from script does not reach it).
const composer = async (value) => {
  await ev(`(() => { const t = document.querySelector('textarea[aria-label="Message"]'); t.focus(); t.select(); })()`);
  if (value) {
    await typeText(value);
  } else {
    await send('Input.dispatchKeyEvent', { type: 'rawKeyDown', key: 'Backspace', code: 'Backspace', windowsVirtualKeyCode: 8, nativeVirtualKeyCode: 8 });
    await send('Input.dispatchKeyEvent', { type: 'keyUp', key: 'Backspace', code: 'Backspace', windowsVirtualKeyCode: 8, nativeVirtualKeyCode: 8 });
  }
  await sleep(200);
};
const optionLabels = () => ev(`[...document.querySelectorAll('[role="listbox"][aria-label="Mention a member"] [role="option"]')].map((o) => o.textContent.trim())`);
await composer('hi @');
await sleep(250);
const everyoneOffered = (await optionLabels()) ?? [];
check('typing @ opens the member autocomplete', everyoneOffered.length >= 2, JSON.stringify(everyoneOffered));
check('@everyone is not offered to a member without the permission', !everyoneOffered.some((l) => l.startsWith('everyone')), JSON.stringify(everyoneOffered));
await composer('hi @Ma');
await sleep(250);
const filtered = (await optionLabels()) ?? [];
check('the autocomplete filters the members as you type', filtered.length > 0 && filtered.every((l) => /ma/i.test(l)), JSON.stringify(filtered));
await press('ArrowDown');
await press('ArrowUp');
await press('Enter');
await sleep(250);
const picked = await ev(`document.querySelector('textarea[aria-label="Message"]').value`);
check('Enter picks the highlighted member and inserts @Name at the caret', picked.startsWith('hi @') && picked.endsWith(' ') && (await optionLabels() ?? []).length === 0, JSON.stringify(picked));
await composer('hi @Ma');
await sleep(200);
await press('Escape');
await sleep(200);
check('Escape closes the autocomplete and keeps the text', (await ev(`!document.querySelector('[role="listbox"][aria-label="Mention a member"]') && document.querySelector('textarea[aria-label="Message"]').value === 'hi @Ma'`)) === true);
await composer('');

// ---- Server rail menu and Create Server: a member's menu, an owner's menu and Server Settings, and a server created with an icon ----
// The reloads earlier in this run drop servers made in it, so the owned server is made again here when it is missing.
if (!(await ev(`[...document.querySelectorAll('button[data-server-id]')].some((b) => b.getAttribute('aria-label') === 'Settings test')`))) {
  await clickRail('Create or join a server');
  await sleep(300);
  await ev(`(() => { const f = document.querySelector('[role="dialog"] input[aria-label="Server Name"]'); f.focus(); f.select(); })()`);
  await typeText('Settings test');
  await ev(`[...document.querySelectorAll('[role="dialog"] button:not([role="tab"])')].find((b) => b.textContent.trim() === 'Create').click()`);
  await sleep(900);
}
const railMenuItems = (name) => ev(`(() => { const b = document.querySelector('button[aria-label=' + JSON.stringify(${JSON.stringify(name)}) + ']'); if (!b) return false; const r = b.getBoundingClientRect(); b.dispatchEvent(new MouseEvent('contextmenu', { bubbles: true, cancelable: true, clientX: r.left + 20, clientY: r.top + 20 })); return true; })()`);
await railMenuItems('Riverside Makers');
await sleep(500);
const memberMenu = (await menuItems()) ?? [];
check('a member sees Mark As Read, Invite to Server and Leave Server on the rail menu', ['Mark As Read', 'Invite to Server', 'Leave Server'].every((t) => memberMenu.some((l) => l.startsWith(t))), JSON.stringify(memberMenu));
check('a member does not see Create Channel or Server Settings on the rail menu', !memberMenu.some((l) => l.startsWith('Create Channel') || l.startsWith('Server Settings')), JSON.stringify(memberMenu));
await press('Escape');
await sleep(250);

// The mock keeps servers in memory, and this run navigates (which reloads it), so this block makes the server it needs.
await ev(`document.querySelector('button[aria-label="Create or join a server"]').click()`);
await sleep(500);
await ev(`(() => { const f = document.querySelector('[role="dialog"] input[aria-label="Server Name"]'); f.focus(); f.select(); })()`);
await typeText('Rail test');
await ev(`[...document.querySelectorAll('[role="dialog"] button:not([role="tab"])')].find((b) => b.textContent.trim() === 'Create').click()`);
await sleep(1200);
const ownerRailShown = await railMenuItems('Rail test');
check('the owned server is on the rail', ownerRailShown === true, await ev(`[...document.querySelectorAll('button[data-server-id]')].map((b) => b.getAttribute('aria-label')).join(' | ')`));
await sleep(500);
const ownerMenu = (await menuItems()) ?? [];
check('an owner sees Server Settings and Create Channel on the rail menu', ownerMenu.some((l) => l.startsWith('Server Settings')) && ownerMenu.some((l) => l.startsWith('Create Channel')), JSON.stringify(ownerMenu));
check('an owner does not see Leave Server on the rail menu', !ownerMenu.some((l) => l.startsWith('Leave Server')), JSON.stringify(ownerMenu));
// Server Settings > Members opens the page at Members.
await ev(`(() => { const m = [...document.querySelectorAll('[role="menuitem"]')].find((x) => x.textContent.startsWith('Server Settings')); m?.focus(); })()`);
await press('ArrowRight');
await sleep(300);
await ev(`[...document.querySelectorAll('[role="menuitem"]')].find((x) => x.textContent.trim() === 'Members')?.click()`);
await sleep(700);
check('Server Settings > Members opens the page at Members', (await ev(`[...document.querySelectorAll('h1')].some((n) => n.textContent === 'Members')`)) === true);
await press('Escape');
await sleep(300);

// Create Server: the name is prefilled with the user's name and selected; the icon goes through the crop editor and shows.
await ev(`document.querySelector('button[aria-label="Create or join a server"]').click()`);
await sleep(500);
check('Customize Your Server shows the prefilled name', (await ev(`document.querySelector('[role="dialog"] input[aria-label="Server Name"]')?.value === "Sami's server"`)) === true);
// The file picker is replaced by a generated image, so the crop editor opens as it does for a picked file.
await ev(`(() => { const orig = HTMLInputElement.prototype.click; HTMLInputElement.prototype.click = function () { if (this.type !== 'file') return orig.call(this); HTMLInputElement.prototype.click = orig; const c = document.createElement('canvas'); c.width = 96; c.height = 96; const g = c.getContext('2d'); g.fillStyle = '#e0483a'; g.fillRect(0, 0, 96, 96); const input = this; c.toBlob((blob) => { const dt = new DataTransfer(); dt.items.add(new File([blob], 'icon.png', { type: 'image/png' })); input.files = dt.files; input.dispatchEvent(new Event('change')); }, 'image/png'); }; document.querySelector('[role="dialog"] button[aria-label="Upload a server icon"]').click(); })()`);
await sleep(1200);
await ev(`[...document.querySelectorAll('button')].find((x) => x.textContent.trim() === 'Apply')?.click()`);
await sleep(1200);
check('the chosen icon shows in the tile', (await ev(`!!document.querySelector('[role="dialog"] button[aria-label="Change server icon"] img')`)) === true);
await ev(`(() => { const f = document.querySelector('[role="dialog"] input[aria-label="Server Name"]'); f.focus(); f.select(); })()`);
await typeText('Icon test');
await sleep(200);
await ev(`[...document.querySelectorAll('[role="dialog"] button')].find((x) => x.textContent.trim() === 'Create')?.click()`);
await sleep(1500);
check('a server created with an icon shows the icon on the rail', (await ev(`!!document.querySelector('button[aria-label="Icon test"] img')`)) === true);

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
check('the DND glyph is red (its colour token resolves)', (await b.ev(`getComputedStyle(document.querySelector('main [role="img"][aria-label="Do Not Disturb"]')).backgroundColor`)) === 'rgb(208, 16, 90)', await b.ev(`getComputedStyle(document.querySelector('main [role="img"][aria-label="Do Not Disturb"]')).backgroundColor`));
check('the accent and danger tokens meet WCAG AA: white on the fills, the accent text on every dark surface', (await b.ev(`(() => {
  const css = getComputedStyle(document.documentElement);
  const token = (name) => css.getPropertyValue(name).trim();
  const rgb = (c) => (c.startsWith('#') ? [1, 3, 5].map((i) => parseInt(c.slice(i, i + 2), 16)) : c.match(/\\d+(\\.\\d+)?/g).slice(0, 3).map(Number));
  const lum = (c) => rgb(c).map((v) => { const x = v / 255; return x <= 0.03928 ? x / 12.92 : ((x + 0.055) / 1.055) ** 2.4; }).reduce((sum, x, i) => sum + x * [0.2126, 0.7152, 0.0722][i], 0);
  const ratio = (a, c) => { const [hi, lo] = [lum(a), lum(c)].sort((x, y) => y - x); return (hi + 0.05) / (lo + 0.05); };
  const white = '#ffffff';
  const fills = ['--accent', '--primary', '--danger'].every((t) => ratio(token(t), white) >= 4.5);
  const surfaces = ['--chat', '--sidebar', '--panel', '--elevated'].every((t) => ratio(token('--accent-text'), token(t)) >= 4.5);
  const dangerText = ['--chat', '--sidebar', '--panel', '--elevated'].every((t) => ratio(token('--danger-text'), token(t)) >= 4.5);
  return fills && surfaces && dangerText;
})()`)) === true);
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
check('a message group header shows the date and a 12-hour clock', (await b.ev(`/\\d{2}\\/\\d{2}\\/\\d{4} \\d{1,2}:\\d{2} (AM|PM)/.test(document.querySelector('.msg-head time')?.textContent ?? '')`)) === true, await b.ev(`document.querySelector('.msg-head time')?.textContent ?? null`));
check('the DM profile panel is open in the right column', (await b.ev(`document.querySelector('${RIGHT}')?.hidden === false && document.querySelector('${RIGHT}').textContent.includes('View Full Profile')`)) === true);
check('the DM panel avatar is 80 px with a presence glyph', (await b.ev(`(() => { const r = document.querySelector('${RIGHT} [role="img"][aria-label="Online"]'); return !!r && Math.round(r.parentElement.getBoundingClientRect().width) === 80; })()`)) === true);
const mutualText = await b.ev(`[...document.querySelectorAll('${RIGHT} button')].find(n => /Mutual Servers?$/.test(n.textContent.trim()))?.textContent.trim() ?? null`);
check('the mutual servers line reads "N Mutual Servers"', /^\d+ Mutual Servers?$/.test(mutualText ?? ''), String(mutualText));
const friendsText = await b.ev(`[...document.querySelectorAll('${RIGHT} button')].find(n => /Mutual Friends?$/.test(n.getAttribute('aria-label') ?? ''))?.getAttribute('aria-label') ?? null`);
check('the mutual friends count reads "N Mutual Friends"', /^\d+ Mutual Friends?$/.test(friendsText ?? ''), String(friendsText));
const friendCount = Number((friendsText ?? '0').split(' ')[0]);
const friendsToggle = `[...document.querySelectorAll('${RIGHT} button')].find(n => /Mutual Friends?$/.test(n.getAttribute('aria-label') ?? ''))`;
await b.ev(`${friendsToggle}.click()`);
await sleep(200);
check('clicking the mutual friends count lists them', (await b.ev(`document.querySelectorAll('${RIGHT} ul li').length === ${friendCount}`)) === true, `count ${friendCount}`);
await b.ev(`${friendsToggle}.click()`);
await sleep(200);
const mutualCount = Number((mutualText ?? '0').split(' ')[0]);
await b.ev(`[...document.querySelectorAll('${RIGHT} button')].find(n => /Mutual Servers?$/.test(n.textContent.trim())).click()`);
await sleep(200);
check('clicking the mutual servers count lists that many servers', (await b.ev(`document.querySelectorAll('${RIGHT} ul li').length === ${mutualCount}`)) === true, `count ${mutualCount}`);
check('the panel shows Member Since with the date and year', (await b.ev(`(() => { const h = [...document.querySelectorAll('${RIGHT} h4')].find(n => n.textContent === 'Member Since'); return !!h && /[A-Z][a-z]{2} \\d{1,2}, \\d{4}/.test(h.nextElementSibling?.textContent ?? ''); })()`)) === true);

// Hide and show the panel from the chat header's toggle.
await b.ev(`document.querySelector('button[aria-label="Hide profile panel"]')?.click()`);
await sleep(300);
check('the header toggle hides the DM panel', (await b.ev(`document.querySelector('${RIGHT}')?.hidden === true`)) === true);
await b.ev(`document.querySelector('button[aria-label="Show profile panel"]')?.click()`);
await sleep(300);

// ---- Full profile ----
await clickByText('button', 'View Full Profile');
await sleep(600);
const dialogText = await b.ev(`document.querySelector('[role="dialog"]')?.textContent ?? ''`);
check('View Full Profile opens the full profile dialog', dialogText.includes('Mutual servers') && dialogText.includes('Safety number') && dialogText.includes('Handles'), dialogText.slice(0, 60));
check('the full profile has the banner, the relay host badge, Member Since and the mutual friends', dialogText.includes('Member Since') && dialogText.includes('Mutual friends') && (await b.ev(`!!document.querySelector('[role="dialog"] [data-profile-banner]') && !!document.querySelector('[role="dialog"] [data-relay-host-badge]')`)) === true);
check('the full profile has Copy contact link under the address', (await b.ev(`[...document.querySelectorAll('[role="dialog"] button')].some(n => n.textContent.trim() === 'Copy contact link')`)) === true);
check('the full profile footer offers Message, Call and Verify safety number', (await b.ev(`['Message', 'Call', 'Verify safety number'].every(l => [...document.querySelectorAll('[role="dialog"] button')].some(n => n.textContent.trim() === l))`)) === true);
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
// Maya set the Teal preset as her profile banner (in the mock), so her card shows it instead of her hue.
check('the card shows the Relay host badge with the servers they host for', (await b.ev(`document.querySelector('[role="dialog"][aria-label="User profile"] [data-relay-host-badge]')?.textContent`)) === 'Relay hostfor Riverside Makers', String(await b.ev(`document.querySelector('[role="dialog"][aria-label="User profile"] [data-relay-host-badge]')?.textContent ?? null`)));
check('the card shows the person\'s banner preset', (await b.ev(`getComputedStyle(document.querySelector('[role="dialog"][aria-label="User profile"] [data-profile-banner]')).backgroundImage.includes('rgb(142, 232, 220)')`)) === true, String(await b.ev(`getComputedStyle(document.querySelector('[role="dialog"][aria-label="User profile"] [data-profile-banner]') ?? document.body).backgroundImage.slice(0, 80)`)));
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
const friendsSel = `document.querySelector('input[aria-label="Show my friends list to my friends"]')`;
check('the friends list switch is on by default', (await b.ev(`${friendsSel}.checked`)) === true);
await b.ev(`${friendsSel}.click()`);
await sleep(300);
check('switching the friends list off sticks', (await b.ev(`${friendsSel}.checked`)) === false);
await b.ev(`${friendsSel}.click()`);
await sleep(300);
check('switching the friends list back on sticks', (await b.ev(`${friendsSel}.checked`)) === true);
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
check('C3 the group call chat says everyone in the group reads it', (await b.ev(`document.querySelector('section[aria-label="Call chat"] [data-call-chat-audience]')?.textContent`)) === '· Messages go to Saturday workshop crew. Everyone in the group can read them.', String(await b.ev(`document.querySelector('section[aria-label="Call chat"] [data-call-chat-audience]')?.textContent`)));
check('C3 the camera and screen share buttons are live', (await b.ev(`(() => { const cam = document.querySelector('button[aria-label="Turn camera on"]'); const share = document.querySelector('button[aria-label="Share screen"]'); return Boolean(cam) && !cam.disabled && Boolean(share) && !share.disabled; })()`)) === true);
check('C3 the header says how many are in the call', (await b.ev(`document.body.textContent.includes('in call') && document.body.textContent.includes('End-to-end encrypted')`)) === true);
check('C3 the tiles report their drawn width to the engine', (await b.ev(`Object.values(window.__tileWidths ?? {}).some((w) => w > 0)`)) === true, JSON.stringify(await b.ev(`window.__tileWidths ?? null`)));
check('C3 the share button opens the picker with the primary display', (await b.ev(`(async () => { const share = document.querySelector('button[aria-label="Share screen"]'); share.click(); await new Promise((r) => setTimeout(r, 300)); const items = [...document.querySelectorAll('[role="menuitem"]')].map((n) => n.textContent); return items.some((t) => t.includes('(primary)')); })()`)) === true);
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
check('C7 the voice channel call chat says anyone who can read the channel reads it', (await b.ev(`document.querySelector('section[aria-label="Call chat"] [data-call-chat-audience]')?.textContent`)) === '· Messages go to #workshop. Anyone who can read #workshop can read them.', String(await b.ev(`document.querySelector('section[aria-label="Call chat"] [data-call-chat-audience]')?.textContent`)));
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
// The echo cancellation engine: Standard by default, and the choice saves when it changes (E33).
const engineSelect = 'select[aria-label="Echo cancellation engine"]';
check('E33 the echo cancellation engine defaults to Standard', (await b.ev(`document.querySelector('${engineSelect}')?.value`)) === 'standard');
await b.ev(`(() => { const s = document.querySelector('${engineSelect}'); Object.getOwnPropertyDescriptor(HTMLSelectElement.prototype, 'value').set.call(s, 'web_rtc'); s.dispatchEvent(new Event('change', { bubbles: true })); })()`);
await sleep(300);
check('E33 choosing WebRTC AEC3 saves the choice', (await b.ev(`document.querySelector('${engineSelect}')?.value`)) === 'web_rtc');
await b.ev(`(() => { const s = document.querySelector('${engineSelect}'); Object.getOwnPropertyDescriptor(HTMLSelectElement.prototype, 'value').set.call(s, 'standard'); s.dispatchEvent(new Event('change', { bubbles: true })); })()`);
await sleep(300);
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
// The call chat renders with the chat's own rows and composer: link previews, formatting and invite cards, as in the chat.
const CALL_CHAT = 'section[aria-label="Call chat"]';
check('C11 the call chat shows a link preview card', (await b.ev(`!!document.querySelector('${CALL_CHAT} article.embed .embed-title')`)) === true);
check('C11 the call chat has the chat composer', (await b.ev(`document.querySelector('${CALL_CHAT} .composer textarea')?.placeholder`)) === 'Message the call');
check('C11 the DM call chat says who reads it: the chat with Maya', (await b.ev(`document.querySelector('${CALL_CHAT} [data-call-chat-audience]')?.textContent`)) === '· Messages go to your chat with Maya.');
await b.ev(`document.querySelector('${CALL_CHAT} .composer textarea').focus()`);
await b.send('Input.insertText', { text: '**Loud** news: dissent://invite/mock-rustaceans' });
await sleep(250);
await b.press('Enter');
await sleep(1500);
check('C11 bold text in the call chat is strong', (await b.ev(`[...document.querySelectorAll('${CALL_CHAT} .msg .body strong')].some((n) => n.textContent === 'Loud')`)) === true, String(await b.ev(`[...document.querySelectorAll('${CALL_CHAT} .msg .body')].map((n) => n.innerHTML).at(-1)`)));
check('C11 an invite link in the call chat shows the invite card', (await b.ev(`[...document.querySelectorAll('${CALL_CHAT} [data-server-card]')].at(-1)?.textContent.includes('Rustaceans')`)) === true);
check('C11 the call chat message has the hover actions and the message menu', (await b.ev(`(() => { const m = [...document.querySelectorAll('${CALL_CHAT} article.msg')].at(-1); return !!m?.querySelector('button[aria-label="More message actions"]'); })()`)) === true);
check('C11 the call chat stays at its end once the invite card has loaded', (await until(() => b.ev(`(() => { const l = document.querySelector('${CALL_CHAT} .messages'); return l.scrollHeight - l.scrollTop - l.clientHeight < 4; })()`))) === true, JSON.stringify(await b.ev(`(() => { const l = document.querySelector('${CALL_CHAT} .messages'); return [l.scrollHeight, l.scrollTop, l.clientHeight]; })()`)));
await b.screenshot(`${SCRATCH}/shots/C11-call-chat.png`);
// The layout at a normal window size: the video gets most of the pane, the chat a capped part, and a 1:1 call's tile is
// large. The mock answers the call after four seconds.
await b.resize(1100, 729);
for (let i = 0; i < 40 && !(await b.ev(`!!document.querySelector('[data-call-tiles] > div > *')`)); i++) await sleep(200);
await sleep(400);
const layoutAt = () => b.ev(`(() => { const pane = document.querySelector('[data-call-layout]').getBoundingClientRect(); const tiles = document.querySelector('[data-call-tiles]').getBoundingClientRect(); return { share: tiles.height / pane.height, chat: !!document.querySelector('section[aria-label="Call chat"]'), tiles: document.querySelectorAll('[data-call-tiles] > div > *').length }; })()`);
const small = await layoutAt();
check('C11 at 1100×729 with the chat open, the tiles take at least 40% of the pane', small.chat && small.share >= 0.4, JSON.stringify(small));
check('C11 a 1:1 call reports a tile wider than 300 px', small.tiles === 1 && Math.max(0, ...Object.values((await b.ev(`window.__tileWidths ?? {}`)) ?? {})) > 300, JSON.stringify(await b.ev(`window.__tileWidths ?? null`)));
await b.screenshot(`${SCRATCH}/shots/C11-call-1100x729.png`);
await clickBy('section[aria-label="Call chat"] button', 'Hide call chat');
await sleep(400);
const hidden = await layoutAt();
check('C11 Hide call chat gives the video the pane', !hidden.chat && hidden.share > small.share, JSON.stringify(hidden));
await clickBy('section[aria-label="Call"] button', 'Show call chat');
await sleep(400);
check('C11 Show call chat brings the chat back', (await layoutAt()).chat === true);
await b.resize(1600, 1000);
await sleep(300);
await clickBy('section[aria-label="Call"] button', 'Leave call');
await sleep(500);
// The same message in the DM chat, once the call is left: the same rows, so the same bold text and card.
check('C11 the message sent in the call chat reads the same in the chat', (await b.ev(`(() => { const rows = [...document.querySelectorAll('.messages[aria-label="Messages"] article.msg')]; const m = rows.at(-1); return !!m && [...m.querySelectorAll('.body strong')].some((n) => n.textContent === 'Loud') && !!m.querySelector('[data-server-card]'); })()`)) === true);

// The call chat checks open their own DM call with Maya, so they do not depend on the call above. The call is left after.
await clickBy('button[aria-label="Direct messages"]');
await sleep(500);
await clickBy('aside[aria-label="Channels and conversations"] button', 'Maya');
await sleep(500);
await clickBy('button[aria-label="Start voice call with Maya"]');
await sleep(1200);
check('E42 a DM call with Maya is open with its chat', (await b.ev(`!!document.querySelector('${CALL_CHAT}')`)) === true);
// The call chat follows its DM while the call is open. A reaction (dm_changed) shows in it; and when two loads overlap,
// the older reply arriving last does not replace the newer list.
await b.ev(`window.__mockReactLastDm('Maya', '\\u{1F389}')`);
await b.ev(`window.__mockEngineEvent({ kind: 'dm_changed', peer: window.__mockRootOf('Maya') })`);
await sleep(600);
check('E42 a reaction arriving during a call shows in the call chat', (await b.ev(`document.querySelector('${CALL_CHAT}')?.textContent.includes('\\u{1F389}') === true`)) === true, (await b.ev(`(document.querySelector('${CALL_CHAT}')?.textContent ?? 'NO CALL CHAT').slice(-240)`)));
await b.ev(`window.__mockStaleReads = true; window.__mockDelay = 900`);
await b.ev(`window.__mockEngineEvent({ kind: 'dm', peer: window.__mockRootOf('Maya') })`);
await b.ev(`window.__mockPostDm('Maya', 'the newest word')`);
await b.ev(`window.__mockDelay = 0`);
await b.ev(`window.__mockEngineEvent({ kind: 'dm', peer: window.__mockRootOf('Maya') })`);
await sleep(1500);
check('E42 two quick loads keep the newest list when the older reply arrives last', (await b.ev(`document.querySelector('${CALL_CHAT}')?.textContent.includes('the newest word') === true`)) === true, (await b.ev(`(document.querySelector('${CALL_CHAT}')?.textContent ?? 'NO CALL CHAT').slice(-240)`)));
await b.ev(`window.__mockStaleReads = false; window.__mockDelay = 0`);
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
    const blob = await (await fetch('app-icon-128.png')).blob();
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

// The profile banner, under the avatar in My identity: a preset applies at once and shows on your own card; an upload
// crops at 16:9 above the page and replaces it; Remove Banner falls back to your hue.
const ownCardBanner = async () => {
  await b.ev(`document.querySelector('button[aria-label="Riverside Makers"]')?.click()`);
  await sleep(800);
  await b.ev(`[...document.querySelectorAll('aside[aria-label="Members"] button, [aria-label="Member list"] button, aside button')].find(n => n.textContent.includes('Sami'))?.click()`);
  await sleep(600);
  const bg = await b.ev(`(() => { const n = document.querySelector('[role="dialog"][aria-label="User profile"] [data-profile-banner]'); return n ? getComputedStyle(n).backgroundImage : null; })()`);
  await pressEsc(b);
  await sleep(300);
  return bg;
};
const openIdentity = async () => {
  await b.press(',', 2);
  await sleep(400);
};
await openIdentity();
check('E21a My identity has a Banner section with ten presets and an upload', (await b.ev(`(() => { const s = [...document.querySelectorAll('[data-settings-page] section')].find(n => n.querySelector('h2')?.textContent === 'Banner'); return !!s && s.querySelectorAll('[data-banner-swatch]').length === 10 && !!s.querySelector('[data-banner-upload]'); })()`)) === true);
await b.ev(`document.querySelector('[data-settings-page] [data-banner-swatch="4"]').click()`);
await sleep(500);
check('E21b a preset is checked at once', (await b.ev(`document.querySelector('[data-settings-page] [data-banner-swatch="4"]').getAttribute('aria-checked')`)) === 'true');
await pressEsc(b);
await sleep(300);
const presetBg = await ownCardBanner();
check('E21c the preset shows on your own profile card', String(presetBg).includes('rgb(251, 229, 138)'), String(presetBg).slice(0, 80));
await openIdentity();
await b.ev(`document.querySelector('[data-settings-page] [data-banner-upload]').click()`);
await sleep(200);
await b.ev(`(async () => {
  const blob = await (await fetch('app-icon-128.png')).blob();
  const dt = new DataTransfer();
  dt.items.add(new File([blob], 'wide.png', { type: 'image/png' }));
  window.__pickerInput.files = dt.files;
  window.__pickerInput.dispatchEvent(new Event('change'));
})()`);
await sleep(700);
const profileCropOnTop = await b.ev(`(() => { const n = document.querySelector('[data-slot="dialog-content"]'); if (!n) return 'missing'; const r = n.getBoundingClientRect(); const hit = document.elementFromPoint(r.left + r.width / 2, r.top + r.height / 2); return !!hit && n.contains(hit); })()`);
check('E21d the banner crop opens above Settings', profileCropOnTop === true, String(profileCropOnTop));
const profileMask = await b.ev(`(() => { const r = document.querySelector('[data-crop-mask]').getBoundingClientRect(); return r.width / r.height; })()`);
check('E21e the profile banner crop is 16:9', Math.abs(profileMask - 16 / 9) < 0.05, String(profileMask));
await b.ev(`document.querySelector('[data-crop-stage]').focus()`);
await pressEnter(b);
await sleep(900);
check('E21f an upload is checked once applied', (await b.ev(`document.querySelector('[data-settings-page] [data-banner-upload]').getAttribute('aria-checked')`)) === 'true');
await pressEsc(b);
await sleep(300);
const imageBg = await ownCardBanner();
check('E21g the upload shows on your own profile card', String(imageBg).startsWith('url('), String(imageBg).slice(0, 40));
await openIdentity();
await b.ev(`[...document.querySelectorAll('[data-settings-page] button')].find(n => n.textContent === 'Remove Banner').click()`);
await sleep(500);
check('E21h Remove Banner unchecks everything', (await b.ev(`document.querySelectorAll('[data-settings-page] [aria-checked="true"][data-banner-swatch], [data-settings-page] [aria-checked="true"][data-banner-upload]').length`)) === 0);
await pressEsc(b);
await sleep(300);
const hueBg = await ownCardBanner();
check('E21i with no banner your card falls back to your hue', String(hueBg).includes('oklch') || (String(hueBg).includes('linear-gradient') && !String(hueBg).includes('url(')), String(hueBg).slice(0, 60));

// Settings pages: Network shows the status table; Updates checks the mock release; Sounds disables its controls.
await b.press(',', 2);
await sleep(400);
await b.ev(`[...document.querySelectorAll('[data-settings-page] nav button')].find(n => n.textContent === 'Network').click()`);
await sleep(500);
check('E22 Network shows Resolved via in the status table', (await b.ev(`[...document.querySelectorAll('[data-settings-page] dt')].some(n => n.textContent === 'Resolved via')`)) === true);
// Relay list: a host's relay names the host, and carries Verified only when the app counts that host.
const relayRowText = (name) => `([...document.querySelectorAll('[aria-label="Relays"] li')].find((li) => li.textContent.includes('${name}'))?.textContent ?? '')`;
check('E24 a verified host relay is listed as hosted, with the mark', (await b.ev(`${relayRowText('hosted-yusuf')}.includes('Hosted by Yusuf') && ${relayRowText('hosted-yusuf')}.includes('Verified')`)) === true);
check('E24 an unverified host relay names its host without the mark', (await b.ev(`${relayRowText('hosted-nadia')}.includes('Hosted by Nadia') && !${relayRowText('hosted-nadia')}.includes('Verified')`)) === true);
// The switch: off, the servers' relays go, and they come back when it is on again.
const offersSwitch = `document.querySelector('[data-settings-page] input[aria-label="Use relays offered by my servers"]')`;
await b.ev(`${offersSwitch}.click()`);
await b.ev(`[...document.querySelectorAll('[data-settings-page] button')].find(n => n.textContent === 'Save network settings').click()`);
await sleep(400);
check('E24 switching off the server relays removes every hosted relay', (await b.ev(`!document.querySelector('[data-settings-page]').textContent.includes('Hosted by')`)) === true);
await b.ev(`${offersSwitch}.click()`);
await b.ev(`[...document.querySelectorAll('[data-settings-page] button')].find(n => n.textContent === 'Save network settings').click()`);
await sleep(400);
check('E24 switching the server relays back on lists the hosted relay again', (await b.ev(`document.querySelector('[data-settings-page]').textContent.includes('Hosted by Yusuf')`)) === true);
// number0's directory: on by default, says what number0 sees, and its value is saved both ways.
const n0Switch = `document.querySelector('[data-settings-page] input[aria-label="Find devices through number0\u2019s directory"]')`;
const saveNetwork = `[...document.querySelectorAll('[data-settings-page] button')].find(n => n.textContent === 'Save network settings').click()`;
check('E25 the number0 directory switch is on by default', (await b.ev(`${n0Switch}.checked`)) === true);
check('E25 the switch says that number0 sees the device ID and addresses', (await b.ev(`document.querySelector('[data-settings-page]').textContent.includes('number0 sees your device ID and addresses.')`)) === true);
await b.ev(`${n0Switch}.click()`);
await b.ev(saveNetwork);
await sleep(400);
check('E25 switching the number0 directory off is saved', (await b.ev(`${n0Switch}.checked`)) === false);
await b.ev(`${n0Switch}.click()`);
await b.ev(saveNetwork);
await sleep(400);
check('E25 switching the number0 directory back on is saved', (await b.ev(`${n0Switch}.checked`)) === true);
// A relay is pinned or turned off, never both: a turned-off relay cannot be pinned, and a pinned one cannot be turned off.
// The draft is put back the way it was, so the save checks after this see the same settings.
const firstRelayUrl = await b.ev(`document.querySelector('[aria-label="Relays"] li span')?.textContent ?? ''`);
const relayButton = (label) => `[...document.querySelectorAll('[aria-label="Relays"] button')].find(n => n.getAttribute('aria-label') === ${JSON.stringify(label)})`;
await b.ev(`${relayButton('Turn off ' + firstRelayUrl)}.click()`);
await sleep(200);
check('E24 a turned-off relay cannot be pinned', (await b.ev(`${relayButton('Pin ' + firstRelayUrl)}.disabled`)) === true);
await b.ev(`${relayButton('Turn on ' + firstRelayUrl)}.click()`);
await sleep(200);
await b.ev(`${relayButton('Pin ' + firstRelayUrl)}.click()`);
await sleep(200);
check('E24 a pinned relay cannot be turned off', (await b.ev(`${relayButton('Turn off ' + firstRelayUrl)}.disabled`)) === true);
await b.ev(`${relayButton('Unpin ' + firstRelayUrl)}.click()`);
await sleep(200);
const typeInto = async (sel, text) => { await b.ev(`(() => { const i = document.querySelector(${JSON.stringify(sel)}); i.focus(); i.select(); })()`); await b.typeText(text); };
// Host a relay: the check reports the mock's latency for its one reachable relay, publishing unlocks only after that pass,
// and changing the address locks it again until the new address is checked.
const publishDisabled = `[...document.querySelectorAll('[data-settings-page] button')].find(n => n.textContent === 'Publish to this server').disabled`;
await typeInto('[data-settings-page] input[aria-label="Hosted relay URL"]', 'https://relay-eu.riverside.example');
await b.ev(`[...document.querySelectorAll('[data-settings-page] button')].find(n => n.textContent === 'Check my relay').click()`);
await sleep(300);
check('E23 a passing check says it was checked by a member, with the latency', (await b.ev(`document.querySelector('[data-settings-page]').textContent.includes('Reachable, checked by a member just now (42 ms).')`)) === true);
check('E23 publishing unlocks after a pass for the same address', (await b.ev(`${publishDisabled} === false`)) === true);
await typeInto('[data-settings-page] input[aria-label="Hosted relay URL"]', 'https://relay.unchecked.example');
check('E23 changing the address locks publishing until it is checked', (await b.ev(`${publishDisabled} === true`)) === true);
await typeInto('[data-settings-page] input[aria-label="Hosted relay URL"]', 'https://relay.unreachable.example');
await b.ev(`[...document.querySelectorAll('[data-settings-page] button')].find(n => n.textContent === 'Check my relay').click()`);
await sleep(300);
check('E23 a failing check says why, and publishing stays locked', (await b.ev(`document.querySelector('[data-settings-page]').textContent.includes('Not checked: not reachable over TLS from this device')`)) === true && (await b.ev(`${publishDisabled} === true`)) === true);
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
// Desktop notifications is its own setting (the desktop app's mute), so Play sounds does not disable it.
check('E24 Play sounds off disables the per-kind toggles and the volume', (await b.ev(`[...document.querySelectorAll('[data-settings-page] input[type="checkbox"]')].slice(1).filter(n => !n.closest('label')?.textContent.includes('Desktop notifications')).every(n => n.disabled) && document.querySelector('[data-settings-page] input[type="range"]').disabled`)) === true, sounds24);
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

async function section_rail() {
const b = await launch({ url: `${BASE}/`, width: 1600, height: 1000, port: 9779, profile: `${SCRATCH}/rail-next-profile` });
const { send, ev } = b;
await sleep(800);
// Real key presses with Alt and Shift held: modifiers Alt=1, Shift=8.
const pressKey = async (key, vk, modifiers) => {
  const base = { key, code: key, windowsVirtualKeyCode: vk, nativeVirtualKeyCode: vk, modifiers };
  await send('Input.dispatchKeyEvent', { type: 'rawKeyDown', ...base });
  await send('Input.dispatchKeyEvent', { type: 'keyUp', ...base });
  await sleep(250);
};
// ---- F. The server rail's order: a drag moves a server, Alt+Shift+Up and Down move the focused one, and the order survives a reload ----
await ev(`localStorage.removeItem('dissent-mock-rail-order')`);
await send('Page.navigate', { url: `${BASE}/` });
await sleep(2200);
const RAIL = 'nav[aria-label="Servers and direct messages"]';
const railNames = () => ev(`[...document.querySelectorAll('${RAIL} button[data-server-id]')].map((b) => b.getAttribute('aria-label'))`);
const railPoint = (name) => ev(`(() => { const r = document.querySelector('${RAIL} button[aria-label=' + JSON.stringify(${JSON.stringify(name)}) + ']').getBoundingClientRect(); return { x: r.left + r.width / 2, y: r.top + r.height / 2 }; })()`);
const before = await railNames();
check('the rail has at least three servers for the order checks', before.length >= 3, JSON.stringify(before));
if (before.length >= 3) {
  const [first, second, third] = before;
  const p0 = await railPoint(first);
  const p1 = await railPoint(second);
  // Press the first server, move the pointer below the second server's middle, and release there.
  await send('Input.dispatchMouseEvent', { type: 'mousePressed', x: p0.x, y: p0.y, button: 'left', clickCount: 1, buttons: 1 });
  for (const y of [p0.y + 10, p1.y + 12]) await send('Input.dispatchMouseEvent', { type: 'mouseMoved', x: p0.x, y, button: 'left', buttons: 1 });
  await send('Input.dispatchMouseEvent', { type: 'mouseReleased', x: p0.x, y: p1.y + 12, button: 'left', clickCount: 1, buttons: 0 });
  await sleep(400);
  const dragged = await railNames();
  check('dragging a server below the next one moves it into that place', dragged[0] === second && dragged[1] === first && dragged[2] === third, JSON.stringify(dragged));
  check('a drag does not open the server it moved', (await ev(`!document.querySelector('${RAIL} button[aria-current="page"][data-server-id]')`)) === true);

  await send('Page.navigate', { url: `${BASE}/` });
  await sleep(2200);
  check('the dragged order survives a reload', JSON.stringify((await railNames()).slice(0, 2)) === JSON.stringify([second, first]), JSON.stringify(await railNames()));

  // Alt+Shift+Down on the focused server moves it one place down, and announces the new position.
  await ev(`document.querySelector('${RAIL} button[aria-label=' + JSON.stringify(${JSON.stringify(second)}) + ']').focus()`);
  await pressKey('ArrowDown', 40, 9);
  await sleep(300);
  const moved = await railNames();
  check('Alt+Shift+Down moves the focused server one place down', moved[0] === first && moved[1] === second, JSON.stringify(moved));
  const note = await ev(`document.querySelector('${RAIL} [role="status"]')?.textContent ?? ''`);
  check('the keyboard move is announced with its new position', note === `Moved ${second} to position 2 of ${moved.length}`, note);
  check('focus stays on the moved server', (await ev(`document.activeElement?.getAttribute('aria-label') === ${JSON.stringify(second)}`)) === true);
  await ev(`localStorage.removeItem('dissent-mock-rail-order')`);
  await send('Page.navigate', { url: `${BASE}/` });
  await sleep(2200);
}

await b.close();
}

// The mentions capability is not reported (an engine before it): nothing mention-related shows, and the composer does not
// open the autocomplete or send a mention argument. The same fixtures as the mentions checks, without the capability.
async function section_nomentions() {
const b = await launch({ url: `${BASE}/?nomentions`, width: 1600, height: 1000, port: 9781, profile: `${SCRATCH}/nomentions-next-profile` });
const { ev } = b;
await sleep(1200);
await ev(`[...document.querySelectorAll('button[aria-label="Riverside Makers"]')][0]?.click()`);
await sleep(600);
await ev(`[...document.querySelectorAll('aside[aria-label="Channels and conversations"] button')].find((x) => x.textContent.includes('hardware'))?.click()`);
await sleep(700);
check('without the mentions capability no mention pill, highlight or badge shows', (await ev(`document.querySelectorAll('.mention-pill, article.msg.mention, [aria-label$=" mentions"]').length === 0`)) === true);
check('the message text is still shown without the pills', (await ev(`[...document.querySelectorAll('article.msg')].some((a) => a.textContent.includes('the 3 mm ply is in the rack'))`)) === true);
await ev(`(() => { const t = document.querySelector('textarea[aria-label="Message"]'); t.focus(); })()`);
await b.typeText('hi @');
await sleep(300);
check('without the capability typing @ does not open the autocomplete', (await ev(`!document.querySelector('[role="listbox"][aria-label="Mention a member"]')`)) === true);
await b.close();
}

// ---- P. Link previews: the card shows the site, the title and the picture, with the site's colour on its bar, the picture at most 400 px wide and the
// description in three lines; the first link in a message asks once in the composer, and "Not now" sends the message without turning previews on. ----
async function section_previews() {
const b = await launch({ url: `${BASE}/`, width: 1600, height: 1000, port: 9782, profile: `${SCRATCH}/previews-next-profile` });
const { send, ev } = b;
await sleep(1500);
const pressEnter = async () => {
  const base = { key: 'Enter', code: 'Enter', windowsVirtualKeyCode: 13, nativeVirtualKeyCode: 13 };
  await send('Input.dispatchKeyEvent', { type: 'rawKeyDown', ...base, text: '\r' });
  await send('Input.dispatchKeyEvent', { type: 'char', ...base, text: '\r' });
  await send('Input.dispatchKeyEvent', { type: 'keyUp', ...base });
  await sleep(400);
};
await ev(`document.querySelector('button[aria-label="Direct messages"]').click()`);
await sleep(400);
await ev(`[...document.querySelectorAll('aside[aria-label="Channels and conversations"] button')].find((x) => x.textContent.includes('Maya'))?.click()`);
await sleep(700);

const card = await ev(`(() => {
  const a = document.querySelector('article.embed');
  if (!a) return null;
  const img = a.querySelector('img.embed-img');
  const r = img?.getBoundingClientRect();
  return {
    accent: a.style.getPropertyValue('--embed-accent'),
    site: a.querySelector('.embed-site')?.textContent ?? '',
    title: a.querySelector('.embed-title')?.textContent ?? '',
    titleHref: a.querySelector('.embed-title')?.getAttribute('href') ?? '',
    pictureLinked: !!a.querySelector('a.embed-picture[href]'),
    w: r ? Math.round(r.width) : 0,
    h: r ? Math.round(r.height) : 0,
    clamp: getComputedStyle(a.querySelector('.embed-desc') ?? a).getPropertyValue('-webkit-line-clamp'),
  };
})()`);
check('a link preview shows its site, its title as a link, and its picture as a link', card !== null && card.site === 'GitHub' && card.title.length > 0 && card.titleHref.startsWith('https://') && card.pictureLinked && card.w > 0, JSON.stringify(card));
check("the card's bar takes the site's theme colour", card?.accent === '#2b6cb0', JSON.stringify(card));
check('the picture is at most 400 px wide and keeps its aspect ratio', card !== null && card.w <= 400 && card.w > 0 && Math.abs(card.w / card.h - 432 / 168) < 0.05, JSON.stringify(card));
check('the description is clamped to three lines', card?.clamp === '3', JSON.stringify(card));

// The first link asks once, in the composer. The text waits until the answer; "Not now" sends it and turns nothing on.
await ev(`(() => { const t = document.querySelector('textarea[aria-label="Message"]'); t.focus(); })()`);
await send('Input.insertText', { text: 'see https://example.com/first' });
await sleep(250);
await pressEnter();
check('the first link in a message asks about previews in the composer', (await ev(`!!document.querySelector('[role="group"][aria-label="Link previews"]')`)) === true);
check('the question says the site sees the IP address', (await ev(`document.querySelector('[role="group"][aria-label="Link previews"]')?.textContent.includes('the site sees your IP address') ?? false`)) === true);
check('the text waits in the box until the question is answered', (await ev(`document.querySelector('textarea[aria-label="Message"]').value`)) === 'see https://example.com/first');
await ev(`[...document.querySelectorAll('[role="group"][aria-label="Link previews"] button')].find((x) => x.textContent === 'Not now')?.click()`);
await sleep(600);
check('Not now sends the message and closes the question', (await ev(`!document.querySelector('[role="group"][aria-label="Link previews"]') && [...document.querySelectorAll('article.msg')].some((a) => a.textContent.includes('see https://example.com/first'))`)) === true);
await ev(`(() => { const t = document.querySelector('textarea[aria-label="Message"]'); t.focus(); })()`);
await send('Input.insertText', { text: 'and https://example.com/second' });
await sleep(250);
await pressEnter();
check('the question is asked once: the next link goes straight out', (await ev(`!document.querySelector('[role="group"][aria-label="Link previews"]') && [...document.querySelectorAll('article.msg')].some((a) => a.textContent.includes('and https://example.com/second'))`)) === true);
await b.close();
}

// ---- M. Markdown in messages: each form renders as its element; a masked link shows its address and a link to another site
// asks first; a spoiler reveals when pressed; code and raw HTML are not read as markup. ----
async function section_markdown() {
const b = await launch({ url: `${BASE}/`, width: 1600, height: 1000, port: 9783, profile: `${SCRATCH}/markdown-next-profile` });
const { send, ev } = b;
await sleep(1500);
await ev(`[...document.querySelectorAll('button[aria-label="Riverside Makers"]')][0]?.click()`);
await sleep(600);
await ev(`[...document.querySelectorAll('aside[aria-label="Channels and conversations"] button')].find((x) => x.textContent.includes('general'))?.click()`);
await sleep(700);
const row = `[...document.querySelectorAll('article.msg')].find((a) => a.textContent.includes('Heading one'))`;
const all = (sel) => ev(`(() => { const r = ${row}; return r ? [...r.querySelectorAll(${JSON.stringify(sel)})].map((n) => n.textContent) : null; })()`);
const attrs = (sel, name) => ev(`(() => { const r = ${row}; return r ? [...r.querySelectorAll(${JSON.stringify(sel)})].map((n) => n.getAttribute(${JSON.stringify(name)})) : null; })()`);

check('the demo message is in the general channel', (await ev(`!!${row}`)) === true);
const strongs = await all('strong');
const strongEms = await all('strong em');
check('bold text is strong, and bold italic nests em inside strong', strongs.includes('Bold') && strongEms.includes('both'));
const ems = await all('em');
check('* and _ both give italic', ems.includes('italic') && ems.includes('also italic'));
check('__ gives underline', (await all('u')).includes('underlined'));
check('~~ gives strikethrough', (await all('s')).includes('struck'));
check('three hash marks give headings one, two and three', (await all('.md-h1')).includes('Heading one') && (await all('.md-h2')).includes('Heading two') && (await all('.md-h3')).includes('Heading three'));
check('-# gives subtext and > gives a block quote', (await all('.md-subtext')).includes('a subtext line') && (await all('.md-quote')).some((t) => t.includes('a quoted line')));
check('nested indent gives a list inside a list item', (await ev(`(() => { const r = ${row}; return !!r && [...r.querySelectorAll('ul.md-list li ul.md-list li')].some((n) => n.textContent.includes('nested item')); })()`)) === true);
check('1. gives an ordered list item', (await ev(`(() => { const r = ${row}; return !!r && [...r.querySelectorAll('ol.md-list li')].some((n) => n.textContent.includes('ordered one')); })()`)) === true);
check('lists show their markers: discs, numbers, and circles for a nested list', (await ev(`(() => { const r = ${row}; const style = (sel) => { const n = r?.querySelector(sel); return n ? getComputedStyle(n).listStyleType : null; }; return [style('ul.md-list'), style('ol.md-list'), style('ul.md-list ul.md-list')]; })()`)).join(',') === 'disc,decimal,circle');
// The channel header's pins button lists the channel's pinned messages, and says so when there are none.
await ev(`document.querySelector('button[aria-label="Pinned messages"]')?.click()`);
await sleep(500);
check('the header pins button lists the pinned messages, or says there are none', (await ev(`[...document.querySelectorAll('[role^="menuitem"]')].some((n) => n.textContent.includes('No pinned messages'))`)) === true);
await ev(`document.activeElement?.dispatchEvent(new KeyboardEvent('keydown', { key: 'Escape', bubbles: true }))`);
await sleep(300);
check('inline code is code and keeps its stars', (await all('code.md-code')).includes('inline *code*'));
check('a fenced block shows its language and its text as code', (await all('.md-codelang')).includes('rust') && (await all('.md-codeblock pre code')).some((t) => t.includes('fn main() {}')), JSON.stringify([await all('.md-codelang'), await all('.md-codeblock pre code')]));
await sleep(1200);
check('a fenced block is coloured once its language loads', (await ev(`(() => { const r = ${row}; return !!r && [...r.querySelectorAll('.md-codeblock .hljs-keyword')].some((n) => n.textContent === 'fn'); })()`)) === true);
check('a spoiler is hidden until pressed', (await ev(`(() => { const s = ${row}?.querySelector('.md-spoiler'); return !!s && s.textContent === 'a secret' && !s.classList.contains('revealed') && s.getAttribute('aria-label') === 'Spoiler. Press to reveal'; })()`)) === true);
await ev(`${row}.querySelector('.md-spoiler').click()`);
await sleep(200);
check('pressing a spoiler reveals it', (await ev(`${row}.querySelector('.md-spoiler').classList.contains('revealed')`)) === true);
check('a masked link shows its label as a link to its address', (await all('a.md-link')).includes('the docs') && (await attrs('a.md-link', 'href')).includes('https://docs.example.com/guide'));
check('a masked link shows its address on hover', (await attrs('a.md-link', 'title')).includes('https://docs.example.com/guide'));
check('a bare link is still a link', (await attrs('a.md-link', 'href')).includes('https://example.com/page'));
check('escaped stars show as text, not as italic', (await ev(`(() => { const r = ${row}; return !!r && r.textContent.includes('*escaped stars*') && ![...r.querySelectorAll('em')].some((n) => n.textContent.includes('escaped')); })()`)) === true);
check('raw HTML in a message is text, never an element', (await ev(`(() => { const r = ${row}; return !!r && r.textContent.includes('<b>raw html</b>') && !r.querySelector('b'); })()`)) === true);
check('a link whose label names another site shows its real address on hover', (await attrs('a.md-link', 'title')).includes('https://evil.example.org/phish'));
await ev(`${row}.querySelector('a[title="https://evil.example.org/phish"]').click()`);
await sleep(400);
check('clicking it asks before it opens', (await ev(`document.body.textContent.includes('This link says github.com but goes to evil.example.org. Open it?')`)) === true);
await send('Input.dispatchKeyEvent', { type: 'rawKeyDown', key: 'Escape', code: 'Escape', windowsVirtualKeyCode: 27, nativeVirtualKeyCode: 27 });
await send('Input.dispatchKeyEvent', { type: 'keyUp', key: 'Escape', code: 'Escape', windowsVirtualKeyCode: 27, nativeVirtualKeyCode: 27 });
await sleep(300);
// The composer's formatting shortcuts wrap the selected text: Ctrl+B, Ctrl+I, Ctrl+U and Ctrl+Shift+X.
const composer = 'textarea[aria-label="Message"]';
const chord = async (key, vk, modifiers) => {
  const base = { key, code: `Key${key.toUpperCase()}`, windowsVirtualKeyCode: vk, nativeVirtualKeyCode: vk, modifiers };
  await send('Input.dispatchKeyEvent', { type: 'rawKeyDown', ...base });
  await send('Input.dispatchKeyEvent', { type: 'keyUp', ...base });
  await sleep(250);
};
const selectAll = () => ev(`(() => { const t = document.querySelector(${JSON.stringify(composer)}); t.focus(); t.setSelectionRange(0, t.value.length); })()`);
const wrapped = async (word, key, vk, modifiers) => {
  await selectAll();
  await send('Input.insertText', { text: word });
  await selectAll();
  await chord(key, vk, modifiers);
  return ev(`document.querySelector(${JSON.stringify(composer)}).value`);
};
check('Ctrl+B wraps the selected text in bold markers', (await wrapped('bold', 'b', 66, 2)) === '**bold**');
check('Ctrl+I wraps the selected text in italic markers', (await wrapped('italic', 'i', 73, 2)) === '*italic*');
check('Ctrl+U wraps the selected text in underline markers', (await wrapped('under', 'u', 85, 2)) === '__under__');
check('Ctrl+Shift+X wraps the selected text in strikethrough markers', (await wrapped('strike', 'X', 88, 10)) === '~~strike~~');
await selectAll();
await send('Input.insertText', { text: '' });
await b.close();
}

// ---- N. The message menu: the items each message offers (own or someone else's), the quick reactions and the emoji
// submenu, Forward, the message link and its Jump, Bookmark with Saved Messages, Pin, Mark Unread, a reminder that
// fires, and Shift+F10. ----
async function section_messages() {
const b = await launch({ url: `${BASE}/`, width: 1600, height: 1000, port: 9784, profile: `${SCRATCH}/messages-next-profile` });
const { send, ev } = b;
await sleep(1500);
await ev(`[...document.querySelectorAll('button[aria-label="Riverside Makers"]')][0]?.click()`);
await sleep(600);
const openGeneral = async () => {
  await ev(`[...document.querySelectorAll('aside[aria-label="Channels and conversations"] button')].find((x) => x.textContent.includes('general'))?.click()`);
  await sleep(700);
};
await openGeneral();
const clickSidebar = async (label) => {
  await ev(`[...document.querySelectorAll('aside[aria-label="Channels and conversations"] button')].find((x) => x.textContent.includes(${JSON.stringify(label)}))?.click()`);
  await sleep(600);
};
const rowByText = (text) => `[...document.querySelectorAll('aside[aria-label="Channels and conversations"] button')].find((b) => b.textContent.includes(${JSON.stringify(text)}))`;

const keyDown = async (key, code, vk, modifiers = 0, text = undefined) => {
  const base = { key, code, windowsVirtualKeyCode: vk, nativeVirtualKeyCode: vk, modifiers };
  await send('Input.dispatchKeyEvent', { type: 'rawKeyDown', ...base, text });
  if (text) await send('Input.dispatchKeyEvent', { type: 'char', ...base, text });
  await send('Input.dispatchKeyEvent', { type: 'keyUp', ...base });
  await sleep(250);
};
const escape = () => keyDown('Escape', 'Escape', 27);
const goHome = async () => { await ev(`document.querySelector('button[aria-label="Direct messages"]')?.click()`); await sleep(700); };

// A message's element, by a text it holds. The first match in the list.
const MSG = (text) => `[...document.querySelectorAll('article.msg')].find((a) => a.textContent.includes(${JSON.stringify(text)}))`;
const menuTexts = () => ev(`[...document.querySelectorAll('[role="menu"] [role^="menuitem"]')].map((n) => n.textContent.trim())`);
const quickEmoji = () => ev(`[...document.querySelectorAll('[role="menu"] [role="menuitem"][aria-label][title]')].map((n) => n.textContent.trim())`);
const toastTexts = () => ev(`[...document.querySelectorAll('[role="status"]')].map((n) => n.textContent).join('|')`);
const reactionsOf = (text) => ev(`(() => { const r = ${MSG(text)}; return r ? [...r.querySelectorAll('.reaction')].map((x) => x.getAttribute('aria-label') + (x.classList.contains('mine') ? ' [mine]' : '')) : []; })()`);

// Right click on a message's element (the menu opens after two lookups, so the wait is longer than a click's).
const openMenuOn = async (msgExpr) => {
  await ev(`(() => { const row = ${msgExpr}; if (!row) return false; const r = row.getBoundingClientRect(); row.dispatchEvent(new MouseEvent('contextmenu', { bubbles: true, cancelable: true, clientX: r.left + 20, clientY: r.top + 10 })); return true; })()`);
  await sleep(450);
};
// Clicks the first menu row (top level or an open submenu) whose text includes `label`.
const clickMenuItem = (label) => ev(`(() => { const n = [...document.querySelectorAll('[role="menu"] [role^="menuitem"]')].find((x) => x.textContent.includes(${JSON.stringify(label)})); if (!n) return false; n.click(); return true; })()`);
// Focuses a row that opens a submenu and presses ArrowRight, as a keyboard user does.
const openSub = async (label) => {
  await ev(`(() => { const n = [...document.querySelectorAll('[role="menu"] [role^="menuitem"]')].find((x) => x.textContent.includes(${JSON.stringify(label)})); n?.focus(); return !!n; })()`);
  await keyDown('ArrowRight', 'ArrowRight', 39);
  await sleep(250);
};
// Types into the composer and sends it.
const typeSend = async (text) => {
  await ev(`(() => { const t = document.querySelector('textarea[aria-label="Message"]'); t.focus(); })()`);
  await send('Input.insertText', { text });
  await sleep(200);
  await keyDown('Enter', 'Enter', 13, 0, '\r');
  await sleep(600);
};

const OWN_TEXT = 'Menu check mine';
const OTHER_TEXT = 'Welcome to Riverside';
await typeSend(OWN_TEXT);
const ownId = await ev(`${MSG(OWN_TEXT)}?.dataset.id ?? null`);
const welcomeId = await ev(`${MSG(OTHER_TEXT)}?.dataset.id ?? null`);
check('the own and the other test messages are in the channel', Boolean(ownId) && Boolean(welcomeId), `own ${ownId} other ${welcomeId}`);

// ---- what a message offers: your own message ----
await openMenuOn(MSG(OWN_TEXT));
const own = await menuTexts();
const ownWanted = ['Add Reaction', 'Reply', 'Edit Message', 'Forward', 'Copy Text', 'Pin Message', 'Bookmark Message', 'Create Reminder', 'Mark Unread', 'Copy Message Link', 'Speak Message', 'Delete Message'];
check('own message offers every item of the menu', ownWanted.every((l) => own.some((t) => t.includes(l))), `missing: ${ownWanted.filter((l) => !own.some((t) => t.includes(l))).join(', ')}`);
const quickOwn = await quickEmoji();
check('the quick reactions row shows four emoji', quickOwn.length === 4 && quickOwn.every((e) => /\p{Extended_Pictographic}/u.test(e)), JSON.stringify(quickOwn));
await clickMenuItem('Copy Text');
await sleep(400);
check('Copy Text confirms with a toast', (await toastTexts()).includes('Text copied'));

// ---- someone else's message ----
await openMenuOn(MSG(OTHER_TEXT));
const other = await menuTexts();
check('someone else\'s message has no Edit Message', !other.some((t) => t.includes('Edit Message')));
const otherWanted = ['Add Reaction', 'Reply', 'Forward', 'Copy Text', 'Pin Message', 'Bookmark Message', 'Create Reminder', 'Mark Unread', 'Copy Message Link', 'Speak Message'];
check('someone else\'s message offers the same items but Edit', otherWanted.every((l) => other.some((t) => t.includes(l))), `missing: ${otherWanted.filter((l) => !other.some((t) => t.includes(l))).join(', ')}`);
check('someone else\'s message shows the same four quick reactions', (await quickEmoji()).length === 4);

// ---- quick reactions toggle: on, then off ----
const firstQuick = await ev(`[...document.querySelectorAll('[role="menu"] [role="menuitem"][aria-label][title]')][0].textContent.trim()`);
await ev(`[...document.querySelectorAll('[role="menu"] [role="menuitem"][aria-label][title]')][0].click()`);
await sleep(700);
check('a quick reaction adds the reaction, as ours', (await reactionsOf(OTHER_TEXT)).some((s) => s.startsWith(firstQuick) && s.endsWith('[mine]')), `${firstQuick}: ${JSON.stringify(await reactionsOf(OTHER_TEXT))}`);
await openMenuOn(MSG(OTHER_TEXT));
await ev(`[...document.querySelectorAll('[role="menu"] [role="menuitem"][aria-label][title]')][0].click()`);
await sleep(700);
check('the same quick reaction removes it again', !(await reactionsOf(OTHER_TEXT)).some((s) => s.startsWith(firstQuick) && s.includes('[mine]')));

// ---- Add Reaction: the emoji by shortcode, then More ----
await openMenuOn(MSG(OTHER_TEXT));
await openSub('Add Reaction');
const emojiSub = await menuTexts();
check('Add Reaction lists the emoji by shortcode, then More', emojiSub.some((t) => t.includes(':thumbsup:')) && emojiSub.some((t) => t.includes('More')));
await clickMenuItem(':thumbsup:');
await sleep(700);
check('a shortcode in the submenu adds that reaction', (await reactionsOf(OTHER_TEXT)).some((s) => s.startsWith('👍') && s.includes('[mine]')));

// ---- Forward: the dialog says attachments are not forwarded, and sends a quote ----
await openMenuOn(MSG(OTHER_TEXT));
await clickMenuItem('Forward');
await sleep(500);
check('Forward opens its dialog', (await ev(`[...document.querySelectorAll('[role="dialog"] h2')].some((h) => h.textContent.trim() === 'Forward Message')`)) === true);
check('the dialog says attachments are not forwarded', (await ev(`document.querySelector('[role="dialog"]')?.textContent.includes('Attachments are not forwarded.') ?? false`)) === true);
await ev(`(() => { const n = [...document.querySelectorAll('[role="dialog"] button')].find((x) => x.textContent.includes('Riverside Makers · #general')); n?.click(); })()`);
await sleep(900);
const quoted = await ev(`(() => { const r = ${MSG('Forwarded from')}; return r ? [...r.querySelectorAll('blockquote')].map((q) => q.textContent.trim()) : null; })()`);
check('Forward sends "Forwarded from" and the original quoted', Array.isArray(quoted) && quoted.some((t) => t.includes(OTHER_TEXT)), JSON.stringify(quoted));
check('the dialog closes after the send', (await ev(`document.querySelectorAll('[role="dialog"]').length`)) === 0);

// ---- Copy Message Link, then a message with that link shows Jump to message ----
await ev(`(() => { window.__copied = []; navigator.clipboard.writeText = async (t) => { window.__copied.push(t); }; })()`);
await openMenuOn(MSG(OWN_TEXT));
await clickMenuItem('Copy Message Link');
await sleep(400);
const copied = await ev(`window.__copied?.[0] ?? null`);
check('Copy Message Link copies dissent://message/<community>/<channel>/<id>', typeof copied === 'string' && new RegExp(`^dissent://message/[^/]+/[^/]+/${ownId}$`).test(copied), String(copied));
await typeSend(copied);
check('a message with a message link offers Jump to message', (await ev(`[...(${MSG(copied)})?.querySelectorAll('button') ?? []].some((x) => x.textContent === 'Jump to message')`)) === true);
await ev(`(() => { const r = ${MSG(copied)}; [...r.querySelectorAll('button')].find((x) => x.textContent === 'Jump to message')?.click(); })()`);
await sleep(900);
check('Jump to message scrolls to the linked message and focuses it', (await ev(`document.activeElement?.dataset.id ?? null`)) === ownId);

// ---- Bookmark Message, Saved Messages, Jump and Remove ----
await openMenuOn(MSG(OTHER_TEXT));
await clickMenuItem('Bookmark Message');
await sleep(500);
check('Bookmark Message confirms with a toast', (await toastTexts()).includes('Message saved'));
await goHome();
await clickSidebar('Saved Messages');
const savedHtml = await ev(`document.querySelector('ul[aria-label="Saved messages"]')?.textContent ?? ''`);
check('Saved Messages lists the bookmarked message', savedHtml.includes(OTHER_TEXT), savedHtml.slice(0, 120));
await ev(`(() => { const li = [...document.querySelectorAll('ul[aria-label="Saved messages"] li')].find((l) => l.textContent.includes(${JSON.stringify(OTHER_TEXT)})); [...li.querySelectorAll('button')].find((x) => x.textContent === 'Jump')?.click(); })()`);
await sleep(1000);
check('Jump opens the channel and focuses the message', (await ev(`!document.querySelector('ul[aria-label="Saved messages"]') && document.activeElement?.dataset.id === ${JSON.stringify(welcomeId)}`)) === true);
await openMenuOn(MSG(OTHER_TEXT));
check('a saved message offers Remove Bookmark', (await menuTexts()).some((t) => t.includes('Remove Bookmark')));
await clickMenuItem('Remove Bookmark');
await sleep(500);
check('Remove Bookmark confirms with a toast', (await toastTexts()).includes('Bookmark removed'));

// ---- Saved Messages draws each message with the chat's row, read-only: formatting, invite cards and link previews ----
const SAVED_FMT = '**Saved** card: dissent://invite/mock-rustaceans';
await typeSend(SAVED_FMT);
await sleep(600);
const savedFmtId = await ev(`${MSG('card')}?.dataset.id ?? null`);
await openMenuOn(`[...document.querySelectorAll('article.msg')].find((a) => a.dataset.id === ${JSON.stringify(savedFmtId)})`);
await clickMenuItem('Bookmark Message');
await sleep(500);
await goHome();
await clickSidebar('Maya');
await sleep(500);
await openMenuOn(MSG('github.com/Kaakati/dissent/tree/main/apps/relay'));
await clickMenuItem('Bookmark Message');
await sleep(500);
await clickSidebar('Saved Messages');
await sleep(800);
const SAVED = 'ul[aria-label="Saved messages"]';
check('a saved message shows its bold text as bold', (await ev(`[...document.querySelectorAll('${SAVED} .msg .body strong')].some((n) => n.textContent === 'Saved')`)) === true, String(await ev(`document.querySelector('${SAVED}')?.innerHTML.slice(0, 200)`)));
check('a saved message with an invite link shows the invite card', (await ev(`!!document.querySelector('${SAVED} [data-server-card]')`)) === true);
check('a saved message with a link shows its preview card', (await ev(`!!document.querySelector('${SAVED} article.embed .embed-title')`)) === true);
check('saved messages are read-only: no message actions or menu', (await ev(`!document.querySelector('${SAVED} button[aria-label="More message actions"]') && !document.querySelector('${SAVED} .msg-actions')`)) === true);
await b.screenshot(`${SCRATCH}/shots/saved-messages.png`);
// Remove the DM one here, then Jump to the channel one and remove it from its menu, so no bookmark is left behind.
await ev(`(() => { const li = [...document.querySelectorAll('${SAVED} > li')].find((l) => l.querySelector('.msg')?.dataset.id !== ${JSON.stringify(savedFmtId)}); [...li.querySelectorAll('button')].find((x) => x.textContent === 'Remove')?.click(); })()`);
for (let i = 0; i < 20 && (await ev(`document.querySelectorAll('${SAVED} > li').length`)) !== 1; i++) await sleep(150);
check('Remove takes a saved message off the page', (await ev(`document.querySelectorAll('${SAVED} > li').length`)) === 1, String(await ev(`[...document.querySelectorAll('${SAVED} > li')].map((l) => l.textContent.slice(0, 40)).join(' | ')`)));
await ev(`(() => { const li = [...document.querySelectorAll('${SAVED} > li')].find((l) => l.querySelector('.msg')?.dataset.id === ${JSON.stringify(savedFmtId)}); [...li.querySelectorAll('button')].find((x) => x.textContent === 'Jump')?.click(); })()`);
await sleep(1200);
check('Jump from a formatted saved message still opens the channel at the message', (await ev(`!document.querySelector('${SAVED}') && document.activeElement?.dataset.id === ${JSON.stringify(savedFmtId)}`)) === true);
await openMenuOn(`[...document.querySelectorAll('article.msg')].find((a) => a.dataset.id === ${JSON.stringify(savedFmtId)})`);
await clickMenuItem('Remove Bookmark');
await sleep(500);

// A saved message from a spoiler channel of a server that is not open stays blurred until clicked: the page reads that
// server's channel setting, and blurs when it cannot.
await ev(`(async () => {
  const invoke = window.__TAURI__.core.invoke;
  const [first] = await invoke('channel_messages', { communityId: 'c2', channelId: 'ch5', limit: 50 });
  await invoke('add_bookmark', { message: { community: 'c2', channel: 'ch5', peer: null, message_id: first.id, text: first.text, author_name: first.author_name, ms: first.ms } });
})()`);
await ev(`window.__mockVisibility('c2', 'ch5', 'spoiler')`);
await goHome();
await clickSidebar('Saved Messages');
await sleep(800);
const spoilerRow = `[...document.querySelectorAll('${SAVED} > li')].find((l) => l.textContent.includes('Dispossessed'))`;
check('a saved message from a spoiler channel of another server is blurred', (await ev(`!!${spoilerRow}?.querySelector('.body.spoiler-hidden')`)) === true, String(await ev(`${spoilerRow}?.querySelector('.body')?.className`)));
await ev(`${spoilerRow}.querySelector('.body.spoiler-hidden').click()`);
await sleep(300);
check('a click reveals the saved spoiler', (await ev(`!${spoilerRow}.querySelector('.body.spoiler-hidden')`)) === true);
await ev(`[...${spoilerRow}.querySelectorAll('button')].find((x) => x.textContent === 'Remove')?.click()`);
await sleep(700);
await ev(`window.__mockVisibility('c2', 'ch5', 'default')`);
await ev(`document.querySelector('nav[aria-label="Servers and direct messages"] button[aria-label="Riverside Makers"]')?.click()`);
await sleep(700);
await openGeneral();

// ---- Pin: channels only, Pin then Unpin ----
await openMenuOn(MSG(OWN_TEXT));
await clickMenuItem('Pin Message');
await sleep(500);
check('Pin Message confirms with a toast', (await toastTexts()).includes('Message pinned'));
await openMenuOn(MSG(OWN_TEXT));
check('a pinned message offers Unpin Message', (await menuTexts()).some((t) => t.includes('Unpin Message')));
await clickMenuItem('Unpin Message');
await sleep(500);
check('Unpin Message confirms with a toast', (await toastTexts()).includes('Message unpinned'));

// ---- a direct message: no Pin Message, but Forward and Bookmark ----
await goHome(); await ev(`${rowByText('Maya')}?.click()`);
await sleep(900);
await openMenuOn(`document.querySelector('article.msg')`);
const dm = await menuTexts();
check('a direct message has no Pin Message, and offers Forward and Bookmark', !dm.some((t) => t.includes('Pin Message')) && dm.some((t) => t.includes('Forward')) && dm.some((t) => t.includes('Bookmark Message')), JSON.stringify(dm));
await escape();
await ev(`[...document.querySelectorAll('button[aria-label="Riverside Makers"]')][0]?.click()`);
await sleep(600);
await openGeneral();

// ---- Mark Unread confirms ----
await openMenuOn(MSG(OWN_TEXT));
await clickMenuItem('Mark Unread');
await sleep(500);
check('Mark Unread confirms with a toast', (await toastTexts()).includes('Marked unread'));

// ---- Create Reminder: the submenu, the confirm, and the reminder firing: a toast with Jump that opens the message ----
await openMenuOn(MSG(OTHER_TEXT));
await openSub('Create Reminder');
const times = await menuTexts();
check('Create Reminder offers the four times', ['In 20 Minutes', 'In 1 Hour', 'In 3 Hours', 'Tomorrow'].every((l) => times.some((t) => t.includes(l))), JSON.stringify(times));
await clickMenuItem('In 1 Hour');
await sleep(500);
check('a reminder is confirmed with its time', (await toastTexts()).includes('Reminder set for'));
await goHome();
await clickSidebar('Friends');
const fired = await ev(`window.__mockFireReminder('reminder-1')`);
await sleep(600);
check('the reminder fires as a toast with a Jump button', fired === true && (await toastTexts()).includes('Reminder: ') && (await ev(`[...document.querySelectorAll('[role="status"] button')].some((x) => x.textContent === 'Jump')`)) === true);
await ev(`[...document.querySelectorAll('[role="status"] button')].find((x) => x.textContent === 'Jump')?.click()`);
await sleep(1000);
check('the reminder\'s Jump opens its message', (await ev(`document.activeElement?.dataset.id ?? null`)) === welcomeId);

// ---- Shift+F10 on a focused message opens the same menu ----
await ev(`(() => { const r = ${MSG(OWN_TEXT)}; r.focus(); })()`);
await keyDown('F10', 'F10', 121, 8);
await sleep(450);
check('Shift+F10 on a focused message opens its menu', (await menuTexts()).some((t) => t.includes('Reply')));
await escape();

noConsoleErrors('messages', b.consoleErrors);
await b.close();
}

// ---- Server preferences: the rail menu's Mute Server, Notification Settings and Hide Muted Channels ----
// The mock logs each alert decision (window.__mockAlertLog), so the checks read what the UI would alert on.
async function section_serverprefs() {
const b = await launch({ url: `${BASE}/`, width: 1600, height: 1000, port: 9781, profile: `${SCRATCH}/serverprefs-next-profile` });
const { send, ev } = b;
await sleep(800);
const openRailMenu = (name) => ev(`(() => { const b = document.querySelector('button[aria-label=' + JSON.stringify(${JSON.stringify(name)}) + ']'); if (!b) return false; const r = b.getBoundingClientRect(); b.dispatchEvent(new MouseEvent('contextmenu', { bubbles: true, cancelable: true, clientX: r.left + r.width / 2, clientY: r.top + r.height / 2 })); return true; })()`);
const menuNames = async () => (await ev(`[...document.querySelectorAll('[role="menu"] [role^="menuitem"]')].map((n) => n.textContent.trim())`)) ?? [];
const pick = (prefix) => ev(`(() => { const n = [...document.querySelectorAll('[role^="menuitem"]')].find((n) => n.textContent.trim().startsWith(${JSON.stringify(prefix)})); if (!n) return false; n.click(); return true; })()`);
const closeMenus = async () => { await ev(`document.activeElement?.dispatchEvent(new KeyboardEvent('keydown', { key: 'Escape', bubbles: true }))`); await sleep(250); };
const alertsSince = (from) => ev(`(window.__mockAlertLog ?? []).slice(${from})`);
const alertCount = async () => (await ev(`(window.__mockAlertLog ?? []).length`)) ?? 0;
// The mock's owner of Riverside Makers, who posts in it: its id is the name's base32 bytes, as the mock makes them.
const GRACE = base32Encode(new TextEncoder().encode('k3f9q2xagrace')).padEnd(52, 'a').slice(0, 52);

// An owned server, for the owner's menu.
await ev(`document.querySelector('button[aria-label="Create or join a server"]').click()`);
await sleep(500);
await ev(`(() => { const f = document.querySelector('[role="dialog"] input[aria-label="Server Name"]'); f.focus(); f.select(); })()`);
await send('Input.insertText', { text: 'Prefs test' });
await ev(`[...document.querySelectorAll('[role="dialog"] button:not([role="tab"])')].find((b) => b.textContent.trim() === 'Create').click()`);
await sleep(1200);
await openRailMenu('Prefs test');
await sleep(400);
const ownerItems = await menuNames();
check('an owner sees Mute Server, Notification Settings and Hide Muted Channels', ['Mute Server', 'Notification Settings', 'Hide Muted Channels'].every((t) => ownerItems.some((l) => l.startsWith(t))), JSON.stringify(ownerItems));
await closeMenus();

// A member (Riverside Makers is not owned here) sees the same three items, with the default choice as the subtitle.
await openRailMenu('Riverside Makers');
await sleep(400);
const memberItems = await menuNames();
check('a member sees Mute Server, Notification Settings and Hide Muted Channels', ['Mute Server', 'Notification Settings', 'Hide Muted Channels'].every((t) => memberItems.some((l) => l.startsWith(t))), JSON.stringify(memberItems));
check('Notification Settings shows Only @mentions by default', memberItems.some((l) => l.startsWith('Notification Settings') && l.includes('Only @mentions')), JSON.stringify(memberItems));
await closeMenus();

// Only @mentions: a plain message does not alert, and a mention does.
let from = await alertCount();
await ev(`window.__mockPost('c1', 'ch1', '${GRACE}', 'A plain message for the alert check')`);
await sleep(700);
const plain = await alertsSince(from);
check('Only @mentions does not alert on a plain message', plain.length === 1 && plain[0].alerts === false, JSON.stringify(plain));
from = await alertCount();
await ev(`window.__mockPost('c1', 'ch1', '${GRACE}', 'A mention for the alert check', true)`);
await sleep(700);
const mention = await alertsSince(from);
check('Only @mentions alerts on a mention', mention.length === 1 && mention[0].alerts === true && mention[0].byName === true, JSON.stringify(mention));

// Nothing silences a mention; All Messages alerts a plain message.
await openRailMenu('Riverside Makers');
await sleep(400);
await pick('Notification Settings');
await sleep(300);
await pick('Nothing');
await sleep(400);
from = await alertCount();
await ev(`window.__mockPost('c1', 'ch1', '${GRACE}', 'A mention while nothing alerts', true)`);
await sleep(700);
const silenced = await alertsSince(from);
check('Nothing silences a mention', silenced.length === 1 && silenced[0].alerts === false, JSON.stringify(silenced));
await openRailMenu('Riverside Makers');
await sleep(400);
await pick('Notification Settings');
await sleep(300);
await pick('All Messages');
await sleep(400);
from = await alertCount();
await ev(`window.__mockPost('c1', 'ch1', '${GRACE}', 'A plain message while all messages alert')`);
await sleep(700);
const all = await alertsSince(from);
check('All Messages alerts a plain message', all.length === 1 && all[0].alerts === true, JSON.stringify(all));

// A timed mute: its end time shows as the subtitle.
await openRailMenu('Riverside Makers');
await sleep(400);
await pick('Mute Server');
await sleep(300);
await pick('For 1 Hour');
await sleep(500);
await openRailMenu('Riverside Makers');
await sleep(400);
const muted = await menuNames();
check('a timed mute shows its end time under Mute Server', muted.some((l) => /^Mute Server.*Muted until \d{1,2}:\d{2} (AM|PM)/.test(l)), JSON.stringify(muted));
await closeMenus();

// Hide Muted Channels: a muted channel leaves the sidebar, and the open channel stays. The mute is saved before a reload.
await ev(`localStorage.setItem('dissent.muted.v1', JSON.stringify({ 'ch:c1:ch2': null }))`);
await send('Page.navigate', { url: `${BASE}/` });
await sleep(2000);
await ev(`document.querySelector('button[aria-label="Riverside Makers"]').click()`);
await sleep(700);
// A channel row's text is its name and, when it has one, its unread count ("general1").
const hasChannel = (name) => ev(`[...document.querySelectorAll('button')].some((b) => b.textContent.trim().startsWith(${JSON.stringify(name)}))`);
check('a muted channel is listed before Hide Muted Channels is on', (await hasChannel('hardware')) === true);
await openRailMenu('Riverside Makers');
await sleep(400);
await pick('Hide Muted Channels');
await sleep(500);
check('Hide Muted Channels hides the muted channel', (await hasChannel('hardware')) === false);
check('Hide Muted Channels keeps the other channels', (await hasChannel('general')) === true);
await openRailMenu('Riverside Makers');
await sleep(400);
const hideChecked = await ev(`[...document.querySelectorAll('[role="menuitemcheckbox"]')].find((n) => n.textContent.trim().startsWith('Hide Muted Channels'))?.getAttribute('aria-checked')`);
check('the Hide Muted Channels item shows it is on', hideChecked === 'true', String(hideChecked));
await closeMenus();

// A timed mute runs out on its own: the rail's unread dot is hidden while the mute lasts and returns when it ends, with no
// click in between. The mock shortens the 15-minute mute to 1 s. announcements is not open, so its message leaves Riverside unread.
await ev(`window.__mockMuteMs = 1000`);
await ev(`window.__mockPost('c1', 'ch3', '${GRACE}', 'A message that leaves Riverside unread')`);
await sleep(700);
const dotOf = (name) => ev(`!!document.querySelector('button[aria-label=' + JSON.stringify(${JSON.stringify(name)}) + ']').closest('.rail-item').classList.contains('unread')`);
check('a server with an unread message shows the unread dot', (await dotOf('Riverside Makers')) === true);
await openRailMenu('Riverside Makers');
await sleep(400);
await pick('Mute Server');
await sleep(300);
await pick('For 15 Minutes');
await sleep(400);
check('a muted server shows no unread dot', (await dotOf('Riverside Makers')) === false);
await sleep(1600);
check('the unread dot returns when the mute runs out, with no interaction', (await dotOf('Riverside Makers')) === true);
await ev(`delete window.__mockMuteMs`);

await b.close();
}

// ---- Unread direct messages on the rail: an avatar per unread conversation with its count, a click opens it and it leaves,
// more than five show a "+N" that opens Home, and a muted conversation is not on the rail. The mock counts unread messages
// from others after its start, as the engine does. ----
async function section_unreaddms() {
const b = await launch({ url: `${BASE}/`, width: 1600, height: 1000, port: 9783, profile: `${SCRATCH}/unreaddms-next-profile` });
const { send, ev } = b;
await sleep(800);
// The mock's ids are the name's base32 bytes, padded to 52 characters.
const mockId = (name) => base32Encode(new TextEncoder().encode(name)).padEnd(52, 'a').slice(0, 52);
const LINUS = mockId('w8r2d6nblinus');
const GRACE = mockId('k3f9q2xagrace');
const KEN = mockId('h5t7c3vyken');
const RADIA = mockId('e4j9m1sqradia');
const RANA = mockId('rana-z6q1e8jd');
const NADIA = mockId('nadia-c5v8m1wk');
const YUSUF = mockId('yusuf-r3j6t0pb');
const post = (peer, text) => ev(`window.__mockDm('${peer}', ${JSON.stringify(text)})`);
const railLabels = async () => (await ev(`[...document.querySelectorAll('[role="group"][aria-label="Unread direct messages"] button')].map((n) => n.getAttribute('aria-label'))`)) ?? [];
const railClick = (label) => ev(`(() => { const n = [...document.querySelectorAll('[role="group"][aria-label="Unread direct messages"] button')].find((n) => n.getAttribute('aria-label') === ${JSON.stringify(label)}); if (!n) return false; n.click(); return true; })()`);

await post(LINUS, 'First unread message');
await sleep(800);
let labels = await railLabels();
check('an unread DM appears on the rail with its count', labels.includes('Jonas, 1 unread message'), JSON.stringify(labels));
await post(LINUS, 'Second unread message');
await sleep(800);
labels = await railLabels();
check('the count on the rail goes up with each unread message', labels.includes('Jonas, 2 unread messages'), JSON.stringify(labels));

await railClick('Jonas, 2 unread messages');
await sleep(900);
check('clicking the avatar opens that DM', (await ev(`!!document.querySelector('button[aria-current="true"][title="Jonas"]')`)) === true);
labels = await railLabels();
check('a DM that is read leaves the rail', !labels.some((l) => l.startsWith('Jonas,')), JSON.stringify(labels));

// Six unread conversations: five avatars and a "+1" that opens Home.
for (const peer of [GRACE, KEN, RADIA, RANA, NADIA, YUSUF]) await post(peer, 'Unread for the overflow check');
await sleep(900);
labels = await railLabels();
check('five unread conversations show and the rest become a +N', labels.filter((l) => l.includes('unread message')).length === 5 && labels.includes('1 more unread conversation'), JSON.stringify(labels));
check('the rail section is labelled Unread direct messages', (await ev(`!!document.querySelector('[role="group"][aria-label="Unread direct messages"]')`)) === true);
await railClick('1 more unread conversation');
await sleep(700);
check('the +N opens Home', (await ev(`document.querySelector('button[aria-label="Direct messages"]')?.getAttribute('aria-current')`)) === 'page');

// A muted conversation is not on the rail. The mute is saved before a reload, as the mute check reads it at start.
await ev(`localStorage.setItem('dissent.muted.v1', JSON.stringify({ 'dm:${KEN}': null }))`);
await send('Page.navigate', { url: `${BASE}/` });
await sleep(2000);
await post(KEN, 'A muted conversation');
await post(GRACE, 'A conversation that is not muted');
await sleep(900);
labels = await railLabels();
check('a muted conversation is not on the rail', !labels.some((l) => l.startsWith('Omar,')), JSON.stringify(labels));
check('an unmuted conversation still is', labels.includes('Maya, 1 unread message'), JSON.stringify(labels));

await b.close();
}

// ---- Close DM and Remove contact: an open pane goes to Friends, the row leaves the list, the unread marker goes, and the
// history comes back when the DM is reopened or a new message arrives. ----
async function section_dmclose() {
const b = await launch({ url: `${BASE}/`, width: 1600, height: 1000, port: 9803, profile: `${SCRATCH}/dmclose-next-profile` });
const { ev } = b;
await sleep(800);
const mockId = (name) => base32Encode(new TextEncoder().encode(name)).padEnd(52, 'a').slice(0, 52);
const LINUS = mockId('w8r2d6nblinus');
const post = (peer, text) => ev(`window.__mockDm('${peer}', ${JSON.stringify(text)})`);
const SIDEBAR_BUTTONS = `[...document.querySelectorAll('aside[aria-label="Channels and conversations"] button')]`;
const dmRows = () => ev(`${SIDEBAR_BUTTONS}.map((n) => n.textContent.trim())`);
const hasRow = async (name) => (await dmRows()).some((t) => t.includes(name));
const openRow = (name) => ev(`${SIDEBAR_BUTTONS}.find((n) => n.textContent.includes(${JSON.stringify(name)}))?.click()`);
const rightClickRow = (name) => ev(`(() => { const row = ${SIDEBAR_BUTTONS}.find((n) => n.textContent.includes(${JSON.stringify(name)})); const r = row.getBoundingClientRect(); row.dispatchEvent(new MouseEvent('contextmenu', { bubbles: true, cancelable: true, clientX: r.left + 20, clientY: r.top + 10 })); })()`);
const menuClick = (prefix) => ev(`(() => { const n = [...document.querySelectorAll('[role="menu"] [role^="menuitem"]')].find((m) => m.textContent.trim().startsWith(${JSON.stringify(prefix)})); if (!n) return false; n.click(); return true; })()`);
const menuItems = () => ev(`[...document.querySelectorAll('[role="menu"] [role^="menuitem"]')].map((n) => n.textContent.trim())`);
const railLabels = async () => (await ev(`[...document.querySelectorAll('[role="group"][aria-label="Unread direct messages"] button')].map((n) => n.getAttribute('aria-label'))`)) ?? [];
const onFriends = () => ev(`document.querySelector('h1')?.textContent === 'Friends'`);
const composerOpen = () => ev(`!!document.querySelector('textarea[aria-label="Message"]')`);
const messageCount = () => ev(`document.querySelectorAll('.messages[aria-label="Messages"] article.msg').length`);

// Close DM from an open pane: the pane goes to Friends and the row leaves the list.
await openRow('Jonas');
await sleep(700);
const before = await messageCount();
check('Jonas opens in a pane with its history', (await composerOpen()) && before > 0, `messages ${before}`);
await rightClickRow('Jonas');
await sleep(300);
const jonasMenu = await menuItems();
check('the DM menu offers Close DM', jonasMenu.some((l) => l.startsWith('Close DM')), JSON.stringify(jonasMenu));
await menuClick('Close DM');
await sleep(700);
check('Close DM on the open pane goes to Friends', (await onFriends()) && !(await composerOpen()));
check('Close DM removes the row from the list', !(await hasRow('Jonas')), JSON.stringify(await dmRows()));

// Message brings the row back, and the history is still there.
await ev(`document.querySelector('button[aria-label="Message Jonas"]')?.click()`);
await sleep(700);
check('reopening the DM brings its row back', await hasRow('Jonas'));
check('reopening the DM shows the history again', (await composerOpen()) && (await messageCount()) === before, `messages ${await messageCount()}, before ${before}`);
// The gutter time on a grouped message is a 12-hour clock with AM or PM, like the group header.
await post(LINUS, 'A first line from Jonas');
await post(LINUS, 'A second line from Jonas, grouped under the first');
await sleep(700);
const gutter = await ev(`[...document.querySelectorAll('.gutter-time')].at(-1)?.textContent ?? ''`);
check('the gutter time is a 12-hour clock with AM or PM', /^\d{1,2}:\d{2} (AM|PM)$/.test(gutter), JSON.stringify(gutter));

// A message sent to a DM that is listed and unread, then Close DM from Friends: the unread marker goes with the row.
await b.press('Escape');
await sleep(400);
check('Escape returns to Friends', await onFriends());
await post(LINUS, 'Sent while Friends is open');
await sleep(900);
check('a message on Friends marks the listed DM unread', (await railLabels()).some((l) => l.startsWith('Jonas,')), JSON.stringify(await railLabels()));
await rightClickRow('Jonas');
await sleep(300);
await menuClick('Close DM');
await sleep(700);
check('Close DM clears the unread marker and the row', !(await railLabels()).some((l) => l.startsWith('Jonas,')) && !(await hasRow('Jonas')), JSON.stringify(await railLabels()));

// A new message brings a closed DM back into the list, unread.
await post(LINUS, 'A message after Close DM');
await sleep(900);
check('a new message brings the closed DM back to the list', await hasRow('Jonas'));
check('the returning DM is unread on the rail', (await railLabels()).some((l) => l.startsWith('Jonas,')), JSON.stringify(await railLabels()));

// Remove contact from an open pane: the same navigation and row removal, and the friend leaves Friends.
await openRow('Maya');
await sleep(700);
check('Maya opens in a pane', await composerOpen());
await rightClickRow('Maya');
await sleep(300);
const mayaMenu = await menuItems();
check('the DM menu offers Remove contact', mayaMenu.some((l) => l.startsWith('Remove contact')), JSON.stringify(mayaMenu));
await menuClick('Remove contact');
await sleep(400);
await ev(`[...document.querySelectorAll('[role="alertdialog"] button')].find((n) => n.textContent.trim() === 'Remove')?.click()`);
await sleep(900);
check('Remove contact on the open pane goes to Friends', (await onFriends()) && !(await composerOpen()));
check('Remove contact removes the row from the list', !(await hasRow('Maya')));
check('the removed contact is gone from Friends', (await ev(`!document.querySelector('button[aria-label="More actions for Maya"]')`)) === true);
check('the removed contact leaves no unread marker', !(await railLabels()).some((l) => l.startsWith('Maya,')), JSON.stringify(await railLabels()));

await b.close();
}

// ---- dissent:// links: each shows a confirm dialog or a notice, and nothing happens until its button is pressed ----
async function section_deeplink() {
const b = await launch({ url: `${BASE}/`, width: 1600, height: 1000, port: 9805, profile: `${SCRATCH}/deeplink-next-profile` });
const { ev } = b;
await sleep(800);
const mockId = (name) => base32Encode(new TextEncoder().encode(name)).padEnd(52, 'a').slice(0, 52);
const GRACE = mockId('k3f9q2xagrace');
const LINUS = mockId('w8r2d6nblinus');
const RANA = mockId('rana-z6q1e8jd');
const link = (url) => ev(`window.__mockDeepLink(${JSON.stringify(url)})`);
const dialogTitle = () => ev(`document.querySelector('[role="dialog"] h2')?.textContent ?? null`);
const dialogText = () => ev(`document.querySelector('[role="dialog"]')?.textContent ?? ''`);
const dialogButtons = () => ev(`[...document.querySelectorAll('[role="dialog"] button')].map((n) => n.textContent.trim())`);
const clickDialogButton = (label) => ev(`(() => { const n = [...document.querySelectorAll('[role="dialog"] button')].find((x) => x.textContent.trim() === ${JSON.stringify(label)}); if (!n) return false; n.click(); return true; })()`);
const railHas = (name) => ev(`[...document.querySelectorAll('button[aria-label]')].some((n) => n.getAttribute('aria-label') === ${JSON.stringify(name)})`);
const toastHas = (text) => ev(`[...document.querySelectorAll('[role="status"]')].some((n) => n.textContent.includes(${JSON.stringify(text)}))`);

// An invite opens the confirm dialog with the server's card. Nothing joins until Join server is pressed.
check('the rail has no Rustaceans server before the link', (await railHas('Rustaceans')) === false);
await link('dissent://invite/mock-rustaceans');
await sleep(900);
check('an invite link opens the Join server dialog', (await dialogTitle()) === 'Join server', String(await dialogTitle()));
check('the invite dialog shows the full server name', (await dialogText()).includes('Rustaceans'));
check('the invite dialog offers Join server and Cancel', (await dialogButtons()).includes('Join server') && (await dialogButtons()).includes('Cancel'), JSON.stringify(await dialogButtons()));
check('opening the invite joins nothing', (await railHas('Rustaceans')) === false);
await clickDialogButton('Join server');
await sleep(1000);
check('Join server joins the server and closes the dialog', (await dialogTitle()) === null && (await railHas('Rustaceans')) === true);

// An invite to a server already joined says so, and offers Open server.
await link('dissent://invite/member-riverside');
await sleep(900);
check('an invite to a joined server says so', (await dialogText()).includes('already a member of Riverside Makers'), (await dialogText()).slice(0, 120));
check('a joined server offers Open server and no Join', (await dialogButtons()).includes('Open server') && !(await dialogButtons()).includes('Join server'), JSON.stringify(await dialogButtons()));
await clickDialogButton('Open server');
await sleep(700);
check('Open server closes the dialog', (await dialogTitle()) === null);

// A contact link for someone already a contact offers Message, and for someone else a contact request.
const grace = await ev(`window.__mockAddressOf(${JSON.stringify(GRACE)})`);
await link(`dissent://contact/${grace}`);
await sleep(900);
check('a contact link opens the Add contact dialog', (await dialogTitle()) === 'Add contact', String(await dialogTitle()));
check('a contact already in the list is offered Message', (await dialogButtons()).includes('Message') && (await dialogText()).includes('Already a contact'), JSON.stringify(await dialogButtons()));
await clickDialogButton('Message');
await sleep(700);
check('Message opens the conversation with that contact', (await dialogTitle()) === null && (await ev(`!!document.querySelector('textarea[aria-label="Message"]')`)) === true);
await b.press('Escape');
await sleep(400);

const rana = await ev(`window.__mockAddressOf(${JSON.stringify(RANA)})`);
await link(`dissent://contact/${rana}`);
await sleep(900);
check('a stranger is offered a contact request', (await dialogButtons()).includes('Send contact request'), JSON.stringify(await dialogButtons()));
await clickDialogButton('Cancel');
await sleep(500);
check('Cancel sends nothing and closes the dialog', (await dialogTitle()) === null);

// A link Dissent does not open, and a malformed one, get a notice and nothing else.
await link('dissent://nope/whatever');
await sleep(500);
check('an unknown dissent:// link gets a notice', await toastHas("isn't one Dissent can open"));
check('an unknown link opens no dialog', (await dialogTitle()) === null);
await link('dissent://contact/dsu1qqqqqqqqqqqqqqqqqqqq');
await sleep(500);
check('a malformed contact link opens no dialog', (await dialogTitle()) === null);
check('a malformed contact link gets the address notice', await toastHas('typo'));

// A message link to a message this device does not have says so.
await link(`dissent://message/dm/${LINUS}/no-such-message`);
await sleep(4800);
check('a message link to a missing message says so', await toastHas("You can't see that message"));

// A device link opens Settings > Devices with the code in the box, and approves nothing.
await link('dissent://link/mock-device-code');
await sleep(900);
check('a device link fills the code box on Settings › Devices', (await ev(`document.querySelector('input[aria-label="Link code from the new device"]')?.value ?? ''`)) === 'dissent://link/mock-device-code');
check('a device link approves nothing by itself', !(await toastHas('New device approved')));

await b.close();
}

// ---- the title bar's DHT and relay latency: shown in ms, coloured by its value, and updated by the net_latency event ----
async function section_netlatency() {
const b = await launch({ url: `${BASE}/`, width: 1600, height: 1000, port: 9807, profile: `${SCRATCH}/netlatency-next-profile` });
const { ev } = b;
await sleep(1200);
const line = () => ev(`document.querySelector('[data-net-line]')?.textContent ?? ''`);
const segments = () => ev(`[...document.querySelectorAll('[data-net-line] [data-net-tone]')].map((n) => ({ text: n.textContent, tone: n.dataset.netTone }))`);
const latencySegment = async () => (await segments()).find((s) => /ms( lookup)?$/.test(s.text) || s.text === '—');

check('the title bar shows the DHT lookup time as a lookup in ms', (await line()).includes('2529 ms lookup'), await line());
check('the title bar shows the relay round trip in ms', (await line()).includes('42 ms'), await line());
check('a normal lookup takes no warning colour', (await latencySegment())?.tone === 'normal', JSON.stringify(await latencySegment()));

await ev(`window.__mockNetLatency(3500, 48)`);
await sleep(500);
check('a new probe reading updates the bar without polling', (await line()).includes('3500 ms lookup') && !(await line()).includes('2529 ms lookup'), await line());
check('a lookup from 3 to 5 s is a warning', (await segments()).some((s) => s.text === '3500 ms lookup' && s.tone === 'warn'), JSON.stringify(await segments()));

await ev(`window.__mockNetLatency(6000, 48)`);
await sleep(500);
check('a lookup over 5 s is slow', (await segments()).some((s) => s.text === '6000 ms lookup' && s.tone === 'slow'), JSON.stringify(await segments()));

await ev(`window.__mockNetLatency(null, 48)`);
await sleep(500);
check('no lookup answered shows a dash', (await line()).includes('—'), await line());

await b.close();
}

// ---- desktop shell: the window and sign-in toggles on Appearance, and the notification toggle on Notifications & sounds ----
async function section_shell() {
const b = await launch({ url: `${BASE}/`, width: 1600, height: 1000, port: 9797, profile: `${SCRATCH}/shell-next-profile` });
const { ev } = b;
await sleep(800);
// A repeated event for one message alerts once. A mention is posted, announced, then announced again 5.5 s later
// (past the 5 s gaps for the sound and the notification): the first announcement notifies and sounds, the repeat neither.
await sleep(5500);
await b.ev(`(() => {
  window.__mockNotified = [];
  window.__mockPlays = 0;
  window.__origPlay = window.__origPlay ?? HTMLMediaElement.prototype.play;
  HTMLMediaElement.prototype.play = function () { window.__mockPlays++; return window.__origPlay.apply(this, arguments); };
  document.hasFocus = () => false;
  window.__mockPostMention('c1', 'ch2');
  window.__mockEngineEvent({ kind: 'message', community_id: 'c1', channel_id: 'ch2' });
})()`);
await sleep(400);
const firstAlert = await b.ev(`JSON.stringify([window.__mockNotified.length, window.__mockPlays])`);
check('E26 a new mention notifies and sounds once', firstAlert === '[1,1]', firstAlert);
await sleep(5500);
await b.ev(`window.__mockEngineEvent({ kind: 'message', community_id: 'c1', channel_id: 'ch2' })`);
await sleep(400);
const repeatAlert = await b.ev(`JSON.stringify([window.__mockNotified.length, window.__mockPlays])`);
check('E26 the same message announced again does not notify or sound again', repeatAlert === '[1,1]', repeatAlert);
await b.ev(`(() => { delete document.hasFocus; HTMLMediaElement.prototype.play = window.__origPlay; })()`);

// A new group message alerts once, the same as a channel message. The repeat, 5.5 s later, notifies and sounds no more.
const startAlerts = `(() => {
  window.__mockNotified = [];
  window.__mockPlays = 0;
  window.__origPlay = window.__origPlay ?? HTMLMediaElement.prototype.play;
  HTMLMediaElement.prototype.play = function () { window.__mockPlays++; return window.__origPlay.apply(this, arguments); };
  document.hasFocus = () => false;
})()`;
const stopAlerts = `(() => { delete document.hasFocus; HTMLMediaElement.prototype.play = window.__origPlay; })()`;
await sleep(5500);
await b.ev(startAlerts);
await b.ev(`window.__mockPostGroupMessage('grp1', 'group announcement')`);
await b.ev(`window.__mockEngineEvent({ kind: 'group', group_id: 'grp1' })`);
await sleep(400);
const groupFirst = await b.ev(`JSON.stringify([window.__mockNotified.length, window.__mockPlays])`);
check('E40 a new group message notifies and sounds once', groupFirst === '[1,1]', groupFirst);
await sleep(5500);
await b.ev(`window.__mockEngineEvent({ kind: 'group', group_id: 'grp1' })`);
await sleep(400);
const groupRepeat = await b.ev(`JSON.stringify([window.__mockNotified.length, window.__mockPlays])`);
check('E40 the same group message announced again does not notify or sound again', groupRepeat === '[1,1]', groupRepeat);
await b.ev(stopAlerts);

// A private-channel message alerts through the same path as a channel message. Whether this device can read it is
// decided by the engine, which only announces readable messages (a Rust test covers that).
await sleep(5500);
await b.ev(startAlerts);
await b.ev(`(async () => { await window.__TAURI__.core.invoke('set_channel_access', { communityId: 'c1', channelId: 'ch4', private: true, roles: [], members: [] }); window.__mockPostMention('c1', 'ch4'); window.__mockEngineEvent({ kind: 'message', community_id: 'c1', channel_id: 'ch4' }); })()`);
await sleep(600);
const privateFirst = await b.ev(`JSON.stringify([window.__mockNotified.length, window.__mockPlays])`);
check('E41 a private channel message notifies and sounds once', privateFirst === '[1,1]', privateFirst);
await b.ev(stopAlerts);

await b.ev(`document.querySelector('button[aria-label="Settings"]').click()`);
await sleep(400);
const nav = (label) => ev(`(() => { const n = [...document.querySelectorAll('[data-settings-page] nav button')].find((x) => x.textContent.includes(${JSON.stringify(label)})); if (!n) return false; n.click(); return true; })()`);
const box = (label) => ev(`(() => { const l = [...document.querySelectorAll('[data-settings-page] label')].find((x) => x.textContent.includes(${JSON.stringify(label)})); return l?.querySelector('input')?.checked ?? null; })()`);
const enabled = (label) => ev(`(() => { const l = [...document.querySelectorAll('[data-settings-page] label')].find((x) => x.textContent.includes(${JSON.stringify(label)})); return l ? !l.querySelector('input').disabled : null; })()`);
const clickBox = (label) => ev(`(() => { const l = [...document.querySelectorAll('[data-settings-page] label')].find((x) => x.textContent.includes(${JSON.stringify(label)})); l?.querySelector('input')?.click(); return !!l; })()`);

await nav('Appearance');
await sleep(300);
check('keep running starts off', (await box('Keep Dissent running when the window is closed')) === false);
check('start at login is offered in the browser preview', (await enabled('Open Dissent when you sign in')) === true);
check('start at login starts off', (await box('Open Dissent when you sign in')) === false);
await clickBox('Keep Dissent running when the window is closed');
await sleep(400);
check('keep running turns on', (await box('Keep Dissent running when the window is closed')) === true);
await clickBox('Open Dissent when you sign in');
await sleep(400);
check('start at login turns on', (await box('Open Dissent when you sign in')) === true);

await nav('Notifications & sounds');
await sleep(300);
check('desktop notifications start on', (await box('Desktop notifications')) === true);
await clickBox('Desktop notifications');
await sleep(400);
check('desktop notifications turn off', (await box('Desktop notifications')) === false);

await nav('Appearance');
await sleep(300);
check('the window choices keep their state across pages', (await box('Keep Dissent running when the window is closed')) === true && (await box('Open Dissent when you sign in')) === true);

await b.close();
}

// ---- Animated server icons: the rail shows the looping GIF, and its still poster while the window is in the background ----
async function section_animatedicon() {
const b = await launch({ url: `${BASE}/`, width: 1600, height: 1000, port: 9784, profile: `${SCRATCH}/animatedicon-next-profile` });
const { ev } = b;
await sleep(800);
const railSrc = () => ev(`document.querySelector('button[aria-label="Riverside Makers"] img')?.getAttribute('src') ?? null`);
check('a server with no icon shows none on the rail', (await railSrc()) === null);
// The mock saves an animated icon the way the engine would: a GIF with its still poster.
await ev(`window.__mockAnimatedIcon('c1')`);
await sleep(900);
const gif = await railSrc();
check('an animated icon shows its GIF on the rail', typeof gif === 'string' && gif.startsWith('data:image/gif'), String(gif).slice(0, 40));
await ev(`window.dispatchEvent(new Event('blur'))`);
await sleep(400);
const still = await railSrc();
check('the rail shows the still poster while the window is in the background', typeof still === 'string' && still.startsWith('data:image/png'), String(still).slice(0, 40));
await ev(`window.dispatchEvent(new Event('focus'))`);
await sleep(400);
const back = await railSrc();
check('the GIF returns when the window is focused again', typeof back === 'string' && back.startsWith('data:image/gif'), String(back).slice(0, 40));
await b.close();

// The invite card shows the same icon: the GIF with the window focused, the poster when motion is reduced. The link names
// "member", so the mock previews Riverside Makers, whose icon is set above.
// The mock's asset URL encodes a GIF's data URL and leaves a PNG's as it is, so the source is decoded before it is compared.
const inviteCardSrc = async (ev) => decodeURIComponent((await ev(`document.querySelector('[data-server-card] img')?.getAttribute('src') ?? ''`)) ?? '');
async function invitePostedAndShown(b) {
  await b.ev(`window.__mockAnimatedIcon('c1')`);
  await b.ev(`window.__mockPost('c1', 'ch1', 'k3f9q2xagrace', 'Join us: dissent://invite/mock-member')`);
  await sleep(700);
  await b.ev(`document.querySelector('button[aria-label="Riverside Makers"]').click()`);
  await sleep(700);
  await b.ev(`[...document.querySelectorAll('button')].find((n) => n.textContent.trim().startsWith('general'))?.click()`);
  await sleep(1200);
}
const focused = await launch({ url: `${BASE}/`, width: 1600, height: 1000, port: 9785, profile: `${SCRATCH}/animatedicon-focus-profile` });
await sleep(800);
await invitePostedAndShown(focused);
const card = await inviteCardSrc(focused.ev);
check('the invite card shows the animated icon GIF when the window has focus', card.includes('data:image/gif'), card.slice(0, 60));
await focused.close();

const reduced = await launch({ url: `${BASE}/`, width: 1600, height: 1000, port: 9786, profile: `${SCRATCH}/animatedicon-reduced-profile`, reducedMotion: true });
await sleep(800);
await invitePostedAndShown(reduced);
const poster = await inviteCardSrc(reduced.ev);
check('the invite card shows the poster under reduced motion', poster.includes('data:image/png') && !poster.includes('data:image/gif'), poster.slice(0, 60));
await reduced.close();
}

// ---- A pin older than the loaded window: the pins list reaches it, loading the older history first ----
async function section_oldpin() {
const b = await launch({ url: `${BASE}/`, width: 1600, height: 1000, port: 9787, profile: `${SCRATCH}/oldpin-next-profile` });
const { ev } = b;
await sleep(800);
await ev(`document.querySelector('button[aria-label="Riverside Makers"]').click()`);
await sleep(700);
await ev(`[...document.querySelectorAll('button')].find((n) => n.textContent.trim().startsWith('general'))?.click()`);
await sleep(700);
// 210 more messages push the oldest one out of the newest 200 the channel loads.
await ev(`(() => { for (let i = 0; i < 210; i++) window.__mockPost('c1', 'ch1', 'k3f9q2xagrace', 'Bulk message ' + i); })()`);
await sleep(2500);
const oldText = await ev(`window.__mockPinOldest('c1', 'ch1')`);
const inView = (needle) => ev(`[...document.querySelectorAll('.messages .msg')].some((n) => n.textContent.includes(${JSON.stringify(needle)}))`);
check('the oldest message is not in the loaded window before the jump', (await inView(oldText)) === false, String(oldText).slice(0, 40));
await ev(`[...document.querySelectorAll('button')].find((n) => n.getAttribute('aria-label') === 'Pinned messages')?.click()`);
await sleep(500);
await ev(`[...document.querySelectorAll('[role^="menuitem"]')][0]?.click()`);
await sleep(2000);
check('a pick jumps to a pin older than the loaded window', (await inView(oldText)) === true);
// A new message arrives: the live reload keeps the wider window, so the jump is not yanked back to the newest 200.
await ev(`window.__mockPost('c1', 'ch1', 'k3f9q2xagrace', 'A new message after the jump')`);
await sleep(1500);
check('a new message does not pull a jumped-to pin out of view', (await inView(oldText)) === true);
await b.close();

// A pin past the cap: the jump says it cannot find the message in this channel's history.
const capped = await launch({ url: `${BASE}/`, width: 1600, height: 1000, port: 9788, profile: `${SCRATCH}/oldpin-cap-profile` });
await sleep(800);
await capped.ev(`document.querySelector('button[aria-label="Riverside Makers"]').click()`);
await sleep(700);
await capped.ev(`[...document.querySelectorAll('button')].find((n) => n.textContent.trim().startsWith('general'))?.click()`);
await sleep(700);
await capped.ev(`(() => { for (let i = 0; i < 520; i++) window.__mockPost('c1', 'ch1', 'k3f9q2xagrace', 'Bulk message ' + i); })()`);
await sleep(2500);
// Lower the cap below the wider windows, so the oldest pin is past it.
await capped.ev(`window.__mockHistoryCap = 300`);
await capped.ev(`window.__mockPinOldest('c1', 'ch1')`);
await capped.ev(`[...document.querySelectorAll('button')].find((n) => n.getAttribute('aria-label') === 'Pinned messages')?.click()`);
await sleep(500);
await capped.ev(`[...document.querySelectorAll('[role^="menuitem"]')][0]?.click()`);
await sleep(1500);
check('a pin past the history cap says it cannot find the message', (await capped.ev(`[...document.querySelectorAll('[role=status]')].some((n) => n.textContent.includes("Couldn't find that message"))`)) === true);
await capped.close();
}

// ---- Scroll back: the list asks for older pages near its top, holds the message at the top in place, and reaches the start ----
async function section_scrollback() {
const b = await launch({ url: `${BASE}/`, width: 1600, height: 1000, port: 9789, profile: `${SCRATCH}/scrollback-next-profile` });
const { ev } = b;
await sleep(800);
await ev(`document.querySelector('button[aria-label="Riverside Makers"]').click()`);
await sleep(700);
await ev(`[...document.querySelectorAll('button')].find((n) => n.textContent.trim().startsWith('general'))?.click()`);
await sleep(700);
// 300 messages: the channel holds more than the newest 200 that it loads first.
await ev(`(() => { for (let i = 0; i < 300; i++) window.__mockPost('c1', 'ch1', 'k3f9q2xagrace', 'Scroll message ' + i); })()`);
await sleep(3000);
// The top message on screen, and how far below the list's top it sits.
const rowAt = (id) => ev(`(() => { const el = document.querySelector('.messages'); const t = el.getBoundingClientRect().top; const row = [...el.querySelectorAll('.msg')].find((n) => n.dataset.id === ${JSON.stringify(id)}); return row ? row.getBoundingClientRect().top - t : null; })()`);
const scrollTop = () => ev(`document.querySelector('.messages').scrollTop = 0`);
// The message at the top when the list is scrolled to its start, read in the same step as the scroll, so it is the message
// the list anchors on (the scroll handler reads the same rows, a moment later, before any page comes in).
const rowsBefore = await ev(`document.querySelectorAll('.messages .msg').length`);
const anchor = await ev(`(() => { const el = document.querySelector('.messages'); el.scrollTop = 0; const t = el.getBoundingClientRect().top; const row = [...el.querySelectorAll('.msg')].find((n) => n.getBoundingClientRect().top >= t); return row ? { id: row.dataset.id, at: row.getBoundingClientRect().top - t } : null; })()`);
// The page is in when the list has more rows and no loader is left, then a short settle for the layout.
await until(async () => (await ev(`document.querySelectorAll('.messages .msg').length`)) > rowsBefore && !(await ev(`!!document.querySelector('.messages [role="status"]')`)));
await sleep(300);
const held = anchor ? await rowAt(anchor.id) : null;
check('a page of older history comes in with the message at the top held in place', anchor !== null && held !== null && Math.abs(held - anchor.at) <= 4, JSON.stringify({ anchor, held }));
for (let i = 0; i < 40; i++) {
  const start = await ev(`document.querySelector('.messages')?.textContent.includes('This is the start of the #general channel')`);
  if (start) break;
  await scrollTop();
  await sleep(500);
}
check('scrolling up reaches the oldest message', (await ev(`document.querySelector('.messages')?.textContent.includes('Scroll message 0')`)) === true);
check('the start of the channel header shows at the true start', (await ev(`document.querySelector('.messages')?.textContent.includes('This is the start of the #general channel')`)) === true);
// A live message reads only the newest page, and the history already shown stays above it.
await ev(`window.__mockPageLog = []`);
await ev(`window.__mockPost('c1', 'ch1', 'k3f9q2xagrace', 'A message while scrolled back')`);
await until(() => ev(`(window.__mockPageLog ?? []).length > 0`));
await sleep(500);
const liveReads = await ev(`window.__mockPageLog`);
check('a live message reads only the newest page', liveReads.length >= 1 && liveReads.every((r) => r.before === null && r.limit === 200), JSON.stringify(liveReads));
check('the history above the new message is kept', (await ev(`document.querySelector('.messages')?.textContent.includes('Scroll message 0')`)) === true);
// A direct conversation has more than its newest 200 too, and scrolls back the same way.
// The mock's id for Jonas: the name's base32 bytes, padded to 52 characters, as the mock makes them.
const JONAS = base32Encode(new TextEncoder().encode('w8r2d6nblinus')).padEnd(52, 'a').slice(0, 52);
await ev(`document.querySelector('button[aria-label="Direct messages"]').click()`);
await sleep(700);
await ev(`document.querySelector('button[title="Jonas"]')?.click()`);
await sleep(900);
await ev(`(() => { for (let i = 0; i < 300; i++) window.__mockDm('${JONAS}', 'DM scroll ' + i); })()`);
await sleep(3000);
await until(async () => {
  if (await ev(`document.querySelector('.messages')?.textContent.includes('DM scroll 0')`)) return true;
  await scrollTop();
  return false;
}, 30000);
check('a direct conversation scrolls back to its oldest message', (await until(() => ev(`document.querySelector('.messages')?.textContent.includes('DM scroll 0')`))) === true);
await b.close();
}

// ---- Emoticons: :D and the like become emoji in the composer, live on a space and on send, in channels and DMs. Code
// keeps them as typed, and with the setting off nothing converts. ----
async function section_emoticons() {
const b = await launch({ url: `${BASE}/`, width: 1600, height: 1000, port: 9809, profile: `${SCRATCH}/emoticons-next-profile` });
const { send, ev } = b;
await sleep(1500);
const SIDE = 'aside[aria-label="Channels and conversations"]';
const openRow = async (label) => {
  await ev(`[...document.querySelectorAll('${SIDE} button')].find((x) => x.textContent.includes(${JSON.stringify(label)}))?.click()`);
  await sleep(700);
};
const composer = `document.querySelector('textarea[aria-label="Message"]')`;
const typeIn = async (text) => {
  await ev(`${composer}.focus()`);
  await send('Input.insertText', { text });
  await sleep(250);
};
const enter = async () => {
  const base = { key: 'Enter', code: 'Enter', windowsVirtualKeyCode: 13, nativeVirtualKeyCode: 13 };
  await send('Input.dispatchKeyEvent', { type: 'rawKeyDown', ...base, text: '\r' });
  await send('Input.dispatchKeyEvent', { type: 'char', ...base, text: '\r' });
  await send('Input.dispatchKeyEvent', { type: 'keyUp', ...base });
  await sleep(900);
};
// The body of the newest message whose text includes `marker`.
const bodyOf = (marker) => ev(`[...document.querySelectorAll('.messages article.msg')].reverse().find((a) => a.textContent.includes(${JSON.stringify(marker)}))?.querySelector('.body')?.textContent ?? null`);

// A channel: sent as typed, converted on send.
await ev(`document.querySelector('nav[aria-label="Servers and direct messages"] button[aria-label="Riverside Makers"]')?.click()`);
await sleep(800);
await openRow('general');
await typeIn(':D hi from the channel');
await enter();
check('EM1 ":D" sent in a channel shows as 😄', (await bodyOf('hi from the channel'))?.startsWith('😄 hi from the channel') === true, String(await bodyOf('hi from the channel')));
// Code keeps the emoticon as typed.
await typeIn('code `:D` and :P stays');
await enter();
check('EM2 an emoticon in inline code stays as typed, and one outside converts', (await ev(`(() => { const a = [...document.querySelectorAll('.messages article.msg')].reverse().find((x) => x.textContent.includes('and')&&x.textContent.includes('stays')); return [a?.querySelector('code')?.textContent, a?.querySelector('.body')?.textContent.includes('😛')]; })()`)).join('|') === ':D|true');
// A link and a time stay as typed. (An https link would first ask about link previews.)
await typeIn('at 10:30 see http://example.com/a:D std::path');
await enter();
check('EM3 a time, a link and a path stay as typed', (await bodyOf('at 10:30'))?.includes('at 10:30 see http://example.com/a:D std::path') === true, String(await bodyOf('at 10:30')));

// A DM: a space after the emoticon converts it at once, before the send.
await ev(`document.querySelector('button[aria-label="Direct messages"]')?.click()`);
await sleep(600);
await openRow('Maya');
await typeIn(':P ');
check('EM4 typing a space after ":P" shows 😛 in the composer at once', (await ev(`${composer}.value`)) === '😛 ', String(await ev(`${composer}.value`)));
await typeIn('typed live');
await enter();
check('EM5 ":P" sent in a DM shows as 😛', (await bodyOf('typed live')) === '😛 typed live', String(await bodyOf('typed live')));
// A backslash keeps it as typed.
await typeIn('\\:D escaped');
await enter();
check('EM6 a backslash keeps the emoticon as typed', (await bodyOf('escaped')) === ':D escaped', String(await bodyOf('escaped')));
// The inline edit converts too, live as you type and on save.
await ev(`(() => { const a = [...document.querySelectorAll('.messages article.msg')].reverse().find((x) => x.textContent.includes('escaped')); a?.querySelector('button[aria-label="Edit message"]')?.click(); })()`);
await sleep(400);
const editBox = `document.querySelector('textarea[aria-label="Edit message"]')`;
await ev(`(() => { const t = ${editBox}; t.focus(); t.select(); })()`);
await send('Input.insertText', { text: 'edited :P ' });
await sleep(250);
check('EM6a typing a space after ":P" in the edit box shows 😛 at once', (await ev(`${editBox}?.value`)) === 'edited 😛 ', String(await ev(`${editBox}?.value`)));
await send('Input.insertText', { text: 'and :D' });
await sleep(200);
await enter();
check('EM6b the saved edit converts the rest on save', (await bodyOf('edited'))?.startsWith('edited 😛 and 😄') === true, String(await bodyOf('edited')));

// The setting off: nothing converts.
await ev(`document.querySelector('button[aria-label="Settings"]').click()`);
await sleep(500);
await ev(`[...document.querySelectorAll('[data-settings-page] nav button')].find((x) => x.textContent.includes('Appearance'))?.click()`);
await sleep(300);
const emoBox = `[...document.querySelectorAll('[data-settings-page] label')].find((x) => x.textContent.includes('Convert emoticons to emoji'))?.querySelector('input')`;
check('EM7 "Convert emoticons to emoji" is in Settings › Appearance and starts on', (await ev(`${emoBox}?.checked`)) === true);
await ev(`${emoBox}.click()`);
await sleep(400);
check('EM8 the setting turns off', (await ev(`${emoBox}?.checked`)) === false);
await send('Input.dispatchKeyEvent', { type: 'rawKeyDown', key: 'Escape', code: 'Escape', windowsVirtualKeyCode: 27, nativeVirtualKeyCode: 27 });
await send('Input.dispatchKeyEvent', { type: 'keyUp', key: 'Escape', code: 'Escape', windowsVirtualKeyCode: 27, nativeVirtualKeyCode: 27 });
await sleep(500);
await typeIn(':P ');
check('EM9 with the setting off, a space after ":P" leaves it as typed', (await ev(`${composer}.value`)) === ':P ', String(await ev(`${composer}.value`)));
await typeIn('setting off');
await enter();
check('EM10 with the setting off, ":P" is sent as typed', (await bodyOf('setting off')) === ':P setting off', String(await bodyOf('setting off')));
await b.close();
}

// ---- Toasts, each kind, and the accent and danger pieces side by side under colour-vision simulation ----
async function section_toasts() {
const b = await launch({ url: `${BASE}/`, width: 1600, height: 1000, port: 9790, profile: `${SCRATCH}/toasts-next-profile` });
const { ev } = b;
await sleep(800);
// One toast of each kind, and one with an action button. Each icon takes its colour from the kind's token.
await ev(`window.__mockToast('info', 'An info toast for the audit')`);
await ev(`window.__mockToast('error', 'An error toast for the audit')`);
await ev(`window.__mockToast('error', 'An error toast with an action', true)`);
await sleep(400);
const iconColour = (text) => ev(`(() => { const t = [...document.querySelectorAll('[role=status]')].find((n) => n.textContent.includes(${JSON.stringify(text)})); const svg = t?.querySelector('svg'); return svg ? getComputedStyle(svg).color : null; })()`);
check('an info toast shows its icon in the link colour', (await iconColour('An info toast for the audit')) === 'rgb(124, 183, 255)', String(await iconColour('An info toast for the audit')));
check('an error toast shows its icon in the danger text colour', (await iconColour('An error toast for the audit')) === 'rgb(255, 122, 160)', String(await iconColour('An error toast for the audit')));
check('a toast with an action shows its button', (await ev(`[...document.querySelectorAll('[role=status] button')].some((n) => n.textContent.trim() === 'Jump')`)) === true);
await b.screenshot(`${SCRATCH}/shots/toasts.png`);
await b.close();

// The accent and danger pieces, as they draw, then under deuteranopia and protanopia simulations.
const s = await launch({ url: `${BASE}/`, width: 1000, height: 700, port: 9791, profile: `${SCRATCH}/swatch-next-profile` });
await sleep(800);
const pieces = (mode) => s.ev(`window.__mockSwatches('${mode}')`);
await pieces('normal');
await sleep(300);
await s.screenshot(`${SCRATCH}/shots/swatches-normal.png`);
await pieces('deuteranopia');
await sleep(300);
await s.screenshot(`${SCRATCH}/shots/swatches-deuteranopia.png`);
await pieces('protanopia');
await sleep(300);
await s.screenshot(`${SCRATCH}/shots/swatches-protanopia.png`);
// The same comparison in numbers: the colour-blindness matrices applied to the live tokens, then a CIE76 distance. Under each
// simulation the accent and the danger fill, and the accent and danger text, must differ by at least 20.
const distances = await s.ev(`(() => {
  const css = getComputedStyle(document.documentElement);
  const rgb = (t) => { const h = css.getPropertyValue(t).trim(); return [1, 3, 5].map((i) => parseInt(h.slice(i, i + 2), 16)); };
  const lin = (v) => { const x = v / 255; return x <= 0.04045 ? x / 12.92 : ((x + 0.055) / 1.055) ** 2.4; };
  const M = { deut: [[0.367322, 0.860646, -0.227968], [0.280085, 0.672501, 0.047413], [-0.011820, 0.042940, 0.968881]], prot: [[0.152286, 1.052583, -0.204868], [0.114503, 0.786281, 0.099216], [-0.003882, -0.048116, 1.051998]] };
  const sim = (t, m) => { const l = rgb(t).map(lin); return m.map((row) => row[0] * l[0] + row[1] * l[1] + row[2] * l[2]); };
  const lab = ([r, g, bl]) => { const X = 0.4124 * r + 0.3576 * g + 0.1805 * bl, Y = 0.2126 * r + 0.7152 * g + 0.0722 * bl, Z = 0.0193 * r + 0.1192 * g + 0.9505 * bl; const f = (t) => (t > 0.008856 ? t ** (1 / 3) : 7.787 * t + 16 / 116); const fx = f(X / 0.95047), fy = f(Y), fz = f(Z / 1.08883); return [116 * fy - 16, 500 * (fx - fy), 200 * (fy - fz)]; };
  const dE = (a, c) => Math.hypot(...lab(a).map((v, i) => v - lab(c)[i]));
  const out = {};
  for (const [name, m] of Object.entries(M)) {
    out[name] = { fill: dE(sim('--accent', m), sim('--danger', m)), text: dE(sim('--accent-text', m), sim('--danger-text', m)) };
  }
  return out;
})()`);
check('accent and danger fills differ under deuteranopia (CIE76 >= 20)', distances.deut.fill >= 20, JSON.stringify(distances));
check('accent and danger fills differ under protanopia (CIE76 >= 20)', distances.prot.fill >= 20, JSON.stringify(distances));
check('accent and danger text differ under deuteranopia (CIE76 >= 20)', distances.deut.text >= 20, JSON.stringify(distances));
await s.close();
}

const SECTIONS = [
  ['core', section_core],
  ['friends', section_friends],
  ['calls', section_calls],
  ['startup', section_startup],
  ['rail', section_rail],
  ['previews', section_previews],
  ['markdown', section_markdown],
  ['messages', section_messages],
  ['nomentions', section_nomentions],
  ['serverprefs', section_serverprefs],
  ['unreaddms', section_unreaddms],
  ['dmclose', section_dmclose],
  ['deeplink', section_deeplink],
  ['netlatency', section_netlatency],
  ['animatedicon', section_animatedicon],
  ['oldpin', section_oldpin],
  ['toasts', section_toasts],
  ['scrollback', section_scrollback],
  ['shell', section_shell],
  ['emoticons', section_emoticons],
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
console.log(`
${results.length - failed.length}/${results.length} checks passed`);
// Every failing check by name, on stdout and in the scratch folder, so a run's failures can be counted across runs.
const failedText = failed.map((r) => `FAILED ${r.name}${r.detail ? ` :: ${r.detail}` : ''}`).join('\n');
writeFileSync(`${SCRATCH}/failed-checks.txt`, failed.length ? `${failedText}\n` : '');
if (failed.length) console.log(`FAILED CHECKS (${failed.length}):\n${failedText}`);
process.exit(failed.length ? 1 : 0);
