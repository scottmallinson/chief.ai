import { useCallback, useEffect, useRef, useState } from 'react';

import {
  checkReadiness,
  onPullProgress,
  pullModel,
  type PullProgress,
  type Readiness,
} from '@/lib/setup';

type Status = 'checking' | 'idle' | 'downloading';

interface UseSetup {
  readiness: Readiness | null;
  status: Status;
  progress: PullProgress | null;
  error: string | null;
  recheck: () => void;
  download: () => void;
}

function describe(cause: unknown): string {
  return cause instanceof Error ? cause.message : String(cause);
}

/** Drive the first-run checks: is Ollama here, and is the model installed? */
export function useSetup(): UseSetup {
  const [readiness, setReadiness] = useState<Readiness | null>(null);
  const [status, setStatus] = useState<Status>('checking');
  const [progress, setProgress] = useState<PullProgress | null>(null);
  const [error, setError] = useState<string | null>(null);
  const listening = useRef<Promise<() => void> | null>(null);

  const recheck = useCallback(() => {
    setStatus('checking');
    setError(null);

    checkReadiness()
      .then((current) => {
        setReadiness(current);
        setStatus('idle');
      })
      .catch((cause: unknown) => {
        setError(describe(cause));
        setStatus('idle');
      });
  }, []);

  useEffect(() => recheck(), [recheck]);

  // One subscription for the lifetime of the screen; the download itself is
  // started on demand.
  useEffect(() => {
    listening.current = onPullProgress(setProgress);

    return () => {
      void listening.current?.then((stop) => stop());
    };
  }, []);

  const download = useCallback(() => {
    setStatus('downloading');
    setProgress(null);
    setError(null);

    pullModel()
      .then(() => {
        setProgress(null);
        recheck();
      })
      .catch((cause: unknown) => {
        setError(describe(cause));
        setProgress(null);
        setStatus('idle');
      });
  }, [recheck]);

  return { readiness, status, progress, error, recheck, download };
}
