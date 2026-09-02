import { invoke } from '@tauri-apps/api/core';

/**
 * What Chief last observed about one account.
 *
 * Mirrors `sync_state::Status`. `authRequired` is deliberately not the same
 * thing as `error`: a credential the user revoked is theirs to fix and is worth
 * interrupting them for, and a host that could not be reached is neither.
 */
export type SyncStatus = 'ok' | 'syncing' | 'authRequired' | 'error';

/** One account's freshness, as `sync_status` returns it. */
export interface SyncState {
  accountId: number;
  source: string;
  status: SyncStatus;
  /** ISO-8601 of the last **successful** read. Null until one has happened. */
  lastSyncedAt: string | null;
  /** Why it is not `ok`, in words a person can act on. Never a credential. */
  errorMessage: string | null;
}

/**
 * How fresh every connected account is.
 *
 * An account that has never been read has no entry at all — a fresh install,
 * or one connected between two passes — so a caller must treat a missing entry
 * as "never synced" rather than as a failure.
 */
export function syncStatus(): Promise<SyncState[]> {
  return invoke<SyncState[]>('sync_status');
}

/**
 * How long ago, in the fewest words that are still true.
 *
 * Deliberately coarse. A sync time is not a deadline and nobody acts on the
 * difference between 91 and 94 seconds; what they act on is "minutes" against
 * "yesterday". Rounding down rather than to nearest, so Chief never claims data
 * is older than it is.
 */
export function howLongAgo(at: string, now: Date = new Date()): string | null {
  const then = new Date(at);

  if (Number.isNaN(then.getTime())) return null;

  const seconds = Math.floor((now.getTime() - then.getTime()) / 1000);

  // A clock that disagrees with the one that wrote the row — a machine
  // corrected since the last pass — must not read as being in the future.
  if (seconds < 60) return 'just now';
  if (seconds < 3600) return `${Math.floor(seconds / 60)}m ago`;
  if (seconds < 86_400) return `${Math.floor(seconds / 3600)}h ago`;

  return `${Math.floor(seconds / 86_400)}d ago`;
}

/** What a row says about itself, in the words the settings screen uses. */
export function describe(state: SyncState | undefined, now?: Date): string {
  if (state === undefined) return 'Never synced';

  switch (state.status) {
    case 'authRequired':
      return 'Sign in again';
    case 'syncing':
      return 'Syncing…';
    case 'error':
      return state.lastSyncedAt === null
        ? 'Could not sync'
        : `Could not sync · last ${howLongAgo(state.lastSyncedAt, now) ?? 'unknown'}`;
    case 'ok':
      return state.lastSyncedAt === null
        ? 'Never synced'
        : `Synced ${howLongAgo(state.lastSyncedAt, now) ?? 'at an unknown time'}`;
  }
}

/**
 * The one thing the header can say about all of them.
 *
 * The **oldest** successful read across every account, because a header that
 * showed the newest would say everything was fresh while one account had been
 * failing for a week. An account that has never synced makes the whole thing
 * unknown for the same reason.
 */
export function oldest(states: readonly SyncState[], accounts: number): string | null {
  if (accounts === 0) return null;

  // Fewer states than accounts means at least one has never been read, and no
  // timestamp is true of all of them.
  if (states.length < accounts) return null;

  const times = states.map((state) => state.lastSyncedAt);

  if (times.some((at) => at === null)) return null;

  return times.filter((at): at is string => at !== null).sort()[0] ?? null;
}
