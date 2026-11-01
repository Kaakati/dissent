// The Screen Share dialog: what to stream, then how. Sources come in two tabs with a thumbnail each. The quality block has a
// preset, the resolution and frame rate, and the bitrate. While streaming the same dialog changes any of them without stopping.
import { useEffect, useRef, useState } from 'react';
import { Monitor, Volume2 } from 'lucide-react';
import { Button } from '@/components/ui/button';
import { DialogFooter } from '@/components/ui/dialog';
import { api } from '@/lib/api';
import { openDialog } from '@/lib/dialog';
import {
  presetOf,
  PRESETS,
  saveShareSettings,
  savedShareSettings,
  SCREEN_BITRATE_MAX,
  SCREEN_BITRATE_MIN,
  SCREEN_BITRATE_STEP,
  sendShareQuality,
  type ShareSettings,
} from '@/lib/share-settings';
import { useShell } from '@/lib/store';
import type { ShareSource } from '@/lib/types';
import { cn } from '@/lib/utils';
import { VideoImage } from './call-body';

const LABEL = 'text-meta font-semibold tracking-[.02em] text-muted-foreground uppercase';

// What this window is streaming now, so the dialog can name it during a share.
let streaming: ShareSource | null = null;
function noteStreaming(source: ShareSource) {
  streaming = source;
}

function SourceThumb({ source }: { source: ShareSource }) {
  const [jpeg, setJpeg] = useState<string | null>(null);
  useEffect(() => {
    let live = true;
    void api.shareThumbnail(source.target).then((j) => { if (live) setJpeg(j); }).catch(() => {});
    return () => {
      live = false;
    };
  }, [source.target]);
  return (
    <span className="relative block aspect-video overflow-hidden rounded-md bg-black">
      {jpeg ? <VideoImage preview={{ jpeg }} label="" /> : <Monitor className="absolute inset-0 m-auto size-6 text-muted-foreground" aria-hidden="true" />}
    </span>
  );
}

function Segments<T extends number>({ label, value, options, onChange }: { label: string; value: T; options: { value: T; text: string }[]; onChange: (v: T) => void }) {
  return (
    <div className="grid min-w-0 flex-1 gap-s-1">
      <span className={LABEL}>{label}</span>
      <div role="radiogroup" aria-label={label} className="grid auto-cols-fr grid-flow-col overflow-hidden rounded-md border border-border">
        {options.map((o) => (
          <button
            key={o.value}
            type="button"
            role="radio"
            aria-checked={value === o.value}
            onClick={() => onChange(o.value)}
            className={cn('h-8 px-s-1 text-meta font-semibold text-text hover:bg-elevated-hover focus-visible:outline-2 focus-visible:-outline-offset-2 focus-visible:outline-ring', value === o.value && 'bg-accent text-white hover:bg-accent')}
          >
            {o.text}
          </button>
        ))}
      </div>
    </div>
  );
}

function ShareBody({ channel, live, close }: { channel: string; live: boolean; close: () => void }) {
  const toggleScreenShare = useShell((s) => s.toggleScreenShare);
  const [sources, setSources] = useState<ShareSource[]>([]);
  const [failed, setFailed] = useState<string | null>(null);
  const [tab, setTab] = useState<'display' | 'window'>('display');
  const [source, setSource] = useState<ShareSource | null>(live ? streaming : null);
  const [choosing, setChoosing] = useState(!live);
  const [settings, setSettings] = useState<ShareSettings>(savedShareSettings);
  const pending = useRef<ReturnType<typeof setTimeout> | null>(null);

  useEffect(() => {
    api.listShareSources().then(setSources, (e) => setFailed(String(e)));
  }, []);

  const change = (next: Partial<ShareSettings>) => {
    const merged = { ...settings, ...next };
    setSettings(merged);
    saveShareSettings(merged);
    // A running share follows a moment after the slider stops, since a new rate is not worth sending for each step of a drag.
    if (!live) return;
    if (pending.current) clearTimeout(pending.current);
    pending.current = setTimeout(() => void sendShareQuality(merged).catch(() => {}), 250);
  };
  const pick = (next: ShareSource) => {
    setSource(next);
    setChoosing(false);
    if (live) {
      // Replaces what is shared, with no gap in the call for anyone else.
      noteStreaming(next);
      void toggleScreenShare(next.target);
    }
  };
  const goLive = () => {
    if (!source) return;
    noteStreaming(source);
    close();
    void toggleScreenShare(source.target);
  };
  const preset = presetOf(settings);
  const shown = sources.filter((s) => s.target.kind === tab);

  if (choosing) {
    return (
      <div className="grid gap-s-3">
        <div role="tablist" aria-label="What to share" className="flex gap-s-1">
          {(
            [
              ['window', 'Applications'],
              ['display', 'Screens'],
            ] as const
          ).map(([kind, text]) => (
            <button
              key={kind}
              type="button"
              role="tab"
              aria-selected={tab === kind}
              onClick={() => setTab(kind)}
              className={cn('h-8 rounded-md px-s-3 text-sm font-medium text-muted-foreground hover:bg-elevated-hover', tab === kind && 'bg-elevated text-text-strong')}
            >
              {text}
            </button>
          ))}
        </div>
        {failed ? <p className="text-meta text-muted-foreground">{failed}</p> : null}
        {!failed && sources.length && !shown.length ? <p className="text-meta text-muted-foreground">{tab === 'window' ? 'No windows to share.' : 'No screens to share.'}</p> : null}
        <div className="dissent-scroll grid max-h-[320px] grid-cols-2 gap-s-2 overflow-y-auto">
          {shown.map((s) => (
            <button
              key={`${s.target.kind}-${s.target.id}`}
              type="button"
              data-share-source
              onClick={() => pick(s)}
              className="grid min-w-0 gap-s-1 rounded-lg p-s-1 text-left text-[13px] text-text hover:bg-elevated-hover focus-visible:outline-2 focus-visible:outline-ring"
            >
              <SourceThumb source={s} />
              <span className="truncate">{s.primary ? `${s.name} (primary)` : s.name}</span>
            </button>
          ))}
        </div>
        <DialogFooter>
          {source ? <Button variant="outline" onClick={() => setChoosing(false)}>Back</Button> : <Button variant="outline" onClick={close}>Cancel</Button>}
        </DialogFooter>
      </div>
    );
  }
  return (
    <div className="grid gap-s-4">
      <div className="grid gap-s-1">
        <span className={LABEL}>What you’re streaming</span>
        <div className="flex items-center gap-s-2 rounded-lg border border-border px-s-3 py-s-2 text-sm text-text-strong">
          <Monitor className="size-4 shrink-0" aria-hidden="true" />
          <span className="min-w-0 flex-1 truncate" data-share-what>{source?.name ?? 'Your screen'}</span>
          <Button variant="outline" size="sm" onClick={() => setChoosing(true)}>Change</Button>
        </div>
        <p className="text-meta text-muted-foreground">Sound isn’t shared with your screen yet. People hear your microphone only.</p>
      </div>
      <div className="grid gap-s-1">
        <span className={LABEL}>Streaming to</span>
        <div className="flex items-center gap-s-2 rounded-lg border border-border px-s-3 py-s-2 text-sm text-text-strong">
          <Volume2 className="size-4 shrink-0" aria-hidden="true" />
          <span className="min-w-0 truncate">{channel}</span>
        </div>
      </div>
      <div className="grid gap-s-1">
        <span className={LABEL}>Stream quality</span>
        <div className="grid gap-s-3 rounded-lg border border-border p-s-3">
          <select
            aria-label="Quality preset"
            value={preset}
            onChange={(e) => {
              const chosen = PRESETS.find((p) => p.name === e.target.value);
              if (chosen) change({ height: chosen.height, fps: chosen.fps });
            }}
            className="h-9 rounded-md border border-input-border bg-input px-s-2 text-sm text-text-strong focus-visible:outline-2 focus-visible:outline-accent"
          >
            {PRESETS.map((p) => (
              <option key={p.name} value={p.name}>{p.name}</option>
            ))}
            {preset === 'Custom' ? <option value="Custom">Custom</option> : null}
          </select>
          <div className="flex gap-s-3">
            <Segments
              label="Resolution"
              value={settings.height}
              onChange={(height) => change({ height })}
              options={[
                { value: 720, text: '720' },
                { value: 1080, text: '1080' },
                { value: 1440, text: '1440' },
                { value: 0, text: 'SOURCE' },
              ]}
            />
            <Segments
              label="Frame rate"
              value={settings.fps}
              onChange={(fps) => change({ fps })}
              options={[
                { value: 15, text: '15' },
                { value: 30, text: '30' },
                { value: 60, text: '60' },
              ]}
            />
          </div>
          <label className="grid gap-s-1 text-[13px] text-text">
            <span className="flex items-baseline justify-between gap-s-3">
              <span className={LABEL}>Bitrate</span>
              <span className="text-meta text-muted-foreground" data-share-bitrate>{`${(settings.kbps / 1000).toFixed(settings.kbps % 100 ? 2 : 1)} Mbps`}</span>
            </span>
            <input
              type="range"
              aria-label="Screen share bitrate"
              min={SCREEN_BITRATE_MIN}
              max={SCREEN_BITRATE_MAX}
              step={SCREEN_BITRATE_STEP}
              value={settings.kbps}
              onChange={(e) => change({ kbps: Number(e.target.value) })}
              className="accent-accent"
            />
            <span className="text-meta text-muted-foreground">A slow connection lowers the frame rate first, so text stays sharp. Your computer may not reach 60 fps at high resolutions.</span>
          </label>
        </div>
      </div>
      <DialogFooter>
        {live ? (
          <Button onClick={close}>Done</Button>
        ) : (
          <>
            <Button variant="outline" onClick={() => setChoosing(true)}>Back</Button>
            <Button onClick={goLive}>Go Live</Button>
          </>
        )}
      </DialogFooter>
    </div>
  );
}

export function openShareDialog(channel: string, live: boolean) {
  openDialog({
    title: live ? 'Stream settings' : 'Screen Share',
    content: (close) => <ShareBody channel={channel} live={live} close={close} />,
  });
}
