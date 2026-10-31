// Settings > Contacts & privacy: whether people can add you by address. On by default. Off takes your address off the
// DHT; contact links keep working. Below it, your profile card as other people see it. Ported from the profile section of
// views/settings.js.
import { useEffect, useState } from 'react';
import { ProfileCardBody } from '@/components/profile/profile-card';
import { useUserProfile } from '@/components/profile/use-user-profile';
import { Block, CheckRow, CHECK_CLASS, Hint } from '@/components/settings/settings-ui';
import { api } from '@/lib/api';
import { setShellPrefs, useShellPrefs } from '@/lib/shell-prefs';
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

// Whether your contacts see your contact list, for mutual friends on their profiles. On by default. Off sends it to nobody.
function FriendsBlock() {
  const [on, setOn] = useState(false);

  useEffect(() => {
    api
      .friendsShared()
      .then((value) => setOn(Boolean(value)))
      .catch(() => {});
  }, []);

  const change = async (next: boolean) => {
    const previous = on;
    setOn(next);
    try {
      await api.setFriendsShared(next);
    } catch (err) {
      setOn(previous);
      toast(err);
    }
  };

  return (
    <Block title="Friends">
      <CheckRow>
        <input
          type="checkbox"
          aria-label="Show my friends list to my friends"
          checked={on}
          onChange={(e) => void change(e.target.checked)}
          className={CHECK_CLASS}
        />
        <span>Show my friends list to my friends</span>
      </CheckRow>
      <Hint>
        Your friends see the full list of your contacts, not only the ones they have in common with you. Strangers and server members never see it.
      </Hint>
    </Block>
  );
}

// Link previews: the sender's device fetches a link once to show its title, description, site name, theme colour and picture.
// Off until it is turned on here, or in the question the composer asks once.
function LinkPreviewsBlock() {
  const [on, setOn] = useState(false);

  useEffect(() => {
    api
      .linkPreviews()
      .then((value) => setOn(Boolean(value)))
      .catch(() => {});
  }, []);

  const change = async (next: boolean) => {
    const previous = on;
    setOn(next);
    try {
      await api.setLinkPreviews(next);
    } catch (err) {
      setOn(previous);
      toast(err);
    }
  };

  return (
    <Block title="Link previews">
      <CheckRow>
        <input
          type="checkbox"
          aria-label="Show link previews in my messages"
          checked={on}
          onChange={(e) => void change(e.target.checked)}
          className={CHECK_CLASS}
        />
        <span>Show link previews in my messages</span>
      </CheckRow>
      <Hint>
        Dissent fetches each link from your device, so the site sees your IP address. Recipients never contact the site: they get the preview in your message.
      </Hint>
    </Block>
  );
}

// Location and camera details in photos and videos. Saved with the desktop settings; on by default.
function PhotoDetailsBlock() {
  const strip = useShellPrefs((s) => s.stripPhotoMetadata);
  return (
    <Block title="Photos and videos">
      <CheckRow>
        <input
          type="checkbox"
          aria-label="Remove identifying details from photos and videos by default"
          checked={strip}
          onChange={(e) => void setShellPrefs({ keep_photo_metadata: !e.target.checked })}
          className={CHECK_CLASS}
          data-strip-photo-metadata
        />
        <span>Remove identifying details from photos and videos by default</span>
      </CheckRow>
      <Hint>
        Phones and cameras write where, when and with what a photo or video was made into the file, and editors add names. With this on, each photo, and each video that names the device it was recorded with, starts with "Remove details" on when you attach it: it goes as a copy without them, called photo.jpg or video.mp4. You can change it for each file before sending. The file on your computer stays as it is.
      </Hint>
    </Block>
  );
}

// Where website handle checks go. Saved with the desktop settings; off by default, so only the system resolver is asked.
function HandleChecksBlock() {
  const publicFallback = useShellPrefs((s) => s.dnsPublicFallback);
  return (
    <Block title="Website handles">
      <CheckRow>
        <input
          type="checkbox"
          aria-label="Ask a public DNS resolver when mine does not answer"
          checked={publicFallback}
          onChange={(e) => void setShellPrefs({ dns_public_fallback: e.target.checked })}
          className={CHECK_CLASS}
          data-dns-public-fallback
        />
        <span>Ask a public DNS resolver when mine does not answer</span>
      </CheckRow>
      <Hint>
        A handle like @example.com is checked only when you open that person's profile or point at the handle. Checking
        it asks your DNS resolver about example.com, and whoever runs that domain can see the lookup. With this on, a
        lookup your resolver cannot answer also goes to Cloudflare's public resolver (1.1.1.1).
      </Hint>
    </Block>
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
      <PhotoDetailsBlock />
      <HandleChecksBlock />
      <LinkPreviewsBlock />
      <FriendsBlock />
      <Block>
        <Hint>This is your card as other people see it.</Hint>
        <CardPreview />
      </Block>
    </div>
  );
}
