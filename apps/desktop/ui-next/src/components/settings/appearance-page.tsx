// Settings > Appearance: message density and font scale, and whether emoticons become emoji. All are saved on this device
// and applied at once. Ported from
// the appearance section of views/settings.js and appearance.js.
import { useState } from 'react';
import { Block, Choices, CheckRow, CHECK_CLASS, Hint } from '@/components/settings/settings-ui';
import { useShellPrefs, setShellPrefs } from '@/lib/shell-prefs';
import { type Density, type FontScale, getAppearance, setAppearance } from '@/lib/appearance';

export function AppearancePage() {
  const [current, setCurrent] = useState(getAppearance);
  const pick = (next: Partial<typeof current>) => setCurrent(setAppearance(next));
  return (
    <div className="flex flex-col">
      <Choices legend="Message density">
        <Radio name="density" value="cozy" label="Cozy" checked={current.density === 'cozy'} onPick={() => pick({ density: 'cozy' as Density })} />
        <Radio name="density" value="compact" label="Compact" checked={current.density === 'compact'} onPick={() => pick({ density: 'compact' as Density })} />
      </Choices>
      <Choices legend="Font scale">
        {([90, 100, 110] as FontScale[]).map((scale) => (
          <Radio key={scale} name="scale" value={String(scale)} label={`${scale}%`} checked={current.scale === scale} onPick={() => pick({ scale })} />
        ))}
      </Choices>
      <MessagesBlock />
      <WindowBlock />
    </div>
  );
}

// How the composer treats what you type. Saved with the desktop settings; the browser preview keeps it for the session.
function MessagesBlock() {
  const convert = useShellPrefs((s) => s.convertEmoticons);
  return (
    <Block title="Text & messages">
      <CheckRow>
        <input
          type="checkbox"
          checked={convert}
          onChange={(e) => void setShellPrefs({ emoticons_as_text: !e.target.checked })}
          className={CHECK_CLASS}
        />
        <span>Convert emoticons to emoji</span>
      </CheckRow>
      <Hint>Typing :D, :P or &lt;3 sends 😄, 😛 or ❤️. Put a backslash before one to keep it as typed, like \:D.</Hint>
    </Block>
  );
}

// The window and sign-in choices. They live in the desktop app, so the browser preview shows them unchecked and inert.
function WindowBlock() {
  const prefs = useShellPrefs();
  return (
    <Block title="Window">
      <CheckRow>
        <input
          type="checkbox"
          checked={prefs.keepRunning}
          onChange={(e) => void setShellPrefs({ keep_running: e.target.checked })}
          className={CHECK_CLASS}
        />
        <span>Keep Dissent running when the window is closed</span>
      </CheckRow>
      <Hint>Dissent stays in the tray, so your servers stay available. Quit from the tray menu to close it.</Hint>
      <CheckRow>
        <input
          type="checkbox"
          checked={prefs.startAtLogin}
          disabled={!prefs.startAtLoginAvailable}
          onChange={(e) => void setShellPrefs({ start_at_login: e.target.checked })}
          className={CHECK_CLASS}
        />
        <span>Open Dissent when you sign in</span>
      </CheckRow>
      <Hint>{prefs.startAtLoginNote ?? 'It starts in the tray when “Keep Dissent running” is on.'}</Hint>
    </Block>
  );
}

function Radio({ name, value, label, checked, onPick }: { name: string; value: string; label: string; checked: boolean; onPick: () => void }) {
  return (
    <CheckRow>
      <input type="radio" name={name} value={value} checked={checked} onChange={onPick} className="m-0 size-5 shrink-0 accent-accent border border-input-border bg-input" />
      <span>{label}</span>
    </CheckRow>
  );
}
