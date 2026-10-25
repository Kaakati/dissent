// Quick switcher (Ctrl+K): one search field over servers, channels, groups and direct messages. Ranking is ours: a
// prefix match first, then a word start, then any substring; servers and conversations show at once and channels are
// added when every server has loaded. Ported from ui/views/switcher.js, on the shadcn Command (cmdk).
import { useEffect, useMemo, useState } from 'react';
import { Hash, Search } from 'lucide-react';
import { create } from 'zustand';
import { Command, CommandEmpty, CommandInput, CommandItem, CommandList } from '@/components/ui/command';
import { Dialog, DialogContent, DialogTitle } from '@/components/ui/dialog';
import { UserAvatar } from '@/components/user-avatar';
import { api } from '@/lib/api';
import { hueBackground, hueOf, initials } from '@/lib/identity';
import { imageOf } from '@/lib/images';
import { convKey, useShell } from '@/lib/store';
import type { CommunityView, ConversationSummary, Id } from '@/lib/types';

interface SwitchItem {
  kind: 'server' | 'channel' | 'dm' | 'group';
  key: string;
  label: string;
  meta: string;
  seed?: Id; // a server's id, for its icon colour; a person's root id for the avatar
  unread: boolean;
  run: () => Promise<void> | void;
}

// Lower rank sorts first: prefix matches, then word starts, then any substring. -1 is no match.
export function rank(label: string, meta: string, q: string): number {
  const l = label.toLowerCase();
  if (l.startsWith(q)) return 0;
  if (l.split(/[\s#·]+/).some((w) => w.startsWith(q))) return 1;
  if (l.includes(q) || meta.toLowerCase().includes(q)) return 2;
  return -1;
}

// Whether the switcher is open. The Ctrl+K shortcut (lib/keyboard.ts) opens it.
interface SwitcherState {
  open: boolean;
  opener: HTMLElement | null; // the element that had the focus when the switcher opened
}
export const useSwitcher = create<SwitcherState>(() => ({ open: false, opener: null }));

export function setSwitcherOpen(open: boolean): void {
  if (!open) {
    useSwitcher.setState({ open: false });
    return;
  }
  const opener = document.activeElement instanceof HTMLElement ? document.activeElement : null;
  useSwitcher.setState({ open: true, opener });
}

// The conversations and servers that are known at once; channels are added later.
function baseItems(): SwitchItem[] {
  const s = useShell.getState();
  const servers: SwitchItem[] = s.communities.map((c) => ({
    kind: 'server',
    key: `server:${c.id}`,
    seed: c.id,
    label: c.name,
    meta: 'Server',
    unread: [...s.unread].some((k) => k.startsWith(`ch:${c.id}:`)),
    run: () => s.openServer(c.id),
  }));
  const dms: SwitchItem[] = s.conversations.map((conv: ConversationSummary) =>
    conv.kind === 'group'
      ? {
        kind: 'group',
        key: convKey(conv),
        seed: conv.id,
        label: conv.name,
        meta: 'Group',
        unread: s.unread.has(convKey(conv)),
        run: () => s.openGroup(conv),
      }
      : {
        kind: 'dm',
        key: convKey(conv),
        label: conv.display_name,
        meta: 'Direct message',
        seed: conv.peer,
        unread: s.unread.has(convKey(conv)),
        run: () => s.openDm(conv.peer, conv.display_name),
      },
  );
  return [...servers, ...dms];
}

// One request per server. A failed one is skipped, as the current UI does.
async function channelItems(): Promise<SwitchItem[]> {
  const s = useShell.getState();
  const out: SwitchItem[] = [];
  for (const c of s.communities) {
    let view: CommunityView;
    try {
      view = await api.communityView(c.id);
    } catch {
      continue;
    }
    for (const ch of view.channels.filter((x) => !x.voice)) {
      out.push({
        kind: 'channel',
        key: `channel:${c.id}:${ch.id}`,
        label: ch.name,
        meta: c.name,
        unread: s.unread.has(`ch:${c.id}:${ch.id}`),
        run: async () => {
          await s.openServer(c.id);
          await s.openChannel(ch.id);
        },
      });
    }
  }
  return out;
}

export function QuickSwitcher() {
  const open = useSwitcher((s) => s.open);
  const [query, setQuery] = useState('');
  const [channels, setChannels] = useState<SwitchItem[]>([]);
  const communities = useShell((s) => s.communities);
  const conversations = useShell((s) => s.conversations);
  const unread = useShell((s) => s.unread);

  // Channels are loaded each time the switcher opens, so a channel made since is found.
  useEffect(() => {
    if (!open) return;
    let live = true;
    void channelItems().then((items) => {
      if (live) setChannels(items);
    });
    return () => {
      live = false;
    };
  }, [open]);

  const all = useMemo(() => {
    const base = baseItems();
    const servers = base.slice(0, communities.length);
    const rest = base.slice(communities.length);
    return [...servers, ...channels, ...rest];
    // The base list reads the store directly, so it is rebuilt when any of its inputs change.
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [communities, conversations, unread, channels]);

  const q = query.trim().toLowerCase();
  const items = useMemo(
    () =>
      all
        .map((item) => ({ item, score: q ? rank(item.label, item.meta, q) : 0 }))
        .filter((s) => s.score >= 0)
        .sort((a, b) => a.score - b.score)
        .slice(0, 50)
        .map((s) => s.item),
    [all, q],
  );

  const close = () => setSwitcherOpen(false);
  const run = (item: SwitchItem) => {
    close();
    void item.run();
  };

  return (
    <Dialog
      open={open}
      onOpenChange={(next) => {
        if (!next) {
          setQuery('');
          close();
        }
      }}
    >
      <DialogContent
        onCloseAutoFocus={(e) => {
          e.preventDefault();
          const back = useSwitcher.getState().opener;
          if (back?.isConnected) back.focus();
        }}
        overlayClassName="bg-black/50"
        className="top-[15vh] max-h-[calc(100vh-30vh)] w-[min(560px,92vw)] translate-y-0 gap-0 overflow-hidden rounded-lg border-0 bg-ink p-0"
      >
        <DialogTitle className="sr-only">Quick switcher</DialogTitle>
        <Command shouldFilter={false} loop className="bg-ink">
          <div className="flex h-12 shrink-0 items-center gap-s-2 px-s-4 text-muted-foreground shadow-[inset_0_-1px_0_var(--border)]">
            <Search className="size-5 shrink-0" aria-hidden="true" />
            <CommandInput
              placeholder="Where do you want to go?"
              aria-label="Search servers, channels and people"
              value={query}
              autoFocus
              onValueChange={setQuery}
              className="h-10 text-body"
            />
          </div>
          <CommandList aria-label="Results" className="max-h-[360px] p-s-2">
            {items.length === 0 ? <CommandEmpty className="px-s-2 py-s-3 text-sm text-muted-foreground">Nothing matches.</CommandEmpty> : null}
            {items.map((item) => (
              <CommandItem
                key={item.key}
                value={item.key}
                onSelect={() => run(item)}
                className="h-10 gap-s-3 rounded-md px-s-2 text-body font-medium text-text data-[selected=true]:bg-selected data-[selected=true]:text-text-strong"
              >
                <Lead item={item} />
                <span className="min-w-0 flex-1 truncate">{item.label}</span>
                <span className="text-meta whitespace-nowrap text-muted-foreground">{item.meta}</span>
                {item.unread ? <span aria-label="unread" className="size-2 shrink-0 rounded-full bg-text-strong" /> : null}
              </CommandItem>
            ))}
          </CommandList>
          <p className="shrink-0 border-t border-border px-s-4 py-s-2 text-meta text-muted-foreground">
            ↑↓ to move · Enter to open · Esc to close
          </p>
        </Command>
      </DialogContent>
    </Dialog>
  );
}

// The icon at the start of a row: a person's avatar, a server's mark, or a hash for a channel.
function Lead({ item }: { item: SwitchItem }) {
  if (item.kind === 'dm') return <UserAvatar name={item.label} seed={item.seed} size="sm" />;
  if (item.kind === 'server' && item.seed) {
    const src = imageOf(item.seed);
    return (
      <span
        className="grid size-6 shrink-0 place-items-center overflow-hidden rounded-[8px] text-[8px] leading-none font-semibold text-white"
        style={{ background: hueBackground(hueOf(item.seed)) }}
      >
        {src ? <img src={src} alt="" className="size-full object-cover" /> : initials(item.label)}
      </span>
    );
  }
  return <Hash className="size-5 shrink-0 text-muted-foreground" aria-hidden="true" />;
}
