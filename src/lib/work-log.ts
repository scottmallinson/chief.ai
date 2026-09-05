import { invoke } from '@tauri-apps/api/core';

import type { SyncState } from '@/lib/sync-state';

/** An entry in the local work log, as stored in SQLite. */
export interface WorkLogEntry {
  id: number;
  /** ISO-8601, UTC. */
  timestamp: string;
  /** Where the entry came from, e.g. `github` or `calendar`. */
  source: string;
  /**
   * What the entry is about: the repository, number and title of a pull
   * request, or the subject of a meeting.
   *
   * **The line a row leads with.** Empty for an entry the user typed by hand —
   * migration 8 gave the column a default rather than inventing one — so a
   * renderer falls back to `content`.
   */
  title: string;
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

/**
 * How long a summary may be and still be a label.
 *
 * Deterministic ingestion writes either a state — "merged", "open", "review
 * requested" — or a meeting's time and who is at it, "09:30, with Dana". Both
 * are short by construction. The daemon that came before it asked the model
 * for a sentence, and those rows are still in everybody's database: "The
 * developer merged a pull request to resolve a bug related to fetching the
 * llama.cpp engine before Rust's checks." is a real one, measured in the
 * running app.
 *
 * 40 characters is comfortably past the longest state and nowhere near a
 * sentence. It is a heuristic, and it is allowed to be one **because nothing
 * breaks when it is wrong**: a misjudged summary renders on the wrong line,
 * where the old behaviour tore the row open. The layout no longer depends on
 * this being right — see the `min-w-0` and the wrap in `WorkLogView`.
 */
const LABEL_LIMIT = 40;

/** One row, split into what it is about, what state it is in, and any prose. */
export interface ReadEntry {
  /** What the row leads with. */
  headline: string;
  /** A short machine fact, fit for a chip. Null when there is none. */
  label: string | null;
  /** A sentence from the old daemon. Prose, and never a chip. */
  note: string | null;
}

/**
 * One row's headline, its label and any prose under it.
 *
 * **`summary` is a state, not a sentence — except where it is.** Deterministic
 * ingestion made it the single word "merged", "open" or "review requested",
 * and every screen was leading with it: the Today feed read `merged`,
 * `merged`, `open` five rows deep with nothing saying what had been merged.
 * The title is what the row is about; the state belongs beside it.
 *
 * What that fix did not account for is that the column holds **both shapes at
 * once**. Migration 8 backfilled `title` from `content`, so rows the old
 * daemon wrote now have a title *and* a model-written sentence — and the
 * sentence was going into the chip beside it, which is how a work log ends up
 * with labels reading "merged" on one row and a full paragraph on the next.
 */
export function readEntry(entry: WorkLogEntry): ReadEntry {
  // **Read defensively, because a missing field here is a white screen.** The
  // column is `NOT NULL DEFAULT ''` so Rust always sends a string — but this
  // renders every row of two feeds, and a row that arrived without it from a
  // stale cache or an older build would throw during render and take the whole
  // app down. CLAUDE.md records three such crashes; this is not the fourth.
  const title = entry.title?.trim() ?? '';
  const summary = entry.summary?.trim() ?? '';

  // What the user typed themselves, which has no title by design.
  const headline = title === '' ? entry.content : title;

  if (summary === '' || summary === headline) {
    return { headline, label: null, note: null };
  }

  return summary.length <= LABEL_LIMIT
    ? { headline, label: summary, note: null }
    : { headline, label: null, note: summary };
}

/** Read the work log, newest first. */
export function listWorkLogs(limit?: number): Promise<WorkLogEntry[]> {
  return invoke<WorkLogEntry[]>('list_work_logs', { limit });
}

/** Append an entry to the work log and return it as stored. */
export function createWorkLog(entry: NewWorkLogEntry): Promise<WorkLogEntry> {
  return invoke<WorkLogEntry>('create_work_log', { entry });
}
