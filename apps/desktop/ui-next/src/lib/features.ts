// Feature detection for surfaces the engine may not implement yet. The backend reports what it supports through the
// `capabilities` command, as an object of booleans or a list of names. A feature not reported renders nothing, so
// the UI never shows a dead control. Ported from the old UI's features.js; the list must stay the same.

export const FEATURES = [
  'groups', 'contacts', 'calls', 'guardians', 'transfers', 'multi_source', 'offline_delivery', 'embeds', 'key_change',
  'safety', 'devices', 'pins', 'notes', 'mute', 'onboarding', 'topics', 'close_dm', 'ignore', 'status', 'avatars', 'mentions', 'server_prefs',
  'private_channels', 'forward_calls', 'channel_order', 'dm_mentions',
] as const;

export type Feature = (typeof FEATURES)[number];

const supported = new Set<string>();

// Reads the capabilities once at boot. A backend without the command (or one that throws) supports nothing new.
export function setCapabilities(caps: unknown): void {
  supported.clear();
  if (Array.isArray(caps)) {
    for (const name of caps) if (typeof name === 'string' && (FEATURES as readonly string[]).includes(name)) supported.add(name);
  } else if (caps && typeof caps === 'object') {
    for (const name of FEATURES) if ((caps as Record<string, unknown>)[name] === true) supported.add(name);
  }
}

export const has = (name: Feature): boolean => supported.has(name);
