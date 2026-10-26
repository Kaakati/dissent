// Settings > Notifications & sounds: message and call sounds, the ringtone, what a desktop notification shows, and the
// volume. Desktop notification content stays available while Play sounds is off, as in the current UI. Ported from the
// settings in views/settings.js; the players are in sounds.ts.
import { Choices, Block, CheckRow, CHECK_CLASS, Hint } from '@/components/settings/settings-ui';
import { setShellPrefs, useShellPrefs } from '@/lib/shell-prefs';
import { setSoundSettings, useSoundSettings } from '@/lib/sound-settings';

export function SoundsPage() {
  const s = useSoundSettings();
  const shell = useShellPrefs();
  // The per-kind toggles and the volume are disabled while Play sounds is off.
  const off = !s.enabled;
  return (
    <div className="flex flex-col">
      <Block title="Sounds">
        <CheckRow>
          <input
            type="checkbox"
            checked={s.enabled}
            onChange={(e) => setSoundSettings({ enabled: e.target.checked })}
            className={CHECK_CLASS}
          />
          <span>Play sounds</span>
        </CheckRow>
        <Hint>Do Not Disturb silences message and call sounds. Incoming calls still ring.</Hint>
      </Block>

      <Block title="What plays">
        <Toggle label="Message notifications" checked={s.messages} disabled={off} onChange={(messages) => setSoundSettings({ messages })} />
        <Toggle label="Call and voice sounds" checked={s.calls} disabled={off} onChange={(calls) => setSoundSettings({ calls })} />
        <Toggle label="Ringtone" checked={s.ringtone} disabled={off} onChange={(ringtone) => setSoundSettings({ ringtone })} />
        <Toggle label="Desktop notifications" checked={!shell.notificationsMuted} disabled={false} onChange={(on) => void setShellPrefs({ notifications_muted: !on })} />
      </Block>

      <Choices legend="Show in notifications">
        {(
          [
            ['both', 'Sender and message'],
            ['sender', 'Sender only'],
            ['none', 'Nothing (just “New message”)'],
          ] as const
        ).map(([value, label]) => (
          <CheckRow key={value}>
            <input
              type="radio"
              name="notification-content"
              value={value}
              checked={s.content === value}
              onChange={() => setSoundSettings({ content: value })}
              className="m-0 size-5 shrink-0 accent-accent border border-input-border bg-input"
            />
            <span>{label}</span>
          </CheckRow>
        ))}
      </Choices>

      <Block title="Volume">
        <div className="flex h-8 items-center gap-s-3 text-body text-text-strong">
          <span>Level</span>
          <input
            type="range"
            min={0}
            max={100}
            step={5}
            value={s.volume}
            disabled={off}
            aria-label="Sound volume"
            onChange={(e) => setSoundSettings({ volume: Number(e.target.value) })}
            className="max-w-[260px] flex-1 accent-accent disabled:opacity-50"
          />
          <output className="min-w-12 text-right font-mono text-meta">{s.volume}%</output>
        </div>
      </Block>
    </div>
  );
}

function Toggle({ label, checked, disabled, onChange }: { label: string; checked: boolean; disabled: boolean; onChange: (next: boolean) => void }) {
  return (
    <CheckRow>
      <input type="checkbox" checked={checked} disabled={disabled} onChange={(e) => onChange(e.target.checked)} className={CHECK_CLASS} />
      <span>{label}</span>
    </CheckRow>
  );
}
