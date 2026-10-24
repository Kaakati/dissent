// Incoming call (IncomingCall board): a modal alertdialog over the current view for a ringing direct or group call.
// It stays until the user answers, declines, or sends a quick reply. The backend announces a ringing call with
// `dissent://call-ringing`. Video answers are left out until set_camera works (Part 6.15).
import { api } from '../api.js';
import * as actions from '../actions.js';
import { el, icon, avatar, toast } from '../ui.js';
import { startRingtone, stopRingtone } from '../sounds.js';

const QUICK_REPLIES = ['Joining in 5 minutes', 'Can’t make it, catch me up', 'Text me instead'];

// `p`: { call_id, scope: 'dm' | 'group', peer?, group_id?, name, caller, verified, in_call, total, members_in }.
export function showIncomingCall(p) {
  if (document.querySelector('dialog.incoming')) return; // one ringing call at a time
  const group = p.scope === 'group';
  const title = group ? p.name : (p.caller?.display_name ?? p.name);
  const titleId = 'incoming-title';
  let settled = false; // the first answer, decline or reply wins

  const dialog = el('dialog', { class: 'dialog incoming', role: 'alertdialog', 'aria-labelledby': titleId, 'aria-describedby': 'incoming-sub' });
  dialog.dataset.callId = p.call_id;

  const decline = async () => {
    if (settled) return;
    settled = true;
    try { await actions.declineCall(p.call_id); } finally { dialog.close(); }
  };
  const answer = async () => {
    if (settled) return;
    settled = true;
    try { await actions.acceptCall(p.call_id, { muted: mute.checked }); } finally { dialog.close(); }
  };
  const quick = async (text) => {
    if (settled) return;
    settled = true;
    try {
      if (group) await api.sendGroupMessage(p.group_id, text);
      else await api.sendDm(p.peer, text);
      await actions.declineCall(p.call_id);
    } catch (err) {
      toast(err);
    } finally {
      dialog.close();
    }
  };

  const rings = el('div', { class: 'ring-stage', 'aria-hidden': 'true' },
    el('span', { class: 'ring r1' }), el('span', { class: 'ring r2' }), el('span', { class: 'ring r3' }),
    group
      ? el('div', { class: 'stack-avatars' },
        avatar(p.members_in?.[0]?.display_name ?? '?', p.members_in?.[0]?.root_id, 'xl'),
        avatar(p.members_in?.[1]?.display_name ?? '?', p.members_in?.[1]?.root_id, 'xl'))
      : avatar(p.caller?.display_name ?? '?', p.caller?.root_id, 'xl'));

  const chips = el('div', { class: 'chips' },
    el('span', { class: 'chip ok' }, icon('lock'), el('span', { text: 'End-to-end encrypted' })),
    p.verified ? el('span', { class: 'chip ok' }, icon('shield-check'), el('span', { text: 'Safety number verified' })) : null,
    el('span', { class: 'chip', text: group ? `${p.in_call} of ${p.total} in call` : 'Direct call' }));

  const names = (p.members_in ?? []).map((m) => m.display_name);
  const who = names.length > 1
    ? `${names.slice(0, -1).join(', ')} and ${names.at(-1)}`
    : names[0] ?? '';
  const inRow = group && names.length
    ? el('div', { class: 'in-row' },
      el('span', { class: 'in-stack' }, (p.members_in ?? []).slice(0, 3).map((m) => avatar(m.display_name, m.root_id, 'sm'))),
      el('span', { text: `${who} ${names.length > 1 ? 'are' : 'is'} in the call` }))
    : null;

  const mute = el('input', { type: 'checkbox', id: 'incoming-mute' });
  const acceptBtn = el('button', { class: 'round accept', 'aria-label': group ? 'Join call' : 'Answer', onclick: answer, icon: 'phone' });
  const declineBtn = el('button', { class: 'round decline', 'aria-label': group ? 'Ignore' : 'Decline', onclick: decline, icon: 'phone-off' });

  dialog.append(
    el('div', { class: 'incoming-body' },
      rings,
      el('p', { class: 'kicker', text: group ? 'Group call' : 'Incoming call' }),
      el('h2', { id: titleId, text: title }),
      el('p', { id: 'incoming-sub', class: 'sub', text: group ? `${p.caller?.display_name ?? 'Someone'} started a voice call in your group. Only the group’s ${p.total} members can join.` : `${p.caller?.display_name ?? 'Someone'} is calling you.` }),
      chips,
      inRow,
      el('div', { class: 'answer-row' },
        el('div', { class: 'answer' }, declineBtn, el('span', { text: group ? 'Ignore' : 'Decline' })),
        el('div', { class: 'answer' }, acceptBtn, el('span', { text: group ? 'Join call' : 'Answer' }))),
      el('div', { class: 'quick' },
        el('p', { class: 'quick-title', text: 'Can’t talk? Send a quick reply' }),
        el('div', { class: 'quick-row' }, QUICK_REPLIES.map((text) => el('button', { class: 'pill', text, onclick: () => quick(text) })))),
      el('label', { class: 'mute-check', for: 'incoming-mute' }, mute, el('span', { text: 'Start with my microphone muted' }))));

  // Esc declines and Enter answers. Enter on a button already activates that button.
  dialog.addEventListener('cancel', (e) => { e.preventDefault(); decline(); });
  dialog.addEventListener('keydown', (e) => {
    if (e.key === 'Enter' && e.target === dialog) { e.preventDefault(); answer(); }
  });
  // Answer, decline, reply, Esc, or the call ending elsewhere: the dialog closes and the ringing stops with it.
  dialog.addEventListener('close', () => {
    stopRingtone();
    dialog.remove();
  });

  document.body.append(dialog);
  dialog.showModal();
  startRingtone();
  acceptBtn.focus();
  return dialog;
}

// Closes the ringing dialog for a call that rang out or was answered on another device.
export function dismissIncomingCall(callId) {
  const dialog = document.querySelector('dialog.incoming');
  if (dialog?.dataset.callId === callId) dialog.close();
}
