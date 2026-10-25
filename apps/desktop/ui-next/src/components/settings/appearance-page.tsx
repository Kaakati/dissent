// Settings > Appearance: message density and font scale. Both are saved on this device and applied at once. Ported from
// the appearance section of views/settings.js and appearance.js.
import { useState } from 'react';
import { Choices, CheckRow } from '@/components/settings/settings-ui';
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
    </div>
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
