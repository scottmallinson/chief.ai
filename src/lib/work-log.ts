import { invoke } from '@tauri-apps/api/core';

import type { SyncState } from '@/lib/sync-state';

/** An entry in the local work log, as stored in SQLite. */
export interface WorkLogEntry {
  id: number;
  /** ISO-8601, UTC. */
  timestamp: string;
  /** Where the entry came from, e.g. `github` or `calendar`. */
  source: string;
  content: string;
  /** One line on what happened, written deterministically at ingestion. */
  summary: string | null;
  /**
   * Where the thing this describes actually is, or null when there is nowhere
   * to go — an entry the user typed, or one logged before migration 8.
   *
   * **Rendered from here and never from a model.** The retrieval context
   * deliberately omits links, because a GitHub URL is 13–15 tokens and more
   * than half a row; the interface putting it back from this column is the
   * other half of that trade, and a model that has never seen a link cannot
   * invent one.
   */
  url: string | null;
  /** What the entry was traced back to, or null when the user wrote it. */
  externalId: string | null;
  /** Which connected account it came from, and 0 when it came from none. */
  accountId: number;
}

/** A new entry. The backend defaults the timestamp to now. */
export interface NewWorkLogEntry {
  source: string;
  content: string;
  timestamp?: string;
  summary?: string;
}

/**
 * What one ingestion pass did, as `daemon::Pass` serialises it.
 *
 * The count alone would not do. A pass that reached every account and found
 * nothing new writes nothing, and so does one whose every account was refused —
 * a per-account failure is recorded against the account and stepped over, so
 * `run_once` returns `Ok(0)` for both. The states are how the two are told
 * apart.
 */
export interface Pass {
  /** How many entries the pass wrote or revised. */
  written: number;
  /** What every connected account says about itself now. Empty when none is. */
  accounts: SyncState[];
}

/**
 * Read the connected accounts now, rather than waiting for the next pass.
 *
 * The expensive one: it goes out to every connected service. "Refresh" used to
 * call `listWorkLogs`, which is a `SELECT` and asked nobody for anything.
 */
export function syncNow(): Promise<Pass> {
  return invoke<Pass>('sync_now');
}

/** Read the work log, newest first. */
export function listWorkLogs(limit?: number): Promise<WorkLogEntry[]> {
  return invoke<WorkLogEntry[]>('list_work_logs', { limit });
}

/** Append an entry to the work log and return it as stored. */
export function createWorkLog(entry: NewWorkLogEntry): Promise<WorkLogEntry> {
  return invoke<WorkLogEntry>('create_work_log', { entry });
}
