// Feature detection for surfaces the engine may not implement yet. The backend reports what it supports through
// the `capabilities` command, as an object of booleans ({ calls: true, groups: false, ... }) or a list of names.
// A feature not reported renders nothing, not even a disabled control, so the UI never shows a dead button.
// Replies, reactions, attachments and presence are detected on message and member fields instead (views/chat.js).

export const FEATURES = [
  'groups', // group conversations: the Create-a-group dialog, group headers and member lists
  'contacts', // contact requests (accept, decline, cancel), blocking and removing contacts
  'calls', // voice and video calls: voice channels, the call board, mic and deafen in the user panel
  'guardians', // recovery guardians in Settings
  'transfers', // the Transfers page and the per-transfer details panel
  'multi_source', // "From 3 sources" and piece-verification indicators on attachments
  'offline_delivery', // "X is offline. Dissent will deliver this when they're back." on queued DMs
  'embeds', // rich link previews taken from the sender's sealed message
  'key_change', // the key-change banner on a DM and in the safety dialog
  'safety', // "They match" (mark a safety number as compared) and verified badges
  'devices', // linked devices in Settings
  'pins', // pinned messages in a channel, pinned conversations in the sidebar
  'notes', // private notes and nicknames on contacts
  'mute', // muting a conversation
  'onboarding', // the backend says this identity is new (first run)
  'topics', // a channel topic line in the chat header (Part 6.13)
  'close_dm', // "Close DM" in the conversation menu (close_conversation)
  'ignore', // "Ignore" and "Unignore" in the DM menu; ignored users' messages collapse
  'status', // your status (online, idle, Do Not Disturb, invisible) from the profile popover on the user bar
  'avatars', // profile pictures in Settings › My identity, and the server icon in the server menu
];

const supported = new Set();

// Reads the capabilities once at boot. A backend without the command (or one that throws) supports nothing new.
export async function loadCapabilities(api) {
  supported.clear();
  let caps;
  try {
    caps = await api.capabilities();
  } catch {
    return; // no capabilities command: every new surface stays hidden
  }
  if (Array.isArray(caps)) {
    for (const name of caps) if (FEATURES.includes(name)) supported.add(name);
  } else if (caps && typeof caps === 'object') {
    for (const name of FEATURES) if (caps[name] === true) supported.add(name);
  }
}

export const has = (name) => supported.has(name);
