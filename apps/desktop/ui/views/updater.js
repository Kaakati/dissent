// In-app updates: a green download button at the top right when a newer release exists, and the dialog that installs it.
import { state, subscribe, notify } from '../state.js';
import { api, listenTo } from '../api.js';
import { el, openDialog, toast } from '../ui.js';

const PROGRESS = 'dissent://update-progress';
let button = null;

export function initUpdater() {
  button = el('button', {
    class: 'update-btn',
    hidden: true,
    icon: 'download',
    'aria-label': 'Update available',
    onclick: () => openUpdateDialog(state.update),
  });
  (document.getElementById('update-host') ?? document.body).append(button);
  subscribe('update', render);
  render();
}

function render() {
  const show = Boolean(state.update) && !state.updateDismissed;
  button.hidden = !show;
  if (show) {
    const label = `Update available: v${state.update.version}`;
    button.title = label;
    button.setAttribute('aria-label', label);
  }
}

// Release notes are plain text. Lines starting with "- " or "* " become a bullet list. Nothing is parsed as HTML.
function notesNodes(text) {
  if (!text || !text.trim()) return el('p', { class: 'muted', text: 'No release notes.' });
  const out = [];
  let list = null;
  for (const raw of text.split(/\r?\n/)) {
    const line = raw.trim();
    const bullet = /^[-*•]\s+/.exec(line);
    if (bullet) {
      if (!list) {
        list = el('ul', { class: 'notes-list' });
        out.push(list);
      }
      list.append(el('li', { text: line.slice(bullet[0].length) }));
      continue;
    }
    list = null;
    if (line) out.push(el('p', { class: 'notes-line', text: line }));
  }
  return el('div', {}, out);
}

const mb = (bytes) => `${(bytes / 1048576).toFixed(1)} MB`;

export function openUpdateDialog(info) {
  if (!info) return;
  let close = () => {};
  const fill = el('div', { class: 'progress-fill' });
  const bar = el('div', { class: 'progress', hidden: true }, fill);
  const status = el('p', { class: 'muted', role: 'status' });
  const later = el('button', {
    class: 'ghost',
    text: 'Later',
    onclick: () => {
      state.updateDismissed = true;
      notify('update');
      close();
    },
  });
  const install = el('button', {
    class: 'primary',
    icon: 'download',
    text: 'Update and restart',
    onclick: async () => {
      install.disabled = true;
      later.disabled = true;
      bar.hidden = false;
      status.textContent = 'Downloading…';
      let unlisten = null;
      try {
        unlisten = await listenTo(PROGRESS, (p) => {
          fill.style.setProperty('--p', String(p.total ? Math.min(1, p.downloaded / p.total) : 0));
          status.textContent = p.total ? `${mb(p.downloaded)} of ${mb(p.total)}` : `${mb(p.downloaded)} downloaded`;
        });
        await api.installUpdate();
        status.textContent = 'Restarting…';
      } catch (err) {
        toast(err);
        install.disabled = false;
        later.disabled = false;
        bar.hidden = true;
        status.textContent = '';
      } finally {
        unlisten?.();
      }
    },
  });
  ({ close } = openDialog({
    title: 'Update available',
    content: el('div', {},
      el('p', { text: `Version ${info.version} is available. You have ${info.current_version}.` }),
      info.date ? el('p', { class: 'muted', text: `Released ${info.date}` }) : null,
      notesNodes(info.notes),
      bar,
      status,
      el('div', { class: 'row end' }, later, install)),
  }));
}
