// Settings > Contacts & privacy: whether people can add you by address. On by default. Off takes your address off the
// DHT; contact links keep working. Below it, your profile card as other people see it. Ported from the profile section of
// views/settings.js.
import { useEffect, useState } from 'react';
import { ProfileCardBody } from '@/components/profile/profile-card';
import { useUserProfile } from '@/components/profile/use-user-profile';
import { Block, CheckRow, CHECK_CLASS, Hint } from '@/components/settings/settings-ui';
import { api } from '@/lib/api';
import { useShell } from '@/lib/store';
import { toast } from '@/lib/toasts';

// Your card, read as others read it, with the actions left out.
function CardPreview() {
  const me = useShell((s) => s.me);
  const load = useUserProfile(me?.root_id ?? null);
  if (!me) return null;
  if (load.status !== 'ready') return <p className="text-sm leading-[19px] text-muted-foreground">Loading preview…</p>;
  return (
    <div className="w-[300px] overflow-hidden rounded-lg bg-ink">
      <ProfileCardBody view={load.view} userId={me.root_id} preview />
    </div>
  );
}

export function PrivacyPage() {
  // Unknown until the engine answers; the box stays unticked until then, as before.
  const [on, setOn] = useState(false);

  useEffect(() => {
    api
      .addressDiscovery()
      .then((value) => setOn(Boolean(value)))
      .catch(() => {});
  }, []);

  const change = async (next: boolean) => {
    const previous = on;
    setOn(next);
    try {
      await api.setAddressDiscovery(next);
    } catch (err) {
      setOn(previous);
      toast(err);
    }
  };

  return (
    <div className="flex flex-col">
      <Block title="Adding by address">
        <CheckRow>
          <input
            type="checkbox"
            aria-label="Let people add me by address"
            checked={on}
            onChange={(e) => void change(e.target.checked)}
            className={CHECK_CLASS}
          />
          <span>Let people add me by address</span>
        </CheckRow>
        <Hint>
          Your address is published on the Mainline DHT with your device ids, so people who have it can add you. Turning this off removes it. Contact links work either way.
        </Hint>
      </Block>
      <Block>
        <Hint>This is your card as other people see it.</Hint>
        <CardPreview />
      </Block>
    </div>
  );
}
