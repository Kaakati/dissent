// Join an active call (JoinCall board): the lobby shown in place of the messages when you open a conversation with a
// live call you are not in. The header shows the live pill and a Join button, the lobby shows the preview, the
// microphone and speaker selectors and the route note, and the right column shows who is in the call.
// The camera is left out until set_camera works (Part 6.15), so the preview says the camera is off.
import * as actions from '../actions.js';
import { api } from '../api.js';
import { state } from '../state.js';
import { el, icon, avatar, clockLabel } from '../ui.js';

const METER_BARS = 16;

// Mic state and the running meter. The header Join button and the lobby share the mic state.
let micOn = true;
let stopMeter = null;

// Header pill and Join button for the open conversation. `title` is the conversation's own title nodes.
export function lobbyHeader({ view, live, title }) {
  const pill = el('span', { class: 'call-pill', role: 'status' },
    el('span', { class: 'dot', 'aria-hidden': 'true' }),
    el('span', { text: `Call in progress · ${clockLabel(live.started_ms ?? Date.now())}` }));
  const join = el('button', { class: 'primary head-join', text: 'Join', onclick: () => joinLobby(view) });
  return { title: [...title, pill], tools: [join] };
}

// The right column while the lobby is open: who is in the call and who is not.
export function lobbyRosterNodes(live) {
  const people = live.participants ?? [];
  const notIn = live.not_in ?? [];
  const row = (p, detail) => el('li', { class: 'roster-row' },
    avatar(p.display_name, p.root_id, 'sm'),
    el('span', { class: 'who' },
      el('span', { class: 'name', text: p.display_name }),
      detail ? el('span', { class: 'state', text: detail }) : null));
  const stateOf = (p) => {
    if (p.forwarder) return p.speaking ? 'Speaking · forwarding the call' : 'Forwarding the call';
    if (p.speaking) return 'Speaking';
    if (p.muted) return 'Mic off';
    return 'Listening';
  };
  return [
    el('div', { class: 'roster-head' }, el('h3', { text: `In the call — ${people.length}` }), el('span', { class: 'tnum', text: clockLabel(live.started_ms ?? Date.now()) })),
    el('ul', { class: 'roster-list', 'aria-label': 'In the call' }, people.map((p) => row(p, stateOf(p)))),
    notIn.length ? el('h4', { class: 'roster-sub', text: `Not in the call — ${notIn.length}` }) : null,
    notIn.length ? el('ul', { class: 'roster-list', 'aria-label': 'Not in the call' }, notIn.map((p) => row(p, null))) : null,
  ];
}

// `view`: the open conversation (group or direct). `live`: its live_call. `members`: how many people the group has.
export function lobbyNodes({ view, live, members, onNotNow }) {
  const me = state.me;
  const people = live.participants ?? [];
  const forwarder = live.route?.kind === 'forwarded' ? live.route.forwarder : null;
  const minutes = Math.max(1, Math.round((Date.now() - (live.started_ms ?? Date.now())) / 60000));
  const started = live.started_by ?? (view.group ? 'Someone' : view.title);
  const subtitle = view.group
    ? `${started} started a voice call ${minutes} minutes ago. ${people.length} of the group’s ${members} members are in.`
    : `${started} started a voice call ${minutes} minutes ago.`;
  const routeNote = forwarder
    ? `End-to-end encrypted. You’ll connect through ${forwarder}, who forwards everyone’s sealed audio and can’t listen in.`
    : 'End-to-end encrypted. You’ll connect directly to everyone in the call.';

  const bars = Array.from({ length: METER_BARS }, () => el('span', { class: 'bar' }));
  const hint = el('span', { class: 'hint', text: `${micOn ? 'Mic on' : 'Mic off'} · camera off` });
  const micBtn = el('button', { class: 'icon-btn mic', 'aria-label': micOn ? 'Mute microphone' : 'Unmute microphone', 'aria-pressed': micOn ? 'false' : 'true', title: micOn ? 'Mute' : 'Unmute', icon: micOn ? 'mic' : 'mic-off' });
  micBtn.addEventListener('click', () => {
    micOn = !micOn;
    micBtn.setAttribute('aria-label', micOn ? 'Mute microphone' : 'Unmute microphone');
    micBtn.setAttribute('aria-pressed', micOn ? 'false' : 'true');
    micBtn.title = micOn ? 'Mute' : 'Unmute';
    micBtn.replaceChildren(icon(micOn ? 'mic' : 'mic-off'));
    hint.textContent = `${micOn ? 'Mic on' : 'Mic off'} · camera off`;
  });

  stopMeter?.();
  stopMeter = startMeter(bars, () => micOn);

  const join = el('button', { class: 'primary join-btn', text: 'Join call', onclick: () => joinLobby(view) });
  const devices = el('div', { class: 'lobby-devices' });
  loadDevices(devices);

  const preview = el('div', { class: 'lobby-preview' },
    el('span', { class: 'preview-tag', text: 'Only you can see this preview' }),
    el('div', { class: 'preview-face' },
      avatar(me?.display_name ?? '?', me?.root_id, 'xl'),
      el('span', { class: 'preview-off', text: 'Your camera is off' })),
    el('div', { class: 'preview-foot' }, el('div', { class: 'meter', role: 'meter', 'aria-label': 'Microphone level' }, bars), micBtn));

  return el('section', { class: 'lobby', 'aria-label': 'Get ready to join' },
    el('h2', { class: 'lobby-title', text: 'Join the call?' }),
    el('p', { class: 'lobby-sub', text: subtitle }),
    el('div', { class: 'lobby-grid' }, preview, devices),
    el('div', { class: 'route-note' }, icon('shield'), el('span', { text: routeNote })),
    el('div', { class: 'lobby-foot' },
      el('button', { class: 'ghost', text: 'Not now', onclick: () => { stopMeter?.(); stopMeter = null; onNotNow(); } }),
      el('div', { class: 'join-end' }, hint, join)));
}

async function joinLobby(view) {
  stopMeter?.();
  stopMeter = null;
  return view.group
    ? actions.joinGroupCall(view.peer, { muted: !micOn })
    : actions.joinDmCall(view.peer, { muted: !micOn });
}

// Microphone and speaker selectors, from audio_devices. Hidden when the backend does not report devices.
async function loadDevices(host) {
  let devices;
  try {
    devices = await api.audioDevices();
  } catch {
    return;
  }
  if (!host.isConnected) return;
  const select = (label, list, current, onchange) => {
    const id = `dev-${label.toLowerCase()}`;
    const options = [el('option', { value: '', text: 'System default' }),
      ...(list ?? []).map((d) => el('option', { value: d.name, text: d.name }))];
    options.forEach((o) => { o.selected = (current ?? '') === o.value; });
    return el('div', { class: 'device' },
      el('label', { class: 'dev-label', for: id, text: label }),
      el('select', { id, class: 'dev-select', onchange: (e) => onchange(e.target.value) }, options));
  };
  const save = async () => {
    const input = host.querySelector('#dev-microphone')?.value || null;
    const output = host.querySelector('#dev-speaker')?.value || null;
    try { await api.setAudioDevices(input, output); } catch { /* the lobby keeps the last device */ }
  };
  host.replaceChildren(
    select('Microphone', devices.inputs, devices.input, save),
    select('Speaker', devices.outputs, devices.output, save));
}

// Drives the bars from the microphone level. Without microphone access the meter stays empty. Stops when the lobby leaves.
function startMeter(bars, isOn) {
  let stream = null;
  let ctx = null;
  let frame = 0;
  let alive = true;
  const stop = () => {
    alive = false;
    cancelAnimationFrame(frame);
    stream?.getTracks().forEach((t) => t.stop());
    ctx?.close().catch(() => {});
  };
  (async () => {
    try {
      stream = await navigator.mediaDevices.getUserMedia({ audio: true });
      if (!alive) { stop(); return; }
      ctx = new AudioContext();
      const analyser = ctx.createAnalyser();
      analyser.fftSize = 256;
      ctx.createMediaStreamSource(stream).connect(analyser);
      const data = new Uint8Array(analyser.fftSize);
      const tick = () => {
        if (!alive) return;
        if (!bars[0].isConnected) { stop(); return; }
        analyser.getByteTimeDomainData(data);
        let peak = 0;
        for (const v of data) peak = Math.max(peak, Math.abs(v - 128));
        const level = isOn() ? Math.min(1, peak / 64) : 0;
        bars.forEach((b, i) => b.classList.toggle('on', level * bars.length > i));
        frame = requestAnimationFrame(tick);
      };
      tick();
    } catch {
      stop(); // no microphone access: the meter stays empty
    }
  })();
  return stop;
}
