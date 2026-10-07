// Deterministic scoring for the model benchmark.
//
// Everything here is a pure function of what the runner wrote: the answer, the
// answer key that travelled with it, and the exact requests Chief sent. No
// model is involved, so the same file always scores the same.
//
// Two kinds of result come out. **Gates** are pass/fail and decide whether a
// model is ranked at all: a model that invents a meeting is not made eligible
// by being fast. **Scores** run 0 to 100 and decide the order among the models
// that passed.

import fs from 'node:fs';
import path from 'node:path';

// ---------------------------------------------------------------------------
// Reading what the runner wrote
// ---------------------------------------------------------------------------

export function readRunnerFile(file) {
  const lines = fs
    .readFileSync(file, 'utf8')
    .split('\n')
    .filter((line) => line.trim());
  const records = lines.map((line) => JSON.parse(line));
  return {
    env: records.find((record) => record.type === 'env') ?? {},
    runs: records.filter((record) => record.type === 'run'),
  };
}

/** The pieces of a server-sent-event stream: text, and tool calls in order. */
export function parseStream(text) {
  let content = '';
  let finish = null;
  const calls = new Map();

  for (const line of text.split('\n')) {
    if (!line.startsWith('data: ') || line.includes('[DONE]')) continue;
    let event;
    try {
      event = JSON.parse(line.slice(6));
    } catch {
      continue;
    }
    for (const choice of event.choices ?? []) {
      const delta = choice.delta ?? {};
      if (typeof delta.content === 'string') content += delta.content;
      for (const call of delta.tool_calls ?? []) {
        const slot = calls.get(call.index ?? 0) ?? { name: '', arguments: '' };
        if (call.function?.name) slot.name += call.function.name;
        if (call.function?.arguments !== undefined) {
          slot.arguments +=
            typeof call.function.arguments === 'string'
              ? call.function.arguments
              : JSON.stringify(call.function.arguments);
        }
        calls.set(call.index ?? 0, slot);
      }
      if (choice.finish_reason) finish = choice.finish_reason;
    }
  }

  return { content, finish, calls: [...calls.values()] };
}

/** Every word the model was shown in a run, including what the tools returned. */
export function promptText(run) {
  const parts = [];
  for (const exchange of run.exchanges) {
    for (const message of exchange.request?.messages ?? []) {
      if (typeof message.content === 'string') parts.push(message.content);
    }
  }
  return parts.join('\n');
}

// ---------------------------------------------------------------------------
// Text checks
// ---------------------------------------------------------------------------

/** Case, curly quotes and dash variants removed, so "1:1" and "Parents’ evening" match. */
export function normal(text) {
  return text
    .toLowerCase()
    .replace(/[‘’]/g, "'")
    .replace(/[“”]/g, '"')
    .replace(/[–—]/g, '-')
    .replace(/\s+/g, ' ');
}

const TIME = /\b\d{1,2}:\d{2}\b(?:\s?[ap]m\b)?|\b\d{1,2}\s?[ap]m\b/gi;

/** Chief's own note about what it left out. Written by code, not by the model. */
const NOTE = /^_?showing the next .*$/gim;

export function withoutNote(answer) {
  return answer.replace(NOTE, '').trim();
}

const COMMON_CAPITALISED = new Set([
  'i',
  "i'm",
  "i've",
  "i'll",
  'monday',
  'tuesday',
  'wednesday',
  'thursday',
  'friday',
  'saturday',
  'sunday',
  'january',
  'february',
  'march',
  'april',
  'may',
  'june',
  'july',
  'august',
  'september',
  'october',
  'november',
  'december',
  'chief',
  'github',
  'outlook',
  'ok',
  'utc',
]);

/**
 * Things in the answer that are not in what the model was shown: times, numbers
 * and capitalised words in the middle of a sentence (names, mostly).
 *
 * A heuristic, and it says so: a capitalised word in the middle of a sentence
 * is nearly always a proper noun, but a sentence-initial one cannot be told
 * from a common word without a dictionary, so those are not checked. The judge
 * and the human ratings cover what this cannot.
 */
export function inventedThings(answer, shown) {
  const text = withoutNote(answer);
  const seen = normal(shown);
  const known = new Set(seen.match(/[a-z0-9]+(?:['][a-z]+)?/g) ?? []);
  const found = [];

  for (const time of text.match(TIME) ?? []) {
    const bare = normal(time).replace(/\s?[ap]m$/, '');
    if (!seen.includes(bare) && !seen.includes(normal(time))) found.push(`time ${time}`);
  }

  for (const number of text.match(/#\d+|\b\d{2,}\b/g) ?? []) {
    if (!seen.includes(number.replace('#', ''))) found.push(`number ${number}`);
  }

  for (const line of text.split('\n')) {
    // The first word of a line (after any bullet) is skipped, and so is the
    // word after a full stop: a capital there means nothing.
    const words = line
      .replace(/^[\s\-*\u2022\d.)]+/, '')
      .split(/\s+/)
      .slice(1);
    let afterStop = false;
    for (const word of words) {
      const clean = word.replace(/^[^A-Za-z]+|[^A-Za-z'\u2019]+$/g, '');
      if (!afterStop && /^[A-Z][a-z]{2,}/.test(clean)) {
        const key = normal(clean);
        if (!known.has(key) && !COMMON_CAPITALISED.has(key)) found.push(`name ${clean}`);
      }
      afterStop = /[.!?:]$/.test(word);
    }
  }

  return [...new Set(found)];
}

/** Which of `terms` the answer mentions, in the order the answer first does. */
function mentions(answer, terms) {
  const text = normal(answer);
  return terms.map((term) => ({ term, at: text.indexOf(normal(term)) }));
}

export function bulletsOf(answer) {
  return withoutNote(answer)
    .split('\n')
    .filter((line) => /^\s*([-*•]|\d+[.)])\s+/.test(line));
}

const LOOP_MIN_CHARS = 12;
const LOOP_REPEATS = 3;

/** One substantial line written three or more times, as Chief's own guard sees it. */
export function isLooping(text) {
  const counts = new Map();
  for (const line of text.split('\n')) {
    const key = line
      .trim()
      .replace(/^[-*•\s]+/, '')
      .toLowerCase();
    if (key.length < LOOP_MIN_CHARS) continue;
    counts.set(key, (counts.get(key) ?? 0) + 1);
    if (counts.get(key) >= LOOP_REPEATS) return true;
  }
  return false;
}

// ---------------------------------------------------------------------------
// Tool calls
// ---------------------------------------------------------------------------

const LEGAL = {
  fetch_github_prs: { state: ['open', 'closed', 'merged', 'all'], whose: ['mine', 'reviewing'] },
  fetch_calendar: { range: ['today', 'tomorrow', 'week'] },
  fetch_recent_mail: {},
};
const DEFAULTS = { state: 'open', whose: 'mine', range: 'today' };

function toolCallsOf(run) {
  const calls = [];
  for (const exchange of run.exchanges) {
    for (const call of parseStream(exchange.response).calls) {
      let args = null;
      try {
        args = call.arguments.trim() === '' ? {} : JSON.parse(call.arguments);
      } catch {
        args = null;
      }
      const legal = LEGAL[call.name];
      const valid =
        legal !== undefined &&
        args !== null &&
        typeof args === 'object' &&
        !Array.isArray(args) &&
        Object.entries(args).every(([key, value]) =>
          key in legal ? legal[key].includes(value) : true,
        );
      calls.push({ name: call.name, args, valid });
    }
  }
  return calls;
}

// ---------------------------------------------------------------------------
// One run
// ---------------------------------------------------------------------------

/**
 * Score one run. Every metric is `null` when the fixture's key does not ask
 * the question, so averages are over the runs that were asked.
 */
export function scoreRun(run) {
  const key = run.key ?? {};
  const answer = run.answer ?? '';
  const body = withoutNote(answer);
  const shown = promptText(run);
  const exchanges = run.exchanges ?? [];
  const last = exchanges.at(-1);
  const final = last ? parseStream(last.response) : { content: '', finish: null, calls: [] };
  const calls = toolCallsOf(run);

  const httpErrors = exchanges.filter((exchange) => exchange.status !== 200).length;
  const ok = !run.error && body.length > 0 && !run.routed;

  const m = { ok, http_errors: httpErrors, routed: Boolean(run.routed) };

  // What the user is owed.
  if (key.must?.length) {
    const found = mentions(answer, key.must).filter((item) => item.at >= 0).length;
    m.facts = found / key.must.length;
    m.missing = mentions(answer, key.must)
      .filter((item) => item.at < 0)
      .map((item) => item.term);
  } else {
    m.facts = null;
    m.missing = [];
  }

  // What the user must not be told.
  // Free chat has no material to invent against, and its [Placeholders] are not names.
  const invented =
    ok && run.group !== 'chat' ? inventedThings(answer.replace(/\[[^\]]*\]/g, ''), shown) : [];
  const forbidden = (key.forbid ?? []).filter((term) => normal(body).includes(normal(term)));
  const stray = [];
  if (key.forbid_times) stray.push(...(body.match(TIME) ?? []).map((time) => `time ${time}`));
  if (key.forbid_numbers)
    stray.push(
      ...(body.match(/\d+/g) ?? [])
        .filter((n) => !new RegExp(`\\b${n}\\b`).test(shown))
        .map((n) => `number ${n}`),
    );
  m.invented = [...new Set([...invented, ...stray])];
  m.forbidden = forbidden;
  m.clean = ok && m.invented.length === 0 && forbidden.length === 0;

  // Order, and whether the answer leads with the time-bound thing.
  if (key.order?.length > 1) {
    const at = mentions(answer, key.order).filter((item) => item.at >= 0);
    let right = 0;
    for (let i = 1; i < at.length; i += 1) if (at[i].at > at[i - 1].at) right += 1;
    m.order =
      at.length < key.order.length
        ? at.length > 1
          ? right / (key.order.length - 1)
          : 0
        : right / (key.order.length - 1);
  } else {
    m.order = null;
  }
  if (key.lead_time_bound) {
    const first = bulletsOf(answer)[0] ?? body.split('\n')[0] ?? '';
    const lead = key.order?.[0];
    m.leads = lead ? normal(first).includes(normal(lead)) : /\d{1,2}:\d{2}/.test(first);
  } else {
    m.leads = null;
  }

  // The shape of the answer.
  const bullets = bulletsOf(answer).length;
  m.bullets = bullets;
  m.within_cap = key.max_bullets ? bullets <= key.max_bullets : null;
  m.note_present = key.note ? answer.includes(key.note) : null;
  const words = body.split(/\s+/).filter(Boolean).length;
  m.words = words;
  m.within_words = key.max_words ? words <= key.max_words : null;
  if (key.standup) {
    m.first_person = /\b(i|i'm|i've|i'll|my)\b/i.test(body);
    m.format_ok =
      m.first_person === true &&
      (key.max_words ? words <= key.max_words : true) &&
      calls.length === 0;
  } else {
    m.first_person = null;
    m.format_ok = null;
  }

  // Looping, as the model wrote it and as the user sees it.
  const raw = exchanges.map((exchange) => parseStream(exchange.response).content).join('\n');
  m.raw_looped = isLooping(raw);
  m.looped = isLooping(body);
  m.hit_length = final.finish === 'length';

  // Tools.
  m.tool_calls = calls.map((call) => ({ name: call.name, args: call.args, valid: call.valid }));
  m.tool_valid = calls.length ? calls.every((call) => call.valid) : null;
  if (key.tool) {
    const wanted = calls.filter((call) => call.name === key.tool.name);
    m.tool_called = wanted.length > 0;
    m.tool_args_ok =
      wanted.length > 0 &&
      wanted.some((call) =>
        Object.entries(key.tool.args ?? {}).every(([name, allowed]) =>
          allowed.includes(call.args?.[name] ?? DEFAULTS[name]),
        ),
      );
    m.extra_calls = Math.max(0, calls.length - 1);
  } else {
    m.tool_called = null;
    m.tool_args_ok = null;
    m.extra_calls = null;
  }
  m.no_tool_ok = key.no_tool ? calls.length === 0 : null;

  // Speed and weight.
  const timings = exchanges.map((exchange) => exchange.timings).filter(Boolean);
  const promptMs = timings.reduce((sum, t) => sum + (t.prompt_ms ?? 0), 0);
  const promptN = timings.reduce((sum, t) => sum + (t.prompt_n ?? 0), 0);
  const predictedMs = timings.reduce((sum, t) => sum + (t.predicted_ms ?? 0), 0);
  const predictedN = timings.reduce((sum, t) => sum + (t.predicted_n ?? 0), 0);
  m.prompt_tokens = promptN;
  m.cached_tokens = timings.reduce((sum, t) => sum + (t.cache_n ?? 0), 0);
  m.output_tokens = predictedN;
  m.prompt_ms = promptMs;
  m.output_ms = predictedMs;
  m.prefill_tps = promptMs > 0 ? (promptN / promptMs) * 1000 : null;
  m.decode_tps = predictedMs > 0 ? (predictedN / predictedMs) * 1000 : null;
  m.ttfw_ms =
    last?.first_ms == null
      ? null
      : exchanges.slice(0, -1).reduce((sum, e) => sum + e.total_ms, 0) + last.first_ms;
  m.wall_ms = run.wall_ms;
  m.peak_resident_mb = run.peak_resident_mb;
  m.swap_in_mb =
    run.swap_used_mb_end != null && run.swap_used_mb_start != null
      ? Math.max(0, run.swap_used_mb_end - run.swap_used_mb_start)
      : null;
  m.phase = run.phase;

  // Did this repeat get everything right?
  const checks = [
    ok,
    httpErrors === 0,
    m.facts === null || m.facts === 1,
    m.clean,
    m.order === null || m.order === 1,
    m.leads === null || m.leads,
    m.within_cap === null || m.within_cap,
    m.within_words === null || m.within_words,
    m.format_ok === null || m.format_ok,
    !m.looped,
    !m.hit_length,
    m.tool_called === null || m.tool_called,
    m.tool_args_ok === null || m.tool_args_ok,
    m.no_tool_ok === null || m.no_tool_ok,
    m.tool_valid === null || m.tool_valid,
  ];
  m.pass = checks.every(Boolean);

  return m;
}

// ---------------------------------------------------------------------------
// One model
// ---------------------------------------------------------------------------

const mean = (values) => {
  const kept = values.filter(
    (value) => value !== null && value !== undefined && !Number.isNaN(value),
  );
  return kept.length ? kept.reduce((sum, value) => sum + value, 0) / kept.length : null;
};

export const median = (values) => {
  const kept = values.filter((value) => value != null).sort((a, b) => a - b);
  if (!kept.length) return null;
  const mid = Math.floor(kept.length / 2);
  return kept.length % 2 ? kept[mid] : (kept[mid - 1] + kept[mid]) / 2;
};

const share = (values) => {
  const kept = values.filter((value) => value !== null && value !== undefined);
  return kept.length ? kept.filter(Boolean).length / kept.length : null;
};

/** What Chief can spend: 8 GB, less 3 GB the user's own work needs, less Chief itself. */
export const MEMORY_BUDGET_MB = 8192 - 3072 - 40;

export function gatesFor(runs) {
  const quiet = runs.filter(
    (run) => run.fixtureGroup === 'brief-quiet' || run.fixture === 'eng-standup-empty',
  );
  const tools = runs.filter((run) => run.fixtureGroup === 'tools');
  const peak = Math.max(0, ...runs.map((run) => run.m.peak_resident_mb ?? 0));
  const swapped = runs.filter((run) => (run.m.swap_in_mb ?? 0) > 0);
  const validCalls = tools.flatMap((run) => run.m.tool_calls);

  const g = {};
  const failedStart = runs.filter((run) => !run.m.ok || run.m.http_errors > 0);
  g.G1 = {
    name: 'Starts and answers',
    pass: failedStart.length === 0,
    detail: `${runs.length - failedStart.length} of ${runs.length} runs answered without an error`,
    evidence: failedStart.slice(0, 5).map((run) => `${run.fixture} #${run.repeat}`),
  };
  g.G2 = {
    name: 'Fits in 8 GB with room left',
    pass: peak <= MEMORY_BUDGET_MB && swapped.length === 0,
    detail: `peak ${peak} MB against ${MEMORY_BUDGET_MB} MB; ${swapped.length} runs swapped`,
    evidence: [],
  };
  const invented = quiet.filter((run) => !run.m.clean);
  g.G3 = {
    name: 'Invents nothing on the quiet days',
    pass: quiet.length > 0 && invented.length === 0,
    detail: `${quiet.length - invented.length} of ${quiet.length} quiet-day runs were clean`,
    evidence: invented
      .slice(0, 5)
      .map(
        (run) =>
          `${run.fixture} #${run.repeat}: ${run.m.invented.concat(run.m.forbidden).slice(0, 4).join(', ')}`,
      ),
  };
  const circled = runs.filter((run) => run.m.looped || run.m.hit_length);
  g.G4 = {
    name: 'Finishes without circling',
    pass: circled.length === 0,
    detail: `${circled.length} runs looped or ran to the length limit (${runs.filter((run) => run.m.raw_looped).length} looped before Chief's own guard)`,
    evidence: circled.slice(0, 5).map((run) => `${run.fixture} #${run.repeat}`),
  };
  const validShare = validCalls.length
    ? validCalls.filter((call) => call.valid).length / validCalls.length
    : null;
  g.G5 = {
    name: 'Tool calls are valid',
    pass: validShare === null ? false : validShare >= 0.9,
    detail:
      validShare === null
        ? 'the model made no tool calls at all'
        : `${Math.round(validShare * 100)}% of ${validCalls.length} tool calls were valid`,
    evidence: tools
      .filter((run) => run.m.tool_valid === false)
      .slice(0, 5)
      .map((run) => `${run.fixture} #${run.repeat}`),
  };
  return g;
}

export function aggregate(runs) {
  const byFixture = {};
  for (const run of runs) (byFixture[run.fixture] ??= []).push(run);

  const fixtures = Object.entries(byFixture).map(([id, list]) => {
    const passRate = share(list.map((run) => run.m.pass));
    return {
      id,
      group: list[0].fixtureGroup,
      source: list[0].source,
      runs: list.length,
      pass_rate: passRate,
      facts: mean(list.map((run) => run.m.facts)),
      clean_rate: share(list.map((run) => run.m.clean)),
      order: mean(list.map((run) => run.m.order)),
      leads: share(list.map((run) => run.m.leads)),
      cap: share(list.map((run) => run.m.within_cap)),
      tool: share(
        list.map((run) =>
          run.m.tool_called === null ? null : run.m.tool_called && run.m.tool_args_ok,
        ),
      ),
      distinct_answers: new Set(list.map((run) => run.answer)).size,
      // 1 when every repeat agrees on pass or fail, 0 when it is a coin toss.
      agreement: 1 - 2 * Math.min(passRate ?? 0, 1 - (passRate ?? 0)),
      ttfw_ms: median(list.map((run) => run.m.ttfw_ms)),
      wall_ms: median(list.map((run) => run.m.wall_ms)),
      decode_tps: median(list.map((run) => run.m.decode_tps)),
      prefill_tps: median(list.map((run) => run.m.prefill_tps)),
    };
  });

  return fixtures;
}

/**
 * Component scores, 0 to 100, for a set of runs. `null` where the runs hold no
 * evidence, so a missing component is left out of the composite rather than
 * counted as zero.
 */
export function components(runs) {
  const pct = (value) => (value === null ? null : value * 100);
  const asked = (field) => runs.map((run) => run.m[field]);

  const quality = mean([
    pct(mean(asked('facts'))),
    pct(share(runs.map((run) => run.m.clean))),
    pct(mean([mean(asked('order')), share(asked('leads'))].filter((value) => value !== null))),
    pct(
      mean([
        share(asked('within_cap')),
        share(asked('within_words')),
        share(asked('format_ok')),
        share(asked('no_tool_ok')),
        share(runs.map((run) => !run.m.looped && !run.m.hit_length)),
      ]),
    ),
  ]);

  const toolRuns = runs.filter((run) => run.m.tool_called !== null);
  const tools = toolRuns.length
    ? pct(
        mean(
          toolRuns.map(
            (run) =>
              (run.m.tool_called ? 0.5 : 0) +
              (run.m.tool_args_ok ? 0.4 : 0) +
              ((run.m.extra_calls ?? 0) === 0 ? 0.1 : 0),
          ),
        ),
      )
    : null;

  // "Fast enough" curves: past the good end more speed buys nothing, past the
  // bad end it is already unusable.
  const curve = (value, good, bad) =>
    value === null ? null : 100 * Math.min(1, Math.max(0, (bad - value) / (bad - good)));
  const ttfw = curve(median(runs.map((run) => run.m.ttfw_ms)), 5000, 60000);
  const finish = curve(median(runs.map((run) => run.m.wall_ms)), 10000, 120000);
  const speed = mean([ttfw, finish]);

  const peak = Math.max(0, ...runs.map((run) => run.m.peak_resident_mb ?? 0));
  const headroom = peak
    ? 100 * Math.min(1, Math.max(0, (MEMORY_BUDGET_MB - peak) / (MEMORY_BUDGET_MB - 1500)))
    : null;
  const byFixture = aggregate(runs);
  const stability = pct(mean(byFixture.map((fixture) => fixture.agreement)));

  return {
    quality,
    tools,
    speed,
    memory: mean([headroom, stability]),
    headroom,
    stability,
    ttfw,
    finish,
  };
}

/** The tier profiles from the plan (section 10). Weights live here so they can change without a re-run. */
export const PROFILES = {
  light: { quality: 45, tools: 10, usefulness: 15, speed: 20, memory: 10 },
  standard: { quality: 55, tools: 15, usefulness: 15, speed: 10, memory: 5 },
};

/**
 * Weighted composite over the components that have evidence. Missing ones are
 * dropped and the rest rescaled, and the caller is told which were dropped.
 */
export function composite(parts, tier, { speedRankable = true, usefulness = null } = {}) {
  const weights = PROFILES[tier] ?? PROFILES.light;
  const values = { ...parts, usefulness };
  if (!speedRankable) values.speed = null;

  let total = 0;
  let used = 0;
  const left = [];
  for (const [name, weight] of Object.entries(weights)) {
    const value = values[name];
    if (value === null || value === undefined) {
      left.push(name);
      continue;
    }
    total += weight * value;
    used += weight;
  }
  return { score: used ? total / used : null, left_out: left, weight_used: used };
}

export function scoreModel(file, model) {
  const { env, runs } = readRunnerFile(file);
  const scored = runs.map((run) => ({
    fixture: run.fixture,
    fixtureGroup: run.group,
    source: run.source,
    repeat: run.repeat,
    answer: run.answer,
    error: run.error,
    m: scoreRun(run),
  }));
  return {
    model,
    runner: env,
    runs: scored,
    fixtures: aggregate(scored),
    gates: gatesFor(scored),
    parts: components(scored),
  };
}

export function scoreRunDir(dir, env) {
  const models = {};
  for (const model of env.models) {
    const file = path.join(dir, `${model.id}.jsonl`);
    if (fs.existsSync(file)) models[model.id] = scoreModel(file, model);
  }
  return models;
}

export { scoreRunDir as scoreRun_ };
