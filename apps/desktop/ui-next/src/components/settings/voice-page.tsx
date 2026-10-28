// Settings › Voice (the old UI's views/settings-voice.js): the devices, the volumes, the mic test with its level meter, the input
// profile, the sensitivity, push to talk, and the Custom profile's processing and automatic gain control. A change
// applies on the next audio tick. While this page is open the mic runs, so its meter moves; leaving the page stops it.
import { useEffect, useRef, useState, type KeyboardEvent } from 'react';
import { api } from '@/lib/api';
import { toast } from '@/lib/toasts';
import { loadVoice, PROFILES, chooseDevice, systemLabel, updateVoice, useVoice } from '@/lib/voice';
import { acceleratorOf, keyLabel } from '@/components/voice/accelerator';
import { levelPosition, useMicMonitor } from '@/components/voice/mic-monitor';
import type { AudioDevice, AudioDevices, VoiceSettings } from '@/lib/types';
import { cn } from '@/lib/utils';
import { Block, CHECK_CLASS, Choices, CheckRow, Hint } from './settings-ui';
import { applyHardwareVideo, hardwareVideoWanted, saveHardwareVideoWanted } from '@/lib/webcodecs';

const BARS = 40;

export function VoicePage() {
  const settings = useVoice((s) => s.settings);
  const [devices, setDevices] = useState<AudioDevices | null>(null);
  const monitor = useMicMonitor();

  useEffect(() => {
    void loadVoice().catch(toast);
    void api.audioDevices().then(setDevices).catch(toast);
  }, []);

  if (!settings || !devices) return null;

  // A new microphone or speaker: the monitor and any running test restart on it.
  const changeDevice = (direction: 'input' | 'output', name: string | null) => {
    chooseDevice(direction, name)
      .then(() => api.audioDevices())
      .then(setDevices)
      .then(() => monitor.restart())
      .catch(toast);
  };

  return (
    <div className="flex min-w-0 flex-1 flex-col gap-s-6">
      <Block title="Devices">
        <div className="grid grid-cols-2 gap-s-3 max-[900px]:grid-cols-1">
          <DeviceField label="Microphone" list={devices.inputs} chosen={devices.input} fallback={devices.default_input} onChoose={(name) => changeDevice('input', name)} />
          <DeviceField label="Speaker" list={devices.outputs} chosen={devices.output} fallback={devices.default_output} onChoose={(name) => changeDevice('output', name)} />
        </div>
      </Block>

      <Block title="Volume">
        <VolumeRow label="Microphone Volume" value={settings.input_volume} onChange={(v) => updateVoice({ input_volume: v })} />
        <VolumeRow label="Speaker Volume" value={settings.output_volume} onChange={(v) => updateVoice({ output_volume: v })} />
      </Block>

      <MicTest db={monitor.db} testing={monitor.testing} onToggle={() => void monitor.toggleTest()} />

      <Choices legend="Input Profile">
        {PROFILES.map((p) => (
          <label key={p.value} className="flex cursor-pointer items-start gap-s-3 py-1 text-body text-text-strong">
            <input
              type="radio"
              name="input-profile"
              value={p.value}
              checked={settings.profile === p.value}
              onChange={() => updateVoice({ profile: p.value }, { now: true })}
              className="mt-1 size-5 shrink-0 accent-accent"
            />
            <span className="flex flex-col">
              <span>{p.label}</span>
              <span className="text-sm leading-[19px] text-muted-foreground">{p.sub}</span>
            </span>
          </label>
        ))}
        {settings.profile === 'custom' ? null : <Hint>{presetNote(settings.profile)}</Hint>}
      </Choices>

      {settings.profile === 'custom' ? <CustomBlocks settings={settings} /> : null}
      <VideoBlock />

      <Block title="Echo Cancellation Engine">
        <label className="flex min-h-8 items-center gap-s-3 text-body text-text-strong">
          <span>Engine</span>
          <select
            value={settings.echo_engine ?? 'standard'}
            aria-label="Echo cancellation engine"
            onChange={(e) => updateVoice({ echo_engine: e.target.value as VoiceSettings['echo_engine'] }, { now: true })}
            className={cn(SELECT, 'w-auto')}
          >
            <option value="standard">Standard (current)</option>
            <option value="web_rtc">WebRTC AEC3 (experimental)</option>
          </select>
        </label>
        <Hint>
          WebRTC AEC3 replaces our echo canceller, noise suppression and gain control while it is selected. Switching during a call restarts the processing.
        </Hint>
      </Block>

      <Sensitivity settings={settings} live={monitor.db} liveThreshold={monitor.thresholdDb} />

      <Block title="Push to Talk">
        <CheckRow>
          <input
            type="checkbox"
            checked={settings.push_to_talk}
            onChange={(e) => updateVoice({ push_to_talk: e.target.checked }, { now: true })}
            className={CHECK_CLASS}
          />
          <span>Push to Talk</span>
        </CheckRow>
        <PushKey settings={settings} />
        <Hint>Hold the key to talk. It works while Dissent is in the background.</Hint>
      </Block>
    </div>
  );
}

function presetNote(profile: VoiceSettings['profile']): string {
  return profile === 'studio'
    ? 'Studio sends the microphone unprocessed: no noise suppression, echo cancellation or gain control.'
    : 'Voice Isolation uses strong noise suppression, with echo cancellation and gain control.';
}

// A device choice: the system default, then every device. Each option names the default when it is the default.
function DeviceField({
  label,
  list,
  chosen,
  fallback,
  onChoose,
}: {
  label: string;
  list: AudioDevice[];
  chosen: string | null;
  fallback: string | null;
  onChoose: (name: string | null) => void;
}) {
  return (
    <label className="flex min-w-0 flex-col gap-s-1 text-sm leading-[19px] text-text-strong">
      <span className="text-muted-foreground">{label}</span>
      <select
        value={chosen ?? ''}
        aria-label={label}
        onChange={(e) => onChoose(e.target.value || null)}
        className={SELECT}
      >
        <option value="">{fallback ? `${systemLabel()} (${fallback})` : systemLabel()}</option>
        {list.map((d) => (
          <option key={d.name} value={d.name}>{d.name}</option>
        ))}
      </select>
    </label>
  );
}

const SELECT = 'min-h-8 w-full min-w-0 rounded-lg border border-input-border bg-input px-s-2 text-ui text-text-strong focus-visible:outline-2 focus-visible:outline-accent';

// A 0 to 200 percent volume, saved after a short pause.
function VolumeRow({ label, value, onChange }: { label: string; value: number; onChange: (v: number) => void }) {
  return (
    <label className="flex min-h-8 items-center gap-s-3 text-body text-text-strong">
      <span>{label}</span>
      <input
        type="range"
        min={0}
        max={200}
        step={1}
        value={value}
        aria-label={label}
        onChange={(e) => onChange(Number(e.target.value))}
        className="max-w-[260px] flex-1 accent-accent"
      />
      <output className="min-w-12 text-right font-mono text-meta">{`${value}%`}</output>
    </label>
  );
}

// The mic test: the button plays the mic back through the speaker, and the 40-bar meter shows the level.
function MicTest({ db, testing, onToggle }: { db: number; testing: boolean; onToggle: () => void }) {
  const lit = Math.round(levelPosition(db) * BARS);
  return (
    <Block title="Mic Test">
      <Hint>Plays your microphone back through your speaker. Use headphones, or the sound feeds back.</Hint>
      <div className="flex flex-wrap items-center gap-s-3">
        <button
          type="button"
          aria-pressed={testing}
          onClick={onToggle}
          className="h-9 rounded-lg border border-border-strong bg-transparent px-s-4 text-ui font-semibold text-text-strong hover:bg-hover"
        >
          {testing ? 'Stop Mic Test' : 'Mic Test'}
        </button>
        <div aria-hidden="true" className="flex h-[22px] min-w-40 flex-1 items-end gap-0.5">
          {Array.from({ length: BARS }, (_, i) => (
            <span key={i} className={cn('h-full flex-1 rounded-[1px]', i < lit ? 'bg-success' : 'bg-input-border')} />
          ))}
        </div>
      </div>
    </Block>
  );
}

// Noise suppression, echo cancellation and gain control, shown for the Custom profile only.
function CustomBlocks({ settings }: { settings: VoiceSettings }) {
  return (
    <>
      <Block title="Processing">
        <label className="flex min-h-8 items-center gap-s-3 text-body text-text-strong">
          <span>Noise Suppression</span>
          <select
            value={settings.noise_suppression}
            aria-label="Noise suppression"
            onChange={(e) => updateVoice({ noise_suppression: e.target.value as VoiceSettings['noise_suppression'] }, { now: true })}
            className={cn(SELECT, 'w-auto')}
          >
            <option value="standard">Standard</option>
            <option value="off">None</option>
          </select>
        </label>
        <Toggle label="Echo Cancellation" checked={settings.echo_cancellation} onChange={(on) => updateVoice({ echo_cancellation: on }, { now: true })} />
      </Block>
      <Block title="Advanced">
        <Toggle label="Automatic Gain Control" checked={settings.agc} onChange={(on) => updateVoice({ agc: on }, { now: true })} />
      </Block>
    </>
  );
}

// Hardware video decoding: this window decodes received video with WebCodecs, which costs about a third of the CPU of the
// JPEG previews. Off by default until measured in release builds; turning it on is opt-in.
function VideoBlock() {
  const [on, setOn] = useState(hardwareVideoWanted);
  const [status, setStatus] = useState<string | null>(null);
  const change = (next: boolean) => {
    saveHardwareVideoWanted(next);
    setOn(next);
    void applyHardwareVideo(next).then((active) =>
      setStatus(next && !active ? 'Not available here: this window cannot decode H.264 with WebCodecs, so previews stay on.' : null),
    );
  };
  return (
    <Block title="Video">
      <Toggle label="Hardware video decoding (experimental)" checked={on} onChange={change} />
      {status ? <Hint>{status}</Hint> : null}
    </Block>
  );
}

function Toggle({ label, checked, onChange }: { label: string; checked: boolean; onChange: (on: boolean) => void }) {
  return (
    <CheckRow>
      <input type="checkbox" checked={checked} onChange={(e) => onChange(e.target.checked)} className={CHECK_CLASS} />
      <span>{label}</span>
    </CheckRow>
  );
}

// The input sensitivity: automatic (the handle follows the room) or manual (drag the handle). The track shows the live
// level, so the handle can be set against the voice being picked up.
function Sensitivity({ settings, live, liveThreshold }: { settings: VoiceSettings; live: number; liveThreshold: number | null }) {
  const auto = settings.auto_sensitivity;
  const shownThreshold = auto && liveThreshold !== null ? Math.round(liveThreshold) : Math.round(settings.threshold_db);
  return (
    <Block title="Input Sensitivity">
      <CheckRow>
        <input
          type="checkbox"
          checked={auto}
          onChange={(e) => updateVoice({ auto_sensitivity: e.target.checked }, { now: true })}
          className={CHECK_CLASS}
        />
        <span>Automatically Adjust Input Sensitivity</span>
      </CheckRow>
      <div className="relative h-8">
        <div className="absolute top-1/2 right-0 left-0 h-2 -translate-y-1/2 overflow-hidden rounded-full border border-input-border bg-input">
          <div
            className="absolute inset-y-0 left-0 bg-success"
            style={{ width: `${levelPosition(live) * 100}%` }}
          />
        </div>
        <input
          type="range"
          min={-80}
          max={-10}
          step={1}
          value={Math.round(settings.threshold_db)}
          disabled={auto}
          aria-label="Input sensitivity threshold"
          onChange={(e) => updateVoice({ threshold_db: Number(e.target.value) })}
          className="relative h-8 w-full accent-text-strong disabled:opacity-55"
        />
      </div>
      <div className="flex items-baseline justify-between gap-s-3 text-meta">
        <span className="text-muted-foreground">
          {auto ? 'Following the room noise. The handle shows the threshold in use.' : 'Drag the handle to set the level. Green shows your mic.'}
        </span>
        <output className="font-mono text-text-strong">{`${shownThreshold} dB`}</output>
      </div>
    </Block>
  );
}

// The push-to-talk key: click it, then press the key (Escape cancels). It shows only while push to talk is on.
function PushKey({ settings }: { settings: VoiceSettings }) {
  const [binding, setBinding] = useState(false);
  const ref = useRef<HTMLButtonElement>(null);
  // The binder takes the keys it is given: it has the focus while it waits, as the current UI's binder does.
  useEffect(() => {
    if (binding) ref.current?.focus();
  }, [binding]);
  if (!settings.push_to_talk) return null;

  const onKeyDown = (e: KeyboardEvent<HTMLButtonElement>) => {
    if (!binding) return;
    e.preventDefault();
    e.stopPropagation();
    if (e.key === 'Escape') {
      setBinding(false);
      return;
    }
    const accel = acceleratorOf(e.nativeEvent);
    if (!accel) return;
    setBinding(false);
    updateVoice({ ptt_key: accel }, { now: true });
  };

  return (
    <button
      ref={ref}
      type="button"
      aria-label="Push to talk key"
      onClick={() => setBinding(true)}
      onKeyDown={onKeyDown}
      className={cn(
        'min-h-8 self-start rounded-lg border border-input-border bg-input px-s-3 text-ui font-medium text-text-strong focus-visible:outline-2 focus-visible:outline-accent',
        binding && 'border-accent text-accent-text',
      )}
    >
      {binding ? 'Press a key (Esc cancels)' : keyLabel(settings.ptt_key)}
    </button>
  );
}
