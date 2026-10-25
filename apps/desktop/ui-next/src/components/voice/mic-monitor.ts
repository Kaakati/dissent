// The microphone monitor of Settings › Voice: the level from dissent://mic-level, the mic test, and the meter falling to
// silence when the mic is closed. Monitoring runs while the page is open (a call already runs the mic, and then the test
// is refused, which is fine). Ported from the level half of ui/views/settings-voice.js.
import { useEffect, useRef, useState } from 'react';
import { api, listenTo } from '@/lib/api';
import { toast } from '@/lib/toasts';
import type { MicLevel } from '@/lib/types';

// Sensitivity and meter scale, in dBFS. Anything quieter reads as the bottom of the track.
export const SCALE_MIN = -80;
export const SCALE_MAX = -10;
export const SILENT_DB = -100;
// No level for this long means the mic is closed (muted, or the page just opened), so the meter falls to silence.
const SILENT_AFTER_MS = 400;

// The position of a level on the track, 0 to 1.
export const levelPosition = (db: number): number => Math.min(1, Math.max(0, (db - SCALE_MIN) / (SCALE_MAX - SCALE_MIN)));

export interface MicMonitor {
  db: number; // the level now, in dBFS
  thresholdDb: number | null; // the threshold the gate uses now (the automatic one follows the room); null before a level
  testing: boolean; // the mic test plays the mic back through the speaker
  toggleTest: () => Promise<void>;
  restart: () => Promise<void>; // a new microphone or speaker: the monitor and any running test restart on it
}

export function useMicMonitor(): MicMonitor {
  const [level, setLevel] = useState<MicLevel | null>(null);
  const [testing, setTesting] = useState(false);
  const lastLevel = useRef(0);
  const testingRef = useRef(false);

  useEffect(() => {
    const unlisten = listenTo<MicLevel>('dissent://mic-level', (next) => {
      lastLevel.current = Date.now();
      setLevel(next);
    }).catch(() => () => {});
    const quiet = setInterval(() => {
      if (Date.now() - lastLevel.current > SILENT_AFTER_MS) setLevel((l) => (l && l.db > SILENT_DB ? { ...l, db: SILENT_DB } : l));
    }, 200);
    void api.micTestStart(false).catch(() => {});
    return () => {
      clearInterval(quiet);
      void unlisten.then((stop) => stop());
      void api.micTestStop().catch(() => {});
    };
  }, []);

  const toggleTest = async () => {
    const next = !testingRef.current;
    try {
      await api.micTestStart(next);
      testingRef.current = next;
      setTesting(next);
    } catch (err) {
      toast(err);
    }
  };

  const restart = async () => {
    await api.micTestStop().catch(() => {});
    await api.micTestStart(testingRef.current).catch(() => {});
  };

  return {
    db: level?.db ?? SILENT_DB,
    thresholdDb: level?.threshold_db ?? null,
    testing,
    toggleTest,
    restart,
  };
}
