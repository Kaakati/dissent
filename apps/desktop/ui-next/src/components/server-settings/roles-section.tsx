// Server Settings, Roles: custom roles with permission bits, who holds them, and bots. Built-in Admin and Moderator stay
// as they are; custom roles rank below them. A role's bits count on everything once the owner turns on "Roles grant
// permissions", which the engine allows only when every member's app knows custom roles.
import { useCallback, useEffect, useState } from 'react';
import { Bot, Plus, Trash2 } from 'lucide-react';
import { BLOCK_TITLE, Block, CHECK_CLASS, CheckRow, Hint } from '@/components/settings/settings-ui';
import { Button } from '@/components/ui/button';
import { Input } from '@/components/ui/input';
import { copyText } from '@/lib/clipboard';
import { confirmGate, gateSummary } from '@/lib/gate';
import { PERM_LABELS, rolesApi, type RoleDraft, type RolesView, type RoleView } from '@/lib/roles';
import { useShell } from '@/lib/store';
import { toast } from '@/lib/toasts';
import type { CommunityView, GateStatus, Id } from '@/lib/types';
import { cn } from '@/lib/utils';

const MAX_NAME = 32; // the engine's limit, in characters

const draftOf = (r: RoleView | null, position: number): RoleDraft =>
  r ? { name: r.name, color: r.color, perms: [...r.perms], position: r.position } : { name: '', color: '', perms: [], position };

export function RolesSection({ community }: { community: CommunityView }) {
  const [view, setView] = useState<RolesView | null>(null);
  const [selected, setSelected] = useState<Id | 'new' | null>(null);
  const [token, setToken] = useState<string | null>(null);
  const [tick, setTick] = useState(0);
  const reload = useCallback(async () => setTick((n) => n + 1), []);
  // Reloads on every change of the community, since a role change arrives as a community update.
  useEffect(() => {
    let live = true;
    rolesApi
      .view(community.id)
      .then((v) => {
        if (live) setView(v);
      })
      .catch((err: unknown) => {
        if (live) toast(err);
      });
    return () => {
      live = false;
    };
  }, [community, tick]);

  if (!view) return <Hint>Loading roles…</Hint>;
  const canManage = view.my_perms.includes('manage_roles');
  const role = selected && selected !== 'new' ? (view.roles.find((r) => r.id === selected) ?? null) : null;
  // A new role goes just under this person's ceiling, or above the current top for moderators and above.
  const top = view.roles.reduce((m, r) => Math.max(m, r.position), 0);
  const nextPosition = view.my_ceiling === null ? top + 1 : Math.max(0, view.my_ceiling - 1);
  const mine = (r: RoleView) => canManage && (view.my_ceiling === null || r.position < view.my_ceiling);

  return (
    <div className="flex max-w-[760px] flex-col gap-7">
      <Block title="Roles">
        <Hint>
          Custom roles rank below Admin and Moderator. Members see a role’s name and colour; its permissions decide what its
          holders may do.
        </Hint>
        <ul role="list" data-roles className="m-0 flex list-none flex-col gap-s-1 p-0">
          {view.roles.map((r) => (
            <li key={r.id}>
              <button
                type="button"
                aria-current={selected === r.id ? 'true' : undefined}
                onClick={() => setSelected(r.id)}
                className={cn(
                  'flex h-11 w-full items-center gap-s-3 rounded-lg border border-border bg-panel px-s-3 text-left hover:bg-hover',
                  selected === r.id && 'border-accent',
                )}
              >
                <span aria-hidden className="size-3 shrink-0 rounded-full" style={{ background: r.color || 'var(--muted-foreground)' }} />
                <span className="min-w-0 flex-1 truncate text-ui font-medium text-text-strong">{r.name}</span>
                <span className="text-sm text-muted-foreground">
                  {r.members.length} {r.members.length === 1 ? 'member' : 'members'}
                </span>
              </button>
            </li>
          ))}
          {!view.roles.length ? <Hint>No custom roles yet.</Hint> : null}
        </ul>
        {canManage ? (
          <Button variant="ghost" className="self-start" onClick={() => setSelected('new')}>
            <Plus className="size-4" /> New role
          </Button>
        ) : null}
      </Block>

      {selected ? (
        <RoleEditor
          key={selected}
          community={community}
          view={view}
          role={role}
          editable={role ? mine(role) : canManage}
          initial={draftOf(role, nextPosition)}
          onDone={(id) => {
            setSelected(id);
            void reload();
          }}
        />
      ) : null}

      <BotsBlock community={community} view={view} onToken={setToken} onChange={reload} />
      {token ? <TokenNotice token={token} onClose={() => setToken(null)} /> : null}

      {community.my_role === 'owner' ? (
        <RolesGrantBlock community={community} honored={view.honored} onChange={reload} />
      ) : null}
    </div>
  );
}

const ROLES_SEEN =
  'Until they update, their app keeps the old rules: channels, deletions and bans made through a custom role do not show there, so they see a different server.';

// The owner's switch. A device on an old app holds it back until the owner overrides it, and an offline one is named
// when turning it on. Either way the owner confirms what those people see.
function RolesGrantBlock({ community, honored, onChange }: { community: CommunityView; honored: boolean; onChange: () => Promise<unknown> }) {
  const [status, setStatus] = useState<GateStatus | null>(null);
  const [busy, setBusy] = useState(false);
  const load = useCallback(async () => {
    try {
      setStatus(await rolesApi.honoredStatus(community.id));
    } catch (err) {
      toast(err);
    }
  }, [community.id]);
  useEffect(() => {
    let live = true;
    if (!honored) rolesApi.honoredStatus(community.id).then((v) => live && setStatus(v), (err) => live && toast(err));
    return () => {
      live = false;
    };
  }, [community.id, honored]);
  const change = async (on: boolean) => {
    setBusy(true);
    try {
      // Asked afresh, since who is online changes while the page stays open.
      const fresh = on ? await rolesApi.honoredStatus(community.id) : null;
      if (fresh) setStatus(fresh);
      const confirmed = fresh
        ? await confirmGate(fresh, { title: 'Let custom roles grant these permissions?', okLabel: 'Turn on', seen: ROLES_SEEN })
        : [];
      if (confirmed) {
        await rolesApi.setHonored(community.id, on, confirmed);
        await onChange();
      }
    } catch (err) {
      toast(err);
    }
    setBusy(false);
  };
  const outdated = !honored && (status?.outdated.length ?? 0) > 0;
  const listed = !honored && status ? gateSummary(status) : '';
  return (
    <Block title="Roles grant permissions">
      <CheckRow>
        <input
          type="checkbox"
          className={CHECK_CLASS}
          checked={honored}
          disabled={busy || outdated}
          onChange={(e) => void change(e.target.checked)}
        />
        Let custom roles manage channels, messages, bans and the server
      </CheckRow>
      <Hint>
        Until this is on, custom roles count only for files, bots and other roles. A member on an older app would disagree
        about who may do what until they update, so you are told who that is before it turns on.
      </Hint>
      {listed ? <p data-roles-gate role="status" className="text-sm text-muted-foreground">{listed}</p> : null}
      {outdated || listed ? (
        <div className="flex gap-s-2">
          <Button variant="secondary" size="sm" disabled={busy} onClick={() => void load()}>Check again</Button>
          {outdated ? <Button variant="secondary" size="sm" disabled={busy} onClick={() => void change(true)}>Turn on anyway…</Button> : null}
        </div>
      ) : null}
    </Block>
  );
}

function RoleEditor({
  community,
  view,
  role,
  editable,
  initial,
  onDone,
}: {
  community: CommunityView;
  view: RolesView;
  role: RoleView | null;
  editable: boolean;
  initial: RoleDraft;
  onDone: (id: Id | null) => void;
}) {
  const [draft, setDraft] = useState<RoleDraft>(initial);
  const [busy, setBusy] = useState(false);
  const togglePerm = (p: string) =>
    setDraft((d) => ({ ...d, perms: d.perms.includes(p) ? d.perms.filter((x) => x !== p) : [...d.perms, p] }));
  const save = async () => {
    if (!draft.name.trim()) {
      toast('Give the role a name');
      return;
    }
    setBusy(true);
    try {
      if (role) {
        await rolesApi.update(community.id, role.id, draft);
        onDone(role.id);
      } else {
        onDone(await rolesApi.create(community.id, draft));
      }
      toast('Role saved', 'info');
    } catch (err) {
      toast(err);
    }
    setBusy(false);
  };
  const remove = async () => {
    if (!role) return;
    try {
      await rolesApi.remove(community.id, role.id);
      onDone(null);
    } catch (err) {
      toast(err);
    }
  };
  const assign = async (userId: Id, on: boolean) => {
    if (!role) return;
    try {
      await rolesApi.assign(community.id, userId, role.id, on);
      await useShell.getState().loadCommunity(community.id);
      onDone(role.id);
    } catch (err) {
      toast(err);
    }
  };
  const people = community.members.filter((m) => !m.banned && !m.storage);

  return (
    <Block title={role ? `Edit ${role.name}` : 'New role'}>
      <div data-role-editor className="flex flex-col gap-s-4">
        <label className="flex flex-col gap-s-1">
          <span className={BLOCK_TITLE}>Name</span>
          <Input value={draft.name} maxLength={MAX_NAME} disabled={!editable} onChange={(e) => setDraft({ ...draft, name: e.target.value })} />
        </label>
        <div className="flex gap-s-4">
          <label className="flex flex-col gap-s-1">
            <span className={BLOCK_TITLE}>Colour</span>
            <input
              type="color"
              aria-label="Role colour"
              disabled={!editable}
              value={draft.color || '#99aab5'}
              onChange={(e) => setDraft({ ...draft, color: e.target.value })}
              className="h-9 w-16 cursor-pointer rounded border border-input-border bg-input"
            />
          </label>
          <label className="flex flex-col gap-s-1">
            <span className={BLOCK_TITLE}>Position</span>
            <Input
              type="number"
              min={0}
              max={view.my_ceiling === null ? 65535 : Math.max(0, view.my_ceiling - 1)}
              value={draft.position}
              disabled={!editable}
              onChange={(e) => setDraft({ ...draft, position: Math.max(0, Math.min(65535, Number(e.target.value) || 0)) })}
              className="w-28"
            />
          </label>
        </div>
        <fieldset className="m-0 flex flex-col gap-s-1 border-0 p-0">
          <legend className={cn(BLOCK_TITLE, 'mb-s-1')}>Permissions</legend>
          {view.all_perms.map((p) => {
            const held = view.my_perms.includes(p);
            return (
              <CheckRow key={p} className="h-auto py-1">
                <input
                  type="checkbox"
                  className={CHECK_CLASS}
                  checked={draft.perms.includes(p)}
                  disabled={!editable || !held}
                  onChange={() => togglePerm(p)}
                />
                <span className="flex flex-col">
                  <span>{PERM_LABELS[p]?.label ?? p}</span>
                  <span className="text-sm text-muted-foreground">{PERM_LABELS[p]?.hint}</span>
                </span>
              </CheckRow>
            );
          })}
          <Hint>You can give only permissions you hold yourself.</Hint>
        </fieldset>
        {editable ? (
          <div className="flex gap-s-2">
            <Button disabled={busy} onClick={() => void save()}>
              Save role
            </Button>
            {role ? (
              <Button variant="ghost" onClick={() => void remove()}>
                <Trash2 className="size-4" /> Delete role
              </Button>
            ) : null}
          </div>
        ) : null}
        {role ? (
          <div className="flex flex-col gap-s-1">
            <span className={BLOCK_TITLE}>Members</span>
            <ul role="list" data-role-members className="m-0 flex list-none flex-col gap-s-1 p-0">
              {people.map((m) => {
                const has = role.members.includes(m.root_id);
                return (
                  <li key={m.root_id}>
                    <CheckRow>
                      <input type="checkbox" className={CHECK_CLASS} checked={has} disabled={!editable} onChange={() => void assign(m.root_id, !has)} />
                      {m.display_name}
                      {m.bot ? <span className="rounded bg-accent px-1 text-meta font-semibold text-white">BOT</span> : null}
                    </CheckRow>
                  </li>
                );
              })}
            </ul>
          </div>
        ) : null}
      </div>
    </Block>
  );
}

function BotsBlock({ community, view, onToken, onChange }: { community: CommunityView; view: RolesView; onToken: (t: string) => void; onChange: () => Promise<void> }) {
  const [name, setName] = useState('');
  const [roleIds, setRoleIds] = useState<Id[]>([]);
  const canManage = view.my_perms.includes('manage_bots');
  const add = async () => {
    if (!name.trim()) {
      toast('Give the bot a name');
      return;
    }
    try {
      const bot = await rolesApi.createBot(community.id, name.trim(), roleIds);
      onToken(bot.token);
      setName('');
      setRoleIds([]);
      await onChange();
    } catch (err) {
      toast(err);
    }
  };
  return (
    <Block title="Bots">
      <Hint>A bot is its own identity with a role. It joins through the invite in its token, and runs headless.</Hint>
      <ul role="list" data-bots className="m-0 flex list-none flex-col gap-s-1 p-0">
        {view.bots.map((b) => (
          <li key={b.root_id} className="flex h-11 items-center gap-s-3 rounded-lg border border-border bg-panel px-s-3">
            <Bot className="size-4 text-muted-foreground" aria-hidden />
            <span className="min-w-0 flex-1 truncate text-ui font-medium text-text-strong">{b.name}</span>
            <span className="text-sm text-muted-foreground">{b.joined ? 'Joined' : 'Not joined yet'}</span>
            {canManage ? (
              <Button
                variant="ghost"
                aria-label={`Remove ${b.name}`}
                onClick={async () => {
                  try {
                    await rolesApi.unregisterBot(community.id, b.root_id);
                    await onChange();
                  } catch (err) {
                    toast(err);
                  }
                }}
              >
                <Trash2 className="size-4" />
              </Button>
            ) : null}
          </li>
        ))}
        {!view.bots.length ? <Hint>No bots yet.</Hint> : null}
      </ul>
      {canManage ? (
        <div className="flex flex-col gap-s-2">
          <Input placeholder="Bot name" aria-label="Bot name" value={name} maxLength={MAX_NAME} onChange={(e) => setName(e.target.value)} />
          {view.roles.length ? (
            <div className="flex flex-wrap gap-s-3">
              {view.roles.map((r) => (
                <CheckRow key={r.id}>
                  <input
                    type="checkbox"
                    className={CHECK_CLASS}
                    checked={roleIds.includes(r.id)}
                    onChange={() => setRoleIds((ids) => (ids.includes(r.id) ? ids.filter((x) => x !== r.id) : [...ids, r.id]))}
                  />
                  {r.name}
                </CheckRow>
              ))}
            </div>
          ) : null}
          <Button className="self-start" onClick={() => void add()}>
            <Plus className="size-4" /> Add bot
          </Button>
        </div>
      ) : null}
    </Block>
  );
}

function TokenNotice({ token, onClose }: { token: string; onClose: () => void }) {
  return (
    <div role="alert" data-bot-token className="flex flex-col gap-s-2 rounded-lg border border-danger bg-elevated p-s-4">
      <p className="m-0 text-ui font-semibold text-text-strong">Copy the bot’s token now</p>
      <Hint>It holds the bot’s key and is shown only once. Anyone with it can act as the bot.</Hint>
      <code className="block overflow-x-auto rounded bg-panel p-s-2 text-sm break-all">{token}</code>
      <div className="flex gap-s-2">
        <Button
          onClick={async () => {
            if (await copyText(token)) toast('Token copied', 'info');
          }}
        >
          Copy token
        </Button>
        <Button variant="ghost" onClick={onClose}>
          Done
        </Button>
      </div>
    </div>
  );
}
