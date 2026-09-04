import { describe, expect, it } from 'vitest';

import { briefDays, readBrief, todayDate } from '@/lib/brief';
import type { CorpusEntry } from '@/lib/corpus';

function entry(path: string, modifiedAt = '2026-08-29T08:00:00Z'): CorpusEntry {
  return { path, size: 200, modifiedAt, estimatedTokens: 50 };
}

describe('briefDays', () => {
  it('keeps only the briefs, newest day first', () => {
    const days = briefDays(
      [
        entry('briefs/2026-08-27.md'),
        entry('notes/standup.md'),
        entry('briefs/2026-08-29.md'),
        entry('briefs/2026-08-28.md'),
      ],
      '2026-08-29',
    );

    expect(days.map((day) => day.date)).toEqual(['2026-08-29', '2026-08-28', '2026-08-27']);
  });

  it('carries the path the brief was read from', () => {
    const days = briefDays([entry('briefs/2026-08-29.md')], '2026-08-29');

    expect(days[0]?.path).toBe('briefs/2026-08-29.md');
  });

  it('ignores anything under briefs that is not a dated markdown file', () => {
    const days = briefDays(
      [entry('briefs/README.md'), entry('briefs/2026-08-29.md')],
      '2026-08-29',
    );

    expect(days.map((day) => day.date)).toEqual(['2026-08-29']);
  });

  /**
   * The guard on the defect this exists for.
   *
   * Proved by dropping the `days.push` in `briefDays`:
   *
   * ```text
   * → today is the day you came here for, and it has to be selectable
   *   - Expected: ["2026-08-29", "2026-08-28"]
   *   + Received: ["2026-08-28"]
   * ```
   *
   * With no row for today there is nothing to click, and the empty state that
   * offers to write one is unreachable for the rest of the session.
   */
  it('offers today even when nothing has been written for it', () => {
    const days = briefDays([entry('briefs/2026-08-28.md')], '2026-08-29');

    expect(
      days.map((day) => day.date),
      'today is the day you came here for, and it has to be selectable',
    ).toEqual(['2026-08-29', '2026-08-28']);
    expect(days[0]?.written).toBe(false);
    expect(days[0]?.path).toBe('briefs/2026-08-29.md');
  });

  it('offers today once, not twice, when it has been written', () => {
    const days = briefDays([entry('briefs/2026-08-29.md')], '2026-08-29');

    expect(days.map((day) => day.date)).toEqual(['2026-08-29']);
    expect(days[0]?.written).toBe(true);
  });

  it('offers today even when the corpus lists nothing at all', () => {
    expect(briefDays([], '2026-08-29').map((day) => day.date)).toEqual(['2026-08-29']);
  });
});

describe('todayDate', () => {
  it('names the local day, not the UTC one', () => {
    // 23:30 on the 29th wherever the test machine is. `toISOString` would say
    // the 30th for anybody east of Greenwich and the 29th for anybody west,
    // which is the bug this avoids by never asking it.
    expect(todayDate(new Date(2026, 7, 29, 23, 30))).toBe('2026-08-29');
  });

  it('pads the month and the day, so the string sorts and matches the path', () => {
    expect(todayDate(new Date(2026, 0, 5, 12, 0))).toBe('2026-01-05');
  });
});

describe('readBrief', () => {
  it('reads the bullets a brief is made of', () => {
    const blocks = readBrief('- Standup at 09:30 with Ana\n- Review chief.ai #44');

    expect(blocks).toEqual([
      { kind: 'bullets', items: ['Standup at 09:30 with Ana', 'Review chief.ai #44'] },
    ]);
  });

  it('accepts the asterisk a model sometimes reaches for instead', () => {
    expect(readBrief('* Standup at 09:30')).toEqual([
      { kind: 'bullets', items: ['Standup at 09:30'] },
    ]);
  });

  it('keeps consecutive bullets in one list and starts a new one after prose', () => {
    const blocks = readBrief('- one\n- two\n\nSomething else.\n\n- three');

    expect(blocks).toEqual([
      { kind: 'bullets', items: ['one', 'two'] },
      { kind: 'paragraph', text: 'Something else.' },
      { kind: 'bullets', items: ['three'] },
    ]);
  });

  it('reads a heading a hand-edit added', () => {
    expect(readBrief('## Later today\n- Retro at 16:00')).toEqual([
      { kind: 'heading', text: 'Later today' },
      { kind: 'bullets', items: ['Retro at 16:00'] },
    ]);
  });

  it('joins the lines of a wrapped paragraph', () => {
    expect(readBrief('A brief the model\nwrapped across lines.')).toEqual([
      { kind: 'paragraph', text: 'A brief the model wrapped across lines.' },
    ]);
  });

  it('strips the emphasis markers rather than showing them', () => {
    expect(readBrief('- Review **chief.ai #44** before the retro')).toEqual([
      { kind: 'bullets', items: ['Review chief.ai #44 before the retro'] },
    ]);
  });

  it('strips every kind of emphasis a hand-edit might add', () => {
    expect(readBrief('- __Ana__ has *the* `release` branch')).toEqual([
      { kind: 'bullets', items: ['Ana has the release branch'] },
    ]);
  });

  it('leaves arithmetic alone', () => {
    // What the lookbehind used to guard, and the reason the passes are
    // ordered: an asterisk with a space beside it is not emphasis.
    expect(readBrief('- The budget is 2 * 3 * 4 tokens')).toEqual([
      { kind: 'bullets', items: ['The budget is 2 * 3 * 4 tokens'] },
    ]);
  });

  it('does not mistake the inside of a bold run for an italic one', () => {
    expect(readBrief('- **Ship it** and *then* rest')).toEqual([
      { kind: 'bullets', items: ['Ship it and then rest'] },
    ]);
  });

  it('has nothing to show for an empty brief', () => {
    expect(readBrief('   \n\n  ')).toEqual([]);
  });
});
