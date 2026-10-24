// Notification, voice and ringtone sounds. Each clip is one HTMLAudioElement, created and preloaded once and rewound
// for every play. Playing is best effort: autoplay policies, a missing file or no audio device must never throw.
import { state } from './state.js';

const CLIPS = {
  notification: { src: 'sounds/notification.mp3', level: 0.8 },
  join: { src: 'sounds/voice-join.mp3', level: 0.6 },
  leave: { src: 'sounds/voice-leave.mp3', level: 0.6 },
  ringtone: { src: 'sounds/ringtone.mp3', level: 0.7, loop: true },
};
const KEY = 'dissent.sounds.v1';
// content: what a desktop notification shows, 'both' (sender and message), 'sender', or 'none' (just "New message").
const DEFAULTS = { enabled: true, messages: true, calls: true, ringtone: true, volume: 80, content: 'both' };
const NOTIFY_GAP_MS = 2000; // at most one message notification per 2 seconds
const RING_LIMIT_MS = 46_000; // the engine ends an unanswered direct call after 45 s, so this only backs it up

const clips = Object.fromEntries(Object.entries(CLIPS).map(([name, clip]) => {
  const audio = new Audio(clip.src);
  audio.preload = 'auto';
  audio.loop = Boolean(clip.loop);
  return [name, audio];
}));

// ---- settings (localStorage, guarded: defaults when storage is unavailable) ----

function readSettings() {
  try {
    const saved = JSON.parse(window.localStorage.getItem(KEY) ?? '{}');
    const merged = { ...DEFAULTS, ...(saved && typeof saved === 'object' ? saved : {}) };
    const content = ['both', 'sender', 'none'].includes(merged.content) ? merged.content : DEFAULTS.content;
    return { ...merged, content, volume: Math.min(100, Math.max(0, Number(merged.volume) || 0)) };
  } catch {
    return { ...DEFAULTS };
  }
}

let settings = readSettings();

export function getSoundSettings() {
  return { ...settings };
}

export function setSoundSettings(next) {
  settings = { ...settings, ...next };
  try {
    window.localStorage.setItem(KEY, JSON.stringify(settings));
  } catch {
    // Unavailable: the choice lasts for this session only.
  }
  if (!ringtoneAllowed()) stopRingtone();
  return getSoundSettings();
}

// ---- playing ----

// Do Not Disturb silences message and voice sounds. Ringing still sounds, as calls still ring.
const isDnd = () => state.status?.status === 'dnd';
const ringtoneAllowed = () => settings.enabled && settings.ringtone;
const callSoundAllowed = () => settings.enabled && settings.calls && !isDnd();

function play(name) {
  try {
    const clip = clips[name];
    clip.volume = Math.min(1, Math.max(0, CLIPS[name].level * settings.volume / 100));
    clip.currentTime = 0;
    clip.play()?.catch(() => {}); // blocked by autoplay policy, or no output device: stay silent
  } catch {
    // Audio is an extra. Nothing here may stop the UI.
  }
}

let lastNotification = 0;

// A message the user should hear about: a DM or group message, or a channel message that @mentions them.
export function playNotification() {
  if (!settings.enabled || !settings.messages || isDnd()) return;
  const now = Date.now();
  if (now - lastNotification < NOTIFY_GAP_MS) return;
  lastNotification = now;
  play('notification');
}

// ---- ringtone: incoming calls ring until the dialog closes; outgoing calls ring until answered or ended ----

export function startRingtone() {
  if (!ringtoneAllowed() || !clips.ringtone.paused) return;
  play('ringtone');
}

export function stopRingtone() {
  try {
    clips.ringtone.pause();
    clips.ringtone.currentTime = 0;
  } catch {
    // Nothing playing to stop.
  }
}

let outgoing = false;
let outgoingTimer = null;

// A direct call we placed is ringing: the callee has not answered yet.
export function beginOutgoingRing() {
  outgoing = true;
  clearTimeout(outgoingTimer);
  outgoingTimer = setTimeout(endOutgoingRing, RING_LIMIT_MS);
  startRingtone();
}

export function endOutgoingRing() {
  if (!outgoing) return;
  outgoing = false;
  clearTimeout(outgoingTimer);
  outgoingTimer = null;
  stopRingtone();
}

// ---- voice: joins and leaves in the call or voice channel this device is in ----

export function playCallJoin() {
  if (callSoundAllowed()) play('join');
}

export function playCallLeave() {
  if (callSoundAllowed()) play('leave');
}

// Called with every change to the call on this device. Joins and leaves come from the participant list.
// Ringing a call we placed makes no sound of its own, and ending it unanswered is silent too.
export function callChanged(prev, next) {
  const ringing = outgoing;
  if (ringing && (!next || next.participants?.length)) endOutgoingRing(); // answered, declined or cancelled
  if (!next) {
    if (prev && !ringing) playCallLeave();
    return;
  }
  if (!prev || prev.id !== next.id) {
    if (prev && !ringing) playCallLeave();
    if (!outgoing) playCallJoin();
    return;
  }
  const before = new Set((prev.participants ?? []).map((p) => p.root_id));
  const after = new Set((next.participants ?? []).map((p) => p.root_id));
  if ([...after].some((id) => !before.has(id))) playCallJoin();
  if ([...before].some((id) => !after.has(id))) playCallLeave();
}
