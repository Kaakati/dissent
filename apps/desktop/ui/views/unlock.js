// Portable exes: the screens that run before anything else starts. A first run asks how to use profiles (the
// chooser). A locked portable profile asks for its passphrase. A linked device shows a code to approve from the
// installed app and takes back the grant. Nothing opens until one of these finishes. The overlays can't be
// dismissed, so only a finished step lets startup go on.
import { api } from '../api.js';
import { el, icon, copyText, toast } from '../ui.js';

const MIN_LENGTH = 8;

// A hint from length and character variety. Only the minimum length is enforced.
export function strengthHint(pass) {
  if (pass.length < MIN_LENGTH) return { level: 'short', text: `Use at least ${MIN_LENGTH} characters.` };
  const kinds = [/[a-z]/, /[A-Z]/, /\d/, /[^A-Za-z0-9]/].filter((re) => re.test(pass)).length;
  if (pass.length >= 16 || (pass.length >= 12 && kinds >= 3)) return { level: 'strong', text: 'Strong passphrase.' };
  if (pass.length >= 12 || kinds >= 3) return { level: 'fair', text: 'Fair. A longer passphrase is safer.' };
  return { level: 'weak', text: 'Weak. Add length or more kinds of character.' };
}

// Resolves once the profile is open. Ordinary profiles are never locked, so this returns at once for them.
export async function unlockIfLocked() {
  const status = await api.vaultStatus();
  // The chooser restarts the app once an option is saved, so this never resolves here.
  if (status.chooser) return new Promise(() => showChooser(status));
  if (!status.locked) return;
  await new Promise((done) => showUnlock(status, done));
}

const OPTIONS = [
  {
    choice: 'installed',
    title: "Use this computer's Dissent profile",
    sentence: 'Opens the profile the installed Dissent app already uses here. Nothing is copied.',
    needsInstalled: true,
  },
  {
    choice: 'link',
    title: 'Link as a new device (recommended for a USB stick)',
    sentence: 'Gives this stick its own device key, which your installed Dissent approves and can remove later.',
  },
  {
    choice: 'copy',
    title: 'Copy my profile into this portable folder',
    sentence: 'Makes a passphrase-protected copy of the installed profile. Both copies then share one device identity, so linking is safer.',
    needsInstalled: true,
  },
  {
    choice: 'fresh',
    title: 'Start fresh / restore from recovery phrase',
    sentence: 'Starts a new identity protected by a passphrase, or restores one from your recovery phrase.',
  },
];

function showChooser(status) {
  const error = el('p', { class: 'error', role: 'alert' });
  const buttons = OPTIONS.map((option) => {
    const missing = option.needsInstalled && !status.installed_profile;
    const button = el('button', {
      class: 'chooser-option',
      type: 'button',
      disabled: missing,
      onclick: async () => {
        error.textContent = '';
        buttons.forEach((b) => { b.disabled = true; });
        try {
          // On success the app restarts in the chosen mode.
          await api.chooseProfile(option.choice);
        } catch (err) {
          error.textContent = String(err);
          buttons.forEach((b, i) => { b.disabled = OPTIONS[i].needsInstalled && !status.installed_profile; });
        }
      },
    },
    el('span', { class: 'chooser-title', text: option.title }),
    el('span', { class: 'muted', text: missing ? 'No Dissent profile was found on this computer, so this option is not available.' : option.sentence }));
    button.dataset.choice = option.choice;
    return button;
  });
  const overlay = el('div', { class: 'unlock', role: 'dialog', 'aria-modal': 'true', 'aria-label': 'How to use Dissent here' },
    el('section', { class: 'unlock-card' },
      el('div', { class: 'onboard-brand' }, el('span', { class: 'onboard-mark' }, icon('dissent-mark')), el('span', { text: 'Dissent' })),
      el('h1', { text: 'How do you want to use Dissent here?' }),
      el('p', { class: 'muted', text: 'This choice is saved next to Dissent.exe. You can run this exe on another computer and choose again there.' }),
      buttons,
      error));
  document.body.append(overlay);
  buttons[0].focus();
}

function showUnlock(status, done) {
  const creating = Boolean(status.new_profile);
  const copying = creating && status.choice === 'copy';
  const pass = el('input', { type: 'password', autocomplete: creating ? 'new-password' : 'current-password', 'aria-label': 'Passphrase' });
  const again = el('input', { type: 'password', autocomplete: 'new-password', 'aria-label': 'Repeat the passphrase' });
  const hint = el('p', { class: 'muted', role: 'status', text: strengthHint('').text });
  const error = el('p', { class: 'error', role: 'alert' });
  // Shown while a copy runs. Cleared on error.
  const busy = el('p', { class: 'muted', role: 'status' });
  const submit = el('button', { class: 'primary', type: 'submit', text: copying ? 'Copy profile' : creating ? 'Create profile' : 'Unlock' });

  pass.addEventListener('input', () => {
    hint.textContent = strengthHint(pass.value).text;
  });

  const form = el('form', {
    class: 'unlock-card',
    onsubmit: async (event) => {
      event.preventDefault();
      error.textContent = '';
      if (!pass.value) return;
      if (creating) {
        if (strengthHint(pass.value).level === 'short') {
          error.textContent = `Use at least ${MIN_LENGTH} characters.`;
          return;
        }
        if (pass.value !== again.value) {
          error.textContent = 'The two passphrases don’t match.';
          return;
        }
      }
      submit.disabled = true;
      if (copying) busy.textContent = 'Copying your profile. This can take a minute. Dissent starts when it is done.';
      try {
        const next = await api.unlockProfile(pass.value);
        overlay.remove();
        if (next === 'link') await showLink(done);
        else done();
      } catch (err) {
        busy.textContent = '';
        error.textContent = String(err);
        submit.disabled = false;
        pass.select();
      }
    },
  },
  el('div', { class: 'onboard-brand' }, el('span', { class: 'onboard-mark' }, icon('dissent-mark')), el('span', { text: 'Dissent' })),
  el('h1', { text: copying ? 'Copy your profile' : creating ? 'Create a passphrase' : 'Unlock Dissent' }),
  el('p', {
    class: 'muted',
    text: copying
      ? 'This copies the profile of the installed Dissent into the DissentData folder next to Dissent.exe, under a new passphrase. Close the installed Dissent first.'
      : creating
        ? 'This portable profile is locked with a passphrase. Its data is kept in the DissentData folder next to Dissent.exe.'
        : 'Enter the passphrase for this portable profile.',
  }),
  pass,
  creating ? hint : null,
  creating ? again : null,
  creating ? el('div', { class: 'callout warn' }, icon('circle-alert'), el('span', { text: 'Keep this safe. Without it this portable profile can’t be opened.' })) : null,
  copying ? el('div', { class: 'callout warn' }, icon('circle-alert'), el('span', { text: 'Both copies now share one device identity. Linking a new device is safer for a USB stick.' })) : null,
  error,
  busy,
  el('div', { class: 'row end' }, submit));

  const overlay = el('div', { class: 'unlock', role: 'dialog', 'aria-modal': 'true', 'aria-label': 'Unlock Dissent' }, form);
  document.body.append(overlay);
  pass.focus();
}

// The new device's link code, to approve from the installed app. Resolves once this device is linked and open.
async function showLink(done) {
  const code = await api.linkDeviceCode();
  const codeOut = el('input', { readOnly: true, value: code, 'aria-label': 'Link code for the installed Dissent' });
  const copy = el('button', {
    class: 'ghost',
    type: 'button',
    icon: 'copy',
    text: 'Copy code',
    onclick: async () => { if (await copyText(code)) toast('Link code copied', 'info'); },
  });
  const grantIn = el('input', { placeholder: 'Paste the grant here', 'aria-label': 'Grant from the installed Dissent' });
  const error = el('p', { class: 'error', role: 'alert' });
  const finish = el('button', { class: 'primary', type: 'submit', text: 'Finish linking' });
  const form = el('form', {
    class: 'unlock-card',
    onsubmit: async (event) => {
      event.preventDefault();
      const grant = grantIn.value.trim();
      if (!grant) return;
      error.textContent = '';
      finish.disabled = true;
      try {
        await api.finishPortableLink(grant);
        overlay.remove();
        done();
      } catch (err) {
        error.textContent = String(err);
        finish.disabled = false;
      }
    },
  },
  el('h1', { text: 'Link this device' }),
  el('p', { class: 'muted', text: 'On the computer with your installed Dissent, open Settings, then Devices. Under "Approve a new device", paste this code and press Approve. It gives you a grant.' }),
  el('div', { class: 'row' }, codeOut, copy),
  el('p', { class: 'muted', text: 'Paste the grant it gives you below. This device then gets its own key, and you can remove it from the Devices list at any time.' }),
  grantIn,
  error,
  el('div', { class: 'row end' }, finish));
  const overlay = el('div', { class: 'unlock', role: 'dialog', 'aria-modal': 'true', 'aria-label': 'Link this device' }, form);
  document.body.append(overlay);
  grantIn.focus();
}
