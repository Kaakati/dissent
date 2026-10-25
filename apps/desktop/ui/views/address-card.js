// An address with a copy button and, optionally, its QR code. Shared by My identity, the full profile and server settings.
// The short form shows on screen; the title and the copy button carry the full address.
import { el, copyText, toast } from '../ui.js';
import { addressOf, shortAddress } from '../address.js';
import { qrSvg } from '../qr.js';

const NAME = { user: 'user', server: 'server', device: 'device' };

export function addressCard(kind, id, { label = null, qr = false } = {}) {
  const full = addressOf(kind, id);
  const what = NAME[kind];
  return el('div', { class: 'address-card' },
    el('div', { class: 'id-row' },
      el('span', { class: 'id-label', text: label ?? `${what[0].toUpperCase()}${what.slice(1)} address` }),
      el('code', { class: 'mono id-value', text: shortAddress(full) || '—', title: full }),
      full ? el('button', {
        class: 'icon-btn',
        'aria-label': `Copy ${what} address`,
        title: `Copy ${what} address`,
        icon: 'copy',
        onclick: async () => {
          if (await copyText(full)) toast(`${what[0].toUpperCase()}${what.slice(1)} address copied`, 'info');
        },
      }) : null),
    qr && full ? el('div', { class: 'qr-code', role: 'img', 'aria-label': `QR code of the ${what} address`, title: full }, qrSvg(full)) : null,
    qr && full ? el('p', { class: 'muted', text: 'Scan to verify this address' }) : null);
}
