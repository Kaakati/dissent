// The header's privacy indicator: shown while "Hide my IP address" is on, so it is always clear how this device goes out.
// Behind relays it is a shield. Through Tor it is a shield that turns red when Tor can't be reached, since the app is then
// offline: nothing goes out directly. A click opens Settings › Network.
import { useEffect, useState } from 'react';
import { Shield, ShieldAlert } from 'lucide-react';
import { Button } from '@/components/ui/button';
import { Tip } from '@/components/ui/tip';
import { api } from '@/lib/api';
import { useShell } from '@/lib/store';
import { cn } from '@/lib/utils';
import type { PrivacyStatus } from '@/lib/types';

// What a call says, and what its camera and screen buttons say, while the IP is hidden through Tor.
export const TOR_CALL_NOTE = 'Calls go through Tor and may lag or drop.';
export const TOR_NO_VIDEO = 'Not available while your IP is hidden through Tor';

// Whether this device hides its IP through Tor. Read once: the mode changes only when the connection restarts.
export function useTorMode(): boolean {
  return usePrivacyStatus(false)?.mode === 'tor';
}

// How often the status is read again. In Tor mode each read opens one connection through Tor to a relay.
const EVERY_MS = 30_000;

// The privacy status, read now and every `EVERY_MS` while `poll` is set.
export function usePrivacyStatus(poll = true): PrivacyStatus | null {
  const [status, setStatus] = useState<PrivacyStatus | null>(null);
  useEffect(() => {
    let live = true;
    const read = () => api.privacyStatus().then((s) => live && setStatus(s), () => live && setStatus(null));
    void read();
    const timer = poll ? setInterval(read, EVERY_MS) : undefined;
    return () => {
      live = false;
      if (timer) clearInterval(timer);
    };
  }, [poll]);
  return status;
}

function describe(status: PrivacyStatus): { label: string; alert: boolean } {
  if (status.mode === 'relays') return { label: 'Your IP address is hidden behind relays.', alert: false };
  if (status.tor_ok === false) return { label: 'Can’t reach Tor, so Dissent is offline. Nothing is sent directly.', alert: true };
  if (status.tor_ok) return { label: 'Your IP address is hidden through Tor.', alert: false };
  return { label: 'Connecting to Tor…', alert: false };
}

export function PrivacyIndicator() {
  const status = usePrivacyStatus();
  const openSettings = useShell((s) => s.openSettings);
  if (!status || status.mode === 'off') return null;
  const { label, alert } = describe(status);
  return (
    <Tip label={label} side="bottom">
      <Button
        variant="ghost"
        size="icon"
        aria-label={label}
        data-privacy-indicator={status.mode}
        onClick={() => openSettings('network')}
        className={cn('size-8 rounded-full [&_svg]:size-[18px]', alert ? 'text-danger-text' : 'text-success')}
      >
        {alert ? <ShieldAlert /> : <Shield />}
      </Button>
    </Tip>
  );
}
