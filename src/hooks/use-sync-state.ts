import { useCallback, useEffect, useState } from 'react';

import { syncStatus, type SyncState } from '@/lib/sync-state';

/**
 * Take the answer only if it is one.
 *
 * The command is typed `Vec<SyncState>`, so this only fires when something has
 * gone wrong upstream — and the point of swallowing a failure below is that
 * freshness is an annotation which must never take down a screen whose job is
 * something else. A shape this cannot use is a failure like any other, and
 * this hook found out the expensive way: the settings cards render freshness
 * per account, so one bad answer took three of them down at once.
 */
function usable(answer: readonly SyncState[]): readonly SyncState[] {
  // Taken through `unknown` because the parameter's declared type is exactly
  // what is in doubt: `Array.isArray` on a value TypeScript already believes
  // is an array narrows to `any[]` and the check reads as dead.
  const answered: unknown = answer;

  return Array.isArray(answered) ? (answered as readonly SyncState[]) : [];
}

interface UseSyncState {
  /** Keyed by account id, because that is what every row looks itself up by. */
  states: ReadonlyMap<number, SyncState>;
  /** Every state, for the header's question about all of them at once. */
  all: readonly SyncState[];
  reload: () => void;
}

/**
 * How fresh each connected account is.
 *
 * **A failure here is swallowed on purpose.** Freshness is an annotation on
 * screens that have their own job — the settings screen still has to let
 * somebody connect an account, and the header still has to say where the data
 * is. An empty map degrades every row to "Never synced", which is the same
 * thing a fresh install shows and is never wrong in a way that misleads.
 *
 * It is read once on mount rather than polled. The daemon writes these on its
 * own cadence and the difference between a timestamp that is current and one
 * that is a few minutes stale is not worth a timer per screen; anything that
 * changes a state in the foreground — connecting, disconnecting — reloads it.
 */
export function useSyncState(): UseSyncState {
  const [all, setAll] = useState<readonly SyncState[]>([]);

  const reload = useCallback(() => {
    syncStatus()
      .then((loaded) => setAll(usable(loaded)))
      .catch(() => setAll([]));
  }, []);

  useEffect(() => {
    let cancelled = false;

    syncStatus()
      .then((loaded) => {
        if (!cancelled) setAll(usable(loaded));
      })
      .catch(() => undefined);

    return () => {
      cancelled = true;
    };
  }, []);

  const states = new Map(all.map((state) => [state.accountId, state]));

  return { states, all, reload };
}
