// Run the model benchmark: `pnpm bench`.
//
// For each enabled model in bench/models.json this runs the Rust runner
// (src-tauri/src/scenarios/bench.rs) once, against the same checkout, then
// scores what came back and writes the report. See docs/benchmark.md.
//
//   pnpm bench                       every enabled model
//   pnpm bench --only qwen3-1.7b     one model
//   pnpm bench --repeats 3           fewer repeats (the plan's default is 5)
//   pnpm bench --fixture em-01       fixtures whose id contains this
//   pnpm bench --judge none|api|claude-code|file   (default none)
//   pnpm bench --max-spend 10        hard cap on judge spend, in dollars
//   pnpm bench --score-only <dir>    re-score and re-report an existing run
//
// A run is one commit, one fixture set, one machine and every model. Results
// from different commits are never ranked against each other.

import { execFileSync, spawn } from 'node:child_process';
import crypto from 'node:crypto';
import fs from 'node:fs';
import os from 'node:os';
import path from 'node:path';
import process from 'node:process';
import { fileURLToPath } from 'node:url';

import { judgeRun } from './judge.mjs';
import { writeReport } from './report.mjs';
import { scoreRunDir } from './score.mjs';

const ROOT = path.resolve(path.dirname(fileURLToPath(import.meta.url)), '..');
const MODELS_DIR = path.join(ROOT, 'bench', '.models');
const RESULTS = path.join(ROOT, 'bench-results');
const FIXTURE_VERSION = 'v1';

function options(argv) {
  const out = { judge: 'none', repeats: 5, maxSpend: 10 };
  for (let i = 0; i < argv.length; i += 1) {
    const flag = argv[i];
    const value = () => argv[++i];
    if (flag === '--only') out.only = value();
    else if (flag === '--repeats') out.repeats = Number(value());
    else if (flag === '--fixture') out.fixture = value();
    else if (flag === '--judge') out.judge = value();
    else if (flag === '--max-spend') out.maxSpend = Number(value());
    else if (flag === '--score-only') out.scoreOnly = path.resolve(value());
    else if (flag === '--out') out.out = path.resolve(value());
    else if (flag === '--machine') out.machine = value();
    else throw new Error(`unknown option ${flag}`);
  }
  return out;
}

function git(...args) {
  return execFileSync('git', args, { cwd: ROOT, encoding: 'utf8' }).trim();
}

/** Which code is under test: the commit, and whether anything is uncommitted. */
function codebase() {
  const sha = git('rev-parse', 'HEAD');
  const diff = git('diff', 'HEAD', '--', 'src-tauri/src', 'bench').concat(
    git('ls-files', '--others', '--exclude-standard', 'src-tauri/src', 'bench'),
  );
  return {
    sha,
    short: sha.slice(0, 7),
    branch: git('rev-parse', '--abbrev-ref', 'HEAD'),
    dirty: diff.length > 0,
    // Two dirty trees with different edits must not look alike.
    dirty_hash: diff ? crypto.createHash('sha256').update(diff).digest('hex').slice(0, 12) : null,
  };
}

function machine(label) {
  let cpu = os.cpus()[0]?.model ?? 'unknown';
  if (process.platform === 'darwin') {
    try {
      cpu = execFileSync('sysctl', ['-n', 'machdep.cpu.brand_string'], { encoding: 'utf8' }).trim();
    } catch {
      /* keep the generic name */
    }
  }
  return {
    // The target is the 2014 Mac mini; anything else is a stand-in, and the
    // report says so beside every speed number.
    label: label ?? process.env.CHIEF_BENCH_MACHINE ?? 'other',
    target: (label ?? process.env.CHIEF_BENCH_MACHINE) === 'mac-mini-2014',
    cpu,
    logical_cores: os.cpus().length,
    memory_gb: Math.round((os.totalmem() / 1024 ** 3) * 10) / 10,
    os: `${os.type()} ${os.release()} ${os.arch()}`,
    node: process.version,
  };
}

function sha256(file) {
  const hash = crypto.createHash('sha256');
  hash.update(fs.readFileSync(file));
  return hash.digest('hex');
}

/** Fetch a model once. curl, because it resumes and honours the proxy settings. */
function fetchModel(model) {
  fs.mkdirSync(MODELS_DIR, { recursive: true });
  const file = path.join(MODELS_DIR, model.file);
  const shared =
    process.env.CHIEF_BENCH_MODELS_DIR && path.join(process.env.CHIEF_BENCH_MODELS_DIR, model.file);
  if (shared && fs.existsSync(shared)) return shared;
  if (!fs.existsSync(file)) {
    console.log(`fetching ${model.name}…`);
    execFileSync('curl', ['-fsSL', '-C', '-', '-o', file, model.url], { stdio: 'inherit' });
  }
  return file;
}

function engineBinary() {
  const dir = path.join(ROOT, 'src-tauri', 'binaries');
  const found = fs.readdirSync(dir).find((name) => name.startsWith('llama-server-'));
  if (!found) throw new Error('no llama-server on disk; run `pnpm engine:fetch` first');
  const stamp = path.join(dir, '.build');
  return {
    path: path.join(dir, found),
    build: fs.existsSync(stamp) ? fs.readFileSync(stamp, 'utf8').trim() : 'unknown',
  };
}

function runRunner(model, weights, server, jsonl, opts) {
  const env = {
    ...process.env,
    CHIEF_BENCH_WEIGHTS: weights,
    CHIEF_BENCH_SERVER: server,
    CHIEF_BENCH_OUT: jsonl,
    CHIEF_BENCH_REPEATS: String(opts.repeats),
    CHIEF_BENCH_TIER: model.tier,
    ...(opts.fixture ? { CHIEF_BENCH_ONLY: opts.fixture } : {}),
  };
  return new Promise((resolve, reject) => {
    const child = spawn(
      'cargo',
      [
        'test',
        '--manifest-path',
        'src-tauri/Cargo.toml',
        '--lib',
        'run_the_benchmark',
        '--',
        '--ignored',
        '--nocapture',
      ],
      { cwd: ROOT, env, stdio: ['ignore', 'inherit', 'inherit'] },
    );
    child.on('exit', (code) =>
      code === 0
        ? resolve()
        : reject(new Error(`the runner failed for ${model.id} (exit ${code})`)),
    );
  });
}

async function main() {
  const opts = options(process.argv.slice(2));

  if (opts.scoreOnly) {
    await finish(opts.scoreOnly, opts);
    return;
  }

  const manifest = JSON.parse(fs.readFileSync(path.join(ROOT, 'bench', 'models.json'), 'utf8'));
  const models = manifest.models.filter((m) => m.enabled && (!opts.only || m.id === opts.only));
  if (models.length === 0) throw new Error('no enabled model matches');

  const code = codebase();
  const where = machine(opts.machine);
  const engine = engineBinary();
  const stamp = new Date().toISOString().slice(0, 10);
  const dir =
    opts.out ??
    path.join(
      RESULTS,
      `${stamp}-${code.short}${code.dirty ? `-dirty-${code.dirty_hash}` : ''}-${where.label}`,
    );
  fs.mkdirSync(dir, { recursive: true });

  const env = {
    fixture_version: FIXTURE_VERSION,
    started_at: new Date().toISOString(),
    code,
    machine: where,
    engine: { build: engine.build },
    repeats: opts.repeats,
    judge: opts.judge,
    models: [],
  };

  for (const model of models) {
    const weights = fetchModel(model);
    env.models.push({
      id: model.id,
      name: model.name,
      tier: model.tier,
      file: model.file,
      bytes: fs.statSync(weights).size,
      sha256: sha256(weights),
    });
    console.log(`\n=== ${model.name} (${model.tier}) ===`);
    await runRunner(model, weights, engine.path, path.join(dir, `${model.id}.jsonl`), opts);
  }

  env.finished_at = new Date().toISOString();
  fs.writeFileSync(path.join(dir, 'env.json'), `${JSON.stringify(env, null, 2)}\n`);
  await finish(dir, opts);
}

async function finish(dir, opts) {
  const env = JSON.parse(fs.readFileSync(path.join(dir, 'env.json'), 'utf8'));
  const scored = scoreRunDir(dir, env);
  const judged = await judgeRun(dir, env, scored, { mode: opts.judge, maxSpend: opts.maxSpend });
  writeReport(dir, env, scored, judged);
  console.log(`\nReport: ${path.join(dir, 'report.md')}`);
}

main().catch((error) => {
  console.error(error.message);
  process.exit(1);
});
