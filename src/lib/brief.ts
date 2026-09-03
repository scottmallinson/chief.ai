import { invoke } from '@tauri-apps/api/core';

import type { CorpusEntry } from '@/lib/corpus';

/** A brief, as `recipe::Brief` serialises it. */
export interface Brief {
  /** The day it covers, `YYYY-MM-DD`. */
  date: string;
  /** Where it was written in the corpus. */
  path: string;
  /** The brief itself. */
  markdown: string;
  /** Which sources had anything to say. */
  sources: string[];
}

/** One day the list pane offers, whether or not a brief has been written. */
export interface BriefDay {
  date: string;
  path: string;
  /**
   * Whether the corpus actually holds this one.
   *
   * False for exactly one day — today, before a brief exists for it. Today is
   * always in the list because it is the day the user came here for, and a list
   * built only from the files that exist has no row for the day with no file:
   * that is how "write today's brief" became unreachable the moment anything
   * else was on screen.
   */
  written: boolean;
}

/** Today's brief, if one has been written. */
export function todaysBrief(): Promise<Brief | null> {
  return invoke<Brief | null>('todays_brief');
}

/** Write today's brief now. Costs a model call. */
export function generateBrief(): Promise<Brief> {
  return invoke<Brief>('generate_brief');
}

/** `briefs/YYYY-MM-DD.md`, and nothing else that happens to live there. */
const BRIEF_PATH = /^briefs\/(\d{4}-\d{2}-\d{2})\.md$/;

/**
 * The days the list pane offers, newest first: today, and every day the corpus
 * has a brief for.
 *
 * Derived from the corpus listing rather than from a command of its own: the
 * corpus is the store, so a brief the user moved, renamed or deleted in the
 * folder is reflected here without Chief keeping a second opinion about what
 * exists. Sorting is on the date in the name, not on `modifiedAt` — a brief
 * edited today is still yesterday's brief.
 *
 * **Today is in the list unconditionally.** It is the one day that can be
 * selected without a file behind it, and it has to be selectable or there is no
 * way back to the screen offering to write one.
 */
export function briefDays(entries: CorpusEntry[], today: string): BriefDay[] {
  const days = entries.flatMap((entry) => {
    const match = BRIEF_PATH.exec(entry.path);

    return match === null ? [] : [{ date: match[1], path: entry.path, written: true }];
  });

  if (!days.some((day) => day.date === today)) {
    days.push({ date: today, path: `briefs/${today}.md`, written: false });
  }

  return days.sort((left, right) => right.date.localeCompare(left.date));
}

/**
 * Today, as the date a brief is filed under — `recipe::today_date`'s answer,
 * worked out here so the list pane does not need a command to know the date.
 *
 * Built from the local parts rather than from `toISOString`, which is UTC and
 * so names the wrong day for anybody west of Greenwich for part of every
 * evening — the same trap `formatDay` and `BriefList.parse` already avoid.
 */
export function todayDate(now: Date = new Date()): string {
  const month = `${now.getMonth() + 1}`.padStart(2, '0');
  const day = `${now.getDate()}`.padStart(2, '0');

  return `${now.getFullYear()}-${month}-${day}`;
}

/** One piece of a brief, ready to render. */
export type BriefBlock =
  | { kind: 'heading'; text: string }
  | { kind: 'bullets'; items: string[] }
  | { kind: 'paragraph'; text: string };

/** `- item`, or the asterisk a model sometimes reaches for instead. */
const BULLET = /^[-*]\s+(.*)$/;
const HEADING = /^#{1,6}\s+(.*)$/;
/**
 * The emphasis markers, stripped in four passes rather than one alternation.
 *
 * **No lookbehind.** The single-asterisk pass has to know it is not standing in
 * the middle of `**bold**`, and the obvious way to write that is `(?<![*\w])` —
 * which is a *parse* error in the WebViews Chief ships in, and took the whole
 * app down to a white screen when it was one. Running the passes in order is
 * what replaces it: bold is gone by the time italics are looked for, so there
 * is nothing left for the guard to guard against. The leading character is
 * captured and put back rather than asserted.
 */
const EMPHASIS: readonly [RegExp, string][] = [
  [/\*\*(.+?)\*\*/g, '$1'],
  [/__(.+?)__/g, '$1'],
  [/`(.+?)`/g, '$1'],
  // `$1` is the character before the opening asterisk, re-emitted. The content
  // may not start or end with a space, so `2 * 3 * 4` is arithmetic and not
  // emphasis.
  [/(^|[^*\w])\*([^\s*](?:[^*]*[^\s*])?)\*/g, '$1$2'],
];

/**
 * Read a brief into the handful of shapes one can be.
 *
 * Not a markdown parser, and deliberately not a dependency that is. A brief is
 * written to a prompt that says "no preamble, no sign-off, no headings, bullets
 * only" — so the shape is known, and the only thing that widens it is the user
 * editing the file by hand, which the corpus invites them to do. Headings and
 * paragraphs are here for that, and everything past them degrades to prose
 * rather than to raw syntax on screen.
 */
export function readBrief(markdown: string): BriefBlock[] {
  const blocks: BriefBlock[] = [];
  // The list or paragraph still being added to, so consecutive lines join.
  let open: BriefBlock | null = null;

  const close = () => {
    if (open !== null) blocks.push(open);
    open = null;
  };

  for (const raw of markdown.split('\n')) {
    const line = raw.trim();

    if (line === '') {
      close();
      continue;
    }

    const heading = HEADING.exec(line);
    if (heading !== null) {
      close();
      blocks.push({ kind: 'heading', text: plain(heading[1]) });
      continue;
    }

    const bullet = BULLET.exec(line);
    if (bullet !== null) {
      if (open?.kind !== 'bullets') {
        close();
        open = { kind: 'bullets', items: [] };
      }

      open.items.push(plain(bullet[1]));
      continue;
    }

    // A wrapped paragraph is one paragraph, but a line after a bullet starts a
    // new one: the alternative is prose silently joining the last bullet.
    if (open?.kind === 'paragraph') {
      open.text = `${open.text} ${plain(line)}`;
      continue;
    }

    close();
    open = { kind: 'paragraph', text: plain(line) };
  }

  close();

  return blocks;
}

/** Drop the emphasis markers. Showing them is worse than losing the emphasis. */
function plain(text: string): string {
  return EMPHASIS.reduce((stripped, [pattern, replacement]) => {
    return stripped.replace(pattern, replacement);
  }, text).trim();
}
