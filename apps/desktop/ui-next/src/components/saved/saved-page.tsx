// The Saved Messages page, from Home: the messages this device has saved (Bookmark Message in a message's menu), newest
// first, each with Jump (opens its conversation and scrolls to it) and Remove. Local only: nothing here is sent.
import { useEffect, useState } from 'react';
import { Bookmark } from 'lucide-react';
import { openMessage } from '@/components/chat/jump';
import { EmptyNode } from '@/components/chat/parts';
import { Button } from '@/components/ui/button';
import { listSaved, unsaveMessage } from '@/lib/saved';
import { stampLabel, whenLabel } from '@/lib/time';
import type { Bookmark as SavedMessage } from '@/lib/types';

const GONE = 'This message is not available here: its conversation is closed or it is not in the loaded history.';

export function SavedBody() {
  const [items, setItems] = useState<SavedMessage[] | null>(null);
  const [reads, setReads] = useState(0); // bumped after a Remove, so the list is read again

  useEffect(() => {
    let live = true;
    void listSaved().then((list) => {
      if (live) setItems([...list].sort((a, b) => b.saved_ms - a.saved_ms));
    });
    return () => {
      live = false;
    };
  }, [reads]);

  if (items === null) return null;
  if (items.length === 0) {
    return (
      <div className="flex min-h-0 items-center justify-center">
        <EmptyNode icon={Bookmark} title="No saved messages" text="Use Bookmark Message in a message's menu to keep it here." />
      </div>
    );
  }
  return (
    <ul aria-label="Saved messages" className="dissent-scroll flex min-h-0 flex-col gap-s-2 overflow-y-auto p-s-4">
      {items.map((b) => (
        <li key={b.key} className="flex items-start gap-s-3 rounded-lg bg-panel p-s-3">
          <div className="flex min-w-0 flex-1 flex-col gap-s-1">
            <div className="flex items-baseline gap-s-2">
              <span className="truncate font-semibold text-text-strong">{b.message.author_name || 'unknown'}</span>
              <time className="shrink-0 text-meta text-faint" title={stampLabel(b.message.ms)}>{whenLabel(b.message.ms)}</time>
            </div>
            <p className="break-words whitespace-pre-wrap text-body text-text">{b.message.text}</p>
          </div>
          <div className="flex shrink-0 gap-s-2">
            <Button type="button" variant="secondary" size="sm" onClick={() => void openMessage(b.message, GONE)}>
              Jump
            </Button>
            <Button
              type="button"
              variant="ghost"
              size="sm"
              onClick={() => void unsaveMessage(b.key).then(() => setReads((n) => n + 1))}
            >
              Remove
            </Button>
          </div>
        </li>
      ))}
    </ul>
  );
}
