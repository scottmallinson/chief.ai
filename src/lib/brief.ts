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

/** One day with a brief on it, as the list pane shows it. */
export interface BriefDay {
  date: string;
  path: string;
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
 * The days that have a brief, newest first.
 *
 * Derived from the corpus listing rather than from a command of its own: the
 * corpus is the store, so a brief the user moved, renamed or deleted in the
 * folder is reflected here without Chief keeping a second opinion about what
 * exists. Sorting is on the date in the name, not on `modifiedAt` — a brief
 * edited today is still yesterday's brief.
 */
export function briefDays(entries: CorpusEntry[]): BriefDay[] {
  return entries
    .flatMap((entry) => {
      const match = BRIEF_PATH.exec(entry.path);

      return match === null ? [] : [{ date: match[1], path: entry.path }];
    })
    .sort((left, right) => right.date.localeCompare(left.date));
}

/** One piece of a brief, ready to render. */
export type BriefBlock =
  | { kind: 'heading'; text: string }
  | { kind: 'bullets'; items: string[] }
  | { kind: 'paragraph'; text: string };

/** `- item`, or the asterisk a model sometimes reaches for instead. */
const BULLET = /^[-*]\s+(.*)$/;
const HEADING = /^#{1,6}\s+(.*)$/;
const EMPHASIS = /\*\*(.+?)\*\*|__(.+?)__|(?<![*\w])\*(?!\s)(.+?)(?<!\s)\*|`(.+?)`/g;

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
  return text
    .replace(
      EMPHASIS,
      (_match: string, bold?: string, underscored?: string, italic?: string, code?: string) =>
        bold ?? underscored ?? italic ?? code ?? '',
    )
    .trim();
}
