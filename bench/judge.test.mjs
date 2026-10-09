import fs from 'node:fs';
import os from 'node:os';
import path from 'node:path';

import { describe, expect, it } from 'vitest';

import {
  judgeRun,
  pairwisePrompt,
  parseJson,
  pointwisePrompt,
  readPairwise,
  readPointwise,
  usefulness,
} from './judge.mjs';

describe('reading a judge reply', () => {
  it('finds the JSON inside prose or a fence', () => {
    expect(parseJson('Sure!\n```json\n{"a": 1}\n```')).toEqual({ a: 1 });
    expect(parseJson('no json here')).toBeNull();
  });
  it('rejects marks it cannot read rather than guessing', () => {
    expect(readPointwise('{"grounded": 5, "useful": "high", "concise": 3}')).toBeNull();
    expect(readPairwise('{"winner": "C"}')).toBeNull();
  });
  it('clamps marks to 1 to 5', () => {
    expect(readPointwise('{"grounded": 9, "useful": 0, "concise": 3}')).toMatchObject({
      grounded: 5,
      useful: 1,
      concise: 3,
    });
  });
  it('maps marks to 0..100', () => {
    expect(usefulness({ grounded: 1, useful: 1, concise: 1 })).toBe(0);
    expect(usefulness({ grounded: 5, useful: 5, concise: 5 })).toBe(100);
  });
});

describe('the prompts', () => {
  it('carry the material and the answer, and no model name', () => {
    const p = pairwisePrompt('MATERIAL', 'ANSWER-A', 'ANSWER-B');
    expect(p).toContain('MATERIAL');
    expect(p).toContain('ANSWER-A');
    expect(pointwisePrompt('M', 'A')).not.toMatch(/qwen|llama/i);
  });
});

describe('judgeRun', () => {
  const fixtureRun = (model, answer) => ({
    type: 'run',
    fixture: 'f',
    repeat: 0,
    exchanges: [{ request: { messages: [{ role: 'user', content: 'material' }] } }],
    answer,
    model,
  });
  function setup(ids) {
    const dir = fs.mkdtempSync(path.join(os.tmpdir(), 'bench-judge-'));
    const scored = {};
    for (const id of ids) {
      fs.writeFileSync(
        path.join(dir, `${id}.jsonl`),
        `${JSON.stringify(fixtureRun(id, `answer from ${id}`))}\n`,
      );
      scored[id] = {
        runs: [
          {
            fixture: 'f',
            fixtureGroup: 'brief-busy',
            source: 's',
            repeat: 0,
            answer: `answer from ${id}`,
          },
        ],
      };
    }
    return { dir, scored };
  }

  it('does nothing and says so when the judge is off', async () => {
    const { dir, scored } = setup(['a']);
    const j = await judgeRun(dir, {}, scored, { mode: 'none' });
    expect(j.skipped).toBe(true);
    expect(j.items).toEqual([]);
  });

  it('writes the requests and scores supplied judgements, mapping a swapped verdict back to the real model', async () => {
    const { dir, scored } = setup(['a', 'b']);
    await judgeRun(dir, {}, scored, { mode: 'file' });
    const requests = JSON.parse(fs.readFileSync(path.join(dir, 'judge-requests.json'), 'utf8'));
    expect(requests.map((r) => r.id)).toEqual(
      expect.arrayContaining(['point/a/f', 'point/b/f', 'pair/a/b/f/ab', 'pair/a/b/f/ba']),
    );

    // "A" wins in both orders: that is model a in the first order and model b in the swapped one.
    fs.writeFileSync(
      path.join(dir, 'judgements.json'),
      JSON.stringify({
        _judge: 'test-judge',
        'point/a/f': '{"grounded":5,"useful":4,"concise":4,"invented":[],"reason":"ok"}',
        'pair/a/b/f/ab': '{"winner":"A"}',
        'pair/a/b/f/ba': '{"winner":"A"}',
      }),
    );
    const j = await judgeRun(dir, {}, scored, { mode: 'file' });
    expect(j.judge_model).toBe('test-judge');
    expect(j.items).toHaveLength(1);
    expect(j.pairs.map((p) => p.winner)).toEqual(['a', 'b']);
  });
});
