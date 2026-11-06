// While "Hide my IP address" is on, updates aren't checked on their own: a check shows the IP to GitHub. These remind the
// user once no check has run for two weeks (`updates::REMIND_AFTER`), so a hidden device doesn't stay on an old version
// without knowing. A check from here is the same as Settings › Updates' button, and says what it shows first.
import { useEffect, useState } from 'react';
import { BellRing } from 'lucide-react';
import { Hint } from '@/components/settings/settings-ui';
import { openUpdateDialog } from '@/components/settings/update-dialog';
import { Button } from '@/components/ui/button';
import { Tip } from '@/components/ui/tip';
import { api } from '@/lib/api';
import { confirm } from '@/lib/dialog';
import { useShell } from '@/lib/store';
import type { UpdatePolicy } from '@/lib/types';

export const HIDDEN_IP_UPDATES = 'Updates aren’t checked automatically while your IP is hidden.';
export const CHECK_SHOWS_IP = 'Check now (shows your IP to GitHub)';

// The update policy, read when shown and after each check.
export function useUpdatePolicy(): [UpdatePolicy | null, () => void] {
  const [policy, setPolicy] = useState<UpdatePolicy | null>(null);
  const reload = () => {
    api.updatePolicy().then(setPolicy, () => setPolicy(null));
  };
  useEffect(reload, []);
  return [policy, reload];
}

// Asks first when the IP is hidden, then checks. Returns the release found, or null; throws when the check fails.
export async function checkUpdates(policy: UpdatePolicy | null) {
  if (policy?.hidden_ip) {
    const ok = await confirm({
      message: 'Checking for updates connects to GitHub directly, so GitHub sees your IP address. Your relay keeps hiding it from everyone else.',
      okLabel: 'Check for updates',
    });
    if (!ok) return undefined;
  }
  return api.checkUpdate();
}

// The reminder in Settings: a line while the IP is hidden, and a button to check once it's been two weeks.
export function UpdateReminder({ onChecked }: { onChecked?: () => void }) {
  const [policy, reload] = useUpdatePolicy();
  const setUpdate = useShell((s) => s.setUpdate);
  const [status, setStatus] = useState('');
  if (!policy?.hidden_ip) return null;
  const check = async () => {
    try {
      const info = await checkUpdates(policy);
      if (info === undefined) return;
      if (info) {
        setUpdate(info);
        openUpdateDialog(info);
      } else {
        setStatus('You’re up to date.');
      }
    } catch {
      setStatus('Couldn’t check for updates right now.');
    } finally {
      reload();
      onChecked?.();
    }
  };
  return (
    <div className="flex flex-col gap-s-2" data-update-reminder>
      <Hint>{HIDDEN_IP_UPDATES}</Hint>
      {policy.remind ? (
        <div>
          <Button variant="outline" onClick={() => void check()}>
            {CHECK_SHOWS_IP}
          </Button>
        </div>
      ) : null}
      {status ? <Hint role="status">{status}</Hint> : null}
    </div>
  );
}

// The status-area reminder, next to the update button: shown only once it's been two weeks without a check.
export function UpdateReminderButton() {
  const [policy] = useUpdatePolicy();
  const openSettings = useShell((s) => s.openSettings);
  if (!policy?.remind) return null;
  const label = `${HIDDEN_IP_UPDATES} ${CHECK_SHOWS_IP}`;
  return (
    <Tip label={label} side="bottom">
      <Button variant="ghost" size="icon" aria-label={label} onClick={() => openSettings('updates')} className="size-8 rounded-full [&_svg]:size-[18px]">
        <BellRing />
      </Button>
    </Tip>
  );
}
