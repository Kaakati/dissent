// Website-verified handles as this app checked them. A server handle shaped like a domain (`@example.com`) is shown as
// verified only when the backend found the person's key in that domain's DNS itself; a peer's word is never used. The
// store asks the backend in batches and takes each finished check from the dissent://website-handle event.
//
// A lookup tells the resolver, and whoever runs the domain, that someone is looking at that handle. So drawing a
// handle only reads what this app already knows; a lookup runs only for `verify`, which the profile asks for when it
// opens and the member list when the pointer is on a handle. The backend reuses a result for hours.
import { useEffect } from 'react';
import { create } from 'zustand';
import { api, listenTo } from './api';
import type { HandleCheck, Id } from './types';

// How often an open profile asks to verify again. The backend decides whether that starts a lookup.
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

// The words for what a check reveals, shown wherever a check can start.
export function dnsCheckHint(domain: string, publicFallback: boolean): string {
  const base = `Checking this asks your DNS resolver about ${domain}.`;
  return publicFallback ? `${base} If it does not answer, Cloudflare's public resolver is asked too.` : base;
}

type Item = { root_id: Id; handle: string };
// Reads and verifies are asked for separately, so a read never stands in for a verify.
const asked = { read: new Map<string, number>(), verify: new Map<string, number>() };
const pending = { read: new Map<string, Item>(), verify: new Map<string, Item>() };
let flush: ReturnType<typeof setTimeout> | null = null;
let listening = false;

function listen(): void {
  if (listening) return;
  listening = true;
  listenTo<HandleCheck>('dissent://website-handle', noteHandleCheck).catch(() => {
    listening = false;
  });
}

// Asks the backend about a handle soon, together with every other badge that asked in the same moment. Only `verify`
// may start a lookup; without it the answer is what this app already knows.
export function requestHandleCheck(root: Id, handle: string, { verify = false, force = false } = {}): void {
  if (!mayBeDomain(handle)) return;
  listen();
  const kind = verify ? 'verify' : 'read';
  const key = keyOf(root, handle);
  const last = asked[kind].get(key);
  if (!force && last !== undefined && Date.now() - last < ASKED_WITHIN_MS) return;
  asked[kind].set(key, Date.now());
  pending[kind].set(key, { root_id: root, handle });
  if (flush) return;
  flush = setTimeout(() => {
    flush = null;
    for (const k of ['read', 'verify'] as const) {
      const items = [...pending[k].values()];
      pending[k].clear();
      if (items.length === 0) continue;
      api.websiteHandleStatus(items, k === 'verify').then((views) => views.forEach(noteHandleCheck)).catch(() => {});
    }
  }, 50);
}

// Verifies a handle the user is pointing at, unless it was asked about moments ago.
export const verifyHandle = (root: Id, handle: string | null | undefined): void => {
  if (handle) requestHandleCheck(root, handle, { verify: true });
};

// The state of `handle` for `root` while the caller is shown. Null when it cannot be a domain. With `verify` (an open
// profile) it is looked up if due, and asked about again every few minutes; without it nothing is looked up.
export function useWebsiteHandle(
  root: Id | null | undefined,
  handle: string | null | undefined,
  verify = false,
): HandleCheck | null {
  const domain = root && mayBeDomain(handle) ? handle : null;
  const check = useHandleChecks((s) => (root && domain ? s.checks[keyOf(root, domain)] : undefined));
  useEffect(() => {
    if (!root || !domain) return;
    requestHandleCheck(root, domain, { verify });
    if (!verify) return;
    const timer = window.setInterval(() => requestHandleCheck(root, domain, { verify, force: true }), RECHECK_EVERY_MS);
    return () => window.clearInterval(timer);
  }, [root, domain, verify]);
  if (!root || !domain) return null;
  return check ?? { root_id: root, handle: domain, state: 'unchecked', checked_ms: null, reason: null, checking: verify };
}
