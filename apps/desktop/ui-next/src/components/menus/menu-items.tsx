// Renders the entries of a menu (see lib/menu.ts) with the shadcn DropdownMenu primitives, restyled to the current UI's
// .menu-item: 32 px rows, the red highlight, danger rows in the danger text colour. Submenus open beside their row.
import { type ReactNode, useState } from 'react';
import { Check } from 'lucide-react';
import {
  DropdownMenuItem,
  DropdownMenuLabel,
  DropdownMenuSeparator,
  DropdownMenuSub,
  DropdownMenuSubContent,
  DropdownMenuSubTrigger,
} from '@/components/ui/dropdown-menu';
import { type MenuEntry, type MenuItem, type MenuQuick, type MenuSlider } from '@/lib/menu';
import { cn } from '@/lib/utils';

const ROW = 'group min-h-8 gap-2.5 rounded-md px-s-2 py-1.5 text-ui font-medium text-text focus:bg-accent focus:text-white data-[disabled]:opacity-50';
const DANGER = 'text-danger-text focus:bg-danger';

// The submenu and the top-level content share one width and inner padding (`.menu.context`: 264 px, 6 px).
export const MENU_WIDTH = 'w-[264px] p-1.5';

export function MenuItems({ entries }: { entries: MenuEntry[] }) {
  return (
    <>
      {entries.map((entry, i) => (
        <MenuEntryNode key={i} entry={entry} />
      ))}
    </>
  );
}

function MenuEntryNode({ entry }: { entry: MenuEntry }) {
  if (!entry) return null;
  if (entry === 'sep') return <DropdownMenuSeparator className="my-1 bg-border-menu" />;
  if ('header' in entry) {
    return <DropdownMenuLabel className="px-s-2 py-s-2 text-ui font-medium text-text">{entry.header}</DropdownMenuLabel>;
  }
  if ('slider' in entry) return <SliderRow slider={entry.slider} />;
  if ('quick' in entry) return <QuickRow quick={entry.quick} />;
  return <Item item={entry} />;
}

function ItemBody({ item, radio = false, checkbox = false }: { item: MenuItem; radio?: boolean; checkbox?: boolean }): ReactNode {
  const Icon = item.icon;
  return (
    <>
      {item.leading ?? (Icon ? <Icon className="size-4 shrink-0" aria-hidden="true" /> : null)}
      <span className="flex min-w-0 flex-1 flex-col">
        <span className="truncate" title={item.label}>{item.label}</span>
        {item.sub ? <span className="text-meta leading-4 font-normal whitespace-normal text-muted-foreground group-focus:text-white/80">{item.sub}</span> : null}
      </span>
      {radio ? (
        <span
          aria-hidden="true"
          className={cn(
            'size-3.5 shrink-0 rounded-full',
            item.checked
              ? 'bg-[radial-gradient(circle,var(--accent)_0_4px,transparent_4.5px)] shadow-[inset_0_0_0_2px_var(--accent)]'
              : 'shadow-[inset_0_0_0_2px_var(--muted-foreground)]',
          )}
        />
      ) : null}
      {checkbox ? (
        <span
          aria-hidden="true"
          className={cn('grid size-3.5 shrink-0 place-items-center rounded-sm shadow-[inset_0_0_0_2px_var(--muted-foreground)]', item.checked && 'bg-accent shadow-none')}
        >
          {item.checked ? <Check className="size-3 text-white" strokeWidth={3} /> : null}
        </span>
      ) : null}
    </>
  );
}

function Item({ item }: { item: MenuItem }) {
  if (item.submenu) {
    return (
      <DropdownMenuSub>
        <DropdownMenuSubTrigger className={cn(ROW, item.danger && DANGER)} disabled={item.disabled}>
          <ItemBody item={item} />
        </DropdownMenuSubTrigger>
        <DropdownMenuSubContent className={cn(MENU_WIDTH, item.scroll && 'max-h-[min(360px,70vh)] overflow-y-auto')}>
          <MenuItems entries={item.submenu} />
        </DropdownMenuSubContent>
      </DropdownMenuSub>
    );
  }
  return (
    <DropdownMenuItem
      role={item.radio ? 'menuitemradio' : item.checkbox ? 'menuitemcheckbox' : 'menuitem'}
      aria-checked={item.radio || item.checkbox ? Boolean(item.checked) : undefined}
      disabled={item.disabled}
      className={cn(ROW, item.danger && DANGER)}
      // Radix closes the menu after the click. The pick runs after that, so a dialog it opens keeps the focus.
      onClick={(e) => {
        if (item.disabled || !item.onPick) return;
        const event = e;
        setTimeout(() => item.onPick?.(event), 0);
      }}
    >
      <ItemBody item={item} radio={item.radio} checkbox={item.checkbox} />
    </DropdownMenuItem>
  );
}

// A row of emoji buttons. Each one is a menu item, so the arrow keys reach it; a pick closes the menu as any item does.
function QuickRow({ quick }: { quick: MenuQuick['quick'] }) {
  return (
    <div role="none" className="flex gap-1 px-0.5 py-0.5">
      {quick.map((q) => (
        <DropdownMenuItem
          key={q.emoji}
          role="menuitem"
          aria-label={q.label}
          title={q.label}
          className="min-h-9 flex-1 justify-center rounded-md px-0 text-[18px] focus:bg-accent"
          onClick={() => setTimeout(q.onPick, 0)}
        >
          <span aria-hidden="true">{q.emoji}</span>
        </DropdownMenuItem>
      ))}
    </div>
  );
}

// A heading over a range input. The row is a menu item, so Up and Down reach it; Left and Right change its value. A
// press on the track does not close the menu.
function SliderRow({ slider }: { slider: MenuSlider['slider'] }) {
  const { label, min, max, step = 1, format = String, onInput, onChange } = slider;
  const [value, setValue] = useState(slider.value);
  const clamp = (n: number) => Math.min(max, Math.max(min, n));
  const set = (next: number) => {
    const v = clamp(next);
    setValue(v);
    onInput?.(v);
  };
  return (
    <DropdownMenuItem
      role="menuitem"
      aria-label={label}
      onSelect={(e) => e.preventDefault()}
      onKeyDown={(e) => {
        if (e.key !== 'ArrowLeft' && e.key !== 'ArrowRight') return;
        // Stops the menu from reading the key as a move to the next row.
        e.preventDefault();
        e.stopPropagation();
        const next = clamp(value + (e.key === 'ArrowRight' ? step : -step));
        if (next === value) return;
        set(next);
        onChange?.(next);
      }}
      className="flex-col items-stretch gap-0 px-0 py-0 text-text focus:bg-transparent focus:text-text"
    >
      <span className="flex items-baseline justify-between gap-s-2 px-s-2 pt-1.5 text-ui font-medium">
        <span>{label}</span>
        <output className="font-mono text-meta text-muted-foreground">{format(value)}</output>
      </span>
      <input
        type="range"
        tabIndex={-1}
        aria-label={label}
        min={min}
        max={max}
        step={step}
        value={value}
        onChange={(e) => set(Number(e.target.value))}
        onPointerUp={(e) => onChange?.(Number(e.currentTarget.value))}
        className="mx-s-2 mt-1 mb-2 w-[calc(100%-16px)] accent-accent"
      />
    </DropdownMenuItem>
  );
}
