import { describe, expect, it } from 'vitest';

import { summarise } from './report.mjs';

const gate = (pass, extra = {}) => ({ name: 'g', pass, detail: '', evidence: [], ...extra });
const model = (g3rate, g4pass) => ({
  runner: { load_ms: 1, resident_after_load_mb: 1 },
  runs: [],
  fixtures: [],
  parts: { quality: 80, tools: 80, speed: 80, memory: 80 },
  gates: {
    G1: gate(true),
    G2: gate(true),
    G3: gate(g3rate === 0, { informational: true, rate: g3rate }),
    G4: gate(g4pass),
    G5: gate(true),
  },
});
const env = {
  machine: { target: true },
  models: [
    { id: 'a', tier: 'light' },
    { id: 'b', tier: 'light' },
  ],
};
const judged = { items: [], pairs: [], skipped: true, notes: [] };

describe('ranking', () => {
  it('ranks a model that invents on quiet days, because the invention rate is reported and not gated', () => {
    const s = summarise(env, { a: model(0.4, true), b: model(0, true) }, judged);
    expect(s.ranked.map((m) => m.id).sort()).toEqual(['a', 'b']);
  });
  it('still refuses a model that fails a real gate', () => {
    const s = summarise(env, { a: model(0, false), b: model(0, true) }, judged);
    expect(s.ranked.map((m) => m.id)).toEqual(['b']);
    expect(s.unranked.map((m) => m.id)).toEqual(['a']);
  });
});
