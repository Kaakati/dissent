// Settings › About: the version, and the Disclaimer and Limitation of Liability, which opens the notice accepted at first
// launch (components/startup/disclaimer.tsx) read-only.
import { FileText } from 'lucide-react';
import { Block, Hint } from '@/components/settings/settings-ui';
import { Button } from '@/components/ui/button';
import { DISCLAIMER_TITLE, DISCLAIMER_UPDATED, openDisclaimer } from '@/lib/disclaimer';
import { useShell } from '@/lib/store';

export function AboutPage() {
  const version = useShell((s) => s.version);
  return (
    <div className="flex flex-col gap-s-6">
      <Block title="Dissent">
        <p className="text-body text-text-strong">{version ? `Version ${version}` : 'Version …'}</p>
        <Hint>Free, open-source and peer-to-peer. Published under the MIT License or the Apache License 2.0, at your choice.</Hint>
      </Block>
      <Block title="Legal">
        <div>
          <Button variant="outline" onClick={openDisclaimer} data-open-disclaimer>
            <FileText />
            {DISCLAIMER_TITLE}
          </Button>
        </div>
        <Hint>{`The notice you accepted when you started Dissent. Last updated ${DISCLAIMER_UPDATED}.`}</Hint>
      </Block>
    </div>
  );
}
