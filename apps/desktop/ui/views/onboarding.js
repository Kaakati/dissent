// First run: the identity screen shown once when the engine reports `me.first_run` (Part 6.12). It covers the app
// until the user confirms they saved the recovery phrase. The engine clears the flag; the UI only hides the screen.
import { api } from '../api.js';
import { el, icon, avatar, copyText, toast, openDialog } from '../ui.js';
import { shortOf } from '../address.js';
import * as actions from '../actions.js';
import { setTitleOverride } from './titlebar.js';
import { state } from '../state.js';

export async function showOnboarding() {
  setTitleOverride('Welcome to Dissent', 'dissent-mark'); // the titlebar reads "Welcome to Dissent" during first run
  let words = [];
  try {
    words = String(await api.identityExportPhrase()).trim().split(/\s+/).filter(Boolean);
  } catch (err) {
    toast(err);
  }
  const me = state.me ?? {};
  const saved = el('input', { type: 'checkbox', id: 'onboard-saved' });
  const cont = el('button', { class: 'primary', text: 'Continue', disabled: true });
  saved.addEventListener('change', () => { cont.disabled = !saved.checked; });
  const copy = el('button', {
    class: 'ghost',
    text: 'Copy',
    icon: 'copy',
    onclick: async () => {
      if (await copyText(words.join(' '))) toast('Recovery phrase copied', 'info');
    },
  });
  const overlay = el('div', { class: 'onboarding', role: 'dialog', 'aria-modal': 'true', 'aria-label': 'Welcome to Dissent' },
    el('section', { class: 'onboard-intro' },
      el('div', { class: 'onboard-brand' }, el('span', { class: 'onboard-mark' }, icon('dissent-mark')), el('span', { text: 'Dissent' })),
      el('h1', { text: 'You are your key, not an account.' }),
      el('p', { class: 'muted', text: 'No sign-up, email or phone number. Your identity was just created on this device, and no company keeps a list of users.' }),
      el('ol', { class: 'onboard-steps' },
        el('li', { class: 'done' }, el('span', { class: 'dot', 'aria-hidden': 'true' }), 'Pick a display name'),
        el('li', { class: 'current', 'aria-current': 'step' }, el('span', { class: 'dot', text: '2' }), 'Save your recovery phrase'),
        el('li', {}, el('span', { class: 'dot ring', text: '3' }), 'Join a server or add a contact'))),
    el('section', { class: 'onboard-body' },
      el('div', { class: 'onboard-id' },
        avatar(me.display_name || '?', me.root_id),
        el('div', { class: 'text' },
          el('div', { class: 'name', text: me.display_name || 'Set a display name later' }),
          el('div', { class: 'tag', text: me.root_id ? `·${shortOf('user', me.root_id)}` : '' })),
        el('p', { class: 'muted', text: 'Names can repeat. The tag after the dot comes from your key, so people can tell you apart.' })),
      el('h2', { text: 'Your recovery phrase' }),
      el('p', { class: 'muted', text: `With these ${words.length} words you can restore yourself on a new computer. Nobody else holds a copy, so nobody can recover it for you.` }),
      el('ol', { class: 'phrase', 'aria-label': 'Recovery phrase' },
        words.map((w, i) => el('li', {}, el('span', { class: 'n', text: String(i + 1) }), el('span', { class: 'word', text: w })))),
      el('div', { class: 'callout warn' }, icon('circle-alert'), el('span', { text: 'Anyone with these words controls your identity. Keep them offline and never share them.' })),
      el('div', { class: 'onboard-actions' },
        el('label', { class: 'saved-check' }, saved, el('span', { text: 'I wrote these words down somewhere safe' })),
        copy,
        cont),
      el('p', { class: 'restore-link' }, 'Already have an identity? ', el('button', { class: 'link', text: 'Restore from a recovery phrase', onclick: () => openRestore(overlay) }))));
  cont.addEventListener('click', async () => {
    cont.disabled = true;
    try {
      await api.completeFirstRun();
      overlay.remove();
      setTitleOverride(null);
    } catch (err) {
      cont.disabled = false;
      toast(err);
    }
  });
  document.body.append(overlay);
  saved.focus();
}

// Restore instead of creating a new identity: the same flow as Settings > Import phrase.
function openRestore(overlay) {
  const phrase = el('textarea', { rows: 3, placeholder: '24 recovery words, separated by spaces', 'aria-label': 'Recovery phrase to restore' });
  let close = () => {};
  const restore = el('button', {
    class: 'primary',
    text: 'Restore',
    onclick: async () => {
      const words = phrase.value.trim();
      if (!words) return;
      if (await actions.restoreIdentity(words)) {
        close();
        overlay.remove();
        setTitleOverride(null);
      }
    },
  });
  const cancel = el('button', { class: 'ghost', text: 'Cancel', onclick: () => close() });
  ({ close } = openDialog({
    title: 'Restore from a recovery phrase',
    sub: 'Replaces the identity on this device with the one from the phrase.',
    content: phrase,
    footer: [cancel, restore],
  }));
}
