// Settings > My identity: the avatar, the display name, the two addresses, the recovery phrase, and importing one.
// Ported from the account section of views/settings.js.
import { useState } from 'react';
import { Image as ImageIcon, KeyRound, Trash2 } from 'lucide-react';
import { AddressCard } from '@/components/address-card';
import { BannerPicker } from '@/components/banner-picker';
import { chooseImage } from '@/components/settings/avatar-editor';
import { Block, Hint, ROW } from '@/components/settings/settings-ui';
import { Button } from '@/components/ui/button';
import { Input } from '@/components/ui/input';
import { Textarea } from '@/components/ui/textarea';
import { UserAvatar } from '@/components/user-avatar';
import { has } from '@/lib/features';
import { api } from '@/lib/api';
import { confirm } from '@/lib/dialog';
import { imageOf } from '@/lib/images';
import { toast } from '@/lib/toasts';
import { clearProfileBanner, setProfileBanner, setProfileBannerAnimated, setProfileBannerPreset } from '@/lib/actions';
import { bannerOf } from '@/lib/banners';
import { useShell } from '@/lib/store';

// Profile picture: the avatar, with change and remove beside it.
function AvatarSection() {
  const me = useShell((s) => s.me);
  const setAvatar = useShell((s) => s.setAvatar);
  const clearAvatar = useShell((s) => s.clearAvatar);
  const hasPicture = imageOf(me?.root_id) !== null;
  return (
    <Block title="Avatar">
      <div className="flex items-center gap-s-4">
        <UserAvatar name={me?.display_name ?? '?'} seed={me?.root_id} size="xl" />
        <div className={ROW}>
          <Button
            variant="outline"
            onClick={() => chooseImage({ shape: 'circle', onApply: (bytes) => setAvatar(bytes) })}
          >
            <ImageIcon />
            Change avatar
          </Button>
          {hasPicture ? (
            <Button variant="secondary" className="text-danger-text" onClick={() => void clearAvatar()}>
              <Trash2 />
              Remove avatar
            </Button>
          ) : null}
        </div>
      </div>
    </Block>
  );
}

// The profile banner, under the avatar: the presets and an upload, as for a server. A pick is published at once.
function BannerSection() {
  const me = useShell((s) => s.me);
  return (
    <Block title="Banner" className="max-w-[440px]">
      <BannerPicker
        value={bannerOf(me?.banner, me?.banner_preset, me?.banner_poster)}
        onPreset={(id) => void setProfileBannerPreset(id)}
        onUpload={(bytes, animated) => (animated ? setProfileBannerAnimated(bytes, animated.poster) : setProfileBanner(bytes))}
        onRemove={() => void clearProfileBanner()}
      />
      <Hint>Shown behind your profile card, in DMs and on your full profile. A 16:9 picture of up to 512 KB, a looping GIF of up to 2 MB, or a colour.</Hint>
    </Block>
  );
}

function DisplayName() {
  const me = useShell((s) => s.me);
  const setDisplayName = useShell((s) => s.setDisplayName);
  const [name, setName] = useState(me?.display_name ?? '');
  const save = () => {
    const next = name.trim();
    if (!next) {
      toast('Display name cannot be empty');
      return;
    }
    void setDisplayName(next);
  };
  return (
    <Block title="Display name">
      <div className={ROW}>
        <Input className="flex-1" maxLength={64} aria-label="Display name" value={name} onChange={(e) => setName(e.target.value)} />
        <Button onClick={save}>Save</Button>
      </div>
    </Block>
  );
}

function RecoveryPhrase() {
  // The phrase is fetched each time it is shown, and dropped when it is hidden.
  const [phrase, setPhrase] = useState<string | null>(null);
  const toggle = async () => {
    if (phrase !== null) {
      setPhrase(null);
      return;
    }
    try {
      setPhrase(await api.identityExportPhrase());
    } catch (err) {
      toast(err);
    }
  };
  return (
    <Block title="Recovery phrase">
      <div className={ROW}>
        <Button variant="outline" onClick={() => void toggle()}>
          <KeyRound />
          {phrase !== null ? 'Hide recovery phrase' : 'Show recovery phrase'}
        </Button>
      </div>
      {phrase !== null ? (
        <>
          <Hint className="text-warn">Anyone with these 24 words controls your identity. Keep them offline and never share them.</Hint>
          <Textarea readOnly rows={3} aria-label="Recovery phrase" value={phrase} />
        </>
      ) : null}
    </Block>
  );
}

function ImportPhrase() {
  const restoreIdentity = useShell((s) => s.restoreIdentity);
  const [phrase, setPhrase] = useState('');
  const importPhrase = async () => {
    const words = phrase.trim();
    if (!words) return;
    if (!(await confirm({ message: 'Replace the identity on this device with the one from this phrase?', okLabel: 'Import' }))) return;
    if (await restoreIdentity(words)) setPhrase('');
  };
  return (
    <Block title="Import recovery phrase">
      <Hint className="text-muted-foreground">Restoring replaces the identity stored on this device.</Hint>
      <Textarea
        rows={3}
        placeholder="24 recovery words, separated by spaces"
        aria-label="Recovery phrase to import"
        value={phrase}
        onChange={(e) => setPhrase(e.target.value)}
      />
      <div className="flex justify-end">
        <Button variant="outline" className="border-danger-text text-danger-text hover:bg-danger-tint" onClick={() => void importPhrase()}>
          <KeyRound />
          Import phrase
        </Button>
      </div>
    </Block>
  );
}

export function AccountPage() {
  const me = useShell((s) => s.me);
  return (
    <div className="flex flex-col">
      {has('avatars') ? <AvatarSection /> : null}
      {has('avatars') ? <BannerSection /> : null}
      <DisplayName />
      <Block title="Identity">
        <AddressCard kind="user" id={me?.root_id} label="User address" qr />
        <AddressCard kind="device" id={me?.device_id} label="Device address" />
      </Block>
      <RecoveryPhrase />
      <ImportPhrase />
    </div>
  );
}
