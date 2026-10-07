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
        request: { messages: [{ role: 'user', content: shown }] },
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
