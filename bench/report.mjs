// Turns scored runs into the files a person reads and the files a machine reads.
//
// The layout of report.md is fixed so two reports can be put side by side and
// read in the same order: headline, what changed, speed and memory, quality,
// fixtures, answers, judge, open problems. Nothing is left out because it is
// unflattering; the open problems section is generated from the data, not
// written by hand.

import fs from 'node:fs';
import path from 'node:path';

import { usefulness } from './judge.mjs';
import { composite, median } from './score.mjs';
import { positionConsistency, rank } from './stats.mjs';

export const SCHEMA_VERSION = 1;

const rate = (runs, tokens, ms) => {
  const t = runs.reduce((a, r) => a + (r.m[tokens] ?? 0), 0);
  const d = runs.reduce((a, r) => a + (r.m[ms] ?? 0), 0);
  return d > 0 ? (t / d) * 1000 : null;
};
const pct = (v) => (v === null || v === undefined ? '–' : `${Math.round(v * 100)}%`);
const num = (v, d = 0) => (v === null || v === undefined ? '–' : v.toFixed(d));
const secs = (ms) => (ms === null || ms === undefined ? '–' : `${(ms / 1000).toFixed(1)} s`);
const mark = (ok) => (ok ? '✓' : '✗');
const csvCell = (v) =>
  v === null || v === undefined
    ? ''
    : /[",\n]/.test(String(v))
      ? `"${String(v).replace(/"/g, '""')}"`
      : String(v);
const csv = (rows) => `${rows.map((r) => r.map(csvCell).join(',')).join('\n')}\n`;

/** Everything derived from the scored runs and the judge, in one object that both report.md and results.json are written from. */
export function summarise(env, scored, judged) {
  const tierOf = Object.fromEntries(env.models.map((m) => [m.id, m.tier]));
  const marks = {};
  for (const item of judged.items) (marks[item.model] ??= []).push(item);

  const models = Object.entries(scored).map(([id, s]) => {
    const judgeMarks = marks[id] ?? [];
    const usefulScore = judgeMarks.length
      ? judgeMarks.reduce((sum, m) => sum + usefulness(m), 0) / judgeMarks.length
      : null;
    const comp = composite(s.parts, tierOf[id], {
      speedRankable: env.machine.target,
      usefulness: usefulScore,
    });
    const runs = s.runs;
    const cold = runs.filter((r) => r.m.phase === 'cold');
    const warm = runs.filter((r) => r.m.phase !== 'cold');
    return {
      id,
      name: env.models.find((m) => m.id === id)?.name ?? id,
      tier: tierOf[id],
      gates: s.gates,
      eligible: Object.values(s.gates).every((g) => g.pass || g.informational),
      parts: { ...s.parts, usefulness: usefulScore },
      composite: comp.score,
      left_out: comp.left_out,
      speed: {
        load_ms: s.runner.load_ms,
        cold_ttfw_ms: median(cold.map((r) => r.m.ttfw_ms)),
        warm_ttfw_ms: median(warm.map((r) => r.m.ttfw_ms)),
        // Totals rather than a median of rates: most requests read a few dozen new tokens, which is slow per token and says little.
        decode_tps: rate(runs, 'output_tokens', 'output_ms'),
        prefill_tps: rate(runs, 'prompt_tokens', 'prompt_ms'),
        finish_ms: median(runs.map((r) => r.m.wall_ms)),
        peak_resident_mb: Math.max(0, ...runs.map((r) => r.m.peak_resident_mb ?? 0)),
        resident_after_load_mb: s.runner.resident_after_load_mb,
      },
      fixtures: s.fixtures,
      overall_pass: runs.length ? runs.filter((r) => r.m.pass).length / runs.length : null,
    };
  });

  const ranked = models
    .filter((m) => m.eligible && m.composite !== null)
    .sort((a, b) => b.composite - a.composite);
  const unranked = models.filter((m) => !ranked.includes(m));

  const bt = judged.pairs.length
    ? rank(
        Object.keys(scored),
        judged.pairs.map((p) => ({ ...p })),
      )
    : null;
  return { models, ranked, unranked, bt, consistency: positionConsistency(judged.pairs) };
}

function previousFor(runDir, model) {
  const file = path.join(path.dirname(runDir), 'trend.csv');
  if (!fs.existsSync(file)) return null;
  const rows = fs
    .readFileSync(file, 'utf8')
    .trim()
    .split('\n')
    .slice(1)
    .map((l) => l.split(','));
  const mine = rows.filter((r) => r[4] === model && r[0] !== path.basename(runDir));
  return mine.at(-1) ?? null;
}

function openProblems(env, summary, judged) {
  const out = [];
  if (!env.machine.target) {
    out.push(
      `**Not the target machine.** This ran on "${env.machine.label}" (${env.machine.cpu}, ${env.machine.logical_cores} cores, ${env.machine.memory_gb} GB), not the 2014 Mac mini. Speed and memory are shown for information, left out of the composite score, and must not be compared with a Mac mini run.`,
    );
  }
  if (env.models.length < 2)
    out.push(
      '**One model only.** There is nothing to rank against, so no pairwise judging, Bradley-Terry ranking or rank bands were produced. Every other part of the pipeline ran.',
    );
  if (judged.skipped)
    out.push(
      '**No AI judge.** Usefulness is not part of the composite, so scores are not comparable with a judged run.',
    );
  if (env.code.dirty)
    out.push(
      `**Uncommitted changes.** The code under test had local edits (hash ${env.code.dirty_hash}), so this run cannot be reproduced from commit ${env.code.short} alone.`,
    );
  for (const m of summary.models) {
    const varied = m.fixtures.filter((f) => f.runs > 1);
    const identical = varied.filter((f) => f.distinct_answers === 1).length;
    if (varied.length && identical / varied.length >= 0.5) {
      out.push(
        `**Little run-to-run variation (${m.name}).** ${identical} of ${varied.length} fixtures produced the same answer on every repeat. At Chief's low temperature that is expected, and it means repeats measure speed more than they measure answer quality. Treat the pass rates as close to a single sample per fixture.`,
      );
    }
    for (const g of Object.values(m.gates))
      if (!g.pass) out.push(`**${m.name} fails "${g.name}":** ${g.detail}.`);
  }
  out.push(
    '**Invention check is heuristic.** It catches times, numbers and names that are not in the material. It does not catch an invented claim in ordinary words; the judge and the human ratings are there for that.',
  );
  out.push(
    "**Fixtures are synthetic.** They are the runbook scenarios, so this measures behaviour on those days. A model that does well here may still do badly on a real person's week.",
  );
  for (const note of judged.notes) out.push(`Judge note: ${note}`);
  return out;
}

function groupTable(m) {
  const groups = {};
  for (const f of m.fixtures) (groups[f.group] ??= []).push(f);
  const rows = Object.entries(groups).map(([g, list]) => {
    const avg = (k) => {
      const v = list.map((f) => f[k]).filter((x) => x !== null && x !== undefined);
      return v.length ? v.reduce((a, b) => a + b, 0) / v.length : null;
    };
    return `| ${g} | ${list.length} | ${pct(avg('pass_rate'))} | ${pct(avg('facts'))} | ${pct(avg('clean_rate'))} | ${pct(avg('order'))} |`;
  });
  return [
    '| Group | Fixtures | Every check passed | Facts kept | Nothing invented | In order |',
    '|---|---|---|---|---|---|',
    ...rows,
  ].join('\n');
}

export function renderMarkdown(dir, env, scored, judged, summary) {
  const L = [];
  const label = env.machine.target ? 'target machine' : 'NOT the target machine';
  L.push(`# Chief model benchmark — ${path.basename(dir)}`);
  L.push('');
  L.push(
    `- **Commit:** \`${env.code.short}\` on \`${env.code.branch}\`${env.code.dirty ? ` (uncommitted changes, hash \`${env.code.dirty_hash}\`)` : ''}`,
  );
  L.push(`- **Fixtures:** ${env.fixture_version} · ${env.repeats} repeats each`);
  L.push(
    `- **Machine:** ${env.machine.label} — ${env.machine.cpu}, ${env.machine.logical_cores} cores, ${env.machine.memory_gb} GB, ${env.machine.os} (${label})`,
  );
  L.push(`- **Engine:** llama.cpp ${env.engine.build ?? 'unknown build'}`);
  L.push(
    `- **Judge:** ${judged.skipped ? 'none' : `${judged.judge_model}, prompt ${judged.prompt_version}, spent $${judged.spend.toFixed(3)}`}`,
  );
  L.push(`- **Started:** ${env.started_at}`);
  L.push('');

  L.push('## 1. Headline');
  L.push('');
  L.push(
    'A model that fails any gate is not ranked, however fast it is. Gates: G1 starts and answers · G2 fits in 8 GB · G4 finishes without circling · G5 tool calls valid. G3 is not a gate: it shows the share of quiet-day runs where the model invented something, and lower is better.',
  );
  L.push('');
  L.push(
    '| # | Model | Tier | G1 | G2 | Invents | G4 | G5 | Score | Quality | Tools | Usefulness | Speed | Memory |',
  );
  L.push('|---|---|---|---|---|---|---|---|---|---|---|---|---|---|');
  const order = [...summary.ranked, ...summary.unranked];
  order.forEach((m) => {
    const place = summary.ranked.includes(m)
      ? summary.ranked.length > 1 || env.models.length > 1
        ? summary.ranked.indexOf(m) + 1
        : '–'
      : 'n/r';
    const g = m.gates;
    L.push(
      `| ${place} | ${m.name} | ${m.tier} | ${mark(g.G1.pass)} | ${mark(g.G2.pass)} | ${pct(g.G3.rate)} | ${mark(g.G4.pass)} | ${mark(g.G5.pass)} | ${num(m.composite)} | ${num(m.parts.quality)} | ${num(m.parts.tools)} | ${num(m.parts.usefulness)} | ${env.machine.target ? num(m.parts.speed) : 'n/a'} | ${num(m.parts.memory)} |`,
    );
  });
  L.push('');
  L.push(
    '`n/r` = not ranked (a gate failed). Score is out of 100, weighted by tier profile (plan §10); components with no evidence are left out and the rest rescaled.',
  );
  for (const m of summary.models)
    if (m.left_out.length) L.push(`${m.name}: left out of the score — ${m.left_out.join(', ')}.`);
  if (summary.bt) {
    L.push('');
    L.push('**Pairwise ranking (Bradley-Terry, bootstrap over fixtures):**');
    L.push('');
    L.push('| Model | Strength | 95% interval | Rank band |');
    L.push('|---|---|---|---|');
    for (const [id, r] of Object.entries(summary.bt).sort(
      (a, b) => b[1].strength - a[1].strength,
    )) {
      L.push(
        `| ${id} | ${num(r.strength, 2)} | ${num(r.low, 2)} to ${num(r.high, 2)} | ${r.best_rank === r.worst_rank ? r.best_rank : `${r.best_rank}–${r.worst_rank}`} |`,
      );
    }
  }
  L.push('');

  L.push('## 2. What changed since the last run');
  L.push('');
  let anyPrev = false;
  for (const m of summary.models) {
    const prev = previousFor(dir, m.id);
    if (prev) {
      anyPrev = true;
      L.push(
        `- ${m.name}: score ${num(m.composite)} against ${prev[5] || '–'} at commit ${prev[1]}.`,
      );
    }
  }
  if (!anyPrev)
    L.push('First run on record for these models, so there is nothing to compare with.');
  L.push('');

  L.push(
    `## 3. Speed and memory${env.machine.target ? '' : ' (not the target machine — for information only)'}`,
  );
  L.push('');
  L.push(
    '| Model | Load | First word (cold) | First word (warm) | Write speed | Read speed | Finish (median) | Memory after load | Peak memory |',
  );
  L.push('|---|---|---|---|---|---|---|---|---|');
  for (const m of summary.models) {
    const s = m.speed;
    L.push(
      `| ${m.name} | ${secs(s.load_ms)} | ${secs(s.cold_ttfw_ms)} | ${secs(s.warm_ttfw_ms)} | ${num(s.decode_tps, 1)} tok/s | ${num(s.prefill_tps)} tok/s | ${secs(s.finish_ms)} | ${num(s.resident_after_load_mb)} MB | ${num(s.peak_resident_mb)} MB |`,
    );
  }
  L.push('');
  L.push(
    'Cold = the first run after the server started. Write speed is generating the answer; read speed is reading the prompt. Memory is the whole server process, measured from outside.',
  );
  L.push('');

  L.push('## 4. Quality by group');
  L.push('');
  for (const m of summary.models) {
    L.push(`### ${m.name}`);
    L.push('');
    L.push(groupTable(m));
    L.push('');
  }

  L.push('## 5. Every fixture');
  L.push('');
  for (const m of summary.models) {
    L.push(`### ${m.name}`);
    L.push('');
    L.push(
      '| Fixture | Group | Passed all checks | Facts | Clean | Distinct answers | Median finish |',
    );
    L.push('|---|---|---|---|---|---|---|');
    for (const f of m.fixtures)
      L.push(
        `| ${f.id} | ${f.group} | ${pct(f.pass_rate)} (${Math.round((f.pass_rate ?? 0) * f.runs)}/${f.runs}) | ${pct(f.facts)} | ${pct(f.clean_rate)} | ${f.distinct_answers} | ${secs(f.wall_ms)} |`,
      );
    L.push('');
  }

  L.push('## 6. Answers, verbatim');
  L.push('');
  L.push(
    'The first repeat of the three best and three worst fixtures. Why each failed is listed under it.',
  );
  L.push('');
  for (const m of summary.models) {
    const runs = scored[m.id].runs.filter((r) => r.repeat === 0);
    const byPass = [...m.fixtures].sort(
      (a, b) => (a.pass_rate ?? 0) - (b.pass_rate ?? 0) || a.id.localeCompare(b.id),
    );
    const picks = [
      ...new Set([...byPass.slice(0, 3), ...byPass.slice(-3).reverse()].map((f) => f.id)),
    ];
    L.push(`### ${m.name}`);
    for (const id of picks) {
      const run = runs.find((r) => r.fixture === id);
      if (!run) continue;
      const why = [];
      if (run.m.missing.length) why.push(`missing: ${run.m.missing.join(', ')}`);
      if (run.m.invented.length) why.push(`invented: ${run.m.invented.join(', ')}`);
      if (run.m.forbidden.length)
        why.push(`mentioned what was out of scope: ${run.m.forbidden.join(', ')}`);
      if (run.m.within_cap === false) why.push(`${run.m.bullets} bullets, over the cap`);
      if (run.m.format_ok === false)
        why.push('stand-up format not followed (first person, short, no tools)');
      if (run.m.looped) why.push('looped');
      if (run.m.tool_called === false) why.push('did not call the expected tool');
      if (run.m.tool_args_ok === false) why.push('tool arguments not as expected');
      if (run.m.no_tool_ok === false) why.push('called a tool when it should not have');
      L.push('');
      L.push(
        `**${id}** — ${run.m.pass ? 'passed every check' : 'failed'}${why.length ? ` (${why.join('; ')})` : ''}`,
      );
      L.push('');
      L.push(
        run.answer
          ? run.answer
              .split('\n')
              .map((l) => `> ${l}`)
              .join('\n')
          : `> (no answer: ${run.error ?? 'empty'})`,
      );
    }
    L.push('');
  }

  L.push('## 7. AI judge');
  L.push('');
  if (judged.skipped) {
    L.push(`Not run. ${judged.notes.join(' ')}`);
  } else {
    L.push(
      `Judge \`${judged.judge_model}\`, prompt ${judged.prompt_version}. Judged the first repeat of each fixture, ${judged.items.length} answers and ${judged.pairs.length} pairwise comparisons. Spend $${judged.spend.toFixed(3)}.`,
    );
    L.push('');
    if (judged.items.length) {
      L.push('| Model | Fixture | Grounded | Useful | Concise | Reason |');
      L.push('|---|---|---|---|---|---|');
      for (const i of judged.items)
        L.push(
          `| ${i.model} | ${i.fixture} | ${i.grounded} | ${i.useful} | ${i.concise} | ${i.reason.replace(/\|/g, '/')}${i.invented.length ? ` — invented: ${i.invented.join('; ').replace(/\|/g, '/')}` : ''} |`,
        );
      L.push('');
      // The check that matters most for trusting the judge: does it agree with the deterministic one?
      const disagreements = [];
      for (const i of judged.items) {
        // The judge saw the first repeat, so compare with that repeat, not the average.
        const run = scored[i.model]?.runs.find((x) => x.fixture === i.fixture && x.repeat === 0);
        if (!run) continue;
        const code = run.m.clean;
        const judge = i.grounded >= 4;
        if (code !== judge)
          disagreements.push(
            `${i.model}/${i.fixture}: checks say ${code ? 'clean' : 'not clean'}, judge says grounded ${i.grounded}/5`,
          );
      }
      L.push(
        `**Judge against the deterministic checks:** disagree on ${disagreements.length} of ${judged.items.length}.${disagreements.length ? ` ${disagreements.join('; ')}.` : ''} Each disagreement is a bug in the checks or in the judge, and is worth reading.`,
      );
    }
    if (summary.consistency)
      L.push(
        `\n**Position consistency:** ${pct(summary.consistency.rate)} of ${summary.consistency.n} pairs got the same verdict in both orders.`,
      );
  }
  L.push('');

  L.push('## 8. Human rating');
  L.push('');
  const ratings = path.join(dir, 'ratings.json');
  if (fs.existsSync(ratings))
    L.push(`ratings.json found: ${humanSummary(JSON.parse(fs.readFileSync(ratings, 'utf8')))}`);
  else
    L.push(
      'Open `human-eval.html` in a browser, rate (about 20 minutes for a full run), press **Download ratings**, and save the file as `ratings.json` in this folder. Run `pnpm bench --score-only <this folder>` to fold it into the report.',
    );
  L.push('');

  L.push('## 9. Open problems');
  L.push('');
  for (const p of openProblems(env, summary, judged)) L.push(`- ${p}`);
  L.push('');
  return L.join('\n');
}

function humanSummary(r) {
  const entries = Object.values(r.ratings ?? {});
  return `${entries.length} ratings recorded.`;
}

function scoresCsv(summary) {
  const rows = [
    [
      'model',
      'fixture',
      'group',
      'source',
      'runs',
      'pass_rate',
      'facts',
      'clean_rate',
      'order',
      'distinct_answers',
      'median_first_word_ms',
      'median_finish_ms',
      'decode_tps',
      'prefill_tps',
    ],
  ];
  for (const m of summary.models) {
    for (const f of m.fixtures)
      rows.push([
        m.id,
        f.id,
        f.group,
        f.source,
        f.runs,
        f.pass_rate,
        f.facts,
        f.clean_rate,
        f.order,
        f.distinct_answers,
        f.ttfw_ms,
        f.wall_ms,
        f.decode_tps,
        f.prefill_tps,
      ]);
  }
  return csv(rows);
}

const TREND_HEAD = [
  'run',
  'commit',
  'dirty',
  'machine',
  'model',
  'score',
  'quality',
  'tools',
  'usefulness',
  'speed',
  'memory',
  'gates_passed',
];

function updateTrend(dir, env, summary) {
  const file = path.join(path.dirname(dir), 'trend.csv');
  const run = path.basename(dir);
  const kept = fs.existsSync(file)
    ? fs
        .readFileSync(file, 'utf8')
        .trim()
        .split('\n')
        .slice(1)
        .filter((l) => !l.startsWith(`${run},`))
    : [];
  const rows = summary.models.map((m) =>
    [
      run,
      env.code.short,
      env.code.dirty ? 'yes' : 'no',
      env.machine.label,
      m.id,
      m.composite?.toFixed(1) ?? '',
      m.parts.quality?.toFixed(1) ?? '',
      m.parts.tools?.toFixed(1) ?? '',
      m.parts.usefulness?.toFixed(1) ?? '',
      env.machine.target ? (m.parts.speed?.toFixed(1) ?? '') : '',
      m.parts.memory?.toFixed(1) ?? '',
      Object.values(m.gates).filter((g) => g.pass).length,
    ]
      .map(csvCell)
      .join(','),
  );
  fs.writeFileSync(file, `${TREND_HEAD.join(',')}\n${[...kept, ...rows].join('\n')}\n`);
}

function writeAnswers(dir, scored) {
  for (const [id, m] of Object.entries(scored)) {
    const folder = path.join(dir, 'answers', id);
    fs.mkdirSync(folder, { recursive: true });
    const byFixture = {};
    for (const r of m.runs) (byFixture[r.fixture] ??= []).push(r);
    for (const [fixture, runs] of Object.entries(byFixture)) {
      const body = runs
        .map(
          (r) =>
            `## Repeat ${r.repeat}${r.m.pass ? '' : ' — failed a check'}\n\n${r.answer ?? `(no answer: ${r.error})`}\n`,
        )
        .join('\n');
      fs.writeFileSync(path.join(folder, `${fixture}.md`), `# ${id} · ${fixture}\n\n${body}`);
    }
  }
}

export function writeReport(dir, env, scored, judged) {
  const summary = summarise(env, scored, judged);
  fs.writeFileSync(path.join(dir, 'report.md'), renderMarkdown(dir, env, scored, judged, summary));
  fs.writeFileSync(path.join(dir, 'scores.csv'), scoresCsv(summary));
  writeAnswers(dir, scored);
  updateTrend(dir, env, summary);
  fs.writeFileSync(path.join(dir, 'human-eval.html'), humanEvalPage(dir, env, scored));
  fs.writeFileSync(
    path.join(dir, 'results.json'),
    `${JSON.stringify({ schema_version: SCHEMA_VERSION, env, judge: { ...judged, items: judged.items, pairs: judged.pairs }, summary: { models: summary.models.map(({ fixtures, ...rest }) => ({ ...rest, fixtures })), ranked: summary.ranked.map((m) => m.id), bradley_terry: summary.bt, position_consistency: summary.consistency } }, null, 2)}\n`,
  );
}

// ---------------------------------------------------------------------------
// The human rating page
// ---------------------------------------------------------------------------

function seeded(seed) {
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
 * One self-contained page, no network. With two or more models it shows pairs
 * of answers in a random order with the model names hidden; with one it shows
 * single answers to mark out of five. The key to which side was which is in the
 * page's data, not on screen, so the ratings can be matched up afterwards.
 */
export function humanEvalPage(dir, env, scored) {
  const ids = Object.keys(scored);
  const random = seeded(11);
  const first = (id, fixture) =>
    scored[id].runs.find((r) => r.fixture === fixture && r.repeat === 0);
  const shown = (id, fixture) => {
    const raw = readMaterial(dir, id, fixture);
    return raw;
  };
  const fixtures = [...new Set(scored[ids[0]].runs.map((r) => r.fixture))];
  const tasks = [];
  if (ids.length >= 2) {
    for (let i = 0; i < ids.length; i += 1) {
      for (let j = i + 1; j < ids.length; j += 1) {
        for (const f of fixtures) {
          const a = first(ids[i], f);
          const b = first(ids[j], f);
          if (!a?.answer || !b?.answer || a.answer === b.answer) continue;
          const flip = random() < 0.5;
          tasks.push({
            id: `pair/${ids[i]}/${ids[j]}/${f}`,
            kind: 'pair',
            fixture: f,
            material: shown(ids[i], f),
            left: flip ? ids[j] : ids[i],
            right: flip ? ids[i] : ids[j],
            leftText: (flip ? b : a).answer,
            rightText: (flip ? a : b).answer,
          });
        }
      }
    }
  } else {
    for (const f of fixtures) {
      const r = first(ids[0], f);
      if (r?.answer)
        tasks.push({
          id: `single/${ids[0]}/${f}`,
          kind: 'single',
          fixture: f,
          material: shown(ids[0], f),
          model: ids[0],
          text: r.answer,
        });
    }
  }
  // Shuffle so the order never groups a model's answers together.
  for (let i = tasks.length - 1; i > 0; i -= 1) {
    const j = Math.floor(random() * (i + 1));
    [tasks[i], tasks[j]] = [tasks[j], tasks[i]];
  }
  const data = JSON.stringify({ run: path.basename(dir), commit: env.code.short, tasks }).replace(
    /</g,
    '\\u003c',
  );

  return `<!doctype html>
<html lang="en"><head><meta charset="utf-8"><meta name="viewport" content="width=device-width,initial-scale=1">
<title>Chief benchmark — rate the answers</title>
<style>
:root{--bg:#f4f5f3;--fg:#14171a;--mut:#5b6168;--card:#fff;--line:#d5d8d4;--acc:#2c5c7a}
@media (prefers-color-scheme:dark){:root{--bg:#14171a;--fg:#ecefec;--mut:#9aa1a8;--card:#1d2226;--line:#333a40;--acc:#7fb1d1}}
body{margin:0;background:var(--bg);color:var(--fg);font:14px/1.5 system-ui,sans-serif}
main{max-width:860px;margin:0 auto;padding:16px}
h1{font-size:20px;margin:8px 0}.mut{color:var(--mut)}
.card{background:var(--card);border:1px solid var(--line);border-radius:8px;padding:16px;margin:16px 0}
.cols{display:grid;grid-template-columns:1fr 1fr;gap:12px}@media(max-width:640px){.cols{grid-template-columns:1fr}}
pre{white-space:pre-wrap;margin:0;font:13px/1.5 ui-monospace,monospace}
.ans{border:1px solid var(--line);border-radius:6px;padding:12px}
button{font:inherit;border:1px solid var(--line);background:var(--card);color:var(--fg);border-radius:6px;padding:6px 12px;cursor:pointer}
button.on{background:var(--acc);color:#fff;border-color:var(--acc)}
textarea{width:100%;box-sizing:border-box;background:var(--bg);color:var(--fg);border:1px solid var(--line);border-radius:6px;padding:6px;font:inherit}
details{margin:8px 0}.bar{position:sticky;top:0;background:var(--bg);padding:8px 0;display:flex;gap:12px;align-items:center;flex-wrap:wrap}
</style></head><body><main>
<h1>Rate the answers</h1>
<p class="mut" id="intro"></p>
<div class="bar"><span id="count"></span><button id="save">Download ratings</button></div>
<div id="tasks"></div>
</main>
<script id="data" type="application/json">${data}</script>
<script>
const D=JSON.parse(document.getElementById('data').textContent);
const KEY='chief-bench-'+D.run;let R={};
try{R=JSON.parse(localStorage.getItem(KEY)||'{}')}catch(e){}
const persist=()=>{try{localStorage.setItem(KEY,JSON.stringify(R))}catch(e){}count()};
const el=(t,a={},...k)=>{const e=document.createElement(t);for(const[x,v]of Object.entries(a))x==='class'?e.className=v:x.startsWith('on')?e[x]=v:e.setAttribute(x,v);for(const c of k)e.append(c);return e};
document.getElementById('intro').textContent=D.tasks[0]&&D.tasks[0].kind==='pair'?'For each day, pick the answer a busy person would rather be handed. Model names are hidden. Anything invented counts heavily against an answer.':'Mark each answer from 1 (poor) to 5 (excellent) on whether it is true to the material, useful, and concise.';
function count(){document.getElementById('count').textContent=Object.keys(R).length+' of '+D.tasks.length+' rated'}
for(const t of D.tasks){
 const card=el('div',{class:'card'});
 card.append(el('strong',{},t.fixture));
 const d=el('details');d.append(el('summary',{},'What the assistant was shown'),el('pre',{},t.material));card.append(d);
 if(t.kind==='pair'){
  const cols=el('div',{class:'cols'});
  cols.append(el('div',{class:'ans'},el('b',{},'A'),el('pre',{},t.leftText)),el('div',{class:'ans'},el('b',{},'B'),el('pre',{},t.rightText)));
  card.append(cols);
  const row=el('p');
  for(const[v,l]of[['left','A is better'],['tie','About the same'],['right','B is better']]){
   const b=el('button',{onclick:()=>{R[t.id]={...(R[t.id]||{}),winner:v==='left'?t.left:v==='right'?t.right:'tie'};[...row.children].forEach(x=>x.classList.remove('on'));b.classList.add('on');persist()}},l);
   const cur=R[t.id]&&R[t.id].winner;if(cur&&((v==='left'&&cur===t.left)||(v==='right'&&cur===t.right)||(v==='tie'&&cur==='tie')))b.classList.add('on');
   row.append(b)}
  card.append(row);
 }else{
  card.append(el('div',{class:'ans'},el('pre',{},t.text)));
  for(const k of['grounded','useful','concise']){
   const row=el('p',{},k+': ');
   for(let n=1;n<=5;n++){const b=el('button',{onclick:()=>{R[t.id]={...(R[t.id]||{}),model:t.model,[k]:n};[...row.querySelectorAll('button')].forEach(x=>x.classList.remove('on'));b.classList.add('on');persist()}},String(n));
    if(R[t.id]&&R[t.id][k]===n)b.classList.add('on');row.append(b)}
   card.append(row)}
 }
 const note=el('textarea',{rows:'2',placeholder:'Optional note: what was wrong or good?'});note.value=(R[t.id]&&R[t.id].note)||'';
 note.oninput=()=>{R[t.id]={...(R[t.id]||{}),note:note.value};persist()};card.append(note);
 document.getElementById('tasks').append(card)}
count();
document.getElementById('save').onclick=()=>{const b=new Blob([JSON.stringify({run:D.run,commit:D.commit,ratings:R},null,2)],{type:'application/json'});const a=el('a',{href:URL.createObjectURL(b),download:'ratings.json'});a.click()};
</script></body></html>
`;
}

function readMaterial(dir, id, fixture) {
  const file = path.join(dir, `${id}.jsonl`);
  for (const line of fs.readFileSync(file, 'utf8').split('\n')) {
    if (!line.trim()) continue;
    const r = JSON.parse(line);
    if (r.type === 'run' && r.fixture === fixture && r.repeat === 0) {
      const last = r.exchanges.at(-1)?.request?.messages ?? [];
      return last
        .map(
          (m) =>
            `[${m.role}] ${typeof m.content === 'string' ? m.content : JSON.stringify(m.content)}`,
        )
        .join('\n\n')
        .slice(-6000);
    }
  }
  return '';
}
