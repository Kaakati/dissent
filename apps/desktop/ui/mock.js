// Dev-only stand-in for the Tauri backend so ui/index.html opens in a plain browser.
// Does nothing inside the desktop app, where window.__TAURI__ is injected before this module runs.
import { addressOf, base32Encode, parseAddressInput } from './address.js';

if (!window.__TAURI__) {
  window.__DISSENT_MOCK__ = true;

  let seq = 0;
  // Mock IDs must be valid base32 (addresses are made from them). The name's bytes are base32-encoded, so
  // distinct names stay distinct: message ids like d1 and d10 must not collapse onto one id.
  const id = (name) => base32Encode(new TextEncoder().encode(name)).padEnd(52, 'a').slice(0, 52);
  const MIN = 60 * 1000;
  const DAY = 24 * 60 * MIN;
  const now = Date.now();

  const ME = id('mz7q4p2cmealice');
  const GRACE = id('k3f9q2xagrace');
  const LINUS = id('w8r2d6nblinus');
  const KEN = id('h5t7c3vyken');
  const RANA = id('rana-z6q1e8jd');
  const RADIA = id('e4j9m1sqradia');
  const people = { [ME]: 'Sami', [GRACE]: 'Maya', [LINUS]: 'Jonas', [KEN]: 'Omar', [RADIA]: 'Leila', [RANA]: 'Rana' };
  const handles = { [ME]: 'sami', [GRACE]: 'maya', [LINUS]: 'jonas', [KEN]: 'omar', [RADIA]: 'leila', [RANA]: 'rana' };
  const suffix = (root) => root.slice(0, 8);
  // "Let people add me by address" (on by default). Off makes our own address not found.
  let addressDiscoveryOn = true;
  // Adding by address: a known person adds at once; a bad or wrong-kind address is refused as the backend does.
  const byAddress = (text) => {
    const found = parseAddressInput(text);
    if (!found) return null;
    need(found.kind === 'user', `That is a ${found.kind} address. Ask for a person's address or contact link.`);
    need(found.id !== ME && people[found.id], "They may be offline, or they don't allow adding by address. Ask for their contact link.");
    return found.id;
  };
  // Presence for the member lists and DM rows: the mock sets a few, and leaves the rest without a dot.
  const MEMBER_STATUS = { [GRACE]: 'online', [LINUS]: 'idle', [KEN]: 'dnd', [RADIA]: 'offline' };

  // Messages are written as [minutes ago, author, text]. Gaps longer than 7 minutes start a new group.
  // Reactions are kept as { emoji, users } and replies as the original's id; messageItem shapes them for the UI.
  const at = (daysAgo, minutesAgo) => now - daysAgo * DAY - minutesAgo * MIN;
  const msg = (daysAgo, minutesAgo, author, text) => ({
    id: id(`m${++seq}`), author, text, ms: at(daysAgo, minutesAgo), edited: false, deleted: false, reactions: [], replyTo: null, queued: false,
  });

  const generalMessages = [
    msg(3, 300, GRACE, 'Welcome to Riverside Makers. Read the pinned rules before posting, please.'),
    msg(3, 296, GRACE, 'Booking sheet for the laser is pinned in #announcements.'),
    msg(3, 180, LINUS, 'Anyone booked the laser cutter for Saturday?'),
    msg(2, 600, KEN, 'Yes, 10:00 works for me. I can bring the spare birch ply.'),
    msg(2, 598, KEN, 'The extraction fan is fixed too, so the room stays clear.'),
    msg(2, 590, ME, 'Counting me in. I will bring the soldering kits.'),
    msg(2, 455, LINUS, 'Cut file for the enclosure is here: https://example.org/files/enclosure-v3'),
    msg(1, 900, RADIA, 'Has anyone tried the new PLA spool on the printer?'),
    msg(1, 880, RADIA, 'Mine jammed twice at 210 degrees. Settings are in the photo.'),
    msg(1, 860, GRACE, 'Please post the settings here so people can reproduce the print.'),
    msg(1, 200, ME, 'Booking the laser for 10:00 now.'),
    msg(0, 90, KEN, 'Morning. The printer is free from 14:00 if anyone needs it.'),
    msg(0, 88, KEN, 'The print log shows a clog at the nozzle again.'),
    msg(0, 86, LINUS, 'I will swap the nozzle on Friday.'),
    msg(0, 20, ME, 'Thanks. The enclosure photos are in the attachments.'),
    msg(0, 4, GRACE, 'Thanks all. The walkthrough video is in the attachments.'),
  ];
  // Sample replies and reactions on #general.
  generalMessages[5].replyTo = generalMessages[3].id; // Sami answers Omar's "Yes, 10:00 works"
  generalMessages[9].replyTo = generalMessages[8].id; // Maya asks for the print settings
  generalMessages[6].reactions = [{ emoji: '👍', users: [KEN, ME, RADIA] }, { emoji: '🔥', users: [RADIA] }];
  generalMessages[15].reactions = [{ emoji: '🙏', users: [KEN, LINUS, RADIA, ME] }];
  generalMessages[13].reactions = [{ emoji: '😅', users: [LINUS] }];

  // ---- file attachments (mock): a generated PNG stands in for a decrypted image on disk ----
  const MB = 1024 * 1024;
  const att = (name, size, mime, state = 'available', extra = {}) => ({ id: id(`a${++seq}`), name, size, mime, state, progress: null, local_path: null, ...extra });

  // 3d-printing shows the two broken reply cases: the original was deleted, or it is sealed for this device.
  function offTopicMessages() {
    const gone = msg(0, 40, RADIA, 'Deleted message');
    gone.deleted = true;
    const reply = msg(0, 30, KEN, 'Who is bringing the spare 3 mm ply on Saturday?');
    reply.replyTo = gone.id;
    const sealed = msg(0, 12, RADIA, 'I cannot open the older photos since I rejoined.');
    sealed.replyTo = id('sealed-elsewhere');
    const trap = msg(0, 6, KEN, 'The shop invoice is attached.');
    trap.attachments = [att('invoice.pdf.exe', 2.3 * MB, 'application/x-msdownload', 'downloaded', { local_path: 'C:/Users/ada/Downloads/invoice.pdf.exe' }), att('diagram.svg', 14 * 1024, 'image/svg+xml')];
    return [gone, reply, sealed, trap];
  }

  const servers = {
    c1: {
      id: 'c1',
      hue: 25,
      name: 'Riverside Makers',
      owner: GRACE,
      voice: [{ id: 'vc1', name: 'workshop', participants: [] }],
      channels: [
        { id: 'vc1', name: 'workshop', voice: true },
        { id: 'ch1', name: 'general', topic: 'Workshop chat, bookings and Saturday plans' },
        { id: 'ch2', name: 'hardware' },
        { id: 'ch3', name: 'announcements' },
        { id: 'ch4', name: '3d-printing' },
      ],
      roles: { [GRACE]: 'owner', [ME]: 'member', [LINUS]: 'admin', [KEN]: 'moderator', [RADIA]: 'member', [RANA]: 'member' },
      banned: new Set(),
      msgs: {
        ch1: generalMessages,
        ch2: [
          msg(1, 300, RADIA, 'Drawings for the new mount are in the hardware folder.'),
          msg(1, 298, GRACE, 'Thanks, I will check the tolerances on the 3 mm ply.'),
          // FilesServer board: one file per state, all in #hardware.
          { ...msg(0, 492, GRACE, 'Updated enclosure drawings for the CNC.'), attachments: [att('cnc-enclosure-v3.pdf', 24.6 * MB, 'application/pdf', 'downloaded', { local_path: 'C:/Users/ada/Downloads/cnc-enclosure-v3.pdf', verified: true })] },
          { ...msg(0, 482, LINUS, 'Walkthrough video of the new dust extraction.'), attachments: [att('workshop-walkthrough.mp4', 86.4 * MB, 'video/mp4', 'downloading', {
            progress: 0.41, transfer_id: 't1', fingerprint: 'b3:7k2q…fz4m', shared_in: '#hardware · only its 6 members get the key',
            pieces: { total: 1383, verified: 567 },
            sources: [
              { display_name: 'Jonas', note: 'Sender', route: 'direct', rate_bps: 3.1 * MB },
              { display_name: 'Shop seeder', note: 'Holds the encrypted copy only', route: 'direct', rate_bps: 2.4 * MB },
              { display_name: 'Maya', note: 'Downloaded it earlier', route: 'relay', rate_bps: 0.7 * MB },
            ] })] },
          { ...msg(0, 470, RADIA, 'Inventory sheet for Saturday.'), attachments: [att('shop-inventory.xlsx', 1.1 * MB, 'application/vnd.openxmlformats-officedocument.spreadsheetml.sheet', 'waiting', { note: 'Only Leila has it and she’s offline. It downloads when she or the seeder is back.' })] },
          { ...msg(0, 450, KEN, 'Full-resolution scan of the old panel.'), attachments: [att('panel-scan-full.tiff', 12.4 * 1024 * MB, 'image/tiff', 'too_large')] },
        ],
        ch3: [msg(5, 0, GRACE, 'The laser cutter is back from repair. Booking sheet is pinned here.')],
        ch4: offTopicMessages(),
      },
    },
    c2: {
      id: 'c2',
      hue: 150,
      name: 'Open Source Arabic',
      owner: ME,
      channels: [{ id: 'ch5', name: 'general' }, { id: 'ch6', name: 'reading-list' }],
      roles: { [ME]: 'owner', [KEN]: 'member', [RADIA]: 'member' },
      banned: new Set(),
      msgs: {
        ch5: [msg(2, 500, KEN, 'This month: the Dispossessed. Starting Monday.'), msg(2, 498, KEN, 'Chapters 1 to 6.')],
        ch6: [],
      },
    },
    c3: {
      id: 'c3',
      hue: 300,
      name: 'Book Club',
      owner: LINUS,
      channels: [{ id: 'ch7', name: 'general' }, { id: 'ch8', name: 'photos' }],
      roles: { [LINUS]: 'owner', [ME]: 'member', [GRACE]: 'member' },
      banned: new Set(),
      msgs: { ch7: [msg(0, 60, GRACE, 'Saturday walk along the ridge, anyone?')], ch8: [] },
    },
  };

  const mockImage = (w, h, hue, label) => {
    const canvas = document.createElement('canvas');
    canvas.width = w;
    canvas.height = h;
    const g = canvas.getContext('2d');
    const grad = g.createLinearGradient(0, 0, w, h);
    grad.addColorStop(0, `hsl(${hue} 50% 35%)`);
    grad.addColorStop(1, `hsl(${hue + 40} 45% 20%)`);
    g.fillStyle = grad;
    g.fillRect(0, 0, w, h);
    g.strokeStyle = 'rgba(255,255,255,.25)';
    g.lineWidth = 3;
    for (let i = 0; i < 6; i++) {
      g.beginPath();
      g.moveTo(40, h - 40 - i * 20);
      g.lineTo(w - 40, h - 60 - i * (h / 9));
      g.stroke();
    }
    g.fillStyle = 'rgba(255,255,255,.85)';
    g.font = `600 ${Math.round(h / 12)}px "Segoe UI", sans-serif`;
    g.fillText(label, 40, 60);
    return canvas.toDataURL('image/png');
  };
  const localPathFor = (name, mime) => (mime.startsWith('image/') ? mockImage(640, 420, 200, name) : `C:/Users/ada/Downloads/${name}`);
  // Real attachments carry a filesystem path; the mock's convertFileSrc hands the data URL straight back.
  generalMessages[8].attachments = [att('print-test.png', 1.7 * MB, 'image/png', 'downloaded', { local_path: mockImage(640, 420, 150, 'print test') }), att('settings.csv', 48 * 1024, 'text/csv')];
  generalMessages[0].attachments = [att('rules.pdf', 180 * 1024, 'application/pdf', 'downloaded', { local_path: 'C:/Users/ada/Downloads/rules.pdf' })];
  generalMessages[12].attachments = [att('print-log.txt', 1.2 * MB, 'text/plain')];
  const GB = 1024 * MB;
  generalMessages[13].attachments = [
    att('print-jobs.zip', 24 * MB, 'application/zip'),
    att('workshop-photos.zip', 12.4 * GB, 'application/zip', 'too_large'),
    att('walkthrough.mkv', 9.8 * GB, 'video/x-matroska', 'paused', { progress: 0.12 }),
    att('shop-inventory.tar', 9.6 * GB, 'application/x-tar', 'disk_full', { needed: 19.6 * GB }),
  ];
  // A message Sami sent while nobody else in the channel was online (Part 6.8, channels).
  generalMessages.push({ ...msg(0, 2, ME, 'Counting me in. I will bring the soldering kits.'), queued: true });
  generalMessages[15].attachments = [att('enclosure.png', 1.1 * MB, 'image/png')];

  const dms = {
    [GRACE]: [
      { id: id(`d${++seq}`), from: GRACE, text: 'https://github.com/Kaakati/dissent/tree/main/apps/relay', ms: at(1, 420), pending: false, embeds: [{ url: 'https://github.com/Kaakati/dissent/tree/main/apps/relay', site: 'GitHub', title: 'dissent/apps/relay: run a relay for your community', description: 'A helper that passes sealed traffic between people who can’t connect directly. Running one near your community makes relayed connections faster.', image: mockImage(432, 168, 200, 'dissent-relay') }] },
      { id: id(`d${++seq}`), from: GRACE, text: 'Can you look at the laser booking sheet when you get a minute?', ms: at(1, 400), pending: false },
      { id: id(`d${++seq}`), from: ME, text: 'Yes, reading it now.', ms: at(1, 380), pending: false },
      { id: id(`d${++seq}`), from: GRACE, text: 'Bring the ply on Saturday?', ms: at(0, 40), pending: false, attachments: [att('booking-sheet.png', 640 * 1024, 'image/png', 'downloaded', { local_path: mockImage(640, 420, 330, 'booking sheet') }), att('notes.md', 12 * 1024, 'text/markdown', 'failed'), att('laser-rates.pdf', 2.4 * MB, 'application/pdf')] },
      // FilesDirect board: a direct upload to Maya, in progress.
      { id: id(`d${++seq}`), from: ME, text: 'All the photos from Saturday.', ms: at(0, 5), pending: false, attachments: [att('saturday-photos.zip', 62.3 * MB, 'application/zip', 'sending', {
        progress: 0.58, transfer_id: 't2', status: 'Sending · 36.1 of 62.3 MB · 11.4 MB/s · about 2 s left',
        direct: { peer: 'Maya', speed_bps: 11.4 * MB, sent: 36.1 * MB, total: 62.3 * MB, time_left_s: 2 },
      })] },
    ],
    [LINUS]: [
      { id: id(`d${++seq}`), from: LINUS, text: 'Have a look at the relay code: https://github.com/Kaakati/dissent/tree/main/apps/relay', ms: at(2, 120), pending: false, embeds: [{ url: 'https://github.com/Kaakati/dissent/tree/main/apps/relay', site: 'GitHub', title: 'dissent/apps/relay: run a relay for your community', description: 'A helper that passes sealed traffic between people who can’t connect directly. Running one near your community makes relayed connections faster.', image: mockImage(432, 168, 200, 'dissent-relay') }] },
    ],
  };

  let mockNetwork = { use_public_relays: true, relays: [] };
  // Voice settings as the desktop saves them (voice_settings / set_voice_settings), and the mic test's fake meter.
  let mockVoice = { profile: 'voice_isolation', input_volume: 100, output_volume: 100, noise_suppression: 'standard', echo_cancellation: true, agc: true, auto_sensitivity: true, threshold_db: -50, push_to_talk: false, ptt_key: null };
  let mockMicTimer = null;
  let mockMicTick = 0;
  let mockStatus = { status: 'online', until_ms: null };
  let mockStatusTimer = null;
  const listeners = [];
  // ?view=dm&peer=maya&plain: the DM board's thread. Maya's messages, one queued message from me (offline delivery,
  // Part 6.8), a verified safety number and no key change. Other mock states are unchanged.
  const PLAIN_DM = new URLSearchParams(location.search).has('plain');
  if (PLAIN_DM) {
    const dayAt = (daysAgo, h, m) => { const d = new Date(); d.setDate(d.getDate() - daysAgo); d.setHours(h, m, 0, 0); return d.getTime(); };
    dms[GRACE] = [
      { id: id(`d${++seq}`), from: GRACE, text: 'Bring the ply on Saturday?', ms: dayAt(1, 22, 40), pending: false },
      { id: id(`d${++seq}`), from: GRACE, text: 'And the 3 mm sheets if you still have some.', ms: dayAt(1, 22, 41), pending: false },
      { id: id(`d${++seq}`), from: ME, text: 'Yes to both. I’ll be there by 09:45 to set up.', ms: dayAt(0, 9, 5), pending: false, queued: true },
    ];
  }
  // Named events, like Tauri's. Engine events use the 'dissent://event' name.
  const emitTo = (name, payload) => listeners.filter((l) => l.name === name).forEach((l) => l.cb({ payload }));
  const emit = (payload) => emitTo('dissent://event', payload);
  const UPDATE = { version: '0.2.0', current_version: '0.1.0', date: '2026-10-24', notes: '- Faster sync for large servers\n- Fixed relay settings on restart\n- Message search keeps focus while typing' };
  const roleOf = (c, user) => c.roles[user] ?? null;
  const myRole = (c) => roleOf(c, ME);
  const canMod = (c) => ['owner', 'admin', 'moderator'].includes(myRole(c));
  const summary = (c) => ({ id: c.id, name: c.name, owner: c.owner, my_role: myRole(c), icon: c.icon ?? null });
  const sharedServers = (user) => Object.values(servers).filter((c) => c.roles[user] && c.roles[ME]);
  const conversations = () =>
    Object.entries(dms)
      .map(([peer, items]) => {
        const last = items[items.length - 1];
        return { peer, display_name: people[peer], avatar: pictures[peer] ?? null, last_text: last.text, last_ms: last.ms };
      })
      .sort((a, b) => b.last_ms - a.last_ms);
  const view = (c) => ({
    id: c.id,
    name: c.name,
    owner: c.owner,
    icon: c.icon ?? null,
    voice_channels: c.voice ?? [],
    // Relays the server can use for voice channels. The engine does not report these yet.
    relays: ['https://relay-eu.riverside.example', 'https://relay-us.riverside.example'],
    my_role: myRole(c),
    channels: c.channels.map((ch) => ({ slowmode_s: 0, visibility: 'default', bitrate_kbps: 64, user_limit: 0, relay: 'automatic', ...ch })),
    members: Object.keys(c.roles).map((r) => ({
      root_id: r,
      suffix: suffix(r),
      display_name: people[r],
      avatar: pictures[r] ?? null,
      handle: handles[r] ?? null,
      role: c.roles[r],
      banned: c.banned.has(r),
      status: MEMBER_STATUS[r],
    })),
  });
  // The reply reference: a snippet of the original, or text null when it is deleted or sealed for this device.
  const replyRef = (c, channelId, originalId) => {
    const orig = (c.msgs[channelId] ?? []).find((m) => m.id === originalId);
    if (!orig) return { id: originalId, author_name: '', text: null, locked: true };
    if (orig.deleted) return { id: orig.id, author: orig.author, author_name: people[orig.author], text: null };
    return { id: orig.id, author: orig.author, author_name: people[orig.author], text: orig.text.slice(0, 120) };
  };
  // `window.__mockNoReactions = true` simulates a backend without replies and reactions: the fields are left out.
  const messageItem = (c, channelId, m) => ({
    id: m.id,
    author: m.author,
    author_name: people[m.author],
    text: m.deleted ? '' : m.text,
    queued: Boolean(m.queued),
    ms: m.ms,
    edited: m.edited,
    deleted: m.deleted,
    ...(window.__mockNoReactions ? {} : {
      reply_to: m.replyTo ? replyRef(c, channelId, m.replyTo) : null,
      reactions: m.reactions.map((r) => ({
        emoji: r.emoji,
        count: r.users.length,
        me: r.users.includes(ME),
        names: r.users.slice(0, 3).map((u) => people[u]),
      })),
    }),
    // `window.__mockNoFiles = true` simulates a backend without file sharing: the field is left out.
    ...(window.__mockNoFiles ? {} : { attachments: (m.attachments ?? []).map((a) => ({ ...a })) }),
  });
  // DM and group items carry the same reply, reaction, edit and delete fields as channel items. `list` is the conversation.
  const dmReplyRef = (list, messageId) => {
    const orig = list.find((m) => m.id === messageId);
    if (!orig) return null;
    const ref = { id: orig.id, author: orig.from, author_name: people[orig.from] };
    return orig.deleted ? { ...ref, text: null } : { ...ref, text: orig.text.slice(0, 120) };
  };
  const dmReactions = (m) => (m.reactions ?? []).filter((r) => r.users.length > 0).map((r) => ({
    emoji: r.emoji, count: r.users.length, me: r.users.includes(ME), names: r.users.slice(0, 3).map((u) => people[u]),
  }));
  const dmItem = (m, list = []) => ({
    id: m.id, from: m.from, text: m.deleted ? '' : m.text, ms: m.ms, pending: m.pending, embeds: m.embeds ?? [], queued: Boolean(m.queued),
    edited: Boolean(m.edited), deleted: Boolean(m.deleted), system: m.system === true,
    ...(window.__mockNoReactions ? {} : { reply_to: m.replyTo ? dmReplyRef(list, m.replyTo) : null, reactions: dmReactions(m) }),
    ...(window.__mockNoFiles ? {} : { attachments: (m.attachments ?? []).map((a) => ({ ...a })) }),
  });
  // The shared bodies of the dm_* and group_* commands; `list` is the DM's or the group's items.
  const dmReplyIn = (list, prefix, messageId, text) => {
    need(list.some((m) => m.id === messageId), 'Cannot reply: message not found');
    const item = { id: id(`${prefix}${++seq}`), from: ME, text, ms: Date.now(), pending: false, replyTo: messageId, reactions: [] };
    list.push(item);
    return dmItem(item, list);
  };
  const dmReactIn = (list, messageId, emoji, on) => {
    const m = list.find((x) => x.id === messageId);
    need(m && !m.deleted, 'Cannot react: message not found');
    m.reactions ??= [];
    const r = m.reactions.find((x) => x.emoji === emoji);
    if (!on) {
      if (r) r.users = r.users.filter((u) => u !== ME);
      m.reactions = m.reactions.filter((x) => x.users.length > 0);
    } else if (r) {
      if (!r.users.includes(ME)) r.users.push(ME);
    } else {
      m.reactions.push({ emoji, users: [ME] });
    }
    return null;
  };
  const dmEditIn = (list, messageId, text) => {
    const m = list.find((x) => x.id === messageId);
    need(m && m.from === ME && !m.deleted, 'You can only edit your own messages');
    m.text = text;
    m.edited = true;
    return null;
  };
  const dmDeleteIn = (list, messageId) => {
    const m = list.find((x) => x.id === messageId);
    need(m && m.from === ME, 'You can only delete your own messages');
    m.deleted = true;
    return null;
  };
  const PICK = [
    { path: 'C:/Users/ada/Pictures/parser-bench.png', name: 'parser-bench.png', size: 3.1 * MB, mime: 'image/png' },
    { path: 'C:/Users/ada/Documents/notes.md', name: 'notes.md', size: 12 * 1024, mime: 'text/markdown' },
  ];
  const PICK_BIG = { path: 'C:/Users/ada/Downloads/dataset.tar', name: 'dataset.tar', size: 11.2 * GB, mime: 'application/x-tar' };
  const guessMime = (name) => ({ png: 'image/png', jpg: 'image/jpeg', md: 'text/markdown', pdf: 'application/pdf', zip: 'application/zip', tar: 'application/x-tar', txt: 'text/plain' })[name.split('.').pop().toLowerCase()] ?? 'application/octet-stream';
  const uploaded = (paths) => paths.map((p) => {
    const name = p.split(/[\\/]/).pop();
    const picked = [...PICK, PICK_BIG].find((f) => f.path === p);
    const size = picked?.size ?? 1 * MB;
    const mime = picked?.mime ?? guessMime(name);
    return att(name, size, mime, 'downloaded', { local_path: localPathFor(name, mime) });
  });
  // Walks the attachment through downloading in five steps, then downloaded (or failed for photos.zip and notes.md).
  const findAttachment = (communityId, messageId, attachmentId) => {
    const items = communityId ? Object.values(servers[communityId].msgs).flat() : Object.values(dms).flat();
    const m = items.find((x) => x.id === messageId);
    const a = m?.attachments?.find((x) => x.id === attachmentId);
    return { m, a };
  };
  const refreshFor = (communityId, m) => {
    if (communityId) {
      const channelId = Object.keys(servers[communityId].msgs).find((ch) => servers[communityId].msgs[ch].includes(m));
      emit({ kind: 'message', community_id: communityId, channel_id: channelId });
    } else {
      const peer = Object.keys(dms).find((p) => dms[p].includes(m));
      emit({ kind: 'dm', peer });
    }
  };
  const findMessage = (c, channelId, messageId) => (c.msgs[channelId] ?? []).find((m) => m.id === messageId);
  const pause = () => new Promise((r) => setTimeout(r, window.__mockDelay ?? 0));
  const need = (cond, reason) => {
    if (!cond) throw reason;
  };
  // Profile pictures by root id. The engine sends them as WebP (or PNG) data, capped at 256 KB like the real blob.
  const pictures = {};
  const MAX_PICTURE = 256 * 1024;
  const pictureFrom = (bytes) => {
    need(atob(bytes).length <= MAX_PICTURE, 'That picture is over 256 KB');
    return `data:image/webp;base64,${bytes}`;
  };

  // The first-run flag stays set until the user completes onboarding (complete_first_run), never on export.
  let firstRunPending = window.__mockFirstRun ?? new URLSearchParams(location.search).has('firstrun');
  // ?unlock=new starts a portable profile locked with no identity yet; ?unlock=1 starts it locked with one.
  // In the mock, the passphrase "wrong" is refused so the retry path can be checked.
  const unlockParam = new URLSearchParams(location.search).get('unlock');
  // ?chooser=1 starts a portable exe's first run, before a choice is saved. ?choice=link|copy|fresh names the saved choice.
  // ?installed=0 hides the options that use the installed profile.
  const params = new URLSearchParams(location.search);
  let chooser = params.has('chooser');
  let choice = params.get('choice');
  const installedProfile = params.get('installed') !== '0';
  let locked = Boolean(unlockParam);
  const handlers = {
    complete_first_run: () => { firstRunPending = false; return null; },
    vault_status: () => ({ chooser, locked: locked && !chooser, new_profile: unlockParam === 'new' || choice === 'link' || choice === 'copy', choice, installed_profile: installedProfile }),
    choose_profile: ({ choice: picked }) => {
      if (picked === 'installed' && !installedProfile) throw 'No Dissent profile was found on this computer.';
      // The real app restarts here. The mock keeps the page and records the choice.
      chooser = false;
      choice = picked;
      locked = picked !== 'installed';
      return null;
    },
    unlock_profile: ({ passphrase }) => {
      if (passphrase === 'wrong') throw 'That passphrase is wrong.';
      // A link keeps the profile locked until the grant is accepted.
      if (choice === 'link' && unlockParam === 'new') return 'link';
      locked = false;
      return 'open';
    },
    finish_portable_link: ({ grant }) => {
      if (grant !== 'mock-grant') throw 'That grant does not match this link.';
      locked = false;
      return null;
    },
    open_release_page: () => null,
    me: () => ({ root_id: ME, device_id: id('medevice'), suffix: suffix(ME), display_name: people[ME], avatar: pictures[ME] ?? null, first_run: firstRunPending }),
    set_display_name: ({ name }) => {
      people[ME] = name;
      return null;
    },
    set_avatar: ({ bytes }) => {
      pictures[ME] = pictureFrom(bytes);
      emitTo('dissent://avatar-changed', { root_id: ME });
      return null;
    },
    clear_avatar: () => {
      delete pictures[ME];
      emitTo('dissent://avatar-changed', { root_id: ME });
      return null;
    },
    set_community_icon: ({ communityId, bytes }) => {
      const c = servers[communityId];
      need(['owner', 'admin'].includes(myRole(c)), 'Only owners and admins can change the server icon');
      c.icon = pictureFrom(bytes);
      emitTo('dissent://avatar-changed', { community_id: communityId });
      return null;
    },
    clear_community_icon: ({ communityId }) => {
      const c = servers[communityId];
      need(['owner', 'admin'].includes(myRole(c)), 'Only owners and admins can change the server icon');
      c.icon = null;
      emitTo('dissent://avatar-changed', { community_id: communityId });
      return null;
    },
    contact_link: () => (addressDiscoveryOn ? `dissent://contact/${addressOf('user', ME)}` : `dissent://contact/mock-${suffix(ME)}`),
    address_discovery: () => addressDiscoveryOn,
    set_address_discovery: ({ on }) => { addressDiscoveryOn = on; return on; },
    add_contact: ({ link }) => {
      const known = byAddress(link);
      if (known) {
        contactSet.add(known);
        return { root_id: known, suffix: suffix(known), display_name: people[known], is_contact: true, handles: [], mutual_communities: [] };
      }
      need(link.startsWith('dissent://contact/'), 'Not a contact link');
      const root = id(`newcomer${++seq}`);
      people[root] = 'Newcomer';
      dms[root] = [];
      return { root_id: root, suffix: suffix(root), display_name: 'Newcomer', is_contact: true, handles: [], mutual_communities: [] };
    },

    list_communities: () => Object.values(servers).map(summary),
    create_community: ({ name }) => {
      const cid = `c${++seq}`;
      const chid = `ch${++seq}`;
      const c = { id: cid, name, owner: ME, channels: [{ id: chid, name: 'general' }], roles: { [ME]: 'owner' }, banned: new Set(), msgs: { [chid]: [] } };
      servers[cid] = c;
      return summary(c);
    },
    create_invite: ({ communityId }) => {
      const c = servers[communityId];
      const link = `dissent://invite/mock-${communityId}-${++seq}`;
      c.invites = [...(c.invites ?? []), { id: `inv${seq}`, link }];
      return link;
    },
    list_invites: ({ communityId }) => servers[communityId].invites ?? [],
    revoke_invite: ({ communityId, inviteId }) => {
      const c = servers[communityId];
      c.invites = (c.invites ?? []).filter((inv) => inv.id !== inviteId);
      return null;
    },
    join_community: ({ invite }) => {
      throw `Mock backend cannot join "${invite.slice(0, 24)}…"`;
    },
    leave_community: ({ communityId }) => {
      delete servers[communityId];
      return null;
    },
    community_view: async ({ communityId }) => {
      await pause();
      return view(servers[communityId]);
    },
    create_channel: ({ communityId, name }) => {
      const c = servers[communityId];
      need(['owner', 'admin'].includes(myRole(c)), 'Only owner or admin can create channels');
      const ch = { id: `ch${++seq}`, name };
      c.channels.push(ch);
      c.msgs[ch.id] = [];
      return ch;
    },
    update_channel: ({ communityId, channelId, ...patch }) => {
      const c = servers[communityId];
      need(['owner', 'admin'].includes(myRole(c)), 'Only owner or admin can change channels');
      const ch = c.channels.find((x) => x.id === channelId);
      need(ch, 'No such channel');
      for (const [key, value] of Object.entries(patch)) if (value !== undefined) ch[key] = value;
      const voice = c.voice?.find((v) => v.id === channelId);
      if (voice && patch.name) voice.name = patch.name;
      return { ...ch };
    },
    delete_channel: ({ communityId, channelId }) => {
      const c = servers[communityId];
      need(['owner', 'admin'].includes(myRole(c)), 'Only owner or admin can delete channels');
      need(c.channels.some((x) => x.id === channelId), 'No such channel');
      c.channels = c.channels.filter((x) => x.id !== channelId);
      c.voice = c.voice?.filter((v) => v.id !== channelId);
      delete c.msgs[channelId];
      return null;
    },
    create_voice_channel: ({ communityId, name }) => {
      const c = servers[communityId];
      need(['owner', 'admin'].includes(myRole(c)), 'Only owner or admin can create channels');
      const ch = { id: `vc${++seq}`, name, voice: true };
      c.channels.push(ch);
      c.voice = [...(c.voice ?? []), { id: ch.id, name, participants: [] }];
      return ch;
    },
    channel_messages: async ({ communityId, channelId, limit }) => {
      await pause();
      const c = servers[communityId];
      return (c.msgs[channelId] ?? []).slice(-limit).map((m) => messageItem(c, channelId, m));
    },
    send_message: ({ communityId, channelId, text }) => {
      const c = servers[communityId];
      const m = { ...msg(0, 0, ME, text), ms: Date.now() };
      c.msgs[channelId].push(m);
      return messageItem(c, channelId, m);
    },
    send_reply: ({ communityId, channelId, messageId, text }) => {
      const c = servers[communityId];
      need(findMessage(c, channelId, messageId), 'Cannot reply: message not found');
      const m = { ...msg(0, 0, ME, text), ms: Date.now(), replyTo: messageId };
      c.msgs[channelId].push(m);
      return messageItem(c, channelId, m);
    },
    react: ({ communityId, channelId, messageId, emoji }) => {
      const m = findMessage(servers[communityId], channelId, messageId);
      need(m && !m.deleted, 'Cannot react: message not found');
      const r = m.reactions.find((x) => x.emoji === emoji);
      if (r) {
        if (!r.users.includes(ME)) r.users.push(ME);
      } else {
        m.reactions.push({ emoji, users: [ME] });
      }
      return null;
    },
    unreact: ({ communityId, channelId, messageId, emoji }) => {
      const m = findMessage(servers[communityId], channelId, messageId);
      need(m, 'Cannot react: message not found');
      const r = m.reactions.find((x) => x.emoji === emoji);
      if (r) r.users = r.users.filter((u) => u !== ME);
      m.reactions = m.reactions.filter((x) => x.users.length > 0);
      return null;
    },
    edit_message: ({ communityId, channelId, messageId, text }) => {
      const m = findMessage(servers[communityId], channelId, messageId);
      need(m && m.author === ME, 'You can only edit your own messages');
      m.text = text;
      m.edited = true;
      return null;
    },
    delete_message: ({ communityId, channelId, messageId }) => {
      const c = servers[communityId];
      const m = findMessage(c, channelId, messageId);
      need(m && (m.author === ME || canMod(c)), 'Not allowed to delete this message');
      m.deleted = true;
      return null;
    },
    moderate: ({ communityId, action, user }) => {
      const c = servers[communityId];
      const mine = myRole(c);
      need(canMod(c), 'Only owner, admin or moderator can moderate');
      if (action === 'ban') c.banned.add(user);
      else if (action === 'unban') c.banned.delete(user);
      else if (action === 'grant_moderator') c.roles[user] = 'moderator';
      else if (action === 'revoke_moderator') c.roles[user] = 'member';
      else if (action === 'grant_admin' || action === 'revoke_admin') {
        need(mine === 'owner', 'Only the owner can change admins');
        c.roles[user] = action === 'grant_admin' ? 'admin' : 'member';
      }
      return null;
    },
    claim_handle: ({ communityId, handle }) => {
      handles[ME] = handle;
      return null;
    },
    pick_files: () => (window.__mockPickBig ? [...PICK, PICK_BIG] : [...PICK]).map((f) => ({ ...f })),
    send_files: ({ communityId, channelId, paths, text, seed }) => {
      const c = servers[communityId];
      const m = { ...msg(0, 0, ME, text ?? ''), ms: Date.now(), attachments: uploaded(paths) };
      c.msgs[channelId].push(m);
      return messageItem(c, channelId, m);
    },
    send_dm_files: ({ peer, paths, text }) => {
      const item = { id: id(`d${++seq}`), from: ME, text: text ?? '', ms: Date.now(), pending: false, attachments: uploaded(paths) };
      (dms[peer] ??= []).push(item);
      return dmItem(item);
    },
    download_attachment: ({ communityId, messageId, attachmentId }) => {
      const { m, a } = findAttachment(communityId, messageId, attachmentId);
      need(a, 'Attachment not found');
      // Resuming a paused download continues from where it stopped; a disk_full retry starts again.
      let step = a.state === 'paused' ? Math.round((a.progress ?? 0) * 5) : 0;
      a.state = 'downloading';
      a.progress = step / 5;
      a.sources = communityId
        ? [{ display_name: 'Maya', route: 'direct', rate_bps: 4 * MB }, { display_name: 'Jonas', route: 'relay', rate_bps: 1.2 * MB }, { display_name: 'Seeder', route: 'seeder', rate_bps: 2 * MB }]
        : [{ display_name: people[GRACE], route: 'direct', rate_bps: 3 * MB }];
      a.pieces = { total: 24, verified: Math.round(step / 5 * 24) };
      refreshFor(communityId, m); // the card reads sources from the reloaded message
      a.needed = undefined;
      const tick = () => {
        step += 1;
        if (['photos.zip', 'notes.md'].includes(a.name) && step === 3) {
          a.state = 'failed';
          a.progress = null;
          refreshFor(communityId, m);
          return;
        }
        if (a.name === 'dataset-2026.tar' && step === 2) {
          a.state = 'disk_full';
          a.needed = 19.6 * GB;
          refreshFor(communityId, m);
          return;
        }
        if (step < 5) {
          a.progress = step / 5;
          emitTo('dissent://attachment-progress', { message_id: messageId, attachment_id: attachmentId, progress: a.progress });
          setTimeout(tick, 350);
          return;
        }
        a.state = 'downloaded';
        a.progress = null;
        a.local_path = localPathFor(a.name, a.mime);
        refreshFor(communityId, m);
      };
      setTimeout(tick, 350);
      return null;
    },
    open_attachment: ({ path }) => {
      need(typeof path === 'string' && path.length > 0, 'Nothing to open');
      return null;
    },
    reveal_attachment: ({ path }) => {
      need(typeof path === 'string' && path.length > 0, 'Nothing to show');
      return null;
    },
    check_update: () => (window.__mockNoUpdate ? null : { ...UPDATE }),
    install_update: async () => {
      for (let i = 1; i <= 10; i++) {
        await new Promise((r) => setTimeout(r, 180));
        emitTo('dissent://update-progress', { downloaded: Math.round((i / 10) * 9437184), total: 9437184 });
      }
      await new Promise((r) => setTimeout(r, 400));
      return null;
    },
    safety_number: ({ user }) => {
      let h = 7;
      for (const ch of ME + user) h = (h * 31 + ch.charCodeAt(0)) >>> 0;
      return Array.from({ length: 12 }, (_, i) => String((h * (i + 3) + i * 7919) % 100000).padStart(5, '0')).join(' ');
    },
    user_profile: ({ user }) => {
      const shared = sharedServers(user);
      return {
        root_id: user,
        suffix: suffix(user),
        display_name: people[user] ?? `anon·${suffix(user)}`,
        avatar: pictures[user] ?? null,
        is_contact: Boolean(dms[user]),
        key_changed_ms: user === GRACE && !PLAIN_DM ? Date.now() - DAY : null,
        verified_ms: user === GRACE ? (PLAIN_DM ? new Date(2026, 9, 12).getTime() : Date.now() - 2 * DAY) : null,
        mutual_contacts: user === GRACE ? 1 : 0,
        first_seen_ms: at(120, 0),
        handles: shared.map((c) => ({ community_id: c.id, community_name: c.name, handle: handles[user] ?? null })),
        mutual_communities: shared.map((c) => ({ id: c.id, name: c.name })),
      };
    },

    list_conversations: () => conversations(),
    dm_messages: ({ peer, limit }) => (dms[peer] ?? []).slice(-limit).map((m) => dmItem(m, dms[peer])),
    dm_reply: ({ peer, messageId, text }) => dmReplyIn((dms[peer] ??= []), 'd', messageId, text),
    dm_react: ({ peer, messageId, emoji }) => dmReactIn(dms[peer] ?? [], messageId, emoji, true),
    dm_unreact: ({ peer, messageId, emoji }) => dmReactIn(dms[peer] ?? [], messageId, emoji, false),
    dm_edit: ({ peer, messageId, text }) => dmEditIn(dms[peer] ?? [], messageId, text),
    dm_delete: ({ peer, messageId }) => dmDeleteIn(dms[peer] ?? [], messageId),
    send_dm: ({ peer, text }) => {
      const item = { id: id(`d${++seq}`), from: ME, text, ms: Date.now(), pending: true };
      (dms[peer] ??= []).push(item);
      // Simulate the outbox delivering after a moment.
      setTimeout(() => {
        item.pending = false;
        emit({ kind: 'dm', peer });
      }, 2500);
      return { id: item.id, from: ME, text, ms: item.ms, pending: true };
    },

    app_version: () => '0.1.0 (mock)',
    identity_export_phrase: () => 'harbor velvet ocean pistol arctic gentle meadow ribbon canyon lunar orbit fabric timber violin elder copper saddle raven maple hollow quartz ember ladder pilot',
    identity_import_phrase: () => null,
    get_network_settings: () => ({ ...mockNetwork, relays: [...mockNetwork.relays] }),
    set_network_settings: ({ settings }) => {
      const relays = settings.relays.map((r) => {
        const url = new URL(r);
        need(url.protocol === 'https:', `A relay must use https: ${r}`);
        return url.toString();
      });
      need(settings.use_public_relays || relays.length > 0, 'Add at least one relay, or turn public relays back on.');
      mockNetwork = { use_public_relays: Boolean(settings.use_public_relays), relays: [...new Set(relays)] };
      return { ...mockNetwork, relays: [...mockNetwork.relays] };
    },
    net_status: () => ({
      dht: { size_estimate: 9200000, confidence: 0.82, firewalled: false },
      peers: 3,
      relay: 'use1-1.relay.n0.iroh.network',
    }),
  };

  // ---- feature surfaces (design boards): contacts, groups, calls, transfers, guardians, devices, pins ----
  // The engine has none of these yet. The mock answers every command so each board can be checked in the browser.
  // `window.__mockNoCapabilities = true` makes `capabilities` return nothing, which hides every new surface.
  const FEATURES = ['groups', 'contacts', 'calls', 'guardians', 'transfers', 'multi_source', 'offline_delivery', 'embeds', 'key_change', 'safety', 'devices', 'pins', 'notes', 'mute', 'onboarding', 'topics', 'close_dm', 'ignore', 'status', 'avatars'];
  const ignoredSet = new Set();
  const NADIA = id('nadia-c5v8m1wk');
  const YUSUF = id('yusuf-r3j6t0pb');
  const KARIM = id('karim-n2w5d8fx');
  Object.assign(people, { [NADIA]: 'Nadia', [YUSUF]: 'Yusuf', [KARIM]: 'Karim' });
  Object.assign(handles, { [NADIA]: 'nadia', [YUSUF]: 'yusuf', [KARIM]: 'karim' });
  window.__mockHues = { [GRACE]: 25, [LINUS]: 250, [RADIA]: 140, [ME]: 200, [KEN]: 80, [RANA]: 330, [NADIA]: 280, [YUSUF]: 60, [KARIM]: 170 };
  const contactSet = new Set([GRACE, LINUS, KEN, RADIA]);
  const incomingSet = new Set([NADIA, YUSUF]);
  const outgoingSet = new Set([KARIM]);
  const calls = { current: null };
  const transfers = [
    { id: 't1', direction: 'download', name: 'workshop-walkthrough.mp4', size: 86.4 * MB, state: 'active', progress: 0.41, route: '3 sources', route_kind: 'direct', route_note: '2 direct, 1 via relay', where: { kind: 'server', name: 'Riverside Makers · #hardware' }, sources: 3, status_text: '41% · 6.2 MB/s · 8 s left' },
    { id: 't2', direction: 'upload', name: 'saturday-photos.zip', size: 62.3 * MB, state: 'active', progress: 0.58, route: 'Direct', route_kind: 'direct', route_note: 'Peer to peer, no server', where: { kind: 'direct', name: 'Maya' }, sources: 1, status_text: '58% · 11.4 MB/s' },
    { id: 't3', direction: 'download', name: 'shop-inventory.xlsx', size: 1.1 * MB, state: 'waiting', progress: 0, route: 'Waiting', route_kind: 'waiting', route_note: 'No one who has it is online', where: { kind: 'server', name: 'Riverside Makers · #hardware' }, sources: 0, status_text: 'Starts when Leila or the seeder is back' },
    { id: 't4', direction: 'download', name: 'cnc-enclosure-v3.pdf', size: 24.6 * MB, state: 'done', progress: 1, route: 'Direct', route_kind: 'direct', route_note: 'From Maya and the seeder', where: { kind: 'server', name: 'Riverside Makers · #hardware' }, sources: 2, status_text: 'Done today 08:13 · verified' },
    { id: 't5', direction: 'download', name: 'panel-scan-full.tiff', size: 12.4 * 1024 * MB, state: 'failed', progress: 0, route: 'Refused', route_kind: 'refused', route_note: 'Nothing was downloaded', where: { kind: 'server', name: 'Riverside Makers · #hardware' }, sources: 0, status_text: 'Over the 10 GB limit', error: 'refused' },
  ];
  const guardianState = { threshold: 2, guardians: [GRACE, LINUS, RADIA].map((r) => ({ root_id: r, display_name: people[r], suffix: suffix(r), is_contact: true, shared_servers: 2 })) };
  const devices = [
    { device_id: id('dev-laptop'), name: 'Sami · laptop', this_device: true, added_ms: at(120, 0), last_seen_ms: Date.now() },
    { device_id: id('dev-phone'), name: 'Sami · phone', this_device: false, added_ms: at(40, 0), last_seen_ms: at(0, 25) },
  ];
  // The demo call is the "workshop" voice channel of Riverside Makers, with the design's six people (Part 6.3).
  const at2 = (h, m) => { const d = new Date(); d.setHours(h, m, 0, 0); return d.getTime(); };
  const CALL_PEOPLE = [
    { root_id: GRACE, display_name: 'Maya', muted: false, video: false, speaking: true },
    { root_id: LINUS, display_name: 'Jonas', muted: false, video: false, speaking: false, forwarder: 'Forwarding' },
    { root_id: RADIA, display_name: 'Leila', muted: true, video: false, speaking: false },
    { root_id: ME, display_name: 'Sami', muted: false, video: false, speaking: false },
    { root_id: KEN, display_name: 'Omar', muted: true, video: false, speaking: false },
    { root_id: RANA, display_name: 'Rana', muted: false, video: false, speaking: false },
  ];
  const CALL_MESSAGES = [
    { id: id('cm1'), root_id: GRACE, author_name: 'Maya', ms: at2(10, 2), text: 'Sharing the drawing so we’re all looking at the same thing.', file: { name: 'cnc-enclosure-v3.pdf', kind: 'PDF', meta: 'Saved · from Maya, direct' } },
    { id: id('cm2'), root_id: LINUS, author_name: 'Jonas', ms: at2(10, 4), text: 'Got it. Page 3 has the new duct size.\nLeila, you’re muted if you’re trying to talk.' },
    { id: id('cm3'), root_id: KEN, author_name: 'Omar', ms: at2(10, 5), text: 'Mic’s off on purpose, I’m on the train. Reading along here.' },
  ];
  servers.c1.voice = [{ id: 'vc1', name: 'workshop', participants: CALL_PEOPLE.filter((p) => p.root_id !== ME).map((p) => ({ ...p })) }];
  const callView = (scope, extra) => ({
    id: 'call1', scope, muted: false, deafened: false, camera: false, started_ms: Date.now(),
    channel_name: 'workshop', community_name: 'Riverside Makers',
    participants: CALL_PEOPLE.map((p) => ({ ...p })),
    messages: CALL_MESSAGES.map((m) => ({ ...m })),
    ...extra,
  });
  // Groups (Part 6.1): the GroupDM board's three groups. `system` items are membership lines, `creator` is a root id.
  const ONLINE = new Set([GRACE, LINUS, ME]);
  const groups = {
    grp1: {
      id: 'grp1', name: 'Saturday workshop crew', creator: GRACE, members: [GRACE, LINUS, ME, RADIA, KEN, RANA],
      // A live voice call the lobby and the sidebar label show (JoinCall board): 3 of 6 are in, forwarded by Jonas.
      live_call: {
        started_ms: Date.now() - 12 * 60 * 1000, started_by: 'Jonas', total: 6,
        route: { kind: 'forwarded', forwarder: 'Jonas', backup: 'Maya' },
        participants: [
          { root_id: LINUS, display_name: 'Jonas', muted: false, speaking: true, forwarder: true },
          { root_id: GRACE, display_name: 'Maya', muted: false, speaking: false },
          { root_id: KEN, display_name: 'Omar', muted: true, speaking: false },
        ],
        not_in: [{ root_id: RADIA, display_name: 'Leila' }, { root_id: RANA, display_name: 'Rana' }],
      },
      items: [
        { id: id('gs1'), system: true, from: GRACE, text: 'created the group with Jonas, Sami and Leila.', ms: at2(8, 40), pending: false },
        { id: id('g1'), from: GRACE, text: 'Group for Saturday logistics so #general stays quiet.', ms: at2(8, 41), pending: false },
        { id: id('gs2'), system: true, from: LINUS, text: 'added Omar and Rana.', ms: at2(8, 52), pending: false },
        { id: id('g2'), from: KEN, text: 'Thanks! I can cover the 3D printers.', ms: at2(8, 55), pending: false },
        { id: id('g3'), from: RANA, text: 'I’ll bring snacks and the extension leads.', ms: at2(9, 2), pending: false },
        { id: id('g4'), from: ME, text: 'Soldering kits are packed. Who has the shop key?', ms: at2(9, 21), pending: false },
      ],
    },
    grp2: { id: 'grp2', name: 'Relay operators', creator: NADIA, members: [NADIA, YUSUF, GRACE, LINUS, ME, RADIA, KEN, RANA, KARIM], items: [] },
    grp3: { id: 'grp3', name: 'Book club organisers', creator: RADIA, members: [RADIA, RANA, NADIA, ME], items: [] },
  };
  // The conversation list in the board's order: groups and direct messages interleaved.
  const BOARD_ORDER = ['group:grp1', LINUS, 'group:grp2', GRACE, 'group:grp3', RADIA, KEN, RANA, YUSUF, NADIA];
  const groupRow = (g) => ({
    kind: 'group', id: g.id, name: g.name, creator: g.creator,
    members: cpeers(g.members).map((m) => ({ ...m, online: ONLINE.has(m.root_id) })),
    last_text: g.items.at(-1)?.text ?? '', last_ms: g.items.at(-1)?.ms ?? 0, live_call: new URLSearchParams(location.search).has('nolive') ? null : (g.live_call ?? null),
  });
  const cpeers = (list) => list.map((r) => ({ root_id: r, display_name: people[r], avatar: pictures[r] ?? null, suffix: suffix(r) }));
  const findTransfer = (transferId) => transfers.find((x) => x.id === transferId);

  servers.c4 = { id: 'c4', hue: 350, name: 'Kinnly', owner: LINUS, channels: [{ id: 'ch9', name: 'general' }], roles: { [LINUS]: 'owner', [ME]: 'member' }, banned: new Set(), msgs: { ch9: [] } };

  // Typing: nobody types for real in the preview. The mock shows `typers` people from the roster, which cycles
  // 1, 2, 3, 5 and none every few seconds, until `window.__mockTypers(n)` sets the count by hand.
  const TYPERS = ['Maya', 'Jonas', 'Leila', 'Omar', 'Rana'];
  const TYPING_CYCLE = [1, 2, 3, 5, 0];
  let typers = 0;
  let typingManual = false;
  let typingStep = 0;
  const typingWatched = new Map(); // target key -> target, for each conversation the page has asked about
  const typingNames = () => TYPERS.slice(0, typers);
  const watchTyping = (target) => { typingWatched.set(JSON.stringify(target), target); return typingNames(); };
  const announceTyping = () => typingWatched.forEach((target) => emitTo('dissent://typing-changed', { target, names: typingNames() }));
  const setTypers = (n) => { typers = n; announceTyping(); };
  window.__mockTypers = (n) => { typingManual = true; setTypers(n); };
  setInterval(() => {
    if (typingManual) return;
    typingStep = (typingStep + 1) % TYPING_CYCLE.length;
    setTypers(TYPING_CYCLE[typingStep]);
  }, 4000);

  Object.assign(handlers, {
    typing: ({ target }) => { watchTyping(target); return null; },
    stop_typing: () => null,
    typing_in: ({ target }) => watchTyping(target),
  });

  Object.assign(handlers, {
    capabilities: () => (window.__mockNoCapabilities ? [] : [...FEATURES]),
    // Desktop notifications: the native plugin shows them in the app. The browser preview only logs them.
    'plugin:notification|notify': ({ options }) => { console.info('[mock] desktop notification', options); return null; },
    // Your status. set_status emits dissent://status-changed the way the engine will; a timed status reverts to online.
    get_status: () => mockStatus,
    // Auto-idle input report: the mock has no idle timer, so it does nothing.
    record_activity: () => null,
    set_status: ({ status, untilMs }) => {
      clearTimeout(mockStatusTimer);
      mockStatus = { status, until_ms: untilMs ?? null };
      if (untilMs) {
        mockStatusTimer = setTimeout(() => {
          mockStatus = { status: 'online', until_ms: null };
          emitTo('dissent://status-changed', mockStatus);
        }, Math.max(0, untilMs - Date.now()));
      }
      emitTo('dissent://status-changed', mockStatus);
      return null;
    },
    list_conversations: () => {
      const rows = [
        ...conversations().map((c) => ({ kind: 'dm', ...c, last_text: c.peer === NADIA ? 'Running a seeder for Book club' : '', online: c.peer !== LINUS, status: MEMBER_STATUS[c.peer] })),
        ...Object.values(groups).map(groupRow),
      ];
      const rank = (r) => { const i = BOARD_ORDER.indexOf(r.kind === 'group' ? `group:${r.id}` : r.peer); return i < 0 ? 99 : i; };
      return rows.sort((x, y) => rank(x) - rank(y));
    },
    create_group: ({ name, peers }) => {
      const gid = `grp${++seq}`;
      groups[gid] = { id: gid, name, creator: ME, members: [ME, ...peers], items: [] };
      return groupRow(groups[gid]);
    },
    group_messages: ({ groupId }) => groups[groupId].items.map((m) => dmItem(m, groups[groupId].items)),
    send_group_message: ({ groupId, text }) => {
      const item = { id: id(`g${++seq}`), from: ME, text, ms: Date.now(), pending: false, reactions: [] };
      groups[groupId].items.push(item);
      return dmItem(item, groups[groupId].items);
    },
    group_reply: ({ groupId, messageId, text }) => dmReplyIn(groups[groupId].items, 'g', messageId, text),
    group_react: ({ groupId, messageId, emoji }) => dmReactIn(groups[groupId].items, messageId, emoji, true),
    group_unreact: ({ groupId, messageId, emoji }) => dmReactIn(groups[groupId].items, messageId, emoji, false),
    group_edit: ({ groupId, messageId, text }) => dmEditIn(groups[groupId].items, messageId, text),
    group_delete: ({ groupId, messageId }) => dmDeleteIn(groups[groupId].items, messageId),
    add_group_members: ({ groupId, peers }) => { groups[groupId].members.push(...peers); return null; },
    leave_group: ({ groupId }) => { delete groups[groupId]; return null; },
    group_invite_link: ({ groupId }) => `dissent://group/${groupId}`,
    list_contacts: () => ({
      incoming: cpeers([...incomingSet]).map((p) => ({ ...p, online: p.root_id === NADIA, shared_servers: [] })),
      outgoing: cpeers([...outgoingSet]).map((p) => ({ ...p, online: false, shared_servers: [] })),
      // The Friends mix: Maya online (and in a call), Jonas idle, Omar Do Not Disturb, Leila offline.
      contacts: cpeers([...contactSet]).map((p) => ({ ...p, status: MEMBER_STATUS[p.root_id], online: MEMBER_STATUS[p.root_id] !== 'offline', shared_servers: ['Riverside Makers'] })),
      blocked: [],
    }),
    send_contact_request: ({ link }) => {
      const known = byAddress(link);
      if (known) {
        outgoingSet.add(known);
        return null;
      }
      need(link.startsWith('dissent://contact/'), 'Not a contact link');
      outgoingSet.add(id(`pending-${link.slice(-8)}`));
      return null;
    },
    accept_contact: ({ rootId }) => { incomingSet.delete(rootId); contactSet.add(rootId); return null; },
    decline_contact: ({ rootId }) => { incomingSet.delete(rootId); return null; },
    cancel_contact_request: ({ rootId }) => { outgoingSet.delete(rootId); return null; },
    remove_contact: ({ rootId }) => { contactSet.delete(rootId); return null; },
    block_user: ({ rootId }) => { contactSet.delete(rootId); return null; },
    unblock_user: () => null,
    close_conversation: () => null,
    ignore_user: ({ user }) => { ignoredSet.add(user); return null; },
    unignore_user: ({ user }) => { ignoredSet.delete(user); return null; },
    ignored_users: () => [...ignoredSet],
    peer_contact_link: ({ rootId }) => `dissent://contact/${suffix(rootId)}`,
    set_note: () => null,
    start_call: ({ peer }) => {
      // The call rings until the callee answers. The mock answers after four seconds, so ringing and the join sound both show.
      const started = callView('dm', { peer, participants: [], route: { kind: 'direct', title: `Calling ${people[peer]}`, detail: 'Ringing. Peer to peer, end-to-end encrypted' } });
      calls.current = started;
      setTimeout(() => {
        if (calls.current !== started) return;
        started.participants = [{ root_id: peer, display_name: people[peer], muted: false, speaking: false }];
        emitTo('dissent://call-state', { ...started });
      }, 4000);
      return { ...calls.current };
    },
    join_call: ({ communityId, channelId }) => {
      const c = servers[communityId];
      const ch = c?.channels.find((x) => x.id === channelId && x.voice);
      need(ch, 'not a voice channel');
      // The workshop channel is the demo call, forwarded by Jonas. Other channels start with only this device.
      const demo = channelId === 'vc1';
      calls.current = callView('channel', {
        channel_name: ch.name,
        community_name: c.name,
        participants: demo ? CALL_PEOPLE.filter((p) => p.root_id !== ME).map((p) => ({ ...p })) : [],
        route: demo
          ? { kind: 'forwarded', title: 'Forwarded by Jonas', detail: 'Their computer passes everyone’s sealed streams along and can’t open them.', forwarder: 'Jonas', backup: 'Leila' }
          : { kind: 'direct', title: `#${ch.name}`, detail: 'Peer to peer, end-to-end encrypted' },
      });
      return { ...calls.current };
    },
    leave_call: () => { calls.current = null; return null; },
    call_state: () => (calls.current ? { ...calls.current } : null),
    // Ringing: the answer and decline commands. A ringing group call is the one the incoming board shows.
    accept_call: ({ callId }) => { if (callId === 'call-grp1') calls.current = callFor('grp1'); return null; },
    decline_call: () => null,
    // Microphone and speaker. Null means the system default. Desktop's audio_devices and set_audio_devices shape.
    audio_devices: () => ({
      inputs: [
        { name: 'Microphone (USB Audio)', is_default: true, subtitle: 'Logitech', kind: 'headset' },
        { name: 'Headset microphone', is_default: false, subtitle: null, kind: 'headset' },
      ],
      outputs: [
        { name: 'Speakers (Realtek)', is_default: true, subtitle: 'Realtek', kind: 'other' },
        { name: 'Headphones', is_default: false, subtitle: null, kind: 'headset' },
        { name: 'Display Audio (NVIDIA High Definition)', is_default: false, subtitle: 'NVIDIA', kind: 'display' },
      ],
      default_input: 'Microphone (USB Audio)', default_output: 'Speakers (Realtek)',
      input: window.__mockAudio?.input ?? null, output: window.__mockAudio?.output ?? null,
    }),
    set_audio_devices: ({ input, output }) => { window.__mockAudio = { input, output }; return null; },
    voice_settings: () => ({ ...mockVoice }),
    set_voice_settings: ({ settings }) => { mockVoice = { ...settings }; return null; },
    // The mic test: a fake level that swings up and down, sent every 50 ms while the test runs.
    mic_test_start: () => {
      if (!mockMicTimer) {
        mockMicTimer = setInterval(() => {
          mockMicTick += 1;
          const db = -62 + 40 * Math.abs(Math.sin(mockMicTick / 6));
          const threshold = mockVoice.auto_sensitivity ? -48 : mockVoice.threshold_db;
          emitTo('dissent://mic-level', { db, open: db >= threshold, threshold_db: threshold });
        }, 50);
      }
      return null;
    },
    mic_test_stop: () => { clearInterval(mockMicTimer); mockMicTimer = null; return null; },
    store_stats: () => ({ used_bytes: 1.4 * 1024 ** 3, files: 6, budget_bytes: 10 * 1024 ** 3, shared_bytes: 0.6 * 1024 ** 3, downloaded_bytes: 0.5 * 1024 ** 3, partial_bytes: 0.1 * 1024 ** 3 }),
    join_group_call: ({ groupId }) => (calls.current = callFor(groupId)),
    join_dm_call: ({ peer }) => (calls.current = callFor(peer)),
    set_muted: ({ muted }) => { if (calls.current) calls.current.muted = muted; return null; },
    set_deafened: ({ deafened }) => { if (calls.current) calls.current.deafened = deafened; return null; },
    set_camera: ({ on }) => { if (calls.current) calls.current.camera = on; return null; },
    send_call_message: ({ text }) => {
      if (calls.current) calls.current.messages.push({ id: id(`cm${++seq}`), root_id: ME, author_name: 'Sami', ms: Date.now(), text });
      return null;
    },
    list_transfers: () => transfers.map((t) => ({ ...t })),
    pause_transfer: ({ transferId }) => { const t = findTransfer(transferId); if (t) t.state = 'paused'; return null; },
    resume_transfer: ({ transferId }) => { const t = findTransfer(transferId); if (t) t.state = 'active'; return null; },
    cancel_transfer: ({ transferId }) => { const t = findTransfer(transferId); if (t) t.state = 'failed'; return null; },
    retry_transfer: ({ transferId }) => { const t = findTransfer(transferId); if (t) t.state = 'active'; return null; },
    remove_transfer: ({ transferId }) => { const i = transfers.findIndex((x) => x.id === transferId); if (i >= 0) transfers.splice(i, 1); return null; },
    transfer_settings: () => ({ max_accept_bytes: 10 * GB, download_dir: 'C:/Users/ada/Downloads/Dissent', auto_accept_bytes: 25 * MB }),
    set_transfer_settings: ({ settings }) => settings,
    pick_download_dir: () => 'C:/Users/ada/Downloads/Dissent',
    list_guardians: () => ({ threshold: guardianState.threshold, guardians: guardianState.guardians.map((g) => ({ ...g })) }),
    sign_guardians: ({ guardians, threshold }) => {
      need(threshold >= 1 && threshold <= guardians.length, 'The threshold must be between 1 and the number of guardians');
      guardianState.guardians = cpeers(guardians).map((p) => ({ ...p, is_contact: contactSet.has(p.root_id), shared_servers: 1 }));
      guardianState.threshold = threshold;
      return { threshold, guardians: guardianState.guardians.map((g) => ({ ...g })) };
    },
    list_devices: () => devices.map((d) => ({ ...d })),
    revoke_device: ({ deviceId }) => {
      const i = devices.findIndex((d) => d.device_id === deviceId);
      if (i >= 0) devices.splice(i, 1);
      return devices.map((d) => ({ ...d }));
    },
    link_device_code: () => 'dissent://link/mock-device',
    mark_safety_verified: () => null,
    approve_device: ({ link }) => { need(link.startsWith('dissent://'), 'Not a device link'); return 'dissent://grant/mock-' + suffix(ME); },
    finish_device_link: () => null,
    pinned_conversations: () => [],
    read_markers: () => ({}),
    pin_message: () => null,
    pinned_messages: () => [],
    pin_conversation: () => null,
    mark_read: () => null,
  });

  Object.assign(dms, {
    [KEN]: [{ id: id(`d${++seq}`), from: KEN, text: 'Thanks!', ms: at(0, 90), pending: false }],
    [RADIA]: [{ id: id(`d${++seq}`), from: RADIA, text: 'Sent you the invite link', ms: at(0, 200), pending: false }],
    [RANA]: [{ id: id(`d${++seq}`), from: RANA, text: 'See you there', ms: at(0, 300), pending: false }],
  });
  dms[LINUS].push({ id: id(`d${++seq}`), from: LINUS, text: 'Seeder logs look clean now', ms: at(0, 20), pending: false });
  Object.assign(dms, {
    [NADIA]: [{ id: id(`d${++seq}`), from: NADIA, text: 'Running a seeder for Book club', ms: at(0, 400), pending: false }],
    [YUSUF]: [{ id: id(`d${++seq}`), from: YUSUF, text: 'Sent you the invite link', ms: at(0, 500), pending: false }],
  });
  if (!PLAIN_DM) dms[GRACE].push({ id: id(`d${++seq}`), from: GRACE, text: 'Peer-forwarded call: failover', ms: at(0, 1), pending: false, embeds: [{ kind: 'card', title: 'Peer-forwarded call: failover', subtitle: '20 participants · 3 speakers · audio', tile: { color: '#e8553a', label: 'Call capacity' } }] });

  // Screenshot helpers: post a message into any channel or DM, as if another person sent it.
  // A call built from a group's members, for the join and accept handlers.
  const callFor = (groupId) => ({
    id: `call-${groupId}`, scope: 'dm', peer: groupId, muted: false, deafened: false, camera: false, started_ms: Date.now(),
    route: { kind: 'direct', title: groups[groupId]?.name ?? 'Call' },
    participants: (groups[groupId]?.members ?? []).filter((r) => r !== ME).map((r) => ({ root_id: r, display_name: people[r], muted: false, speaking: false, video: false })),
    messages: [],
  });
  // Ringing: emits a ringing call the way the desktop will (dissent://call-ringing).
  window.__mockRing = (scope = 'group') => {
    const payload = scope === 'group'
      ? { call_id: 'call-grp1', scope: 'group', group_id: 'grp1', name: 'Saturday workshop crew', caller: { root_id: LINUS, display_name: 'Jonas' }, verified: false, in_call: 3, total: 6,
          members_in: [{ root_id: LINUS, display_name: 'Jonas' }, { root_id: GRACE, display_name: 'Maya' }, { root_id: KEN, display_name: 'Omar' }] }
      : { call_id: 'call-maya', scope: 'dm', peer: GRACE, name: 'Maya', caller: { root_id: GRACE, display_name: 'Maya' }, verified: true, in_call: 1, total: 2, members_in: [] };
    emitTo('dissent://call-ringing', payload);
  };
  window.__mockPost = (communityId, channelId, author, text) => {
    const m = { ...msg(0, 0, author, text), ms: Date.now() };
    servers[communityId].msgs[channelId].push(m);
    emit({ kind: 'message', community_id: communityId, channel_id: channelId });
  };
  window.__mockDm = (peer, text) => {
    (dms[peer] ??= []).push({ id: id(`d${++seq}`), from: peer, text, ms: Date.now(), pending: false });
    emit({ kind: 'dm', peer });
  };

  // Sound previews. Each helper makes the event the engine would send, so the join, leave, missed and group sounds play.
  // Participants are replaced as a new array, the way the engine's serialised state arrives.
  window.__mockCallPeople = (change = 'join') => {
    if (!calls.current) return;
    const others = calls.current.participants.filter((p) => p.root_id !== RADIA);
    calls.current.participants = change === 'join'
      ? [...others, { root_id: RADIA, display_name: people[RADIA], muted: false, speaking: false }]
      : others;
    emitTo('dissent://call-state', { ...calls.current });
  };
  window.__mockCallMissed = () => emit({ kind: 'call_missed', call_id: 'call-maya', from: GRACE });
  // Driver hook: sets a channel's content visibility on the mock server, as an owner's ChannelUpdate would.
  window.__mockVisibility = (communityId, channelId, visibility) => {
    const ch = servers[communityId].channels.find((x) => x.id === channelId);
    if (ch) ch.visibility = visibility;
  };
  window.__mockGroupPost = (groupId, from, text) => {
    groups[groupId].items.push({ id: id(`g${++seq}`), from, text, ms: Date.now(), pending: false });
    emit({ kind: 'group', group_id: groupId });
  };

  // Drag and drop: the desktop backend sends dissent://drag-over, dissent://drag-leave and dissent://files-dropped.
  // In the browser the same events are made from the HTML5 drag events, with File metadata as the payload.
  const dragHasFiles = (e) => [...(e.dataTransfer?.types ?? [])].includes('Files');
  document.addEventListener('dragenter', (e) => { if (dragHasFiles(e)) emitTo('dissent://drag-over', null); });
  document.addEventListener('dragover', (e) => { if (dragHasFiles(e)) { e.preventDefault(); emitTo('dissent://drag-over', null); } });
  document.addEventListener('dragleave', (e) => { if (dragHasFiles(e) && !e.relatedTarget) emitTo('dissent://drag-leave', null); });
  document.addEventListener('drop', (e) => {
    if (!dragHasFiles(e)) return;
    e.preventDefault();
    emitTo('dissent://files-dropped', [...e.dataTransfer.files].map((f) => ({ path: `C:/Users/ada/Desktop/${f.name}`, name: f.name, size: f.size, mime: f.type || 'application/octet-stream' })));
    emitTo('dissent://drag-leave', null);
  });

  // The mock reports an update shortly after load, so the header icon is visible in previews.
  setTimeout(() => {
    if (!window.__mockNoUpdate) emitTo('dissent://update-available', { version: '0.2.0', current_version: '0.1.0', date: '2026-10-24', notes: '- Faster sync for large servers\n- Fixed relay settings on restart' });
  }, 1500);

  window.__TAURI__ = {
    core: {
      invoke: async (cmd, args = {}) => {
        const handler = handlers[cmd];
        if (!handler) throw `mock: unknown command ${cmd}`;
        return handler(args);
      },
      // Tauri turns a local path into an asset: URL. The mock's "paths" for images are already data URLs.
      convertFileSrc: (path) => (String(path).startsWith('data:image/png;base64,') ? path : `asset://localhost/${encodeURIComponent(path)}`),
    },
    event: {
      listen: async (name, cb) => {
        const entry = { name, cb };
        listeners.push(entry);
        return () => listeners.splice(listeners.indexOf(entry), 1);
      },
    },
  };
}
