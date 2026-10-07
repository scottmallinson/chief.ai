// The AI judge.
//
// It only ever sees synthetic fixtures (the runbook's invented school run and
// invented engineering day), never anything from a real machine: that is what
// makes sending it to a hosted model compatible with Chief's one rule.
//
// Pinned on purpose. The judge model and the prompt are recorded in the report,
// and a change to either is a change of ruler: results judged by different
// rulers are not compared.

import { execFileSync } from 'node:child_process';
import fs from 'node:fs';
import os from 'node:os';
import path from 'node:path';

import { promptText } from './score.mjs';

export const JUDGE_PROMPT_VERSION = 'v1';
export const API_MODEL = 'claude-haiku-5-5';
// Dollars per million tokens, from the pinned price table. Haiku 5.5.
export const PRICE = { input: 0.1, output: 0.5 };
const MAX_MATERIAL = 6000;

const RUBRIC = `You are marking one answer written by a small AI assistant that acts as a user's chief of staff.
You are given the exact material the assistant was shown, and its answer. Mark the answer against the material only.

Score each from 1 (poor) to 5 (excellent):
- grounded: every claim is supported by the material. Invented meetings, people, times or numbers score 1 or 2.
- useful: what a busy person needs first comes first, nothing important is missing, and the answer would let them act.
- concise: no padding, no preamble, no repeating itself, and it follows the format the material asks for.

Reply with a single JSON object and nothing else:
{"grounded": n, "useful": n, "concise": n, "invented": ["anything stated that the material does not say"], "reason": "one sentence"}`;

const PAIR_RULES = `You are comparing two answers (A and B) written by small AI assistants that act as a user's chief of staff.
You are given the exact material they were shown. Prefer the answer that is grounded in the material (no invented meetings, people, times or numbers), then the one that puts what is time-bound first and misses nothing important, then the one that is shorter and follows the requested format. Do not prefer an answer for its position or length.

Reply with a single JSON object and nothing else:
{"winner": "A" | "B" | "tie", "reason": "one sentence"}`;

/** What the model was shown for a run, trimmed from the front so the end (the data) survives. */
export function material(run) {
  const text = promptText(run);
  return text.length > MAX_MATERIAL ? text.slice(-MAX_MATERIAL) : text;
}

export function pointwisePrompt(shown, answer) {
  return `${RUBRIC}\n\n<material>\n${shown}\n</material>\n\n<answer>\n${answer}\n</answer>`;
}

export function pairwisePrompt(shown, a, b) {
  return `${PAIR_RULES}\n\n<material>\n${shown}\n</material>\n\n<answer_A>\n${a}\n</answer_A>\n\n<answer_B>\n${b}\n</answer_B>`;
}

/** The first JSON object in a reply, or null. Judges sometimes wrap it in prose or a fence. */
export function parseJson(text) {
  const start = text.indexOf('{');
  const end = text.lastIndexOf('}');
  if (start < 0 || end < start) return null;
  try {
    return JSON.parse(text.slice(start, end + 1));
  } catch {
    return null;
  }
}

const clamp = (n) => (Number.isFinite(n) ? Math.min(5, Math.max(1, Math.round(n))) : null);

export function readPointwise(text) {
  const j = parseJson(text);
  if (!j) return null;
  const grounded = clamp(Number(j.grounded));
  const useful = clamp(Number(j.useful));
  const concise = clamp(Number(j.concise));
  if (grounded === null || useful === null || concise === null) return null;
  return {
    grounded,
    useful,
    concise,
    invented: Array.isArray(j.invented) ? j.invented.map(String) : [],
    reason: String(j.reason ?? ''),
  };
}

export function readPairwise(text) {
  const j = parseJson(text);
  const winner = j?.winner;
  if (winner !== 'A' && winner !== 'B' && winner !== 'tie') return null;
  return { winner, reason: String(j.reason ?? '') };
}

/** 0 to 100 from the three 1-to-5 marks. */
export const usefulness = (m) => (((m.grounded + m.useful + m.concise) / 3 - 1) / 4) * 100;

// ---------------------------------------------------------------------------
// Ways of asking
// ---------------------------------------------------------------------------

function viaApi() {
  const key = process.env.ANTHROPIC_API_KEY;
  if (!key) throw new Error('--judge api needs ANTHROPIC_API_KEY in the environment');
  const base = (process.env.ANTHROPIC_BASE_URL ?? 'https://api.anthropic.com').replace(/\/$/, '');
  return async (prompt) => {
    const response = await fetch(`${base}/v1/messages`, {
      method: 'POST',
      headers: {
        'content-type': 'application/json',
        'x-api-key': key,
        'anthropic-version': '2023-06-01',
      },
      body: JSON.stringify({
        model: API_MODEL,
        max_tokens: 400,
        temperature: 0,
        messages: [{ role: 'user', content: prompt }],
      }),
    });
    if (!response.ok) throw new Error(`judge API answered ${response.status}`);
    const body = await response.json();
    const usage = body.usage ?? {};
    const cost =
      ((usage.input_tokens ?? 0) * PRICE.input + (usage.output_tokens ?? 0) * PRICE.output) / 1e6;
    return { text: (body.content ?? []).map((part) => part.text ?? '').join(''), cost };
  };
}

/** The user's own `claude -p`, unmodified. Never a token lifted out of its login. */
function viaClaudeCode(model) {
  return async (prompt) => {
    const out = execFileSync(
      'claude',
      ['-p', '--model', model, '--output-format', 'json', '--tools', ''],
      {
        input: prompt,
        cwd: os.tmpdir(),
        encoding: 'utf8',
        maxBuffer: 16 * 1024 * 1024,
        timeout: 180000,
      },
    );
    const body = JSON.parse(out);
    return { text: String(body.result ?? ''), cost: Number(body.total_cost_usd ?? 0) };
  };
}

// ---------------------------------------------------------------------------

/** The runs that are judged: the first repeat of every fixture. Judging all five costs five times as much to learn almost nothing, because the answers barely differ. */
function chosen(scored) {
  const out = [];
  for (const [id, model] of Object.entries(scored)) {
    const seen = new Set();
    for (const run of model.runs) {
      if (run.repeat !== 0 || seen.has(run.fixture) || !run.answer) continue;
      seen.add(run.fixture);
      out.push({ model: id, fixture: run.fixture, group: run.fixtureGroup, answer: run.answer });
    }
  }
  return out;
}

export async function judgeRun(
  dir,
  env,
  scored,
  { mode = 'none', maxSpend = 10, claudeModel = 'haiku' } = {},
) {
  const result = {
    mode,
    judge_model: null,
    prompt_version: JUDGE_PROMPT_VERSION,
    items: [],
    pairs: [],
    spend: 0,
    notes: [],
    skipped: false,
  };
  if (mode === 'none') {
    result.skipped = true;
    result.notes.push('No AI judge was run. Quality scores are the deterministic checks only.');
    return result;
  }

  const rawRuns = readRawRuns(dir, Object.keys(scored));
  const picks = chosen(scored);

  const jobs = [];
  for (const pick of picks) {
    const shown = material(rawRuns.get(`${pick.model}/${pick.fixture}`));
    jobs.push({
      kind: 'point',
      id: `point/${pick.model}/${pick.fixture}`,
      pick,
      prompt: pointwisePrompt(shown, pick.answer),
    });
  }
  const ids = Object.keys(scored);
  for (let i = 0; i < ids.length; i += 1) {
    for (let j = i + 1; j < ids.length; j += 1) {
      for (const fixture of new Set(
        picks.filter((p) => p.model === ids[i]).map((p) => p.fixture),
      )) {
        const a = picks.find((p) => p.model === ids[i] && p.fixture === fixture);
        const b = picks.find((p) => p.model === ids[j] && p.fixture === fixture);
        if (!a || !b) continue;
        const shown = material(rawRuns.get(`${a.model}/${fixture}`));
        // Both orders: a judge that likes the first answer cancels itself out.
        jobs.push({
          kind: 'pair',
          id: `pair/${a.model}/${b.model}/${fixture}/ab`,
          a,
          b,
          swapped: false,
          prompt: pairwisePrompt(shown, a.answer, b.answer),
        });
        jobs.push({
          kind: 'pair',
          id: `pair/${a.model}/${b.model}/${fixture}/ba`,
          a,
          b,
          swapped: true,
          prompt: pairwisePrompt(shown, b.answer, a.answer),
        });
      }
    }
  }

  if (mode === 'file') {
    return judgeFromFile(dir, jobs, result);
  }

  let ask;
  if (mode === 'api') {
    ask = viaApi();
    result.judge_model = API_MODEL;
  } else if (mode === 'claude-code') {
    ask = viaClaudeCode(claudeModel);
    result.judge_model = `claude-code:${claudeModel}`;
    result.notes.push(
      "Judged through the maintainer's own Claude Code subscription. Fine for a dry run; use --judge api for any decision, because subscription limits and routing are not under this tool's control.",
    );
  } else {
    throw new Error(`unknown judge mode ${mode}`);
  }

  for (const job of jobs) {
    if (result.spend >= maxSpend) {
      result.notes.push(
        `Stopped at the $${maxSpend} spend cap with ${jobs.length - result.items.length - result.pairs.length} judgements left.`,
      );
      break;
    }
    let reply;
    try {
      reply = await ask(job.prompt);
    } catch (error) {
      result.notes.push(`${job.id}: ${error.message}`);
      continue;
    }
    result.spend += reply.cost;
    record(job, reply.text, result);
  }
  return result;
}

function record(job, text, result) {
  if (job.kind === 'point') {
    const marks = readPointwise(text);
    if (!marks) return result.notes.push(`${job.id}: the reply could not be read`);
    result.items.push({
      model: job.pick.model,
      fixture: job.pick.fixture,
      group: job.pick.group,
      ...marks,
    });
  } else {
    const verdict = readPairwise(text);
    if (!verdict) return result.notes.push(`${job.id}: the reply could not be read`);
    // Put the winner back in terms of the real models, whatever order they were shown in.
    const shownFirst = job.swapped ? job.b : job.a;
    const shownSecond = job.swapped ? job.a : job.b;
    const winner =
      verdict.winner === 'tie'
        ? 'tie'
        : verdict.winner === 'A'
          ? shownFirst.model
          : shownSecond.model;
    result.pairs.push({
      a: job.a.model,
      b: job.b.model,
      fixture: job.a.fixture,
      swapped: job.swapped,
      winner,
      reason: verdict.reason,
    });
  }
}

/**
 * Judgements made elsewhere (a person, another tool) are read from
 * `judgements.json`: `{ "<job id>": "<the judge's raw reply>" }`. The requests
 * are written to `judge-requests.json` first, so the same jobs can be answered
 * by anything and fed back without the tool caring who answered.
 */
function judgeFromFile(dir, jobs, result) {
  fs.writeFileSync(
    path.join(dir, 'judge-requests.json'),
    `${JSON.stringify(
      jobs.map(({ id, prompt }) => ({ id, prompt })),
      null,
      2,
    )}\n`,
  );
  const file = path.join(dir, 'judgements.json');
  if (!fs.existsSync(file)) {
    result.skipped = true;
    result.notes.push('judge-requests.json was written; add judgements.json to score it.');
    return result;
  }
  const given = JSON.parse(fs.readFileSync(file, 'utf8'));
  result.judge_model = given._judge ?? 'supplied in judgements.json';
  for (const note of given._notes ?? []) result.notes.push(note);
  for (const job of jobs)
    if (given[job.id] !== undefined)
      record(
        job,
        typeof given[job.id] === 'string' ? given[job.id] : JSON.stringify(given[job.id]),
        result,
      );
  return result;
}

function readRawRuns(dir, ids) {
  const map = new Map();
  for (const id of ids) {
    const file = path.join(dir, `${id}.jsonl`);
    for (const line of fs.readFileSync(file, 'utf8').split('\n')) {
      if (!line.trim()) continue;
      const record = JSON.parse(line);
      if (record.type === 'run' && record.repeat === 0 && !map.has(`${id}/${record.fixture}`))
        map.set(`${id}/${record.fixture}`, record);
    }
  }
  return map;
}
