// External links. Every link in the app goes through one handler: a click, a middle click or Enter on a link to another
// site opens this dialog first, and only Open hands the link to the backend (`open_external`), which opens it in the
// default browser. dissent:// links stay in the app. A link whose text names one site but goes to another gets a stronger
// warning. The backend also refuses anything but http, https and mailto, and passes back any link the webview tried to
// open by itself (`dissent://external-link`), so that asks first too.
import { useEffect, useState } from 'react';
import { CircleAlert, Copy, ExternalLink } from 'lucide-react';
import { Button } from '@/components/ui/button';
import { api, listenTo } from '@/lib/api';
import { copyText } from '@/lib/clipboard';
import { handleDeepLink } from '@/lib/deep-link';
import { openDialog } from '@/lib/dialog';
import { mismatchedHost } from '@/lib/markdown';
import { toast } from '@/lib/toasts';
import { cn } from '@/lib/utils';

const EXTERNAL = ['http:', 'https:', 'mailto:'];
// How much of a long address shows on each side of the cut.
const KEEP_START = 60;
const KEEP_END = 24;

// The part after the host, cut in the middle when it is long, so where it goes and how it ends both show.
export function shortRest(rest: string): string {
  return rest.length <= KEEP_START + KEEP_END + 1 ? rest : `${rest.slice(0, KEEP_START)}…${rest.slice(-KEEP_END)}`;
}

// The link split for showing: scheme, host (as the browser will see it, so punycode stays punycode), and the rest.
export function linkParts(href: string): { scheme: string; host: string; rest: string } {
  const url = new URL(href);
  if (url.protocol === 'mailto:') return { scheme: 'mailto:', host: url.pathname, rest: url.search };
  const userinfo = url.username ? `${url.username}${url.password ? ':…' : ''}@` : '';
  return { scheme: `${url.protocol}//${userinfo}`, host: url.host, rest: `${url.pathname === '/' && !url.search && !url.hash ? '' : url.pathname}${url.search}${url.hash}` };
}

function ExternalLinkBody({ href, claimed, close }: { href: string; claimed: string | null; close: () => void }) {
  const [busy, setBusy] = useState(false);
  // While this device hides its IP, a link opens in the browser, which goes out directly and shows the IP to the site.
  const [hidden, setHidden] = useState(false);
  useEffect(() => {
    api.getNetworkSettings().then((s) => setHidden(Boolean(s.hide_ip)), () => {});
  }, []);
  const { scheme, host, rest } = linkParts(href);
  const open = async () => {
    setBusy(true);
    try {
      await window.__TAURI__?.core.invoke('open_external', { url: href });
      close();
    } catch (err) {
      toast(String(err));
    }
    setBusy(false);
  };
  return (
    <div data-external-link className="flex flex-col gap-s-4">
      {claimed ? (
        <p data-link-mismatch role="alert" className="m-0 flex items-start gap-s-2 rounded-md border border-danger-text bg-danger-tint px-s-3 py-s-2 text-sm text-text-strong">
          <CircleAlert className="mt-0.5 size-4 shrink-0 text-danger-text" aria-hidden />
          <span>
            This link says <strong>{claimed}</strong> but goes to <strong>{host}</strong>. Open it only if you trust where it goes.
          </span>
        </p>
      ) : null}
      <p className="m-0 rounded-md bg-panel px-s-3 py-s-2 font-mono text-sm break-all text-muted-foreground" title={href} data-link-url>
        <span>{scheme}</span>
        <strong className="text-text-strong" data-link-host>{host}</strong>
        <span>{shortRest(rest)}</span>
      </p>
      {hidden ? (
        <p data-link-outside-protection className="m-0 text-sm text-muted-foreground">
          Opens in your browser, outside Dissent’s IP protection: the site sees your IP address unless your browser hides it.
        </p>
      ) : null}
      <div className="flex items-center gap-s-2">
        <Button variant="ghost" className="mr-auto" onClick={() => void copyText(href).then((ok) => toast(ok ? 'Link copied' : 'Could not copy the link', ok ? 'info' : 'error'))}>
          <Copy className="size-4" /> Copy link
        </Button>
        <Button variant="ghost" onClick={close}>Cancel</Button>
        <Button className={cn(claimed && 'bg-danger hover:bg-danger')} disabled={busy} onClick={() => void open()}>
          <ExternalLink className="size-4" /> Open
        </Button>
      </div>
    </div>
  );
}

// Asks before `href` opens outside the app. `label` is the link's text, for the mismatched-site warning.
export function confirmExternalLink(href: string, label = ''): void {
  const claimed = label ? mismatchedHost(label, href) : null;
  openDialog({
    title: claimed ? 'This link goes somewhere else' : 'You’re about to open an external link',
    content: (close) => <ExternalLinkBody href={href} claimed={claimed} close={close} />,
  });
}

// What a link does in the app: opens the dialog, stays in the app, is refused, or is left to the page.
export function routeLink(href: string, label = ''): 'external' | 'internal' | 'refused' | 'page' {
  let url: URL;
  try {
    url = new URL(href, window.location.href);
  } catch {
    return 'refused';
  }
  if (url.protocol === 'dissent:') {
    handleDeepLink(url.href);
    return 'internal';
  }
  if (EXTERNAL.includes(url.protocol)) {
    if (url.origin === window.location.origin) return 'page';
    confirmExternalLink(href, label);
    return 'external';
  }
  toast(`Links starting with ${url.protocol} are not opened.`);
  return 'refused';
}

// Every anchor in the app, caught before it can navigate or open a window: a click, a middle click and Enter (which
// clicks). Call once at start-up.
let installed = false;
export function installLinkGuard(): void {
  if (installed) return;
  installed = true;
  const onClick = (e: MouseEvent) => {
    if (e.defaultPrevented && e.type === 'click') return;
    const a = (e.target as Element | null)?.closest?.('a[href]') as HTMLAnchorElement | null;
    if (!a || e.button > 1) return;
    const raw = a.getAttribute('href') ?? '';
    if (raw.startsWith('#')) return;
    const where = routeLink(a.href || raw, a.textContent ?? '');
    if (where !== 'page') {
      e.preventDefault();
      e.stopPropagation();
    }
  };
  document.addEventListener('click', onClick, true);
  document.addEventListener('auxclick', onClick, true);
  listenTo<string>('dissent://external-link', (url) => {
    if (typeof url === 'string') confirmExternalLink(url);
  }).catch(() => {});
}
