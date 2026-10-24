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

  // #off-topic shows the two broken reply cases: the original was deleted, or it is sealed for this device.
  function offTopicMessages() {
    const gone = msg(0, 40, RADIA, 'Deleted joke');
    gone.deleted = true;
    const reply = msg(0, 30, KEN, 'Anyone else tracking the weather radar in the background?');
    reply.replyTo = gone.id;
    const sealed = msg(0, 12, RADIA, 'Still cannot read the old ones since I rejoined.');
    sealed.replyTo = id('sealed-elsewhere');
    return [gone, reply, sealed];
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

  const dms = {
    [GRACE]: [
      { id: id(`d${++seq}`), from: GRACE, text: 'Can you review my PR when you get a minute?', ms: at(1, 400), pending: false },
      { id: id(`d${++seq}`), from: ME, text: 'Yes, reading it now.', ms: at(1, 380), pending: false },
      { id: id(`d${++seq}`), from: GRACE, text: 'Thanks. The diff is mostly the replay tests.', ms: at(0, 40), pending: false },
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
  });
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
    dm_messages: ({ peer, limit }) =>
      (dms[peer] ?? []).slice(-limit).map((m) => ({ id: m.id, from: m.from, text: m.text, ms: m.ms, pending: m.pending })),
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
