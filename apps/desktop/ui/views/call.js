// The call pane: a forwarding banner, a 3x2 grid of participants, the call controls, and the call chat below.
// It replaces the message list while a call is open in the chat's voice channel or DM. Needs the calls feature.
import * as actions from '../actions.js';
import { el, icon, avatar, setHue, clockLabel } from '../ui.js';

export function callPane(call) {
  const route = call.route ?? {};
  const people = call.participants ?? [];
  const banner = route.kind === 'forwarded'
    ? el('div', { class: 'call-route', role: 'status' },
      icon('shield'),
      el('span', {}, el('strong', { text: `Forwarded by ${route.forwarder ?? 'a member'}.` }), ' ',
        el('span', { text: `Their computer passes everyone’s sealed streams along and can’t open them. ${route.backup ?? 'Someone else'} is ready as backup.` })))
    : null;
  const tiles = people.map((p) => setHue(el('div', { class: `call-tile${p.speaking ? ' speaking' : ''}` },
    avatar(p.display_name, p.root_id, 'xl'),
    el('span', { class: 'label' }, p.muted ? icon('mic-off') : null, el('span', { text: p.display_name })),
    p.forwarder ? el('span', { class: 'fwd', text: 'Forwarding' }) : null), p.root_id));
  const controls = el('div', { class: 'call-controls' },
    el('button', {
      class: `icon-btn${call.muted ? ' on' : ''}`,
      'aria-label': call.muted ? 'Unmute microphone' : 'Mute microphone',
      'aria-pressed': call.muted ? 'true' : 'false',
      title: call.muted ? 'Unmute' : 'Mute',
      icon: call.muted ? 'mic-off' : 'mic',
      onclick: () => actions.toggleMuted(),
    }),
    el('button', {
      class: `icon-btn${call.deafened ? ' on' : ''}`,
      'aria-label': call.deafened ? 'Undeafen' : 'Deafen',
      'aria-pressed': call.deafened ? 'true' : 'false',
      title: call.deafened ? 'Undeafen' : 'Deafen',
      icon: 'headphones',
      onclick: () => actions.toggleDeafened(),
    }),
    el('button', { class: 'icon-btn leave', 'aria-label': 'Leave call', title: 'Leave call', icon: 'phone-off', onclick: () => actions.leaveCall() }));
  const stage = el('div', { class: 'call-stage' },
    banner ?? el('div', { class: 'call-route' }, icon('shield'), el('span', {}, el('strong', { text: route.title ?? 'In a call' }))),
    el('div', { class: 'call-grid' }, tiles),
    controls);
  return el('div', { class: 'call-pane' }, stage, callChat(call, people.length));
}

// The call chat: messages grouped under the author, a file card, and a composer that sends through the engine.
function callChat(call, count) {
  const input = el('input', { placeholder: 'Message the call', 'aria-label': 'Message the call' });
  const send = async () => {
    const text = input.value.trim();
    if (!text) return;
    if (await actions.sendCallMessage(text)) input.value = '';
  };
  input.addEventListener('keydown', (e) => { if (e.key === 'Enter' && !e.isComposing) send(); });
  return el('section', { class: 'call-chat', 'aria-label': 'Call chat' },
    el('div', { class: 'call-chat-head' },
      icon('message-circle'),
      el('h3', { text: 'Call chat' }),
      el('span', { class: 's', text: `· Only the ${count} people in this call can read it` })),
    el('div', { class: 'call-chat-log' }, (call.messages ?? []).map(chatMessage)),
    el('div', { class: 'composer' }, input, el('button', { class: 'icon-btn send', 'aria-label': 'Send to the call', icon: 'send-horizontal', onclick: send })));
}

function chatMessage(m) {
  const body = [];
  if (m.text) body.push(el('div', { class: 'text', text: m.text }));
  if (m.file) {
    body.push(el('div', { class: 'call-chat-file' },
      el('span', { class: 'tile xs', text: m.file.kind }),
      el('span', { class: 'n', text: m.file.name }),
      el('span', { class: 's', text: m.file.meta })));
  }
  return el('div', { class: 'call-msg start' },
    el('div', { class: 'gutter' }, avatar(m.author_name, m.root_id, 'sm')),
    el('div', { class: 'msg-body' },
      el('div', { class: 'head' }, el('span', { class: 'author', text: m.author_name }), el('time', { text: clockLabel(m.ms) })),
      ...body));
}

// The right column during a channel call: call details, not the profile (Call board).
export function callDetailsNodes(call) {
  const route = call.route ?? {};
  const section = (title, text) => el('section', { class: 'cd-section' }, el('h4', { text: title }), el('p', { text }));
  return [
    el('div', { class: 'right-head' }, el('h3', { text: 'Call details' })),
    el('div', { class: 'right-scroll call-details' },
      section('Forwarded by', route.forwarder ? `${route.forwarder} · chosen automatically for a strong, wired connection` : 'No forwarder'),
      section('Backup', route.backup ? `${route.backup} · connected and ready to take over` : 'No backup'),
      section('Encryption', 'Streams are sealed on each device. The forwarder passes them on and can’t open them.'),
      section('What the forwarder sees', 'Who is speaking and how loudly, so it can pick the active speakers. Never what is said.')),
  ];
}
