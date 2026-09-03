import { useCallback, useEffect, useState } from 'react';

import { syncNow } from '@/lib/work-log';
import { syncStatus, type SyncState } from '@/lib/sync-state';

interface UseSync {
  /** What each connected account says about itself. Empty when none is. */
  accounts: SyncState[];
  /** Whether a pass is running because somebody asked for one. */
  syncing: boolean;
  /** Why the last pass could not be started at all, in the user's words. */
  error: string | null;
  /** Read the connected services now. */
  sync: () => void;
}

/**
 * Bringing the local rows up to date with the services behind them.
 *
 * **Deliberately separate from [`useWorkLog`], which is a `SELECT`.** The two
 * were one button: the work log's "Refresh" called `list_work_logs`, re-read
 * the rows a pass had already written, and asked GitHub for nothing. A user
 * whose log had stopped three days earlier could click it all day and learn
 * nothing — not that it was stale, not why, and not that clicking had asked
 * nobody anything.
 *
 * Separate hooks rather than one, because `TodayView` wants the rows and not
 * this: folding the freshness read into `useWorkLog` would have put a second
 * command on a screen that has no use for the answer.
 *
 * @param afterPass Run when a pass finishes, to re-read what it wrote.
 */
export function useSync(afterPass: () => void): UseSync {
  const [accounts, setAccounts] = useState<SyncState[]>([]);
  const [syncing, setSyncing] = useState(false);
  const [error, setError] = useState<string | null>(null);

  useEffect(() => {
    let cancelled = false;

    // Freshness is what makes the log honest, but the rows are the point: a
    // freshness read that fails leaves the states empty rather than emptying
    // the screen.
    syncStatus()
      .then((states) => {
        if (!cancelled) setAccounts(states);
      })
      .catch(() => {
        if (!cancelled) setAccounts([]);
      });

    return () => {
      cancelled = true;
    };
  }, []);

  const sync = useCallback(() => {
    setSyncing(true);
    setError(null);

    syncNow()
      .then((pass) => {
        // The states come back with the pass rather than from a later query,
        // so what is on screen is what that pass recorded.
        setAccounts(pass.accounts);
        afterPass();
      })
      .catch((cause: unknown) => {
        // A pass that could not be *started* — no database, no corpus. A pass
        // that started and had an account refused comes back as a state on
        // that account instead, which is the distinction the screen draws.
        setError(cause instanceof Error ? cause.message : String(cause));
      })
      .finally(() => setSyncing(false));
  }, [afterPass]);

  return { accounts, syncing, error, sync };
}
