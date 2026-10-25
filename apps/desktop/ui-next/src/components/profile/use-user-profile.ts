// The user_profile view of one person, read when they change and again when a profile-relevant action ran (see
// lib/profiles.ts). A failed read leaves `status` at 'failed', so the card can close instead of showing nothing.
import { useEffect, useState } from 'react';
import { api } from '@/lib/api';
import { useProfileVersion } from '@/lib/profiles';
import { useShell } from '@/lib/store';
import type { UserProfileView } from '@/lib/types';

export type ProfileLoad =
  | { status: 'loading' }
  | { status: 'failed' }
  | { status: 'ready'; view: UserProfileView };

export function useUserProfile(userId: string | null): ProfileLoad {
  const version = useProfileVersion((s) => s.version);
  const contacts = useShell((s) => s.contacts);
  const [result, setResult] = useState<{ userId: string; load: ProfileLoad } | null>(null);

  useEffect(() => {
    if (!userId) return;
    let live = true;
    api
      .userProfile(userId)
      .then((view) => {
        if (live) setResult({ userId, load: { status: 'ready', view } });
      })
      .catch(() => {
        if (live) setResult({ userId, load: { status: 'failed' } });
      });
    return () => {
      live = false;
    };
  }, [userId, version, contacts]);

  if (!userId || result?.userId !== userId) return { status: 'loading' };
  return result.load;
}
