import { describe, expect, it } from 'vitest';

import { agreement, bradleyTerry, positionConsistency, rank } from './stats.mjs';

const games = (a, b, aWins, total, offset = 0) =>
  Array.from({ length: total }, (_, i) => ({
    a,
    b,
    fixture: `f${(i + offset) % 10}`,
    winner: i < aWins ? a : b,
  }));

describe('bradleyTerry', () => {
  it('puts the model that wins more on top', () => {
    const s = bradleyTerry(['x', 'y'], games('x', 'y', 8, 10));
    expect(s.x).toBeGreaterThan(s.y);
    expect(s.x + s.y).toBeCloseTo(0);
  });
  it('keeps a model that never lost finite', () => {
    expect(Number.isFinite(bradleyTerry(['x', 'y'], games('x', 'y', 10, 10)).x)).toBe(true);
  });
  it('treats ties as an even split', () => {
    const s = bradleyTerry(
      ['x', 'y'],
      [
        { a: 'x', b: 'y', winner: 'tie' },
        { a: 'x', b: 'y', winner: 'tie' },
      ],
    );
    expect(Math.abs(s.x - s.y)).toBeLessThan(1e-6);
  });
});

describe('rank', () => {
  it('needs two models', () => {
    expect(rank(['x'], [])).toBeNull();
  });
  it('gives a clear gap a single rank and an even match overlapping ones', () => {
    const results = [...games('x', 'z', 10, 10), ...games('y', 'z', 10, 10)];
    for (let i = 0; i < 10; i += 1)
      results.push({ a: 'x', b: 'y', fixture: `f${i}`, winner: i % 2 ? 'x' : 'y' });
    const r = rank(['x', 'y', 'z'], results);
    expect(r.z.best_rank).toBe(3);
    expect(r.x.best_rank).toBeLessThan(r.x.worst_rank + 1);
    expect(r.x.worst_rank).toBeGreaterThanOrEqual(2);
  });
  it('is the same twice', () => {
    const results = games('x', 'y', 7, 10);
    expect(rank(['x', 'y'], results)).toEqual(rank(['x', 'y'], results));
  });
});

describe('agreement and consistency', () => {
  it('measures overlap only', () => {
    expect(agreement({ a: 1, b: 2 }, { a: 1, b: 3, c: 4 })).toEqual({ n: 2, rate: 0.5 });
    expect(agreement({}, {})).toBeNull();
  });
  it('finds a judge that follows position rather than content', () => {
    const swapped = [
      { a: 'x', b: 'y', fixture: 'f', winner: 'x' },
      { a: 'x', b: 'y', fixture: 'f', winner: 'y' },
    ];
    expect(positionConsistency(swapped)).toEqual({ n: 1, rate: 0 });
  });
});
