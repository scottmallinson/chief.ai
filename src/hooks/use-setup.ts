import { useCallback, useEffect, useRef, useState } from 'react';

import {
  checkReadiness,
  downloadModel,
  onDownloadProgress,
  startEngine,
  type DownloadProgress,
  type Readiness,
} from '@/lib/setup';

type Status = 'checking' | 'idle' | 'downloading' | 'starting';

interface UseSetup {
  readiness: Readiness | null;
  status: Status;
  progress: DownloadProgress | null;
  error: string | null;
  recheck: () => void;
  download: () => void;
  start: () => void;
}

/**
 * How long to leave a loading engine alone before asking again. Reading a
 * couple of gigabytes takes a few seconds, and the screen should notice it
 * finishing without the user pressing anything.
 */
const WHILE_LOADING = 1500;

function describe(cause: unknown): string {
  return cause instanceof Error ? cause.message : String(cause);
}

/** Drive the first-run checks: is the model here, and is the engine answering? */
export function useSetup(): UseSetup {
  const [readiness, setReadiness] = useState<Readiness | null>(null);
  const [status, setStatus] = useState<Status>('checking');
  const [progress, setProgress] = useState<DownloadProgress | null>(null);
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
    listening.current = onDownloadProgress(setProgress);

    return () => {
      void listening.current?.then((stop) => stop());
    };
  }, []);

  // An engine that is loading becomes an engine that is ready without anyone
  // asking it to, so keep looking until it is.
  useEffect(() => {
    if (readiness?.engine !== 'loading' || status !== 'idle') return;

    const timer = setTimeout(recheck, WHILE_LOADING);
    return () => clearTimeout(timer);
  }, [readiness, status, recheck]);

  const download = useCallback(() => {
    setStatus('downloading');
    setProgress(null);
    setError(null);

    downloadModel()
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

  const start = useCallback(() => {
    setStatus('starting');
    setError(null);

    startEngine()
      .then(() => recheck())
      .catch((cause: unknown) => {
        setError(describe(cause));
        setStatus('idle');
      });
  }, [recheck]);

  return { readiness, status, progress, error, recheck, download, start };
}
