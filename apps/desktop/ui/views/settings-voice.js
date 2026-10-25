// Settings › Voice: the devices, the volumes, the mic test with its level meter, the input profile, sensitivity,
// the Custom processing choices, push to talk and automatic gain control. A change applies on the next audio tick.
// While this page is open the mic runs, so its meter moves; leaving the page stops it.
import { api, listenTo } from '../api.js';
import { el, toast } from '../ui.js';
import { chooseDevice, PROFILES, settled, shownDevice, systemLabel, updateVoice, voiceSnapshot, loadVoice } from './voice-prefs.js';

const BARS = 40;
// Sensitivity and meter scale, in dBFS. Anything quieter reads as the bottom of the track.
const SCALE_MIN = -80;
const SCALE_MAX = -10;
const SILENT_DB = -100;
// No level for this long means the mic is closed (muted, or the page just opened), so the meter falls to silence.
const SILENT_AFTER_MS = 400;
const MODIFIERS = new Set(['ControlLeft', 'ControlRight', 'ShiftLeft', 'ShiftRight', 'AltLeft', 'AltRight', 'MetaLeft', 'MetaRight', 'OSLeft', 'OSRight', 'CapsLock']);

let teardown = null;
let session = 0;

// Stops what the Voice page started: the mic test, the level listener and the meter timer. Safe to call at any time.
export function stopVoicePage() {
  session += 1;
  teardown?.();
  teardown = null;
}

// The position of a level on the track, 0 to 1.
const position = (db) => Math.min(1, Math.max(0, (db - SCALE_MIN) / (SCALE_MAX - SCALE_MIN)));

// An accelerator, such as Control+KeyV, as a short label: Ctrl + V.
function keyLabel(accel) {
  if (!accel) return 'Set a key';
  return accel
    .split('+')
    .map((part) => part.replace('Control', 'Ctrl').replace('Super', 'Win').replace(/^Key/, '').replace(/^Digit/, '').replace(/^Numpad/, 'Num '))
    .join(' + ');
}

function volumeRow(label, key, value) {
  const readout = el('output', { class: 'mono', text: `${value}%` });
  return el('label', { class: 'volume-row' },
    el('span', { text: label }),
    el('input', {
      type: 'range',
      min: '0',
      max: '200',
      step: '1',
      value: String(value),
      'aria-label': label,
      oninput: (e) => {
        readout.textContent = `${e.target.value}%`;
        updateVoice({ [key]: Number(e.target.value) });
      },
    }),
    readout);
}

function deviceSelect(label, list, chosen, direction, afterSave) {
  const fallback = list.find((d) => d.is_default)?.name;
  const select = el('select', {
    class: 'voice-select',
    'aria-label': label,
    onchange: (e) => saveDevice(direction, e.target.value || null).then(afterSave),
  },
  el('option', { value: '', text: fallback ? `${systemLabel()} (${fallback})` : systemLabel(), selected: chosen == null }));
  for (const d of list) {
    select.append(el('option', { value: d.name, text: d.name, selected: chosen === d.name }));
  }
  return select;
}

async function saveDevice(direction, name) {
  try {
    await chooseDevice(direction, name);
  } catch (e) {
    toast(e);
    await loadVoice().catch(() => {});
  }
}

function toggleRow(label, checked, onchange) {
  const input = el('input', { type: 'checkbox', checked, onchange: (e) => onchange(e.target.checked) });
  return { input, row: el('label', { class: 'toggle-row' }, input, el('span', { text: label })) };
}

// Builds the page. Returns its nodes, and starts the mic monitor and the level listener, torn down by stopVoicePage.
export async function voiceNodes() {
  stopVoicePage();
  const mine = session;
  const [voice, devices] = await Promise.all([loadVoice(), api.audioDevices()]);
  if (mine !== session) return [];

  // ---- the level meter, the sensitivity track and the mic test ----
  const bars = Array.from({ length: BARS }, () => el('span', { class: 'level-bar' }));
  const meter = el('div', { class: 'level-meter', 'aria-hidden': 'true' }, ...bars);
  const fill = el('div', { class: 'sens-fill' });
  let lastLevel = 0;
  let shownDb = SILENT_DB;

  const paint = (db) => {
    shownDb = db;
    const lit = Math.round(position(db) * BARS);
    bars.forEach((bar, i) => bar.classList.toggle('on', i < lit));
    fill.style.width = `${position(db) * 100}%`;
  };

  const threshold = el('input', {
    type: 'range',
    class: 'sens-range',
    min: String(SCALE_MIN),
    max: String(SCALE_MAX),
    step: '1',
    value: String(Math.round(voice.threshold_db)),
    disabled: voice.auto_sensitivity,
    'aria-label': 'Input sensitivity threshold',
  });
  const thresholdReadout = el('output', { class: 'mono', text: `${Math.round(voice.threshold_db)} dB` });
  threshold.addEventListener('input', (e) => {
    thresholdReadout.textContent = `${e.target.value} dB`;
    updateVoice({ threshold_db: Number(e.target.value) });
  });

  const onLevel = (level) => {
    lastLevel = Date.now();
    paint(level.db);
    // Under the automatic threshold the handle follows the level the gate is using.
    if (voiceSnapshot()?.auto_sensitivity) {
      threshold.value = String(Math.round(level.threshold_db));
      thresholdReadout.textContent = `${Math.round(level.threshold_db)} dB`;
    }
  };
  const unlisten = listenTo('dissent://mic-level', onLevel).catch(() => () => {});
  const quiet = setInterval(() => {
    if (Date.now() - lastLevel > SILENT_AFTER_MS && shownDb > SILENT_DB) paint(SILENT_DB);
  }, 200);
  // Monitoring starts with the page. A call already runs the mic, and then the test is refused, which is fine.
  api.micTestStart(false).catch(() => {});

  let testing = false;
  // A new microphone or speaker: the monitor and any running test restart on it.
  const restartMonitor = async () => {
    await api.micTestStop().catch(() => {});
    await api.micTestStart(testing).catch(() => {});
  };
  const testButton = el('button', { type: 'button', class: 'btn-sm', 'aria-pressed': 'false', text: 'Mic Test' });
  testButton.addEventListener('click', async () => {
    const next = !testing;
    try {
      await api.micTestStart(next);
      testing = next;
    } catch (e) {
      toast(e);
      return;
    }
    testButton.textContent = testing ? 'Stop Mic Test' : 'Mic Test';
    testButton.setAttribute('aria-pressed', String(testing));
  });

  teardown = () => {
    clearInterval(quiet);
    unlisten.then((stop) => stop?.());
    api.micTestStop().catch(() => {});
  };

  // ---- sensitivity ----
  const autoToggle = toggleRow('Automatically Adjust Input Sensitivity', voice.auto_sensitivity, (on) => {
    threshold.disabled = on;
    updateVoice({ auto_sensitivity: on }, { now: true });
  });

  // ---- push to talk ----
  const binder = el('button', {
    type: 'button',
    class: 'ptt-key',
    'aria-label': 'Push to talk key',
    text: keyLabel(voice.ptt_key),
    hidden: !voice.push_to_talk,
  });
  binder.addEventListener('click', () => startBinding());
  const pttToggle = toggleRow('Push to Talk', voice.push_to_talk, (on) => {
    binder.hidden = !on;
    updateVoice({ push_to_talk: on }, { now: true });
  });

  function startBinding() {
    binder.setAttribute('data-binding', '');
    binder.classList.add('listening');
    binder.textContent = 'Press a key (Esc cancels)';
    binder.addEventListener('keydown', onBindKey);
    binder.focus();
  }

  function finishBinding(accel) {
    binder.removeEventListener('keydown', onBindKey);
    binder.removeAttribute('data-binding');
    binder.classList.remove('listening');
    if (!accel) {
      binder.textContent = keyLabel(voiceSnapshot()?.ptt_key);
      return;
    }
    binder.textContent = keyLabel(accel);
    updateVoice({ ptt_key: accel }, { now: true });
    settled().then(() => {
      binder.textContent = keyLabel(voiceSnapshot()?.ptt_key);
    });
  }

  function onBindKey(e) {
    e.preventDefault();
    e.stopPropagation();
    if (e.key === 'Escape') {
      finishBinding(null);
      return;
    }
    // A modifier on its own is not a key. Wait for the key it is held with.
    if (MODIFIERS.has(e.code) || !e.code) return;
    const modifiers = [e.ctrlKey && 'Control', e.altKey && 'Alt', e.shiftKey && 'Shift', e.metaKey && 'Super'];
    finishBinding([...modifiers.filter(Boolean), e.code].join('+'));
  }

  // ---- Custom processing ----
  const noise = el('select', {
    class: 'voice-select',
    'aria-label': 'Noise suppression',
    onchange: (e) => updateVoice({ noise_suppression: e.target.value }, { now: true }),
  },
  el('option', { value: 'standard', text: 'Standard', selected: voice.noise_suppression === 'standard' }),
  el('option', { value: 'off', text: 'None', selected: voice.noise_suppression === 'off' }));
  const echo = toggleRow('Echo Cancellation', voice.echo_cancellation, (on) => updateVoice({ echo_cancellation: on }, { now: true }));
  const agc = toggleRow('Automatic Gain Control', voice.agc, (on) => updateVoice({ agc: on }, { now: true }));
  const customBlock = el('div', { class: 'settings-card', hidden: voice.profile !== 'custom' },
    el('section', { class: 'settings-block' },
      el('h2', { text: 'Processing' }),
      el('label', { class: 'volume-row' }, el('span', { text: 'Noise Suppression' }), noise),
      echo.row),
    el('section', { class: 'settings-block' },
      el('h2', { text: 'Advanced' }),
      agc.row));
  const presetNote = el('p', {
    class: 'muted',
    hidden: voice.profile === 'custom',
    text: voice.profile === 'studio'
      ? 'Studio sends the microphone unprocessed: no noise suppression, echo cancellation or gain control.'
      : 'Voice Isolation uses strong noise suppression, with echo cancellation and gain control.',
  });

  // ---- the input profile ----
  const profileRadios = PROFILES.map((p) => el('label', { class: 'radio-row radio-stack' },
    el('input', {
      type: 'radio',
      name: 'input-profile',
      value: p.value,
      checked: voice.profile === p.value,
      onchange: () => {
        customBlock.hidden = p.value !== 'custom';
        presetNote.hidden = p.value === 'custom';
        updateVoice({ profile: p.value }, { now: true });
      },
    }),
    el('span', { class: 'radio-text' }, el('span', { text: p.label }), el('span', { class: 'muted', text: p.sub }))));

  return [
    el('div', { class: 'settings-card' },
      el('section', { class: 'settings-block' },
        el('h2', { text: 'Devices' }),
        el('div', { class: 'voice-devices' },
          el('label', { class: 'voice-field' }, el('span', { text: 'Microphone' }),
            deviceSelect('Microphone', devices.inputs, devices.input, 'input', restartMonitor)),
          el('label', { class: 'voice-field' }, el('span', { text: 'Speaker' }),
            deviceSelect('Speaker', devices.outputs, devices.output, 'output', restartMonitor)))),
      el('section', { class: 'settings-block' },
        el('h2', { text: 'Volume' }),
        volumeRow('Microphone Volume', 'input_volume', voice.input_volume),
        volumeRow('Speaker Volume', 'output_volume', voice.output_volume)),
      el('section', { class: 'settings-block' },
        el('h2', { text: 'Mic Test' }),
        el('p', { class: 'muted', text: 'Plays your microphone back through your speaker. Use headphones, or the sound feeds back.' }),
        el('div', { class: 'mic-test-row' }, testButton, meter)),
      el('fieldset', { class: 'settings-block' },
        el('legend', { text: 'Input Profile' }),
        ...profileRadios,
        presetNote),
      customBlock,
      el('section', { class: 'settings-block' },
        el('h2', { text: 'Input Sensitivity' }),
        autoToggle.row,
        el('div', { class: 'sens' },
          el('div', { class: 'sens-track' }, fill),
          threshold),
        el('div', { class: 'sens-meta' },
          el('span', { class: 'muted', text: voice.auto_sensitivity ? 'Following the room noise. The handle shows the threshold in use.' : 'Drag the handle to set the level. Green shows your mic.' }),
          thresholdReadout)),
      el('section', { class: 'settings-block' },
        el('h2', { text: 'Push to Talk' }),
        pttToggle.row,
        binder,
        el('p', { class: 'muted', text: 'Hold the key to talk. It works while Dissent is in the background.' }))),
  ];
}
