// Dev-only stand-in for the Tauri backend so ui/index.html opens in a plain browser.
// Does nothing inside the desktop app, where window.__TAURI__ is injected before this module runs.
if (!window.__TAURI__) {
  window.__DISSENT_MOCK__ = true;

  let seq = 0;
  const id = (name) => name.padEnd(52, 'a').slice(0, 52);
  const MIN = 60 * 1000;
  const DAY = 24 * 60 * MIN;
  const now = Date.now();

  const ME = id('mz7q4p2cmealice');
  const GRACE = id('k3f9q2xagrace');
  const LINUS = id('w8r2d6nblinus');
  const KEN = id('h5t7c3vyken');
  const RADIA = id('e4j9m1sqradia');
  const people = { [ME]: 'Ada', [GRACE]: 'Grace', [LINUS]: 'Linus', [KEN]: 'Ken', [RADIA]: 'Radia' };
  const handles = { [GRACE]: 'grace', [LINUS]: 'torvalds' };
  const suffix = (root) => root.slice(0, 8);

  // Messages are written as [minutes ago, author, text]. Gaps longer than 7 minutes start a new group.
  // Reactions are kept as { emoji, users } and replies as the original's id; messageItem shapes them for the UI.
  const at = (daysAgo, minutesAgo) => now - daysAgo * DAY - minutesAgo * MIN;
  const msg = (daysAgo, minutesAgo, author, text) => ({
    id: id(`m${++seq}`), author, text, ms: at(daysAgo, minutesAgo), edited: false, deleted: false, reactions: [], replyTo: null,
  });

  const generalMessages = [
    msg(3, 300, GRACE, 'Welcome to Rust Chat. Read the pinned rules before posting, please.'),
    msg(3, 296, GRACE, 'Help channel is for questions. Announcements are for releases only.'),
    msg(3, 180, LINUS, 'Anyone tried the new async trait stabilisation yet?'),
    msg(2, 600, KEN, 'Yes, it compiled first try on my side. Nice.'),
    msg(2, 598, KEN, 'The error messages got a lot clearer too.'),
    msg(2, 590, ME, 'Agreed. I was worried about the lifetimes but they just work now.'),
    msg(2, 455, LINUS, 'The reference page for it is up: https://doc.rust-lang.org/stable/releases.html'),
    msg(1, 900, RADIA, 'Has anyone benchmarked the new allocator against the old one?'),
    msg(1, 880, RADIA, 'I got about 8% on the parser benchmark. Numbers in the gist.'),
    msg(1, 860, GRACE, 'Please post the gist link here so people can reproduce it.'),
    msg(1, 200, ME, 'Working on the event log replay tonight. Hopefully done by Friday.'),
    msg(0, 90, KEN, 'Morning. Did the nightly build go out?'),
    msg(0, 88, KEN, 'It failed on the Windows runner again.'),
    msg(0, 86, LINUS, 'Known flake, retrying now.'),
    msg(0, 20, ME, 'Retry passed. Tagging the release candidate after lunch.'),
    msg(0, 4, GRACE, 'Thanks all. Changelog is in the repo under docs/.'),
  ];
  // Sample replies and reactions on #general.
  generalMessages[5].replyTo = generalMessages[3].id; // Ada answers Ken's "Yes, it compiled first try"
  generalMessages[9].replyTo = generalMessages[8].id; // Grace asks for the gist link
  generalMessages[6].reactions = [{ emoji: '👍', users: [KEN, ME, RADIA] }, { emoji: '🔥', users: [RADIA] }];
  generalMessages[15].reactions = [{ emoji: '🙏', users: [KEN, LINUS, RADIA, ME] }];
  generalMessages[13].reactions = [{ emoji: '😅', users: [LINUS] }];

  // ---- file attachments (mock): a generated PNG stands in for a decrypted image on disk ----
  const MB = 1024 * 1024;
  const att = (name, size, mime, state = 'available', extra = {}) => ({ id: id(`a${++seq}`), name, size, mime, state, progress: null, local_path: null, ...extra });

  // #off-topic shows the two broken reply cases: the original was deleted, or it is sealed for this device.
  function offTopicMessages() {
    const gone = msg(0, 40, RADIA, 'Deleted joke');
    gone.deleted = true;
    const reply = msg(0, 30, KEN, 'Anyone else tracking the weather radar in the background?');
    reply.replyTo = gone.id;
    const sealed = msg(0, 12, RADIA, 'Still cannot read the old ones since I rejoined.');
    sealed.replyTo = id('sealed-elsewhere');
    const trap = msg(0, 6, KEN, 'Here is the invoice you asked for.');
    trap.attachments = [att('invoice.pdf.exe', 2.3 * MB, 'application/x-msdownload', 'downloaded', { local_path: 'C:/Users/ada/Downloads/invoice.pdf.exe' }), att('diagram.svg', 14 * 1024, 'image/svg+xml')];
    return [gone, reply, sealed, trap];
  }

  const servers = {
    c1: {
      id: 'c1',
      name: 'Rust Chat',
      owner: GRACE,
      channels: [
        { id: 'ch1', name: 'general' },
        { id: 'ch2', name: 'help' },
        { id: 'ch3', name: 'announcements' },
        { id: 'ch4', name: 'off-topic' },
      ],
      roles: { [GRACE]: 'owner', [ME]: 'member', [LINUS]: 'admin', [KEN]: 'moderator', [RADIA]: 'member' },
      banned: new Set(),
      msgs: {
        ch1: generalMessages,
        ch2: [msg(1, 300, RADIA, 'How do I borrow from a struct field inside a closure?'), msg(1, 298, GRACE, 'Capture the field by reference, or use a local binding.')],
        ch3: [msg(5, 0, GRACE, 'Dissent 0.1 is tagged. Release notes are in the announcements post.')],
        ch4: offTopicMessages(),
      },
    },
    c2: {
      id: 'c2',
      name: 'Book Club',
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
      name: 'Field Notes',
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
  generalMessages[8].attachments = [att('parser-bench.png', 1.7 * MB, 'image/png', 'downloaded', { local_path: mockImage(640, 420, 150, 'parser benchmark') }), att('bench-results.csv', 48 * 1024, 'text/csv')];
  generalMessages[0].attachments = [att('rules.pdf', 180 * 1024, 'application/pdf', 'downloaded', { local_path: 'C:/Users/ada/Downloads/rules.pdf' })];
  generalMessages[12].attachments = [att('runner-log.txt', 1.2 * MB, 'text/plain')];
  const GB = 1024 * MB;
  generalMessages[13].attachments = [
    att('ci-snapshot.zip', 24 * MB, 'application/zip'),
    att('photos.zip', 12.4 * GB, 'application/zip', 'too_large'),
    att('raw-capture.mkv', 9.8 * GB, 'video/x-matroska', 'paused', { progress: 0.12 }),
    att('dataset-2026.tar', 9.6 * GB, 'application/x-tar', 'disk_full', { needed: 19.6 * GB }),
  ];
  generalMessages[15].attachments = [att('screenshot.png', 1.1 * MB, 'image/png')];

  const dms = {
    [GRACE]: [
      { id: id(`d${++seq}`), from: GRACE, text: 'Can you review my PR when you get a minute?', ms: at(1, 400), pending: false },
      { id: id(`d${++seq}`), from: ME, text: 'Yes, reading it now.', ms: at(1, 380), pending: false },
      { id: id(`d${++seq}`), from: GRACE, text: 'Thanks. The diff is mostly the replay tests.', ms: at(0, 40), pending: false, attachments: [att('replay-flow.png', 640 * 1024, 'image/png', 'downloaded', { local_path: mockImage(640, 420, 330, 'replay flow') }), att('notes.md', 12 * 1024, 'text/markdown', 'failed')] },
    ],
    [LINUS]: [
      { id: id(`d${++seq}`), from: LINUS, text: 'Did you see the kernel mailing list thread? https://lore.kernel.org/', ms: at(2, 120), pending: false },
    ],
  };

  let mockNetwork = { use_public_relays: true, relays: [] };
  const listeners = [];
  // Named events, like Tauri's. Engine events use the 'dissent://event' name.
  const emitTo = (name, payload) => listeners.filter((l) => l.name === name).forEach((l) => l.cb({ payload }));
  const emit = (payload) => emitTo('dissent://event', payload);
  const UPDATE = { version: '0.2.0', current_version: '0.1.0', date: '2026-10-24', notes: '- Faster sync for large servers\n- Fixed relay settings on restart\n- Message search keeps focus while typing' };
  const roleOf = (c, user) => c.roles[user] ?? null;
  const myRole = (c) => roleOf(c, ME);
  const canMod = (c) => ['owner', 'admin', 'moderator'].includes(myRole(c));
  const summary = (c) => ({ id: c.id, name: c.name, owner: c.owner, my_role: myRole(c) });
  const sharedServers = (user) => Object.values(servers).filter((c) => c.roles[user] && c.roles[ME]);
  const conversations = () =>
    Object.entries(dms)
      .map(([peer, items]) => {
        const last = items[items.length - 1];
        return { peer, display_name: people[peer], last_text: last.text, last_ms: last.ms };
      })
      .sort((a, b) => b.last_ms - a.last_ms);
  const view = (c) => ({
    id: c.id,
    name: c.name,
    owner: c.owner,
    my_role: myRole(c),
    channels: c.channels,
    members: Object.keys(c.roles).map((r) => ({
      root_id: r,
      suffix: suffix(r),
      display_name: people[r],
      handle: handles[r] ?? null,
      role: c.roles[r],
      banned: c.banned.has(r),
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
  const dmItem = (m) => ({
    id: m.id, from: m.from, text: m.text, ms: m.ms, pending: m.pending,
    ...(window.__mockNoFiles ? {} : { attachments: (m.attachments ?? []).map((a) => ({ ...a })) }),
  });
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

  const handlers = {
    me: () => ({ root_id: ME, device_id: id('medevice'), suffix: suffix(ME), display_name: people[ME] }),
    set_display_name: ({ name }) => {
      people[ME] = name;
      return null;
    },
    contact_link: () => `dissent://contact/mock-${suffix(ME)}`,
    add_contact: ({ link }) => {
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
    create_invite: ({ communityId }) => `dissent://invite/mock-${communityId}`,
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
    send_files: ({ communityId, channelId, paths, text }) => {
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
        is_contact: Boolean(dms[user]),
        handles: shared.map((c) => ({ community_id: c.id, community_name: c.name, handle: handles[user] ?? null })),
        mutual_communities: shared.map((c) => ({ id: c.id, name: c.name })),
      };
    },

    list_conversations: () => conversations(),
    dm_messages: ({ peer, limit }) => (dms[peer] ?? []).slice(-limit).map(dmItem),
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
    identity_export_phrase: () => 'abandon abandon abandon abandon abandon abandon abandon abandon abandon abandon abandon abandon abandon abandon abandon abandon abandon abandon abandon abandon abandon abandon abandon art',
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

  // Screenshot helpers: post a message into any channel or DM, as if another person sent it.
  window.__mockPost = (communityId, channelId, author, text) => {
    const m = { ...msg(0, 0, author, text), ms: Date.now() };
    servers[communityId].msgs[channelId].push(m);
    emit({ kind: 'message', community_id: communityId, channel_id: channelId });
  };
  window.__mockDm = (peer, text) => {
    (dms[peer] ??= []).push({ id: id(`d${++seq}`), from: peer, text, ms: Date.now(), pending: false });
    emit({ kind: 'dm', peer });
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
