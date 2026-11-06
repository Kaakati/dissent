// Server Settings, Files: the server's file categories (each may be linked to a folder on this computer, which members
// never see), who may add to each, allowed types, default tags and download rules, and the reputation levels.
import { useState } from 'react';
import { Link2, Plus, Trash2 } from 'lucide-react';
import { BLOCK_TITLE, Block, CHECK_CLASS, CheckRow, Hint } from '@/components/settings/settings-ui';
import { SizeLimitInput } from '@/components/settings/size-limit-input';
import { limitLabel, limitsApi, OLDER_VERSIONS_NOTE, RELEASED_LIMIT } from '@/lib/limits';
import { useShell } from '@/lib/store';
import { Button } from '@/components/ui/button';
import { Input } from '@/components/ui/input';
import { formatBytes } from '@/components/transfers/format';
import { confirm, openDialog } from '@/lib/dialog';
import { filesApi, reloadFiles, useFilesOf, type AddSpecView, type CategoryInput, type CategoryView, type FilesView, type ReputationRules } from '@/lib/files';
import { toast } from '@/lib/toasts';
import type { CommunityView } from '@/lib/types';
import { cn } from '@/lib/utils';

const WHO: { value: AddSpecView['min_rank']; label: string }[] = [
  { value: 'member', label: 'Every member' },
  { value: 'moderator', label: 'Moderators+' },
  { value: 'admin', label: 'Owner, admins' },
  { value: 'owner', label: 'Owner only' },
  { value: 'none', label: 'Only listed roles, members or levels' },
];
const GB = 1_000_000_000;

const whoLabel = (a: AddSpecView): string => {
  const base = WHO.find((w) => w.value === a.min_rank)?.label ?? '';
  const extra = [a.roles.length ? `${a.roles.length} role${a.roles.length === 1 ? '' : 's'}` : '', a.min_level ? `Lv ${a.min_level}+` : ''].filter(Boolean);
  return a.min_rank === 'none' ? extra.join(', ') || 'Nobody' : [base, ...extra].join(', ');
};

const inputOf = (c: CategoryView | null): CategoryInput =>
  c
    ? { name: c.name, icon: c.icon, types: [...c.types], default_tags: [...c.default_tags], rules: { ...c.rules }, add: { ...c.add }, visible: c.visible ? { ...c.visible } : null }
    : {
        name: '',
        icon: 'folder',
        types: [],
        default_tags: [],
        rules: { min_ratio_milli: null, ratio_after_bytes: 0, seed_hours: null },
        add: { min_rank: 'admin', roles: [], members: [], min_level: 0 },
        visible: null,
      };

export function FilesSection({ community }: { community: CommunityView }) {
  const view = useFilesOf(community.id, community);
  const [selected, setSelected] = useState<string | null>(null);
  if (!view) return <Hint>Loading files…</Hint>;
  const cat = selected && selected !== 'new' ? (view.categories.find((c) => c.id === selected) ?? null) : null;
  const maxFile = community.max_file_size ?? RELEASED_LIMIT;
  const saveMaxFile = async (bytes: number) => {
    try {
      await limitsApi.setServer(community.id, bytes);
      await useShell.getState().loadCommunity(community.id);
    } catch (err) {
      toast(err);
    }
  };
  return (
    <div className="flex max-w-[900px] flex-col gap-7">
      <Block title="File size">
        {community.can_set_max_file_size ? (
          <SizeLimitInput
            name="server"
            label="Largest file members may share"
            value={maxFile}
            hint={maxFile > RELEASED_LIMIT ? OLDER_VERSIONS_NOTE : 'Each member’s app keeps to it. Larger files are not downloaded on arrival.'}
            onSave={saveMaxFile}
          />
        ) : (
          <Hint data-size-limit="server">Members may share files up to {limitLabel(maxFile)}.</Hint>
        )}
      </Block>
      <Block title="Categories">
        <Hint>Each category can be linked to a folder on your computer. Files you drop in it are listed and seeded from where they are.</Hint>
        <div role="table" aria-label="Categories" data-categories className="flex flex-col">
          <div role="row" className="grid h-8 grid-cols-[minmax(0,2fr)_90px_minmax(0,1.3fr)_minmax(0,1fr)] items-center gap-s-2 text-meta font-semibold text-muted-foreground uppercase">
            <span>Category · linked folder</span>
            <span className="text-right">Files</span>
            <span>Who can add</span>
            <span>Visible to</span>
          </div>
          {view.categories.map((c) => (
            <button
              key={c.id}
              type="button"
              role="row"
              aria-current={selected === c.id ? 'true' : undefined}
              onClick={() => setSelected(c.id)}
              className={cn(
                'grid h-12 grid-cols-[minmax(0,2fr)_90px_minmax(0,1.3fr)_minmax(0,1fr)] items-center gap-s-2 rounded-md border-t border-border px-1 text-left hover:bg-hover',
                selected === c.id && 'bg-selected',
              )}
            >
              <span className="min-w-0">
                <span className="block truncate text-ui font-medium text-text-strong">{c.readable ? c.name : 'Locked category'}</span>
                <span className="flex items-center gap-1 truncate text-meta text-muted-foreground">
                  {c.folder ? <Link2 className="size-3" aria-hidden /> : null}
                  {c.folder ?? 'Not linked · members add files'}
                </span>
              </span>
              <span className="text-right text-sm tabular-nums">
                {c.files} <span className="text-muted-foreground">{formatBytes(c.bytes)}</span>
              </span>
              <span className="truncate text-sm">{whoLabel(c.add)}</span>
              <span className="truncate text-sm">{c.restricted ? `${c.visible?.members.length ?? 0} members, admins` : 'Everyone'}</span>
            </button>
          ))}
          {!view.categories.length ? <Hint>No categories yet.</Hint> : null}
        </div>
        {view.can_manage ? (
          <Button variant="ghost" className="self-start" onClick={() => setSelected('new')}>
            <Plus className="size-4" /> New category
          </Button>
        ) : null}
      </Block>
      {selected ? <CategoryEditor key={selected} community={community} view={view} category={cat} onDone={setSelected} /> : null}
      <LevelsBlock community={community} view={view} />
    </div>
  );
}

export function CategoryEditor({ community, view, category, onDone }: { community: CommunityView; view: FilesView; category: CategoryView | null; onDone: (id: string | null) => void }) {
  const [draft, setDraft] = useState<CategoryInput>(() => inputOf(category));
  const [tags, setTags] = useState(draft.default_tags.map((t) => `#${t}`).join(' '));
  const [ratioOn, setRatioOn] = useState(draft.rules.min_ratio_milli !== null);
  const [seedOn, setSeedOn] = useState(draft.rules.seed_hours !== null);
  const [busy, setBusy] = useState(false);
  const editable = view.can_manage;
  const toggleType = (t: string) => setDraft((d) => ({ ...d, types: d.types.includes(t) ? d.types.filter((x) => x !== t) : [...d.types, t] }));
  const save = async () => {
    const input: CategoryInput = {
      ...draft,
      default_tags: tags.split(/[\s,]+/).filter(Boolean),
      rules: {
        min_ratio_milli: ratioOn ? (draft.rules.min_ratio_milli ?? 500) : null,
        ratio_after_bytes: draft.rules.ratio_after_bytes,
        seed_hours: seedOn ? (draft.rules.seed_hours ?? 24) : null,
      },
    };
    setBusy(true);
    try {
      if (category) {
        await filesApi.updateCategory(community.id, category.id, input);
        onDone(category.id);
      } else {
        onDone(await filesApi.createCategory(community.id, input));
      }
      await reloadFiles(community.id);
      toast('Category saved', 'info');
    } catch (err) {
      toast(err);
    }
    setBusy(false);
  };
  const remove = async () => {
    if (!category) return;
    if (!(await confirm({ title: `Delete ${category.name}?`, message: 'Removes the listings. Members who already have the files keep them.', okLabel: 'Delete', danger: true }))) return;
    try {
      await filesApi.deleteCategory(community.id, category.id);
      onDone(null);
      await reloadFiles(community.id);
    } catch (err) {
      toast(err);
    }
  };
  const folder = async () => {
    if (!category) return;
    try {
      if (category.folder) await filesApi.unlinkFolder(community.id, category.id);
      else await filesApi.pickFolder(community.id, category.id);
      await reloadFiles(community.id);
    } catch (err) {
      toast(err);
    }
  };
  return (
    <Block title={category ? `Edit ${category.name}` : 'New category'}>
      <div data-category-editor className="flex flex-col gap-s-4">
        <Hint>Signed by you when saved.</Hint>
        <label className="flex flex-col gap-s-1">
          <span className={BLOCK_TITLE}>Name</span>
          <Input aria-label="Category name" value={draft.name} maxLength={48} disabled={!editable} onChange={(e) => setDraft({ ...draft, name: e.target.value })} />
        </label>
        {category ? (
          <div className="flex flex-col gap-s-1">
            <span className={BLOCK_TITLE}>Linked folder</span>
            <div className="flex items-center gap-s-2">
              <code className="min-w-0 flex-1 truncate rounded bg-panel px-s-2 py-1 text-sm">{category.folder ?? 'Not linked'}</code>
              <Button variant="ghost" onClick={() => void folder()}>{category.folder ? 'Unlink' : 'Choose folder'}</Button>
            </div>
            <Hint>Only on this computer. Members see the category, never the path.</Hint>
            {category.folder ? (
              <CheckRow>
                <input
                  type="checkbox"
                  className={CHECK_CLASS}
                  checked={category.auto_import}
                  onChange={async (e) => {
                    try {
                      await filesApi.setAutoImport(community.id, category.id, e.target.checked);
                      await reloadFiles(community.id);
                    } catch (err) {
                      toast(err);
                    }
                  }}
                />
                Auto-import new files in this folder
              </CheckRow>
            ) : null}
          </div>
        ) : null}
        <fieldset className="m-0 flex flex-col gap-s-1 border-0 p-0">
          <legend className={cn(BLOCK_TITLE, 'mb-s-1')}>Allowed types</legend>
          <div className="flex flex-wrap gap-s-2" data-allowed-types>
            {view.all_types.map((t) => (
              <button
                key={t}
                type="button"
                aria-pressed={draft.types.includes(t)}
                disabled={!editable}
                onClick={() => toggleType(t)}
                className={cn('rounded-full px-s-3 py-1 text-sm', draft.types.includes(t) ? 'bg-selected text-text-strong ring-1 ring-accent' : 'text-muted-foreground ring-1 ring-border')}
              >
                {t}
              </button>
            ))}
          </div>
          <Hint>None picked allows every type.</Hint>
        </fieldset>
        <label className="flex flex-col gap-s-1">
          <span className={BLOCK_TITLE}>Default tags</span>
          <Input aria-label="Default tags" value={tags} disabled={!editable} onChange={(e) => setTags(e.target.value)} placeholder="#stable #signed" />
        </label>
        <fieldset className="m-0 flex flex-col gap-s-1 border-0 p-0" data-visible-to>
          <legend className={cn(BLOCK_TITLE, 'mb-s-1')}>Visible to</legend>
          <label className="flex items-center gap-s-2 text-ui text-text">
            <input type="radio" name="visible-to" checked={!draft.visible} disabled={!editable || !!category} onChange={() => setDraft({ ...draft, visible: null })} />
            Every member
          </label>
          <label className="flex items-center gap-s-2 text-ui text-text">
            <input
              type="radio"
              name="visible-to"
              checked={!!draft.visible}
              disabled={!editable || !!category}
              onChange={() => setDraft({ ...draft, visible: { min_rank: 'admin', roles: [], members: [] } })}
            />
            Only some members
          </label>
          {draft.visible ? (
            <div className="ml-s-6 flex flex-col gap-1">
              {community.members
                .filter((m) => !m.banned && !m.storage)
                .map((m) => {
                  const v = draft.visible!;
                  const on = v.members.includes(m.root_id);
                  return (
                    <CheckRow key={m.root_id} className="h-7">
                      <input
                        type="checkbox"
                        className={CHECK_CLASS}
                        checked={on}
                        disabled={!editable}
                        onChange={() => setDraft({ ...draft, visible: { ...v, members: on ? v.members.filter((x) => x !== m.root_id) : [...v.members, m.root_id] } })}
                      />
                      {m.display_name}
                    </CheckRow>
                  );
                })}
              <Hint>Owner and admins always see it. Its files have their own key; members who are not listed never get it, and seeders never hold the files.</Hint>
            </div>
          ) : null}
          {category ? <Hint>Fixed when the category was made.</Hint> : null}
        </fieldset>
        <label className="flex flex-col gap-s-1">
          <span className={BLOCK_TITLE}>Who can add</span>
          <select
            aria-label="Who can add"
            disabled={!editable}
            value={draft.add.min_rank}
            onChange={(e) => setDraft({ ...draft, add: { ...draft.add, min_rank: e.target.value as AddSpecView['min_rank'] } })}
            className="h-9 rounded-md border border-input-border bg-input px-s-2 text-ui text-text"
          >
            {WHO.map((w) => (
              <option key={w.value} value={w.value}>{w.label}</option>
            ))}
          </select>
        </label>
        <label className="flex items-center gap-s-2 text-ui text-text">
          Also members at level
          <Input
            aria-label="Minimum level"
            type="number"
            min={0}
            max={view.rules.levels.length}
            className="w-20"
            disabled={!editable}
            value={draft.add.min_level}
            onChange={(e) => setDraft({ ...draft, add: { ...draft.add, min_level: Math.max(0, Math.min(10, Number(e.target.value) || 0)) } })}
          />
          and above (0 for none)
        </label>
        <fieldset className="m-0 flex flex-col gap-s-2 border-0 p-0">
          <legend className={cn(BLOCK_TITLE, 'mb-s-1')}>Rules for downloading</legend>
          <CheckRow>
            <input type="checkbox" className={CHECK_CLASS} checked={ratioOn} disabled={!editable} onChange={() => setRatioOn(!ratioOn)} />
            Ask for a ratio of at least
            <Input
              aria-label="Ratio"
              type="number"
              step="0.1"
              min={0}
              className="w-20"
              disabled={!editable || !ratioOn}
              value={((draft.rules.min_ratio_milli ?? 500) / 1000).toString()}
              onChange={(e) => setDraft({ ...draft, rules: { ...draft.rules, min_ratio_milli: Math.round(Number(e.target.value) * 1000) } })}
            />
            after
            <Input
              aria-label="After gigabytes"
              type="number"
              min={0}
              className="w-20"
              disabled={!editable || !ratioOn}
              value={Math.round(draft.rules.ratio_after_bytes / GB)}
              onChange={(e) => setDraft({ ...draft, rules: { ...draft.rules, ratio_after_bytes: Math.max(0, Number(e.target.value) || 0) * GB } })}
            />
            GB
          </CheckRow>
          <CheckRow>
            <input type="checkbox" className={CHECK_CLASS} checked={seedOn} disabled={!editable} onChange={() => setSeedOn(!seedOn)} />
            Ask people to seed for
            <Input
              aria-label="Seed hours"
              type="number"
              min={1}
              className="w-20"
              disabled={!editable || !seedOn}
              value={draft.rules.seed_hours ?? 24}
              onChange={(e) => setDraft({ ...draft, rules: { ...draft.rules, seed_hours: Math.max(1, Number(e.target.value) || 1) } })}
            />
            h after they download
          </CheckRow>
          <Hint>Rules are requests every app enforces for its own member. They hold among honest clients, not against a modified one.</Hint>
        </fieldset>
        {editable ? (
          <div className="flex gap-s-2">
            <Button disabled={busy} onClick={() => void save()}>Save changes</Button>
            {category ? (
              <Button variant="ghost" onClick={() => void remove()}>
                <Trash2 className="size-4" /> Delete {category.name}
              </Button>
            ) : null}
          </div>
        ) : null}
      </div>
    </Block>
  );
}

// The same editor in a dialog, for the Files page. `onDone` gets the category saved, or null once one is deleted.
export function openCategoryEditor(community: CommunityView, view: FilesView, category: CategoryView | null, onDone: (id: string | null) => void): void {
  openDialog({
    title: category ? `Edit ${category.name}` : 'New category',
    wide: true,
    content: (close) => (
      <div data-category-dialog>
        <CategoryEditor
          community={community}
          view={view}
          category={category}
          onDone={(id) => {
            close();
            onDone(id);
          }}
        />
      </div>
    ),
  });
}

function LevelsBlock({ community, view }: { community: CommunityView; view: FilesView }) {
  // The shown rules follow the engine's when they change, as the profile draft does.
  const [rules, setRules] = useState<ReputationRules>(view.rules);
  const key = JSON.stringify(view.rules);
  const [seen, setSeen] = useState(key);
  if (key !== seen) {
    setSeen(key);
    setRules(view.rules);
  }
  const save = async (next: ReputationRules) => {
    setRules(next);
    try {
      await filesApi.setRules(community.id, next);
      await reloadFiles(community.id);
    } catch (err) {
      toast(err);
    }
  };
  return (
    <Block title="Reputation rules">
      <Hint>
        Members earn reputation for what they deliver. When a download finishes, the downloader’s app signs a receipt for the pieces each seeder sent. Only
        receipts from different members count, and old credit fades.
      </Hint>
      <ol data-levels className="m-0 grid list-none grid-cols-5 gap-s-2 p-0">
        {rules.levels.map((l, i) => (
          <li key={i} className="rounded-lg border border-border bg-panel p-s-2">
            <span className="block text-meta text-muted-foreground">Level {i + 1}</span>
            <span className="block text-ui font-semibold text-text-strong">{l.name}</span>
            <span className="block text-sm text-muted-foreground">{i === 0 ? 'Joined the server' : `${Math.round(l.bytes / GB)} GB to ${l.members} members`}</span>
          </li>
        ))}
      </ol>
      {view.can_manage ? (
        <div className="flex flex-col gap-s-1">
          <CheckRow>
            <input type="checkbox" className={CHECK_CLASS} checked={rules.distinct} onChange={() => void save({ ...rules, distinct: !rules.distinct })} />
            Count each member once
          </CheckRow>
          <CheckRow>
            <input type="checkbox" className={CHECK_CLASS} checked={rules.decay_days > 0} onChange={() => void save({ ...rules, decay_days: rules.decay_days > 0 ? 0 : 30 })} />
            Fade credit after 30 days
          </CheckRow>
          <CheckRow>
            <input type="checkbox" className={CHECK_CLASS} checked={rules.admins_reset} onChange={() => void save({ ...rules, admins_reset: !rules.admins_reset })} />
            Admins can reset reputation
          </CheckRow>
        </div>
      ) : null}
    </Block>
  );
}
