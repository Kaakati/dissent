// The row above the composer: who is typing. Always one height, so the list does not jump when someone starts or stops.
// The names come from the shell store (dissent://typing-changed). Ported from the old UI's typing.js (paint).
import { Fragment } from 'react';
import { typingKey, useShell } from '@/lib/store';
import { typingParts } from '@/lib/typing-words';
import { typingTarget } from '@/lib/typing';
import type { ChatTarget } from '@/lib/types';

const NOBODY: string[] = [];

export function TypingRow({ target }: { target: ChatTarget | null }) {
  const names = useShell((s) => (target ? s.typing[typingKey(typingTarget(target))] : undefined) ?? NOBODY);
  return (
    <div className="typing-row" aria-live="polite">
      {names.length > 0 ? (
        <>
          <span className="typing-dots" aria-hidden="true">
            <i />
            <i />
            <i />
          </span>
          <span className="typing-text">
            {typingParts(names).map((part, i) => (typeof part === 'string' ? <Fragment key={i}>{part}</Fragment> : <strong key={i}>{part.name}</strong>))}
          </span>
        </>
      ) : null}
    </div>
  );
}
