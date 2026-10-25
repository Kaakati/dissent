// Small parts the chat pane shares: icon buttons, the centred empty state, skeleton rows, and search highlights.
import type { ButtonHTMLAttributes, ComponentType, KeyboardEvent, MouseEvent, ReactNode } from 'react';
import type { LucideProps } from 'lucide-react';
import { toast } from '@/lib/toasts';
import { cn } from '@/lib/utils';

// The current UI's .icon-btn: a 32 px square, muted, with the surface on hover.
export function IconBtn({
  label,
  icon: Icon,
  pressed,
  lg = false,
  danger = false,
  className,
  ...rest
}: {
  label: string;
  icon: ComponentType<LucideProps>;
  pressed?: boolean;
  lg?: boolean;
  danger?: boolean;
} & ButtonHTMLAttributes<HTMLButtonElement>) {
  return (
    <button
      type="button"
      className={cn('icon-btn', lg && 'lg', pressed && 'on', danger && 'danger-text', className)}
      aria-label={label}
      title={label}
      aria-pressed={pressed === undefined ? undefined : pressed}
      {...rest}
    >
      <Icon aria-hidden="true" />
    </button>
  );
}

// The centred message of an empty pane: an icon, a title and a line under it.
export function EmptyNode({ icon: Icon, title, text, children }: { icon: ComponentType<LucideProps>; title: string; text?: string; children?: ReactNode }) {
  return (
    <div className="empty">
      <Icon aria-hidden="true" />
      <p className="empty-title">{title}</p>
      {text ? <p className="muted">{text}</p> : null}
      {children}
    </div>
  );
}

// Shimmer placeholders while messages load.
export function SkeletonRows({ count = 6 }: { count?: number }) {
  return (
    <>
      {Array.from({ length: count }, (_, i) => (
        <div key={i} className="skel-msg" aria-hidden="true">
          <span className="skel skel-circle" />
          <div className="skel-lines">
            <span className="skel skel-line" />
            <span className="skel skel-line short" />
          </div>
        </div>
      ))}
    </>
  );
}

// Wraps each match of the search text in a <mark>. Plain text only; the caller keeps links as they are.
export function highlight(text: string, query: string): ReactNode[] {
  const needle = query.trim().toLowerCase();
  if (!needle) return [text];
  const lower = text.toLowerCase();
  const out: ReactNode[] = [];
  let last = 0;
  for (let i = lower.indexOf(needle); i !== -1; i = lower.indexOf(needle, last)) {
    if (i > last) out.push(text.slice(last, i));
    out.push(<mark key={i} className="search-hit">{text.slice(i, i + needle.length)}</mark>);
    last = i + needle.length;
  }
  if (last < text.length) out.push(text.slice(last));
  return out;
}

const FLASH_MS = 1200; // how long a jumped-to message stays highlighted

// Scrolls to a loaded message and flashes it. The flash is an opacity animation on the row's overlay.
export function jumpTo(messageId: string): void {
  const node = [...document.querySelectorAll<HTMLElement>('.messages .msg')].find((n) => n.dataset.id === messageId);
  if (!node) {
    toast('The original message is not in the loaded history.', 'info');
    return;
  }
  node.scrollIntoView({ block: 'center' });
  node.focus();
  node.classList.remove('flash');
  void node.offsetWidth; // restart the animation when flashing the same row twice
  node.classList.add('flash');
  setTimeout(() => node.classList.remove('flash'), FLASH_MS);
}

// A spoiler container's handlers: a click or Enter on it reveals the media, and the capture phase keeps the click from
// reaching a download or open button while it is hidden. Empty when nothing is hidden.
export function spoilerGuard(hidden: boolean, reveal: () => void) {
  if (!hidden) return {};
  return {
    tabIndex: 0,
    role: 'button',
    'aria-label': 'Spoiler media. Press to reveal',
    onClickCapture: (e: MouseEvent<HTMLElement>) => {
      e.preventDefault();
      e.stopPropagation();
      reveal();
    },
    onKeyDownCapture: (e: KeyboardEvent<HTMLElement>) => {
      if (e.key === 'Enter' || e.key === ' ') {
        e.preventDefault();
        e.stopPropagation();
        reveal();
      }
    },
  };
}
