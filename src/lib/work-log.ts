import { invoke } from '@tauri-apps/api/core';

/** An entry in the local work log, as stored in SQLite. */
export interface WorkLogEntry {
  id: number;
  /** ISO-8601, UTC. */
  timestamp: string;
  /** Where the entry came from, e.g. `github` or `calendar`. */
  source: string;
  content: string;
  /** A one-line achievement, written by the local model in a later step. */
  summary: string | null;
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

/** Read the work log, newest first. */
export function listWorkLogs(limit?: number): Promise<WorkLogEntry[]> {
  return invoke<WorkLogEntry[]>('list_work_logs', { limit });
}

/** Append an entry to the work log and return it as stored. */
export function createWorkLog(entry: NewWorkLogEntry): Promise<WorkLogEntry> {
  return invoke<WorkLogEntry>('create_work_log', { entry });
}
