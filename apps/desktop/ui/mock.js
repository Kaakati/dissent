// Dev-only stand-in for the Tauri backend so ui/index.html opens in a plain browser.
// Does nothing inside the desktop app, where window.__TAURI__ is injected before this module runs.
if (!window.__TAURI__) {
  window.__DISSENT_MOCK__ = true;

  let seq = 0;
  const id = (name) => name.padEnd(52, 'a').slice(0, 52);
  const ME = id('mealice');
  const GRACE = id('gracehopper');
  const LINUS = id('linustorvalds');
  const KEN = id('kenthompson');
  const people = { [ME]: 'Ada', [GRACE]: 'Grace', [LINUS]: 'Linus', [KEN]: 'Ken' };
  const suffix = (root) => root.slice(0, 8);

  const servers = {
    c1: {
      id: 'c1',
      name: 'Rust Chat',
      owner: GRACE,
      channels: [{ id: 'ch1', name: 'general' }, { id: 'ch2', name: 'random' }],
      roles: { [GRACE]: 'owner', [ME]: 'member', [LINUS]: 'admin', [KEN]: 'moderator' },
      banned: new Set(),
      msgs: {
        ch1: [{ id: id('m1'), author: GRACE, text: 'Welcome to Rust Chat.', ms: Date.now() - 3600e3, edited: false, deleted: false }],
        ch2: [],
      },
    },
    c2: {
      id: 'c2',
      name: 'Book Club',
      owner: ME,
      channels: [{ id: 'ch3', name: 'general' }],
      roles: { [ME]: 'owner', [KEN]: 'member' },
      banned: new Set(),
      msgs: { ch3: [] },
    },
  };
  const dms = {
    [GRACE]: [{ id: id('d1'), from: GRACE, text: 'Can you review my PR?', ms: Date.now() - 7200e3, pending: false }],
  };

  const listeners = [];
  const emit = (payload) => listeners.forEach((cb) => cb({ payload }));
  const roleOf = (c, user) => c.roles[user] ?? null;
  const myRole = (c) => roleOf(c, ME);
  const canMod = (c) => ['owner', 'admin', 'moderator'].includes(myRole(c));
  const summary = (c) => ({ id: c.id, name: c.name, owner: c.owner, my_role: myRole(c) });
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
      handle: null,
      role: c.roles[r],
      banned: c.banned.has(r),
    })),
  });
  const messageItem = (m) => ({
    id: m.id,
    author: m.author,
    author_name: people[m.author],
    text: m.deleted ? '' : m.text,
    ms: m.ms,
    edited: m.edited,
    deleted: m.deleted,
  });
  const findMessage = (c, channelId, messageId) => (c.msgs[channelId] ?? []).find((m) => m.id === messageId);
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
    community_view: ({ communityId }) => view(servers[communityId]),
    create_channel: ({ communityId, name }) => {
      const c = servers[communityId];
      need(['owner', 'admin'].includes(myRole(c)), 'Only owner or admin can create channels');
      const ch = { id: `ch${++seq}`, name };
      c.channels.push(ch);
      c.msgs[ch.id] = [];
      return ch;
    },
    channel_messages: ({ communityId, channelId, limit }) =>
      (servers[communityId].msgs[channelId] ?? []).slice(-limit).map(messageItem),
    send_message: ({ communityId, channelId, text }) => {
      const m = { id: id(`m${++seq}`), author: ME, text, ms: Date.now(), edited: false, deleted: false };
      servers[communityId].msgs[channelId].push(m);
      return messageItem(m);
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
    claim_handle: () => null,
    user_profile: ({ user }) => ({
      root_id: user,
      suffix: suffix(user),
      display_name: people[user] ?? `anon·${suffix(user)}`,
      is_contact: Boolean(dms[user]),
      handles: Object.values(servers)
        .filter((c) => c.roles[user] && c.roles[ME])
        .map((c) => ({ community_id: c.id, community_name: c.name, handle: null })),
      mutual_communities: Object.values(servers)
        .filter((c) => c.roles[user] && c.roles[ME])
        .map((c) => ({ id: c.id, name: c.name })),
    }),

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
    net_status: () => ({
      dht: { size_estimate: 9200000, confidence: 0.82, firewalled: false },
      peers: 3,
      relay: 'https://use1-1.relay.n0.iroh.network./',
    }),
    identity_export_phrase: () => 'abandon abandon abandon abandon abandon abandon abandon abandon abandon abandon abandon abandon abandon abandon abandon abandon abandon abandon abandon abandon abandon abandon abandon art',
    identity_import_phrase: () => null,
  };

  window.__TAURI__ = {
    core: {
      invoke: async (cmd, args = {}) => {
        const handler = handlers[cmd];
        if (!handler) throw `mock: unknown command ${cmd}`;
        return handler(args);
      },
    },
    event: {
      listen: async (_name, cb) => {
        listeners.push(cb);
        return () => listeners.splice(listeners.indexOf(cb), 1);
      },
    },
  };
}
