// Ranking from pairwise results.
//
// Bradley-Terry fits one strength per model from "A beat B" counts. The point
// of fitting it rather than counting wins is the interval: a bootstrap over the
// fixtures says how far each rank could move if the fixtures were a different
// sample, and models whose ranks overlap are reported as a tie. That is the
// difference between "the 3B model is first" and "the 3B model is first, and
// the 1.7B could be anywhere from first to second".

/** A small seeded generator, so a report built twice from the same data agrees with itself. */
export function rng(seed) {
  let a = seed >>> 0;
  return () => {
    a += 0x6d2b79f5;
    let t = a;
    t = Math.imul(t ^ (t >>> 15), t | 1);
    t ^= t + Math.imul(t ^ (t >>> 7), t | 61);
    return ((t ^ (t >>> 14)) >>> 0) / 4294967296;
  };
}

/**
 * Strengths (log scale, mean zero) by minorisation-maximisation. A tie counts as
 * half a win to each. A tiny prior win each way keeps a model that never lost
 * from running off to infinity.
 */
export function bradleyTerry(models, results, { iterations = 200 } = {}) {
  const index = new Map(models.map((m, i) => [m, i]));
  const n = models.length;
  const wins = Array.from({ length: n }, () => new Array(n).fill(0));
  for (const { a, b, winner } of results) {
    const i = index.get(a);
    const j = index.get(b);
    if (i === undefined || j === undefined || i === j) continue;
    if (winner === 'tie') {
      wins[i][j] += 0.5;
      wins[j][i] += 0.5;
    } else if (winner === a) wins[i][j] += 1;
    else if (winner === b) wins[j][i] += 1;
  }
  for (let i = 0; i < n; i += 1) for (let j = 0; j < n; j += 1) if (i !== j) wins[i][j] += 0.1;

  let p = new Array(n).fill(1);
  for (let step = 0; step < iterations; step += 1) {
    const next = p.map((_, i) => {
      let w = 0;
      let d = 0;
      for (let j = 0; j < n; j += 1) {
        if (i === j) continue;
        w += wins[i][j];
        d += (wins[i][j] + wins[j][i]) / (p[i] + p[j]);
      }
      return d ? w / d : p[i];
    });
    const norm = Math.exp(next.reduce((s, v) => s + Math.log(v), 0) / n);
    p = next.map((v) => v / norm);
  }
  return Object.fromEntries(models.map((m, i) => [m, Math.log(p[i])]));
}

const quantile = (sorted, q) =>
  sorted[Math.min(sorted.length - 1, Math.max(0, Math.floor(q * sorted.length)))];

/**
 * Bootstrap by fixture: resample whole fixtures with replacement, refit, and
 * record each model's rank. Returns strengths, a 95% interval on each, and a
 * rank band (5th to 95th percentile). Needs at least two models.
 */
export function rank(models, results, { rounds = 500, seed = 7 } = {}) {
  if (models.length < 2) return null;
  const strength = bradleyTerry(models, results);
  const byFixture = new Map();
  for (const r of results)
    (byFixture.get(r.fixture) ?? byFixture.set(r.fixture, []).get(r.fixture)).push(r);
  const fixtures = [...byFixture.keys()];
  const random = rng(seed);

  const draws = Object.fromEntries(models.map((m) => [m, []]));
  const ranks = Object.fromEntries(models.map((m) => [m, []]));
  for (let round = 0; round < rounds; round += 1) {
    const sample = [];
    for (let k = 0; k < fixtures.length; k += 1)
      sample.push(...byFixture.get(fixtures[Math.floor(random() * fixtures.length)]));
    const fit = bradleyTerry(models, sample, { iterations: 60 });
    const order = [...models].sort((x, y) => fit[y] - fit[x]);
    order.forEach((m, i) => ranks[m].push(i + 1));
    for (const m of models) draws[m].push(fit[m]);
  }

  const out = {};
  for (const m of models) {
    const d = draws[m].sort((x, y) => x - y);
    const r = ranks[m].sort((x, y) => x - y);
    out[m] = {
      strength: strength[m],
      low: quantile(d, 0.025),
      high: quantile(d, 0.975),
      best_rank: quantile(r, 0.05),
      worst_rank: quantile(r, 0.95),
    };
  }
  return out;
}

/** Share of judgements that agree between two sets keyed by the same ids. */
export function agreement(left, right) {
  let both = 0;
  let same = 0;
  for (const [id, value] of Object.entries(left)) {
    if (!(id in right)) continue;
    both += 1;
    if (right[id] === value) same += 1;
  }
  return both ? { n: both, rate: same / both } : null;
}

/** How often a judge changes its mind when the same pair is shown in the other order. */
export function positionConsistency(pairs) {
  const groups = new Map();
  for (const p of pairs) {
    const k = `${p.a}|${p.b}|${p.fixture}`;
    (groups.get(k) ?? groups.set(k, []).get(k)).push(p.winner);
  }
  let n = 0;
  let same = 0;
  for (const w of groups.values()) {
    if (w.length < 2) continue;
    n += 1;
    if (w[0] === w[1]) same += 1;
  }
  return n ? { n, rate: same / n } : null;
}
