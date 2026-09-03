import { describe as group, expect, it } from 'vitest';

import { describe, howLongAgo, oldest, type SyncState } from '@/lib/sync-state';

function state(over: Partial<SyncState> = {}): SyncState {
  return {
    accountId: 1,
    source: 'github',
    status: 'ok',
    lastSyncedAt: '2026-09-02T09:00:00.000Z',
    errorMessage: null,
    ...over,
  };
}

const NOW = new Date('2026-09-02T09:30:00.000Z');

group('howLongAgo', () => {
  it('rounds down, so Chief never claims data is older than it is', () => {
    expect(howLongAgo('2026-09-02T09:29:30.000Z', NOW)).toBe('just now');
    expect(howLongAgo('2026-09-02T09:00:00.000Z', NOW)).toBe('30m ago');
    expect(howLongAgo('2026-09-02T05:59:00.000Z', NOW)).toBe('3h ago');
    expect(howLongAgo('2026-08-30T09:00:00.000Z', NOW)).toBe('3d ago');
  });

  it('does not read as being in the future when the clock was corrected', () => {
    // A machine whose clock moved backwards since the pass that wrote the row.
    expect(howLongAgo('2026-09-02T10:00:00.000Z', NOW)).toBe('just now');
  });

  it('answers null rather than NaN for a timestamp it cannot read', () => {
    expect(howLongAgo('not a date', NOW)).toBeNull();
  });
});

group('describe', () => {
  it('reads an account with no state at all as never synced', () => {
    // A fresh install, or an account connected between two passes. It is a
    // legitimate state, not a failure.
    expect(describe(undefined)).toBe('Never synced');
  });

  it('asks for a sign-in when the credential is gone', () => {
    expect(describe(state({ status: 'authRequired' }))).toBe('Sign in again');
  });

  it('keeps the last good read visible when a later one failed', () => {
    // "Last read at 09:00, failing since" is the useful thing to be able to
    // say; dropping the timestamp on failure loses half of it.
    expect(describe(state({ status: 'error' }), NOW)).toBe('Could not sync · last 30m ago');
  });

  it('does not claim a successful read that never happened', () => {
    expect(describe(state({ status: 'ok', lastSyncedAt: null }))).toBe('Never synced');
  });
});

group('oldest', () => {
  // A header showing the newest would say everything was fresh while one
  // account had been failing for a week. Proved by sorting the other way:
  //
  //   expected '2026-09-02T09:20:00.000Z' to be '2026-09-01T09:00:00.000Z'
  it('is the stalest account, not the freshest', () => {
    const stalest = oldest(
      [
        state({ accountId: 1, lastSyncedAt: '2026-09-02T09:20:00.000Z' }),
        state({ accountId: 2, lastSyncedAt: '2026-09-01T09:00:00.000Z' }),
      ],
      2,
    );

    expect(stalest).toBe('2026-09-01T09:00:00.000Z');
  });

  // Two accounts, one state: no timestamp is true of both of them. Proved by
  // dropping the count comparison:
  //
  //   expected '2026-09-02T09:00:00.000Z' to be null
  it('is unknown while any connected account has never been read', () => {
    expect(oldest([state({ accountId: 1 })], 2)).toBeNull();
    expect(oldest([state({ accountId: 1, lastSyncedAt: null })], 1)).toBeNull();
  });

  it('says nothing at all when nothing is connected', () => {
    expect(oldest([], 0)).toBeNull();
  });
});
