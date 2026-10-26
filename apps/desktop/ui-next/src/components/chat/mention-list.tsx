// The "@" autocomplete above the composer: the server's members that match what is typed after "@", each with its avatar,
// name and handle, and @everyone when the user may send it. The composer owns the keys; this draws the list and reports
// a click. Options are a listbox, so a screen reader reads the active one.
import { UserAvatar } from '@/components/user-avatar';
import type { Candidate } from '@/lib/mentions';
import { cn } from '@/lib/utils';

export function MentionList({
  rows,
  total,
  query,
  active,
  onPick,
}: {
  rows: Candidate[];
  total: number; // every match; the list shows the first rows and says when there are more
  query: string;
  active: number;
  onPick: (row: Candidate) => void;
}) {
  const more = total > rows.length;
  return (
    <div
      role="listbox"
      aria-label="Mention a member"
      className="absolute right-0 bottom-full left-0 z-10 mb-2 max-h-80 overflow-y-auto rounded-lg border border-border-menu bg-ink py-1 shadow-[var(--shadow-2)]"
    >
      <div data-mention-header className="px-s-3 pt-1 pb-1.5 text-tiny font-bold tracking-wide text-muted-foreground uppercase">
        {query ? `Members matching @${query}` : 'Members'}
        {more ? <span className="font-normal normal-case">{` · ${rows.length} of ${total}, keep typing to narrow`}</span> : null}
      </div>
      {rows.map((row, i) => (
        <div
          key={row.pick.kind === 'everyone' ? 'everyone' : row.pick.id}
          role="option"
          aria-selected={i === active}
          // The mouse picks on press, so the textarea keeps the focus and the caret.
          onMouseDown={(e) => {
            e.preventDefault();
            onPick(row);
          }}
          className={cn(
            'flex h-10 cursor-pointer items-center gap-s-3 px-s-3 text-ui text-text',
            i === active && 'bg-accent text-white',
          )}
        >
          {row.pick.kind === 'everyone' ? (
            <span aria-hidden="true" className="grid size-6 place-items-center rounded-full bg-elevated text-meta font-semibold text-text-strong">@</span>
          ) : (
            <UserAvatar name={row.label} seed={row.avatar} size="sm" />
          )}
          <span className="min-w-0 flex-1 truncate font-semibold">{row.label}</span>
          {row.handle ? <span className={cn('truncate text-meta', i === active ? 'text-white/80' : 'text-muted-foreground')}>@{row.handle}</span> : null}
        </div>
      ))}
    </div>
  );
}
