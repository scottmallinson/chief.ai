import { describe, expect, it } from 'vitest';

import {
  bulletsOf,
  composite,
  gatesFor,
  inventedThings,
  isLooping,
  MEMORY_BUDGET_MB,
  parseStream,
  scoreRun,
} from './score.mjs';

const sse = (...events) => events.map((e) => `data: ${JSON.stringify(e)}\n\n`).join('');
const text = (content) =>
  sse({ choices: [{ delta: { content } }] }, { choices: [{ delta: {}, finish_reason: 'stop' }] });

/** A run in the shape the Rust runner writes, with just what a test needs. */
function run({
  key = {},
  answer = '',
  shown = 'material',
  response,
  group = 'brief-busy',
  fixture = 'f',
  offered = true,
  extra = {},
} = {}) {
  return {
    fixture,
    group,
    answer,
    key,
    error: null,
    routed: false,
    wall_ms: 1000,
    peak_resident_mb: 2500,
    phase: 'warm',
    exchanges: [
      {
        status: 200,
        request: {
          messages: [{ role: 'user', content: shown }],
          ...(offered ? { tools: [{ type: 'function' }] } : {}),
        },
        response: response ?? text(answer),
        first_ms: 100,
        total_ms: 900,
        timings: { prompt_n: 100, prompt_ms: 500, predicted_n: 20, predicted_ms: 1000, cache_n: 0 },
      },
    ],
    ...extra,
  };
}

describe('parseStream', () => {
  it('joins streamed text and reassembles a tool call that arrived in fragments', () => {
    const body = sse(
      {
        choices: [
          {
            delta: {
              tool_calls: [{ index: 0, function: { name: 'fetch_calendar', arguments: '{"ra' } }],
            },
          },
        ],
      },
      {
        choices: [
          { delta: { tool_calls: [{ index: 0, function: { arguments: 'nge":"today"}' } }] } },
        ],
      },
    );
    expect(parseStream(body).calls).toEqual([
      { name: 'fetch_calendar', arguments: '{"range":"today"}' },
    ]);
    expect(parseStream(text('hello')).content).toBe('hello');
  });
});

describe('inventedThings', () => {
  it('does not take the word after a bold label for a name', () => {
    const shown = 'Nothing is waiting.';
    expect(
      inventedThings('**What is waiting on me:** None reported.\n**Plan:** Connect GitHub', shown),
    ).toEqual([]);
    expect(inventedThings('We met Zorbo today', shown)).toEqual(['name Zorbo']);
  });
  it('reads "17 days ago" as worked out from a date, only when the material has one', () => {
    expect(inventedThings('Updated 17 days ago', 'updated 2026-10-02')).toEqual([]);
    expect(inventedThings('Updated 17 days ago', 'updated recently')).toEqual(['number 17']);
    expect(inventedThings('Raised 17 PRs', 'updated 2026-10-02')).toEqual(['number 17']);
  });
  it('accepts a 12-hour time that is the 24-hour one in the material, and flags a wrong one', () => {
    const shown = 'start 15:00 end 15:30 Quarterly planning';
    expect(inventedThings('- Ends at 3:30 PM, starts 3:00 PM', shown)).toEqual([]);
    expect(inventedThings('- Ends at 4:30 PM', shown)).toEqual(['time 4:30 PM']);
  });

  const shown = '## Today\n- 09:00 Standup with Priya\n- acme/api #41 Retry';
  it('lets through what the material says', () => {
    expect(inventedThings('- 09:00 Standup with Priya, then #41', shown)).toEqual([]);
  });
  it('catches a time, a number and a name that are not in the material', () => {
    const found = inventedThings('- 15:30 Lunch with Marcus about #99', shown);
    expect(found).toEqual(expect.arrayContaining(['time 15:30', 'number #99', 'name Marcus']));
  });
  it('does not treat a word followed by a colon in the material as unknown', () => {
    expect(
      inventedThings('- Mail from Oakfield Primary', 'Oakfield Primary: Non-uniform day'),
    ).toEqual([]);
  });
});

describe('inventedThings and possessives', () => {
  it('does not call a possessive of a word the material has an invention', () => {
    expect(inventedThings("- 17:30 Parents' evening", "17:30 Parents' evening — Year 3")).toEqual(
      [],
    );
    expect(
      inventedThings("- Review of Oakfield Primary's day", 'Oakfield Primary: Non-uniform day'),
    ).toEqual([]);
  });
  it('still catches a possessive of a name the material does not have', () => {
    expect(inventedThings("- Reply to Marcus's note", 'Mail from Priya')).toEqual([
      "name Marcus's",
    ]);
  });
});

describe('inventedThings and lines that open with a time', () => {
  const shown = '14:00 Vendor call\n14:30 Budget check-in';
  it('does not read the verb after a time as a name', () => {
    expect(
      inventedThings('• 14:00: Attend vendor call\n• 14:30 Meet for the check-in', shown),
    ).toEqual([]);
  });
  it('still reads a name later in the line', () => {
    expect(inventedThings('• 14:00: Attend vendor call with Marcus', shown)).toEqual([
      'name Marcus',
    ]);
  });
});

describe('inventedThings and the names it cannot tell from the material', () => {
  it('reads a bare name as known when the material has it possessed', () => {
    expect(inventedThings('- Wish Sam a happy birthday', "all day Sam's birthday")).toEqual([]);
  });
  it('reads a hyphenated word as known when each part is', () => {
    expect(inventedThings('- Meeting: All-day Bank holiday', 'all day Bank holiday')).toEqual([]);
    expect(inventedThings('- With the Parent-Teacher group', 'School pick-up')).toEqual([
      'name Parent-Teacher',
    ]);
  });
  it('checks the name after a title', () => {
    expect(inventedThings('- 11:30: Dentist with Dr. Smith', '11:30 Dentist')).toEqual([
      'name Smith',
    ]);
  });
});

describe('isLooping', () => {
  it('fires on the third copy of a line and not the second', () => {
    expect(isLooping('- review the open pull requests\n- review the open pull requests')).toBe(
      false,
    );
    expect(
      isLooping(
        '- review the open pull requests\n- Review the open pull requests\n* review the open pull requests',
      ),
    ).toBe(true);
  });
});

describe('bulletsOf', () => {
  it('ignores the note Chief adds itself', () => {
    expect(
      bulletsOf('- a\n- b\n\n_Showing the next 3 of 14 meetings still to come._'),
    ).toHaveLength(2);
  });
});

describe('scoreRun', () => {
  it('passes a grounded answer that keeps the facts, in order', () => {
    const m = scoreRun(
      run({
        answer: '- 09:00 Standup with Priya',
        shown: '09:00 Standup with Priya',
        key: { must: ['Standup'], order: ['Standup'], max_bullets: 5 },
      }),
    );
    expect(m.pass).toBe(true);
    expect(m.facts).toBe(1);
  });

  it('fails an invented meeting on a day with none', () => {
    const m = scoreRun(
      run({
        answer: '- 10:00 Team sync with Dana',
        key: { forbid_times: true },
        group: 'brief-quiet',
      }),
    );
    expect(m.clean).toBe(false);
    expect(m.pass).toBe(false);
  });

  it('does not hold a brief written in code to a prompt no model saw', () => {
    const m = scoreRun(
      run({
        answer: '- Riverside Library: Your reserved book is ready',
        key: { must: ['reserved book'], forbid_numbers: true },
        group: 'brief-quiet',
        extra: { check: { by_code: true, fell_back: false, first: [] } },
      }),
    );
    expect(m.invented).toEqual([]);
    expect(m.clean).toBe(true);
  });

  it('still checks a brief written in code against what must not appear', () => {
    const m = scoreRun(
      run({
        answer: '- Team meeting on Friday',
        key: { forbid: ['meeting'] },
        group: 'brief-quiet',
        extra: { check: { by_code: true, fell_back: false, first: [] } },
      }),
    );
    expect(m.clean).toBe(false);
  });

  it('fails a missing fact and reports which one', () => {
    const m = scoreRun(run({ answer: '- hello', key: { must: ['Dentist', 'Standup'] } }));
    expect(m.facts).toBe(0);
    expect(m.missing).toEqual(['Dentist', 'Standup']);
  });

  it('fails more bullets than the cap, and counts the note as no bullet', () => {
    const six = Array.from({ length: 6 }, (_, i) => `- item ${i}`).join('\n');
    expect(scoreRun(run({ answer: six, key: { max_bullets: 5 } })).within_cap).toBe(false);
    expect(
      scoreRun(
        run({
          answer: '- a\n\n_Showing the next 3 of 14 meetings still to come._',
          key: { max_bullets: 1 },
        }),
      ).within_cap,
    ).toBe(true);
  });

  it('fails out-of-order facts', () => {
    const m = scoreRun(
      run({ answer: '- Dentist\n- Standup', key: { order: ['Standup', 'Dentist'] } }),
    );
    expect(m.order).toBe(0);
  });

  it('fails a stand-up written about "the user"', () => {
    const m = scoreRun(
      run({ answer: 'Today the user shipped two things.', key: { standup: true, max_words: 50 } }),
    );
    expect(m.format_ok).toBe(false);
  });

  it('does not count a [Placeholder] in free chat as an invented name', () => {
    const m = scoreRun(
      run({
        answer: 'Thanks. Dear [Colleague Name], thank you.',
        group: 'chat',
        key: { must: ['thank'], no_tool: true },
      }),
    );
    expect(m.clean).toBe(true);
  });

  it('passes a correct tool call and fails a wrong argument value', () => {
    const call = (args) =>
      sse({
        choices: [
          {
            delta: {
              tool_calls: [
                { index: 0, function: { name: 'fetch_calendar', arguments: JSON.stringify(args) } },
              ],
            },
          },
        ],
      });
    const key = { tool: { name: 'fetch_calendar', args: { range: ['tomorrow'] } } };
    expect(
      scoreRun(run({ answer: 'ok', response: call({ range: 'tomorrow' }), key })).tool_args_ok,
    ).toBe(true);
    const wrong = scoreRun(run({ answer: 'ok', response: call({ range: 'fortnight' }), key }));
    expect(wrong.tool_args_ok).toBe(false);
    expect(wrong.tool_valid).toBe(false);
  });

  it('does not hold a run to the tool checks when no tools were offered', () => {
    const key = { tool: { name: 'fetch_calendar', args: { range: ['tomorrow'] } } };
    const read = scoreRun(run({ answer: 'ok', key, offered: false }));

    expect(read.routed_read).toBe(true);
    expect(read.tool_called).toBeNull();
    expect(read.tool_args_ok).toBeNull();

    // The same answer with tools on offer and none called is a miss, so the
    // exemption is the router's and not a way to stop a model failing.
    const missed = scoreRun(run({ answer: 'ok', key }));
    expect(missed.routed_read).toBe(false);
    expect(missed.tool_called).toBe(false);
  });

  it('counts an answer written in code, with no model call at all, as a read', () => {
    const key = { tool: { name: 'fetch_calendar', args: { range: ['tomorrow'] } } };
    const empty = scoreRun({
      ...run({ answer: 'Nothing found in my calendar tomorrow.', key }),
      exchanges: [],
    });

    expect(empty.routed_read).toBe(true);
    expect(empty.tool_called).toBeNull();

    // No answer and no exchange is a run that failed, not a read.
    const failed = scoreRun({ ...run({ answer: '', key }), exchanges: [] });
    expect(failed.routed_read).toBe(false);
  });

  it('fails a tool call when none was expected', () => {
    const response = sse({
      choices: [
        {
          delta: {
            tool_calls: [{ index: 0, function: { name: 'fetch_calendar', arguments: '{}' } }],
          },
        },
      ],
    });
    expect(scoreRun(run({ answer: 'ok', response, key: { no_tool: true } })).no_tool_ok).toBe(
      false,
    );
  });

  it('fails an answer that stopped at the length limit', () => {
    const response = sse({ choices: [{ delta: { content: 'abc' }, finish_reason: 'length' }] });
    expect(scoreRun(run({ answer: 'abc', response })).pass).toBe(false);
  });

  it('computes read and write speed from the server timings', () => {
    const m = scoreRun(run({ answer: 'x' }));
    expect(m.prefill_tps).toBe(200);
    expect(m.decode_tps).toBe(20);
  });
});

describe('gatesFor', () => {
  const scored = (overrides) => {
    const r = run({
      answer: '- fine',
      group: 'brief-quiet',
      fixture: 'quiet',
      key: { forbid_times: true },
    });
    return {
      fixture: r.fixture,
      fixtureGroup: r.group,
      repeat: 0,
      m: { ...scoreRun(r), ...overrides },
    };
  };
  it('passes a clean quiet day and fails an invention on it', () => {
    expect(gatesFor([scored({})]).G3.pass).toBe(true);
    expect(
      gatesFor([scored({ clean: false, invented: ['time 10:00'], forbidden: [] })]).G3.pass,
    ).toBe(false);
  });
  it('fails memory over the budget and any swapping', () => {
    expect(gatesFor([scored({ peak_resident_mb: MEMORY_BUDGET_MB + 1 })]).G2.pass).toBe(false);
    expect(gatesFor([scored({ swap_in_mb: 50 })]).G2.pass).toBe(false);
  });
  it('fails G6 when a shipped draft names someone the prompt never did', () => {
    const draft = (overrides) => ({ ...scored(overrides), fixtureGroup: 'draft' });
    expect(gatesFor([draft({ invented: ['Maria'], shipped_invented: ['Maria'] })]).G6.pass).toBe(
      false,
    );
    expect(gatesFor([draft({ invented: [], shipped_invented: [] })]).G6.pass).toBe(true);
    expect(
      gatesFor([draft({ invented: ['Maria'], shipped_invented: [], blocked: true })]).G6.pass,
    ).toBe(true);
  });
  it('reports no G6 on a run with no drafts', () => {
    expect(gatesFor([scored({})]).G6).toBeUndefined();
  });
  it('fails a model that never calls a tool', () => {
    expect(gatesFor([scored({})]).G5.pass).toBe(false);
  });
});

describe('composite', () => {
  const parts = { quality: 80, tools: 60, speed: 40, memory: 100 };
  it('leaves out missing components and rescales the rest', () => {
    const c = composite(parts, 'light', { speedRankable: false, usefulness: null });
    expect(c.left_out.sort()).toEqual(['speed', 'usefulness']);
    expect(c.score).toBeCloseTo((45 * 80 + 10 * 60 + 10 * 100) / 65);
  });
  it('uses speed only on the target machine', () => {
    expect(composite(parts, 'light', { speedRankable: true, usefulness: 50 }).left_out).toEqual([]);
  });
});
