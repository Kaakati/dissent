// Settings > My identity: the avatar, the display name, the two addresses, a website handle, the recovery phrase, and
// importing one.
// Ported from the account section of views/settings.js.
import { useEffect, useState } from 'react';
import { AtSign, Copy, Image as ImageIcon, KeyRound, RefreshCw, Trash2 } from 'lucide-react';
import { AddressCard } from '@/components/address-card';
import { BannerPicker } from '@/components/banner-picker';
import { chooseImage } from '@/components/settings/avatar-editor';
import { Block, Hint, ROW } from '@/components/settings/settings-ui';
import { Button } from '@/components/ui/button';
import { Input } from '@/components/ui/input';
import { Textarea } from '@/components/ui/textarea';
import { UserAvatar } from '@/components/user-avatar';
import { VerifiedMark, isVerified } from '@/components/website-handle';
import { has } from '@/lib/features';
import { api } from '@/lib/api';
import { copyText } from '@/lib/clipboard';
import { confirm } from '@/lib/dialog';
import { imageOf } from '@/lib/images';
import { toast } from '@/lib/toasts';
import { clearProfileBanner, setProfileBanner, setProfileBannerAnimated, setProfileBannerPreset } from '@/lib/actions';
import { bannerOf } from '@/lib/banners';
import { useShell } from '@/lib/store';
import type { HandleCheck, WebsiteHandleSetup } from '@/lib/types';
import { noteHandleCheck } from '@/lib/website-handles';

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

// One line of the record to publish: its label, the value in monospace, and a copy button.
function RecordRow({ label, value }: { label: string; value: string }) {
  const copy = async () => {
    if (await copyText(value)) toast(`${label} copied`, 'info');
    else toast('Could not copy it', 'error');
  };
  return (
    <div className="flex min-w-0 items-center gap-s-2">
      <span className="w-12 shrink-0 text-sm text-muted-foreground">{label}</span>
      <code data-record={label.toLowerCase()} className="mono min-w-0 flex-1 rounded-sm bg-input px-s-2 py-1 text-sm break-all text-text-strong">{value}</code>
      <button type="button" className="icon-btn" aria-label={`Copy ${label.toLowerCase()}`} title={`Copy ${label.toLowerCase()}`} onClick={() => void copy()}>
        <Copy className="size-4" aria-hidden="true" />
      </button>
    </div>
  );
}

// The last check of your own record: verified, the reason it failed, or not checked yet.
function CheckLine({ check }: { check: HandleCheck | null }) {
  if (isVerified(check)) {
    return (
      <p data-website-check="verified" className="flex items-center gap-1.5 text-sm text-text">
        <VerifiedMark />
        Verified. People who share a server with you see the mark once their app checks it too.
      </p>
    );
  }
  if (check?.state === 'failed') {
    return <p data-website-check="failed" role="alert" className="text-sm text-danger-text">{`Not verified. ${check.reason ?? ''}`}</p>;
  }
  return (
    <p data-website-check="unchecked" className="text-sm text-muted-foreground">
      {check?.reason ? `Not checked yet. ${check.reason}` : 'Not checked yet.'}
    </p>
  );
}

// A handle like @example.com, proved by a TXT record on a domain you own. Each viewer's app checks the record itself.
// Others see it as your server handle, so after a pass it can be claimed in every server you are in.
function WebsiteHandle() {
  const communities = useShell((s) => s.communities);
  const [setup, setSetup] = useState<WebsiteHandleSetup | null>(null);
  const [domain, setDomain] = useState('');
  const [checking, setChecking] = useState(false);
  const [claiming, setClaiming] = useState(false);

  useEffect(() => {
    api.websiteHandleSetup().then(setSetup).catch(() => setSetup(null));
  }, []);

  const save = async () => {
    const next = domain.trim();
    if (!next) return;
    try {
      setSetup(await api.setWebsiteHandle(next));
      setDomain('');
    } catch (err) {
      toast(err);
    }
  };
  const check = async () => {
    setChecking(true);
    try {
      const result = await api.checkWebsiteHandle();
      noteHandleCheck(result);
      setSetup((s) => (s ? { ...s, check: result } : s));
    } catch (err) {
      toast(err);
    } finally {
      setChecking(false);
    }
  };
  const remove = async () => {
    const message = 'Stop using this website handle on this device? Servers keep it as your handle until you claim another one, and nobody sees the mark once the TXT record is gone.';
    if (!(await confirm({ message, okLabel: 'Remove' }))) return;
    try {
      setSetup(await api.removeWebsiteHandle());
    } catch (err) {
      toast(err);
    }
  };
  const claimEverywhere = async (handle: string) => {
    setClaiming(true);
    let claimed = 0;
    for (const c of communities) {
      try {
        await api.claimHandle(c.id, handle);
        claimed += 1;
      } catch {
        // Taken or refused in that server: the count below says so.
      }
    }
    setClaiming(false);
    const open = useShell.getState().community;
    if (open) await useShell.getState().loadCommunity(open.id);
    const servers = `${communities.length} server${communities.length === 1 ? '' : 's'}`;
    toast(`@${handle} is your handle in ${claimed} of ${servers}`, claimed ? 'info' : 'error');
  };

  if (!setup) return null;
  const handle = setup.handle;
  return (
    <Block title="Website handle">
      {setup.domain && setup.record_name && handle ? (
        <div data-website-handle className="flex max-w-[560px] flex-col gap-s-3">
          <p className="flex items-center gap-1.5 text-ui font-semibold text-text-strong">
            <AtSign className="size-4 text-muted-foreground" aria-hidden="true" />
            {handle}
          </p>
          <Hint>{`In the DNS settings for ${setup.domain}, add this TXT record. Some DNS providers want only "_dissent" as the name.`}</Hint>
          <div className="flex flex-col gap-s-2">
            <RecordRow label="Type" value={setup.record_type} />
            <RecordRow label="Name" value={setup.record_name} />
            <RecordRow label="Value" value={setup.record_value} />
          </div>
          <CheckLine check={setup.check} />
          <div className={`${ROW} flex-wrap`}>
            <Button onClick={() => void check()} disabled={checking}>
              <RefreshCw className={checking ? 'animate-spin' : undefined} />
              {checking ? 'Checking…' : 'Check now'}
            </Button>
            {isVerified(setup.check) && communities.length ? (
              <Button variant="outline" onClick={() => void claimEverywhere(handle)} disabled={claiming}>
                <AtSign />
                Use as my handle in my servers
              </Button>
            ) : null}
            <Button variant="secondary" className="text-danger-text" onClick={() => void remove()}>
              <Trash2 />
              Remove
            </Button>
          </div>
        </div>
      ) : (
        <>
          <Hint>If you own a website, prove a handle like @example.com. You publish one DNS record, and everyone's app checks it for itself.</Hint>
          <div className={ROW}>
            <Input
              className="flex-1"
              maxLength={253}
              placeholder="example.com"
              aria-label="Your domain"
              value={domain}
              onChange={(e) => setDomain(e.target.value)}
              onKeyDown={(e) => {
                if (e.key === 'Enter') void save();
              }}
            />
            <Button onClick={() => void save()} disabled={!domain.trim()}>Set up</Button>
          </div>
        </>
      )}
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
      <WebsiteHandle />
      <RecoveryPhrase />
      <ImportPhrase />
    </div>
  );
}
