// Where finished downloads go. The same control on the Transfers page and in Settings, over the one stored setting: both
// read it from useTransferSettings, so a change in either shows in the other.
import { useEffect } from 'react';
import { Button } from '@/components/ui/button';
import { changeDownloadDir, resetDownloadDir, useTransferSettings } from '@/lib/transfers';

export function DownloadFolder() {
  const settings = useTransferSettings((s) => s.settings);
  const load = useTransferSettings((s) => s.load);
  const dirError = useTransferSettings((s) => s.dirError);
  useEffect(() => {
    void load();
  }, [load]);
  const problem = dirError ?? settings?.download_dir_problem ?? null;
  return (
    <div data-download-folder className="flex flex-col gap-s-2">
      <h4 className="text-sm font-semibold text-text-strong">Save downloads to</h4>
      <div data-download-path className="text-meta break-all text-muted-foreground">{settings?.download_dir ?? 'Not set'}</div>
      {problem ? <p role="alert" data-download-problem className="m-0 text-sm text-danger-text">{problem}</p> : null}
      <Button variant="outline" className="self-start" disabled={!settings} onClick={() => void changeDownloadDir()}>
        Change
      </Button>
      {settings && !settings.download_dir_is_default ? (
        <Button variant="ghost" className="self-start" onClick={() => void resetDownloadDir()}>
          Reset to default
        </Button>
      ) : null}
      <p className="m-0 text-meta text-muted-foreground">Downloads already running finish in the folder they started in. New ones and retries use this one.</p>
    </div>
  );
}
