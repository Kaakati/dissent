// Website-verified handles as this app checked them. A server handle shaped like a domain (`@example.com`) is shown as
// verified only when the backend found the person's key in that domain's DNS itself; a peer's word is never used. The
// store asks the backend in batches, takes each finished check from the dissent://website-handle event, and asks
// again every few minutes, so a pass that goes stale (or a record that is removed) stops showing.
import { useEffect } from 'react';
import { create } from 'zustand';
import { api, listenTo } from './api';
import type { HandleCheck, Id } from './types';

// How often a shown handle is asked about again. The backend decides whether that starts a lookup.
export const RECHECK_EVERY_MS = 5 * 60_000;
// A handle asked about within this long is not asked about again by another badge.
const ASKED_WITHIN_MS = 30_000;

const keyOf = (root: Id, handle: string) => `${root}|${handle}`;

// Only a handle with a dot can be a domain. The backend does the real check.
export const mayBeDomain = (handle: string | null | undefined): handle is string =>
  !!handle && handle.includes('.') && !handle.startsWith('.') && !handle.endsWith('.');

interface ChecksState {
  checks: Record<string, HandleCheck>;
}

export const useHandleChecks = create<ChecksState>()(() => ({ checks: {} }));

export function noteHandleCheck(check: HandleCheck): void {
  useHandleChecks.setState((s) => ({ checks: { ...s.checks, [keyOf(check.root_id, check.handle)]: check } }));
}

const asked = new Map<string, number>();
let pending = new Map<string, { root_id: Id; handle: string }>();
let flush: ReturnType<typeof setTimeout> | null = null;
let listening = false;

function listen(): void {
  if (listening) return;
  listening = true;
  listenTo<HandleCheck>('dissent://website-handle', noteHandleCheck).catch(() => {
    listening = false;
  });
}

// Asks the backend about a handle soon, together with every other badge that asked in the same moment.
export function requestHandleCheck(root: Id, handle: string, force = false): void {
  if (!mayBeDomain(handle)) return;
  listen();
  const key = keyOf(root, handle);
  const last = asked.get(key);
  if (!force && last !== undefined && Date.now() - last < ASKED_WITHIN_MS) return;
  asked.set(key, Date.now());
  pending.set(key, { root_id: root, handle });
  if (flush) return;
  flush = setTimeout(() => {
    const items = [...pending.values()];
    pending = new Map();
    flush = null;
    api.websiteHandleStatus(items).then((views) => views.forEach(noteHandleCheck)).catch(() => {});
  }, 50);
}

// The state of `handle` for `root`, kept fresh while the caller is shown. Null when it cannot be a domain.
export function useWebsiteHandle(root: Id | null | undefined, handle: string | null | undefined): HandleCheck | null {
  const domain = root && mayBeDomain(handle) ? handle : null;
  const check = useHandleChecks((s) => (root && domain ? s.checks[keyOf(root, domain)] : undefined));
  useEffect(() => {
    if (!root || !domain) return;
    requestHandleCheck(root, domain);
    const timer = window.setInterval(() => requestHandleCheck(root, domain, true), RECHECK_EVERY_MS);
    return () => window.clearInterval(timer);
  }, [root, domain]);
  if (!root || !domain) return null;
  return check ?? { root_id: root, handle: domain, state: 'unchecked', checked_ms: null, reason: null, checking: true };
}
