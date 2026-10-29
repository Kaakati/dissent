// The 30 px custom title bar: in-app Back and Forward, the context label, and the window controls. The bar is a drag
// region; every element that is not a button carries the drag attribute too, as the current UI does.
import { useEffect, useState, type MouseEvent, type ReactNode } from 'react';
import { ArrowLeft, ArrowRight, ArrowUpDown, Bookmark, Server, Settings, Users } from 'lucide-react';
import { Button } from '@/components/ui/button';
import { OwlMark } from '@/components/owl-mark';
import { Tip } from '@/components/ui/tip';
import { NetStatusLine } from '@/components/network/net-status';
import { imageOf } from '@/lib/images';
import { currentWindow } from '@/lib/tauri';
import { initials, hueBackground, hueOf } from '@/lib/identity';
import { useShell } from '@/lib/store';
import { cn } from '@/lib/utils';

const SVG_NS = 'http://www.w3.org/2000/svg';

// Window controls use 1 px strokes at 12 px, as in the current UI. Restore is two overlapping squares.
function Glyph({ children }: { children: ReactNode }) {
  return (
    <svg width="12" height="12" viewBox="0 0 12 12" fill="none" stroke="currentColor" strokeWidth="1" aria-hidden="true" focusable="false" xmlns={SVG_NS}>
      {children}
    </svg>
  );
}

function TitleText() {
  const page = useShell((s) => s.page);
  const route = useShell((s) => s.route);
  const community = useShell((s) => s.community);
  const titleOverride = useShell((s) => s.titleOverride);
  const settings = useShell((s) => s.settings !== null);
  const serverMark = (id: string, name: string) => (
    <span
      className="relative inline-grid size-4 place-items-center overflow-hidden rounded-[5px] text-[7px] font-bold text-white"
      style={{ background: hueBackground(hueOf(id)) }}
      aria-hidden="true"
    >
      {initials(name)}
      {imageOf(id) ? <img src={imageOf(id) ?? undefined} alt="" draggable={false} className="absolute inset-0 size-full object-cover" /> : null}
    </span>
  );
  let text = 'Direct Messages';
  let mark: ReactNode = <OwlMark className="size-4 text-brand" />;
  if (titleOverride) {
    text = titleOverride;
  } else if (settings) {
    // The full-page Settings covers the place, so the bar names the page, not the conversation behind it.
    text = 'Settings';
    mark = <Settings className="size-4" aria-hidden="true" />;
  } else if (page === 'contacts') {
    text = 'Friends';
    mark = <Users className="size-4" aria-hidden="true" />;
  } else if (page === 'transfers') {
    text = 'Transfers';
    mark = <ArrowUpDown className="size-4" aria-hidden="true" />;
  } else if (page === 'saved') {
    text = 'Saved Messages';
    mark = <Bookmark className="size-4" aria-hidden="true" />;
  } else if (route.kind === 'server' && community) {
    text = community.name;
    mark = serverMark(community.id, community.name);
  } else if (route.kind === 'server') {
    text = '';
    mark = <Server className="size-4" aria-hidden="true" />;
  }
  return (
    <div className="flex items-center gap-s-2 whitespace-nowrap text-sm font-semibold text-text" data-tauri-drag-region>
      <span className="flex shrink-0 items-center" data-tauri-drag-region>{mark}</span>
      <span data-tauri-drag-region>{text}</span>
    </div>
  );
}

export function Titlebar() {
  const canBack = useShell((s) => s.back.length > 0);
  const canForward = useShell((s) => s.forward.length > 0);
  const goBack = useShell((s) => s.goBack);
  const goForward = useShell((s) => s.goForward);
  const [maximized, setMaximized] = useState(false);
  const win = currentWindow();

  // Alt+Left and Alt+Right, as in the current UI.
  useEffect(() => {
    const onKey = (e: KeyboardEvent) => {
      if (!e.altKey || e.ctrlKey || e.metaKey) return;
      const s = useShell.getState();
      if (e.key === 'ArrowLeft' && s.back.length) {
        e.preventDefault();
        void s.goBack();
      }
      if (e.key === 'ArrowRight' && s.forward.length) {
        e.preventDefault();
        void s.goForward();
      }
    };
    document.addEventListener('keydown', onKey);
    return () => document.removeEventListener('keydown', onKey);
  }, []);

  // The button shows restore while the window is maximized. Asked again on every resize.
  useEffect(() => {
    const sync = () => {
      const w = currentWindow();
      w?.isMaximized?.().then(setMaximized).catch(() => {});
    };
    sync();
    window.addEventListener('resize', sync);
    return () => window.removeEventListener('resize', sync);
  }, []);

  // Double-click on the bar toggles maximize, as a native title bar does. Buttons are not part of the gesture.
  const onDoubleClick = (e: MouseEvent<HTMLElement>) => {
    if ((e.target as HTMLElement).closest('button')) return;
    try {
      currentWindow()?.toggleMaximize();
    } catch {
      // no window: a browser preview
    }
  };

  const control = (act: (w: NonNullable<typeof win>) => unknown) => () => {
    try {
      if (win) void act(win);
    } catch {
      // the window API is unavailable or refused the call; nothing else to do here
    }
  };

  return (
    <header
      data-tauri-drag-region
      onDoubleClick={onDoubleClick}
      className="grid h-[30px] shrink-0 grid-cols-[minmax(0,1fr)_auto_minmax(0,1fr)] items-center gap-x-s-8 bg-rail text-muted-foreground select-none"
    >
      {/* The side columns are equal and the bar has no padding of its own, so the title sits at the exact centre of the window;
          the inset of Back is the nav's. The 32 px column gap keeps the network line clear of the title. */}
      <nav className="flex gap-s-1 pl-s-2" aria-label="History" data-tauri-drag-region>
        <Tip label="Back" shortcut="Alt+←" side="bottom" wrap>
          <Button variant="ghost" size="icon" className="size-7 disabled:opacity-100 disabled:text-faint" aria-label="Back" disabled={!canBack} onClick={() => void goBack()}>
            <ArrowLeft className="size-4" />
          </Button>
        </Tip>
        <Tip label="Forward" shortcut="Alt+→" side="bottom" wrap>
          <Button variant="ghost" size="icon" className="size-7 disabled:opacity-100 disabled:text-faint" aria-label="Forward" disabled={!canForward} onClick={() => void goForward()}>
            <ArrowRight className="size-4" />
          </Button>
        </Tip>
      </nav>

      <div data-tauri-drag-region data-titlebar-title className="flex items-center justify-center">
        <TitleText />
      </div>

      <div className="flex min-w-0 items-center justify-end gap-s-3" data-tauri-drag-region data-titlebar-end>
        {window.__DISSENT_MOCK__ ? <span className="shrink-0 whitespace-nowrap text-tiny font-semibold text-warn">Mock backend</span> : null}
        <NetStatusLine />
        <div className="flex shrink-0" data-tauri-drag-region>
          <Tip label="Minimize" side="bottom">
            <button
              type="button"
              aria-label="Minimize"
              onClick={control((w) => w.minimize())}
              className={cn('flex h-[30px] w-[46px] items-center justify-center hover:bg-hover hover:text-text-strong', !win && 'cursor-default hover:bg-transparent hover:text-muted-foreground')}
            >
              <Glyph><path d="M1 6h10" /></Glyph>
            </button>
          </Tip>
          <Tip label={maximized ? 'Restore' : 'Maximize'} side="bottom">
            <button
              type="button"
              aria-label="Maximize"
              onClick={control(async (w) => {
                await w.toggleMaximize();
                setMaximized(await (w.isMaximized?.() ?? Promise.resolve(false)));
              })}
              className={cn('flex h-[30px] w-[46px] items-center justify-center hover:bg-hover hover:text-text-strong', !win && 'cursor-default hover:bg-transparent hover:text-muted-foreground')}
            >
              <Glyph>
                {maximized ? (
                  <>
                    <path d="M3 3V1.5h7.5V9H9" />
                    <rect x="1.5" y="3.5" width="7" height="7" />
                  </>
                ) : (
                  <rect x="1.5" y="1.5" width="9" height="9" />
                )}
              </Glyph>
            </button>
          </Tip>
          <Tip label="Close" side="bottom">
            <button
              type="button"
              aria-label="Close"
              onClick={control((w) => w.close())}
              className={cn('flex h-[30px] w-[46px] items-center justify-center hover:bg-danger hover:text-white', !win && 'cursor-default hover:bg-transparent hover:text-muted-foreground')}
            >
              <Glyph><path d="M1 1l10 10M11 1 1 11" /></Glyph>
            </button>
          </Tip>
        </div>
      </div>
    </header>
  );
}
