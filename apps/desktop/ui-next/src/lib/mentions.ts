// Mentions in the composer: the "@" token under the caret, the members a token matches, and the mentions a sent text
// carries. The engine validates every mention on arrival (a member, and @everyone only with the permission), so the
// composer only offers what the user may send; it does not decide what others are notified about.
import type { Id, MentionRef, Member } from './types';

// A mention the user picked. `everyone` stands for the whole server.
export type MentionPick = { kind: 'user'; id: Id; name: string } | { kind: 'everyone' };

// The "@" that starts the word the caret is in, with the text typed after it. Null when the caret is not in one.
export function activeMention(text: string, caret: number): { start: number; query: string } | null {
  const before = text.slice(0, caret);
  const m = /(^|\s)@([^\s@]*)$/.exec(before);
  if (!m) return null;
  return { start: before.length - m[2].length - 1, query: m[2] };
}

export interface Candidate {
  pick: MentionPick;
  label: string; // what the row shows: the name, or "everyone"
  handle: string | null; // the @handle, when the member has one
  avatar: string | null; // the member's root id (the avatar's seed) or null for @everyone
  rank: number; // lower sorts first: a prefix of the name ranks above a word start, which ranks above a substring
  online: boolean;
}

// The most rows the list shows. More matches are counted in its header, and typing narrows them.
export const MAX_ROWS = 8;

// The members a query matches, by name or handle, ignoring case, best first: by match rank, then online before
// offline, then by name. Banned members and the user (`me`) are left out. @everyone is offered only when `everyone` is
// set (the user holds the permission), and only for a query it matches. All matches are returned; the list shows
// the first MAX_ROWS.
export function candidatesFor(members: Member[], query: string, everyone: boolean, me?: Id | null): Candidate[] {
  const q = query.toLowerCase();
  const rankOf = (text: string | null | undefined): number => {
    if (!text) return -1;
    const t = text.toLowerCase();
    if (t.startsWith(q)) return 0;
    if (t.split(/[\s._-]+/).some((w) => w.startsWith(q))) return 1;
    if (t.includes(q)) return 2;
    return -1;
  };
  const out: Candidate[] = [];
  for (const m of members) {
    if (m.banned || m.root_id === me) continue;
    const rank = Math.min(...[rankOf(m.display_name), rankOf(m.handle)].filter((r) => r >= 0), 3);
    if (rank === 3) continue;
    out.push({
      pick: { kind: 'user', id: m.root_id, name: m.display_name },
      label: m.display_name,
      handle: m.handle ?? null,
      avatar: m.root_id,
      rank,
      online: m.online === true || (m.status !== undefined && m.status !== 'offline'),
    });
  }
  if (everyone && rankOf('everyone') >= 0) {
    out.push({ pick: { kind: 'everyone' }, label: 'everyone', handle: null, avatar: null, rank: rankOf('everyone'), online: true });
  }
  return out.sort((a, b) => a.rank - b.rank || Number(b.online) - Number(a.online) || a.label.localeCompare(b.label));
}

// The text a pick inserts: "@Name" for a person, "@everyone" for the server.
export const mentionText = (pick: MentionPick): string => (pick.kind === 'everyone' ? '@everyone' : `@${pick.name}`);

// The text split into plain runs and mentions. A mention is "@Name" for a resolved person (ref), or "@everyone" when
// `everyone` is set (ref 'everyone'). The token must start a word and end one, so "@Maya" does not match "@Mayan".
export interface MentionSegment {
  text: string;
  ref?: MentionRef | 'everyone';
}

const isWordChar = (ch: string | undefined): boolean => ch !== undefined && /[\p{L}\p{N}_]/u.test(ch);

export function splitMentions(text: string, refs: MentionRef[], everyone: boolean): MentionSegment[] {
  const tokens: { token: string; ref: MentionRef | 'everyone' }[] = refs.map((r) => ({ token: `@${r.display_name}`, ref: r }));
  if (everyone) tokens.push({ token: '@everyone', ref: 'everyone' });
  const out: MentionSegment[] = [];
  let plain = '';
  let i = 0;
  while (i < text.length) {
    const hit = tokens.find(
      (t) => text.startsWith(t.token, i) && !isWordChar(text[i - 1]) && !isWordChar(text[i + t.token.length]),
    );
    if (hit) {
      if (plain) out.push({ text: plain });
      plain = '';
      out.push({ text: hit.token, ref: hit.ref });
      i += hit.token.length;
    } else {
      plain += text[i];
      i += 1;
    }
  }
  if (plain) out.push({ text: plain });
  return out;
}

// The mentions argument of a send: the member ids and whether @everyone is in it. Undefined when the text carries none,
// so an ordinary send keeps the arguments it always had.
export function mentionArg(picks: MentionPick[]): { users: Id[]; everyone: boolean } | undefined {
  if (picks.length === 0) return undefined;
  return {
    users: picks.flatMap((p) => (p.kind === 'user' ? [p.id] : [])),
    everyone: picks.some((p) => p.kind === 'everyone'),
  };
}

// The picks a sent text still carries: a person whose "@Name" is in the text, and @everyone when its token is. A pick
// removed from the text (the user deleted it) is not sent.
export function picksInText(text: string, picks: MentionPick[]): MentionPick[] {
  const seen = new Set<string>();
  const out: MentionPick[] = [];
  for (const p of picks) {
    const key = p.kind === 'everyone' ? 'everyone' : p.id;
    if (seen.has(key)) continue;
    const token = mentionText(p);
    const at = text.indexOf(token);
    const end = at + token.length;
    const bounded = at >= 0 && (end === text.length || !/[\p{L}\p{N}_]/u.test(text[end]));
    if (bounded) {
      seen.add(key);
      out.push(p);
    }
  }
  return out;
}
