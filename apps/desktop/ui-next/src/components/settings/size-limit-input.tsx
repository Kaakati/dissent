// A file size limit, typed as a number and a unit (MB, GB, TB), from 1 MB to 1 TB. Saved with its own button, so a half
// typed number is never applied.
import { useState } from 'react';
import { Button } from '@/components/ui/button';
import { Input } from '@/components/ui/input';
import { parseLimit, splitLimit, UNITS, type SizeUnit } from '@/lib/limits';

export function SizeLimitInput({
  label,
  hint,
  value,
  name,
  disabled,
  onSave,
}: {
  label: string;
  hint?: string;
  value: number;
  name: string;
  disabled?: boolean;
  onSave: (bytes: number) => Promise<unknown> | void;
}) {
  // What is typed, until it is saved; the stored value shows otherwise, so a change made elsewhere shows at once.
  const [draft, setDraft] = useState<{ text: string; unit: SizeUnit } | null>(null);
  const stored = splitLimit(value);
  const { text, unit } = draft ?? { text: stored.value, unit: stored.unit };
  const setText = (next: string) => setDraft({ text: next, unit });
  const setUnit = (next: SizeUnit) => setDraft({ text, unit: next });
  const parsed = parseLimit(text, unit);
  const error = 'error' in parsed ? parsed.error : null;
  const changed = !error && 'bytes' in parsed && parsed.bytes !== value;
  return (
    <div data-size-limit={name} className="flex flex-col gap-s-2">
      <label className="text-sm font-semibold text-text-strong" htmlFor={`limit-${name}`}>
        {label}
      </label>
      <div className="flex items-center gap-s-2">
        <Input
          id={`limit-${name}`}
          aria-label={`${label}, amount`}
          inputMode="decimal"
          className="h-8 w-28"
          disabled={disabled}
          value={text}
          aria-invalid={error ? true : undefined}
          onChange={(e) => setText(e.target.value)}
        />
        <select
          aria-label={`${label}, unit`}
          className="h-8 rounded-md border border-border bg-transparent px-s-2 text-ui text-text"
          disabled={disabled}
          value={unit}
          onChange={(e) => setUnit(e.target.value as SizeUnit)}
        >
          {(Object.keys(UNITS) as SizeUnit[]).map((u) => (
            <option key={u} value={u}>
              {u}
            </option>
          ))}
        </select>
        <Button variant="outline" className="h-8" disabled={disabled || !changed} onClick={() => {
            if ('bytes' in parsed) void Promise.resolve(onSave(parsed.bytes)).then(() => setDraft(null));
          }}>
          Save
        </Button>
      </div>
      {error ? (
        <p role="alert" data-limit-error className="m-0 text-sm text-danger-text">
          {error}
        </p>
      ) : hint ? (
        <p className="m-0 text-meta text-muted-foreground">{hint}</p>
      ) : null}
    </div>
  );
}
