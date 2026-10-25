// Settings > Updates: the version, the update check, and the licences. A newer release found here opens the same dialog
// as the header button. Ported from the updates section of views/settings.js.
import { useEffect, useState } from 'react';
import { Download } from 'lucide-react';
import { Block, Hint } from '@/components/settings/settings-ui';
import { openUpdateDialog } from '@/components/settings/update-dialog';
import { Button } from '@/components/ui/button';
import { api } from '@/lib/api';
import { useShell } from '@/lib/store';
import type { UpdateInfo } from '@/lib/types';

const Link = ({ href, children }: { href: string; children: string }) => (
  <a href={href} target="_blank" rel="noopener noreferrer">
    {children}
  </a>
);

export function UpdatesPage() {
  const setUpdate = useShell((s) => s.setUpdate);
  const [version, setVersion] = useState('Version …');
  const [status, setStatus] = useState('');
  const [checking, setChecking] = useState(false);
  const [found, setFound] = useState<UpdateInfo | null>(null);

  useEffect(() => {
    api
      .appVersion()
      .then((v) => setVersion(`Dissent ${v}`))
      .catch(() => setVersion('Dissent'));
  }, []);

  const check = async () => {
    setChecking(true);
    setStatus('Checking…');
    setFound(null);
    try {
      const info = await api.checkUpdate();
      if (info) {
        setStatus(`Update available: v${info.version}`);
        setFound(info);
        setUpdate(info);
      } else {
        setStatus('You’re up to date.');
      }
    } catch {
      setStatus('Couldn’t check for updates right now.');
    } finally {
      setChecking(false);
    }
  };

  return (
    <div className="flex flex-col">
      <Block>
        <img src="/app-icon-96.png" alt="Dissent logo" width={64} height={64} className="block size-16" />
        <p>{version}</p>
        <div className="flex items-center gap-s-2">
          <Button variant="outline" disabled={checking} onClick={() => void check()}>
            <Download />
            Check for updates
          </Button>
        </div>
        <Hint role="status">{status}</Hint>
        {found ? (
          <div className="flex items-center gap-s-2">
            <Button onClick={() => openUpdateDialog(found)}>View update</Button>
          </div>
        ) : null}
        <Hint>Decentralised chat. Your identity and messages stay on your devices and in the network, with no central server.</Hint>
      </Block>

      <Block title="Licenses">
        <ul className="m-0 list-none space-y-1 p-0 text-sm leading-[19px]">
          <li>
            Dissent is licensed under <Link href="https://opensource.org/license/mit">MIT</Link> or{' '}
            <Link href="https://www.apache.org/licenses/LICENSE-2.0">Apache-2.0</Link>.
          </li>
          <li>
            Icons by Lucide (ISC). <Link href="https://lucide.dev/license">Lucide license</Link>.
          </li>
        </ul>
      </Block>
    </div>
  );
}
