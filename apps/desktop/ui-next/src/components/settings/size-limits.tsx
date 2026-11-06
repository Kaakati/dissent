// Settings › Downloads: the largest file this device shares and the largest it downloads. 10 GB each until changed.
import { useEffect } from 'react';
import { SizeLimitInput } from '@/components/settings/size-limit-input';
import { DEFAULT_LIMIT, useSizeLimits } from '@/lib/limits';

export function SizeLimits() {
  const limits = useSizeLimits((s) => s.limits);
  const load = useSizeLimits((s) => s.load);
  const save = useSizeLimits((s) => s.save);
  useEffect(() => {
    void load();
  }, [load]);
  const upload = limits?.max_upload ?? DEFAULT_LIMIT;
  const download = limits?.max_download ?? DEFAULT_LIMIT;
  return (
    <div data-size-limits className="flex flex-col gap-s-4">
      <SizeLimitInput
        name="upload"
        label="Largest file I share"
        value={upload}
        disabled={!limits}
        hint="A server can set a lower limit for what its members share."
        onSave={(bytes) => (limits ? save({ ...limits, max_upload: bytes }) : undefined)}
      />
      <SizeLimitInput
        name="download"
        label="Largest file I download"
        value={download}
        disabled={!limits}
        hint="Larger files never download on their own. You can still choose to download one."
        onSave={(bytes) => (limits ? save({ ...limits, max_download: bytes }) : undefined)}
      />
    </div>
  );
}
