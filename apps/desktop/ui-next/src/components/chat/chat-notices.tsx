// The blocks a conversation shows around its messages: the welcome at the top of a history, the key-change notice, the
// age gate of an age-restricted channel, and the empty pane. Ported from the old UI's views/chat.js (welcomeNode, keyChangeBanner,
// ageGate, emptyPane).
import { CircleAlert, Hash, MessageCircle, Shield } from 'lucide-react';
import { openSafetyDialog } from '@/components/dialogs/safety-dialog';
import { UserAvatar } from '@/components/user-avatar';
import { shortOf } from '@/lib/address';
import { ageConfirmed, ageKey, rememberAge, useChat, type PeerProfile } from '@/lib/chat';
import { useShell } from '@/lib/store';
import { EmptyNode, SkeletonRows } from './parts';
import type { ChatView } from './use-chat-view';

// The top of a history that has no older messages: the channel's welcome, or the start of a DM or a group.
export function Welcome({ view, peerProfile }: { view: ChatView; peerProfile: PeerProfile | null }) {
  const group = useShell((s) => s.group);
  if (view.group) {
    const members = group?.members ?? [];
    const creator = members.find((m) => m.root_id === group?.creator)?.display_name;
    const lead = creator ? `${creator} started this group. ` : '';
    return (
      <div className="welcome">
        <div className="faces">
          {members.map((m) => (
            <UserAvatar
              key={m.root_id}
              name={m.display_name}
              seed={m.root_id}
              size="xl"
              className="-mr-3 size-14 text-[17px] shadow-[0_0_0_4px_var(--chat)]"
            />
          ))}
        </div>
        <h2>{view.title}</h2>
        <p className="muted">{`${lead}Only its current members can read it, and anyone removed loses access to new messages.`}</p>
      </div>
    );
  }
  if (view.dm && view.peer) {
    const mutual = peerProfile?.mutual_communities?.length ?? 0;
    const suffix = shortOf('user', view.peer);
    return (
      <div className="welcome">
        <UserAvatar name={view.title} seed={view.peer} size="xl" />
        <h2>{view.title}</h2>
        <div className="suffix font-mono">{`${view.title}·${suffix}`}</div>
        <p className="muted">This conversation goes straight between your devices. Nobody in between keeps a copy.</p>
        <p className="muted">{mutual ? `${mutual} mutual server${mutual === 1 ? '' : 's'}` : 'No mutual servers'}</p>
      </div>
    );
  }
  return (
    <div className="welcome">
      <div className="welcome-mark">
        <Hash aria-hidden="true" />
      </div>
      <h2>{`Welcome to #${view.title}!`}</h2>
      <p className="muted">{`This is the start of the #${view.title} channel.`}</p>
    </div>
  );
}

// Amber notice at the top of a DM whose peer's key changed. The button opens the safety dialog to compare.
export function KeyChangeBanner({ view, keyChangedMs }: { view: ChatView; keyChangedMs: number }) {
  if (!view.peer) return null;
  const peer = view.peer;
  return (
    <div className="key-banner inline" role="status">
      <CircleAlert aria-hidden="true" />
      <span>{`${view.title}'s key changed. Compare the safety number before you trust new messages.`}</span>
      <button type="button" className="ghost" onClick={() => void openSafetyDialog(peer, { name: view.title, keyChangedMs })}>
        Compare
      </button>
    </div>
  );
}

// An age-restricted channel asks once per channel on this device before it shows anything.
export function AgeGate({ view }: { view: ChatView }) {
  const key = view.target.kind === 'channel' ? ageKey(view.target.communityId, view.target.channelId) : '';
  const confirm = () => {
    rememberAge(key);
    useChat.getState().confirmAge(key);
  };
  return (
    <div className="empty age-gate">
      <Shield aria-hidden="true" />
      <p className="empty-title">Age-restricted channel</p>
      <p className="muted">Confirm that you are of age to view this channel. You confirm once per channel on this device.</p>
      <button type="button" className="primary" onClick={confirm}>I am of age</button>
    </div>
  );
}

// Whether an age gate is still in front of a channel: not confirmed in this session, nor remembered on this device.
export function needsAgeGate(view: ChatView, sessionSet: ReadonlySet<string>): boolean {
  if (view.target.kind !== 'channel' || view.visibility !== 'age') return false;
  return !ageConfirmed(ageKey(view.target.communityId, view.target.channelId), sessionSet);
}

// What the list shows with no conversation open: a server's loading state, or the pick-a-channel prompt.
export function EmptyPane() {
  const route = useShell((s) => s.route);
  const community = useShell((s) => s.community);
  if (route.kind === 'server') {
    return community ? (
      <EmptyNode icon={Hash} title="Pick a channel" text="Choose a channel on the left to start reading." />
    ) : (
      <SkeletonRows count={6} />
    );
  }
  return <EmptyNode icon={MessageCircle} title="Your direct messages" text="Pick a conversation, or add a contact to start one." />;
}
