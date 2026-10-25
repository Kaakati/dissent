// Notification, voice and ringtone sounds. Each clip is one HTMLAudioElement, created and preloaded once and rewound for
// every play. Playing is best effort: autoplay policies, a missing file or no audio device must never throw. Ported
// from ui/sounds.js; the settings are in sound-settings.ts.
import { useSoundSettings } from './sound-settings';
import { useShell } from './store';
import type { CallState } from './types';

const CLIPS = {
  notification: { src: 'sounds/notification.mp3', level: 0.8, loop: false },
  join: { src: 'sounds/voice-join.mp3', level: 0.6, loop: false },
  leave: { src: 'sounds/voice-leave.mp3', level: 0.6, loop: false },
  ringtone: { src: 'sounds/ringtone.mp3', level: 0.7, loop: true },
} as const;
type ClipName = keyof typeof CLIPS;

const NOTIFY_GAP_MS = 2000; // at most one message notification per 2 seconds
const RING_LIMIT_MS = 46_000; // the engine ends an unanswered direct call after 45 s, so this only backs it up

const clips = Object.fromEntries(
  (Object.keys(CLIPS) as ClipName[]).map((name) => {
    const audio = new Audio(import.meta.env.BASE_URL + CLIPS[name].src);
    audio.preload = 'auto';
    audio.loop = CLIPS[name].loop;
    return [name, audio];
  }),
) as Record<ClipName, HTMLAudioElement>;

// Do Not Disturb silences message and voice sounds. Ringing still sounds, as calls still ring.
const isDnd = (): boolean => useShell.getState().status.status === 'dnd';
const ringtoneAllowed = (): boolean => {
  const s = useSoundSettings.getState();
  return s.enabled && s.ringtone;
};
const callSoundAllowed = (): boolean => {
  const s = useSoundSettings.getState();
  return s.enabled && s.calls && !isDnd();
};

function play(name: ClipName): void {
  try {
    const clip = clips[name];
    const { volume } = useSoundSettings.getState();
    clip.volume = Math.min(1, Math.max(0, (CLIPS[name].level * volume) / 100));
    clip.currentTime = 0;
    clip.play().catch(() => {}); // blocked by autoplay policy, or no output device: stay silent
  } catch {
    // Audio is an extra. Nothing here may stop the UI.
  }
}

let lastNotification = 0;

// A message the user should hear about. The caller decides whether the message alerts at all.
export function playNotification(): void {
  const s = useSoundSettings.getState();
  if (!s.enabled || !s.messages || isDnd()) return;
  const now = Date.now();
  if (now - lastNotification < NOTIFY_GAP_MS) return;
  lastNotification = now;
  play('notification');
}

// ---- ringtone: an incoming call rings until its dialog closes; an outgoing call rings until answered or ended ----

export function startRingtone(): void {
  if (!ringtoneAllowed() || !clips.ringtone.paused) return;
  play('ringtone');
}

export function stopRingtone(): void {
  try {
    clips.ringtone.pause();
    clips.ringtone.currentTime = 0;
  } catch {
    // Nothing playing to stop.
  }
}

let outgoing = false;
let outgoingTimer: ReturnType<typeof setTimeout> | null = null;

// A direct call we placed is ringing: the callee has not answered yet.
export function beginOutgoingRing(): void {
  outgoing = true;
  if (outgoingTimer !== null) clearTimeout(outgoingTimer);
  outgoingTimer = setTimeout(endOutgoingRing, RING_LIMIT_MS);
  startRingtone();
}

export function endOutgoingRing(): void {
  if (!outgoing) return;
  outgoing = false;
  if (outgoingTimer !== null) clearTimeout(outgoingTimer);
  outgoingTimer = null;
  stopRingtone();
}

// ---- voice: joins and leaves in the call or voice channel this device is in ----

export function playCallJoin(): void {
  if (callSoundAllowed()) play('join');
}

export function playCallLeave(): void {
  if (callSoundAllowed()) play('leave');
}

// The same call while the room id is unknown: scope and place stand in for it.
const callIdentity = (c: CallState): string => c.id ?? [c.scope, c.community_name, c.channel_name, c.where].join('|');

// Called with every change to the call on this device. Joins and leaves come from the participant list.
// Ringing a call we placed makes no sound of its own, and ending it unanswered is silent too.
export function callChanged(prev: CallState | null, next: CallState | null): void {
  const ringing = outgoing;
  if (ringing && (!next || next.participants.length > 0)) endOutgoingRing(); // answered, declined or cancelled
  if (!next) {
    if (prev && !ringing) playCallLeave();
    return;
  }
  if (!prev || callIdentity(prev) !== callIdentity(next)) {
    if (prev && !ringing) playCallLeave();
    if (!outgoing) playCallJoin();
    return;
  }
  const before = new Set(prev.participants.map((p) => p.root_id));
  const after = new Set(next.participants.map((p) => p.root_id));
  if ([...after].some((id) => !before.has(id))) playCallJoin();
  if ([...before].some((id) => !after.has(id))) playCallLeave();
}
