// Settings > Devices: the devices that hold this identity, with Remove for the others, and the code that links a new one.
// Device linking: on the root device, paste the new device's code, approve, and copy the grant back. On the new device,
// paste the grant and finish, which stores the identity and restarts the engine. Ported from views/guardians.js.
import { useEffect, useState } from 'react';
import { api } from '@/lib/api';
import { copyText } from '@/lib/clipboard';
import { useShell } from '@/lib/store';
import { stampLabel } from '@/lib/time';
import { toast } from '@/lib/toasts';
import type { DeviceView } from '@/lib/types';
import { Block, Hint } from '@/components/settings/settings-ui';
import { Button } from '@/components/ui/button';
import { Input } from '@/components/ui/input';

const BLOCK_HEAD = 'mt-s-2 text-meta font-semibold tracking-[.02em] text-muted-foreground uppercase';

export function DevicesPage() {
  const [devices, setDevices] = useState<DeviceView[] | null>(null);
  const [error, setError] = useState('');
  const [version, setVersion] = useState(0);

  // Reloaded after a device is removed, so its row goes.
  useEffect(() => {
    api
      .listDevices()
      .then((list) => setDevices(list))
      .catch((err) => setError(`Could not load devices: ${err?.message ?? err}`));
  }, [version]);

  if (error) return <p className="text-danger-text">{error}</p>;
  if (!devices) return <p className="text-muted-foreground">Loading…</p>;

  const remove = async (id: string) => {
    try {
      await api.revokeDevice(id);
      toast('Device removed', 'info');
      setVersion((v) => v + 1);
    } catch (err) {
      toast(err);
    }
  };
  const linkCode = async () => {
    try {
      const code = await api.linkDeviceCode();
      if (await copyText(code)) toast('Link code copied', 'info');
    } catch (err) {
      toast(err);
    }
  };

  return (
    <div className="flex min-h-full max-w-[740px] flex-col gap-s-3">
      <h2 className="mb-s-2 text-head font-bold text-text-strong">Devices</h2>
      <p className="text-muted-foreground">Devices that hold your identity. Remove one to revoke its access.</p>
      <ul aria-label="Devices" className="m-0 flex list-none flex-col gap-0.5 rounded-lg border border-border bg-panel p-s-2">
        {devices.map((d) => (
          <li key={d.device_id} className="flex min-h-14 items-center gap-s-3 rounded-lg px-s-2">
            <span className="flex min-w-0 flex-1 flex-col">
              <span className="text-body text-text-strong">{d.name}</span>
              <span className="text-meta text-muted-foreground">
                {d.this_device ? 'This device' : `Last seen ${stampLabel(Number(d.last_seen_ms))}`}
              </span>
            </span>
            {d.this_device ? null : (
              <Button variant="outline" className="border-danger-text text-danger-text hover:bg-danger-tint" onClick={() => void remove(d.device_id)}>
                Remove
              </Button>
            )}
          </li>
        ))}
      </ul>
      <div className="flex justify-end">
        <Button variant="outline" onClick={() => void linkCode()}>
          Link a new device
        </Button>
      </div>
      <Linking />
    </div>
  );
}

function Linking() {
  // A dissent://link/ link fills the code box, whether this page was open or not. The person still presses Approve.
  const pendingLink = useShell((s) => s.pendingDeviceLink);
  const [codeIn, setCodeIn] = useState(() => useShell.getState().pendingDeviceLink ?? '');
  const [shownLink, setShownLink] = useState(pendingLink);
  if (pendingLink !== shownLink) {
    setShownLink(pendingLink);
    if (pendingLink) setCodeIn(pendingLink);
  }
  useEffect(() => {
    if (pendingLink) useShell.setState({ pendingDeviceLink: null });
  }, [pendingLink]);
  const [grantOut, setGrantOut] = useState('');
  const [grantIn, setGrantIn] = useState('');

  const approve = async () => {
    const link = codeIn.trim();
    if (!link) return;
    try {
      setGrantOut(String(await api.approveDevice(link)));
      toast('New device approved. Send it the grant.', 'info');
    } catch (err) {
      toast(err);
    }
  };
  const copyGrant = async () => {
    if (grantOut && (await copyText(grantOut))) toast('Grant copied', 'info');
  };
  const finish = async () => {
    const grant = grantIn.trim();
    if (!grant) return;
    try {
      await api.finishDeviceLink(grant);
      toast('This device is linked. Dissent is restarting.', 'info');
    } catch (err) {
      toast(err);
    }
  };

  return (
    <Block className="mt-s-4 gap-s-2">
      <h3 className={BLOCK_HEAD}>Approve a new device</h3>
      <Hint className="text-muted-foreground">Paste the link code shown on the new device, then send back the grant it produces.</Hint>
      <div className="flex items-center gap-s-2">
        <Input placeholder="dissent://link/…" aria-label="Link code from the new device" value={codeIn} onChange={(e) => setCodeIn(e.target.value)} />
        <Button onClick={() => void approve()}>Approve</Button>
      </div>
      <div className="flex items-center gap-s-2">
        <Input readOnly placeholder="The grant appears here" aria-label="Grant for the new device" value={grantOut} />
        <Button variant="outline" onClick={() => void copyGrant()}>
          Copy grant
        </Button>
      </div>
      <h3 className={BLOCK_HEAD}>Link this device</h3>
      <Hint className="text-muted-foreground">Paste the grant from a device that already has your identity.</Hint>
      <div className="flex items-center gap-s-2">
        <Input placeholder="Paste the grant from the other device" aria-label="Grant from the other device" value={grantIn} onChange={(e) => setGrantIn(e.target.value)} />
        <Button onClick={() => void finish()}>Finish linking</Button>
      </div>
    </Block>
  );
}
