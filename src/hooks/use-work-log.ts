import { useCallback, useEffect, useState } from 'react';

import { listWorkLogs, type WorkLogEntry } from '@/lib/work-log';

type Status = 'loading' | 'ready' | 'error';

interface UseWorkLog {
  entries: WorkLogEntry[];
  status: Status;
  error: string | null;
  reload: () => void;
}

/** Load the work log from the local database, newest first. */
export function useWorkLog(): UseWorkLog {
  const [entries, setEntries] = useState<WorkLogEntry[]>([]);
  const [status, setStatus] = useState<Status>('loading');
  const [error, setError] = useState<string | null>(null);

  const load = useCallback(() => {
    let cancelled = false;

    setStatus('loading');
    setError(null);

    listWorkLogs()
      .then((loaded) => {
        if (cancelled) return;
        setEntries(loaded);
        setStatus('ready');
      })
      .catch((cause: unknown) => {
        if (cancelled) return;
        setError(cause instanceof Error ? cause.message : String(cause));
        setStatus('error');
      });

    return () => {
      cancelled = true;
    };
  }, []);

  useEffect(() => load(), [load]);

  return { entries, status, error, reload: load };
}
