// Full-page Settings. It takes over the window below the title bar: a nav column on the sidebar surface, content on the
// chat surface. Esc or the close button returns to the window, and focus goes back to whatever opened it. Ported from
// views/settings.js.
import { useEffect, useRef, useState } from 'react';
import { Download, Info, KeyRound, Mic, Palette, Server, ShieldCheck, User, Volume2, Wifi, X, type LucideIcon } from 'lucide-react';
import { AboutPage } from '@/components/settings/about-page';
import { AccountPage } from '@/components/settings/account-page';
import { AppearancePage } from '@/components/settings/appearance-page';
import { DevicesPage } from '@/components/settings/devices-page';
import { NetworkPage } from '@/components/settings/network-page';
import { PrivacyPage } from '@/components/settings/privacy-page';
import { RecoveryPage } from '@/components/settings/recovery-page';
import { SoundsPage } from '@/components/settings/sounds-page';
import { UpdatesPage } from '@/components/settings/updates-page';
import { VoicePage } from '@/components/settings/voice-page';
import { Button } from '@/components/ui/button';
import { has, type Feature } from '@/lib/features';
import { useShell, type SettingsSection } from '@/lib/store';
import { cn } from '@/lib/utils';

// One group, in the current UI's order. Recovery and Devices appear only when the engine reports them.
const NAV: { key: SettingsSection; label: string; icon: LucideIcon; feature?: Feature }[] = [
  { key: 'account', label: 'My identity', icon: User },
  { key: 'recovery', label: 'Recovery', icon: KeyRound, feature: 'guardians' },
  { key: 'devices', label: 'Devices', icon: Server, feature: 'devices' },
  { key: 'profile', label: 'Contacts & privacy', icon: ShieldCheck },
  { key: 'appearance', label: 'Appearance', icon: Palette },
  { key: 'sounds', label: 'Notifications & sounds', icon: Volume2 },
  { key: 'voice', label: 'Voice', icon: Mic },
  { key: 'network', label: 'Network', icon: Wifi },
  { key: 'updates', label: 'Updates', icon: Download },
  { key: 'about', label: 'About', icon: Info },
];

const TITLES = Object.fromEntries(NAV.map((n) => [n.key, n.label])) as Record<SettingsSection, string>;

function Body({ section }: { section: SettingsSection }) {
  switch (section) {
    case 'account': return <AccountPage />;
    case 'recovery': return <RecoveryPage />;
    case 'devices': return <DevicesPage />;
    case 'profile': return <PrivacyPage />;
    case 'appearance': return <AppearancePage />;
    case 'sounds': return <SoundsPage />;
    case 'voice': return <VoicePage />;
    case 'network': return <NetworkPage />;
    case 'updates': return <UpdatesPage />;
    case 'about': return <AboutPage />;
  }
}

export function SettingsPage() {
  const section = useShell((s) => s.settings);
  const open = section !== null;
  const closeSettings = useShell((s) => s.closeSettings);
  const openSettings = useShell((s) => s.openSettings);
  const [query, setQuery] = useState('');
  const navRef = useRef<HTMLElement>(null);

  // While open: Esc closes the page, unless a dialog or a key binding holds the Escape first. The opener gets focus back.
  useEffect(() => {
    if (!open) return;
    const opener = document.activeElement instanceof HTMLElement ? document.activeElement : null;
    navRef.current?.querySelector<HTMLButtonElement>('button[aria-current="true"]')?.focus();
    const onKey = (e: KeyboardEvent) => {
      if (e.key !== 'Escape' || document.querySelector('[role="dialog"]:not([data-settings-page]), [data-binding]')) return;
      e.preventDefault();
      e.stopPropagation();
      closeSettings();
    };
    document.addEventListener('keydown', onKey, true);
    return () => {
      document.removeEventListener('keydown', onKey, true);
      if (opener?.isConnected) opener.focus();
    };
  }, [open, closeSettings]);

  if (!section) return null;

  const q = query.trim().toLowerCase();
  const items = NAV.filter((n) => (!n.feature || has(n.feature)) && n.label.toLowerCase().includes(q));

  return (
    <div
      role="dialog"
      aria-modal="true"
      aria-label="Settings"
      data-settings-page
      className="fixed inset-x-0 top-[30px] bottom-0 z-40 grid grid-cols-[560px_minmax(0,1fr)_auto] bg-rail"
    >
      <div className="flex min-h-0 justify-end overflow-y-auto bg-sidebar px-s-4 pt-[60px] pb-s-6">
        <nav ref={navRef} className="flex w-[232px] flex-col" aria-label="Settings sections">
          <input
            type="search"
            className="mb-s-3 h-10 w-full rounded-lg border border-white/[0.06] bg-rail px-3 text-sm text-text placeholder:text-muted-foreground focus-visible:outline-none"
            placeholder="Search"
            aria-label="Search settings"
            value={query}
            onChange={(e) => setQuery(e.target.value)}
          />
          <h2 className="flex h-8 items-center px-2.5 text-meta font-semibold tracking-[.02em] text-muted-foreground uppercase">You</h2>
          {items.map((n) => {
            const current = n.key === section;
            return (
              <button
                key={n.key}
                type="button"
                aria-current={current ? 'true' : undefined}
                onClick={() => openSettings(n.key)}
                className={cn(
                  // The page focuses the current section on open; the ring shows only for keyboard focus (:focus-visible), so a
                  // mouse open shows the selected row alone.
                  'flex h-9 w-full items-center gap-s-2 rounded-lg px-2.5 text-left text-ui font-medium text-muted-foreground outline-none hover:bg-hover hover:text-text focus-visible:outline-2 focus-visible:-outline-offset-2 focus-visible:outline-ring',
                  current && 'bg-selected text-text-strong',
                )}
              >
                <n.icon className="size-4 shrink-0" aria-hidden="true" />
                {n.label}
              </button>
            );
          })}
          {items.length ? null : <p className="px-2.5 text-body text-muted-foreground">No matching settings</p>}
        </nav>
      </div>

      {/* The title sits above the page, and the page keeps a readable width, as in the reference client: a title beside the
          page wrapped onto two lines at 1440 px and left every section a different width. */}
      <div className="flex min-h-0 min-w-0 flex-col gap-s-6 overflow-y-auto bg-chat px-s-10 pt-[60px] pb-s-10" data-settings-content>
        <header className="w-full max-w-[740px]">
          <h1 className="text-title font-bold text-text-strong">{TITLES[section]}</h1>
        </header>
        <div className="w-full min-w-0 max-w-[740px]">
          <Body section={section} />
        </div>
      </div>

      <div className="flex flex-col items-center gap-1.5 pt-[60px] pr-s-4">
        <Button
          variant="ghost"
          size="icon"
          aria-label="Close settings"
          title="Close (Esc)"
          onClick={closeSettings}
          className="size-10 rounded-full text-muted-foreground shadow-[inset_0_0_0_2px_var(--faint)] hover:text-text-strong"
        >
          <X className="size-[18px]" />
        </Button>
        <span className="text-meta font-semibold text-muted-foreground">ESC</span>
      </div>
    </div>
  );
}
