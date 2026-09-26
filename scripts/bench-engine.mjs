// Measure what the GPU buys on this machine.
//
// Starts the `llama-server` that `pnpm engine:fetch` put in `src-tauri/binaries/`
// twice — once as Chief starts it, which offloads to a GPU if there is one, and
// once with `--device none`, which keeps everything on the CPU — asks each the
// same question a few times, and prints the server's own timings side by side.
// The engine's numbers are the measurement, not a stopwatch around the request:
// they exclude the HTTP round trip and they split reading the prompt from
// writing the answer, which fail in different ways.
//
//   pnpm engine:bench [--model <gguf>] [--runs 3] [--predict 128]
//
// The model defaults to whichever of Chief's own is already downloaded. The
// prompt is built fresh for every run with `cache_prompt: false`, so no run is
// flattered by the one before it. Everything happens on loopback.

import { execFileSync, spawn } from 'node:child_process';
import fs from 'node:fs';
import http from 'node:http';
import net from 'node:net';
import os from 'node:os';
import path from 'node:path';
import process from 'node:process';
import { fileURLToPath } from 'node:url';

const ROOT = path.resolve(path.dirname(fileURLToPath(import.meta.url)), '..');
const BINARIES = path.join(ROOT, 'src-tauri', 'binaries');
const LIBRARIES = path.join(BINARIES, 'lib');

/** Chief's models, largest first, as `weights.rs` names them. */
const MODELS = ['Llama-3.2-3B-Instruct-Q4_K_M.gguf', 'Llama-3.2-1B-Instruct-Q4_K_M.gguf'];

/** The configurations compared. `args` are added to the flags Chief uses. */
export const MODES = [
  { name: 'default', args: [] },
  { name: 'cpu', args: ['--device', 'none'] },
];

/**
 * A prompt shaped like Chief's: a list of work items for the model to read, and
 * a question after them. About a thousand tokens, which is a typical brief —
 * and the count the engine reports is printed, because a throughput without the
 * context length it was measured at is not comparable to anything.
 */
export function prompt(items = 48) {
  const lines = [];

  for (let index = 0; index < items; index += 1) {
    lines.push(
      `- #${1200 + index} ${['Fix', 'Add', 'Refactor', 'Document'][index % 4]} the ` +
        `${['sync worker', 'calendar parser', 'brief renderer', 'token refresh'][index % 4]} ` +
        `retry path (acme/${['api', 'web', 'desktop'][index % 3]}), opened ${index % 9} days ago, ` +
        `waiting on review from @reviewer${index % 5}.`,
    );
  }

  return `You are a chief of staff. Here is the user's open work:\n${lines.join('\n')}\n\nWhat should they look at first, and why?`;
}

/** The median of some numbers, which a single slow run cannot drag around. */
export function median(values) {
  const sorted = [...values].sort((a, b) => a - b);
  const middle = Math.floor(sorted.length / 2);

  return sorted.length % 2 === 0 ? (sorted[middle - 1] + sorted[middle]) / 2 : sorted[middle];
}

function options(argv) {
  const read = (flag, fallback) => {
    const at = argv.indexOf(flag);
    return at === -1 ? fallback : argv[at + 1];
  };

  return {
    model: read('--model', process.env.CHIEF_BENCH_MODEL),
    runs: Number.parseInt(read('--runs', '3'), 10),
    predict: Number.parseInt(read('--predict', '128'), 10),
  };
}

/** Where the app keeps its data, which is where its models are. */
function appDataDir() {
  const id = 'com.scottmallinson.chief';

  if (process.platform === 'darwin')
    return path.join(os.homedir(), 'Library', 'Application Support', id);
  if (process.platform === 'win32') return path.join(process.env.APPDATA ?? '', id);
  return path.join(process.env.XDG_DATA_HOME || path.join(os.homedir(), '.local', 'share'), id);
}

function findModel(configured) {
  if (configured) return configured;

  const found = MODELS.map((name) => path.join(appDataDir(), 'models', name)).find((candidate) =>
    fs.existsSync(candidate),
  );

  if (found === undefined) {
    throw new Error('no model found. Download one in Chief, or pass --model <path to a .gguf>.');
  }

  return found;
}

function findServer() {
  const host = /^host:\s*(\S+)$/m.exec(execFileSync('rustc', ['-vV'], { encoding: 'utf8' }))[1];
  const server = path.join(
    BINARIES,
    `llama-server-${host}${process.platform === 'win32' ? '.exe' : ''}`,
  );

  if (!fs.existsSync(server)) throw new Error(`${server} is missing. Run \`pnpm engine:fetch\`.`);

  return server;
}

/** A port nothing is listening on, so this never lands on Chief's own engine. */
function freePort() {
  return new Promise((resolve, reject) => {
    const server = net.createServer();
    server.once('error', reject);
    server.listen(0, '127.0.0.1', () => {
      const { port } = server.address();
      server.close(() => resolve(port));
    });
  });
}

/**
 * Start a server the way `engine.rs` does: the same flags, the libraries on the
 * loader's path, and the library directory as the working directory, which is
 * where ggml looks for its backends.
 */
function loaderPath() {
  const variable =
    process.platform === 'darwin'
      ? 'DYLD_LIBRARY_PATH'
      : process.platform === 'win32'
        ? 'PATH'
        : 'LD_LIBRARY_PATH';
  const inherited = process.env[variable];

  return {
    variable,
    value: inherited ? `${LIBRARIES}${path.delimiter}${inherited}` : LIBRARIES,
  };
}

function start(server, model, port, extra) {
  const { variable, value } = loaderPath();

  const child = spawn(
    server,
    [
      '--model',
      model,
      '--host',
      '127.0.0.1',
      '--port',
      String(port),
      '--ctx-size',
      '8192',
      '--parallel',
      '1',
      '--jinja',
      ...extra,
    ],
    {
      cwd: LIBRARIES,
      env: { ...process.env, [variable]: value },
      stdio: ['ignore', 'ignore', 'pipe'],
    },
  );

  // Kept for the one case it is wanted: a server that dies on the way up.
  const said = [];
  child.stderr.setEncoding('utf8');
  child.stderr.on('data', (chunk) => said.push(chunk));

  return { child, said };
}

async function waitUntilReady(port, child, said) {
  const deadline = Date.now() + 180_000;

  while (Date.now() < deadline) {
    if (child.exitCode !== null) {
      throw new Error(`the server exited with ${child.exitCode}:\n${said.join('').slice(-2000)}`);
    }

    try {
      const response = await fetch(`http://127.0.0.1:${port}/health`);
      if (response.ok) return;
    } catch {
      // Not listening yet.
    }

    await new Promise((resolve) => setTimeout(resolve, 250));
  }

  throw new Error('the server never became ready');
}

/**
 * One completion, through `node:http` rather than `fetch`. `fetch` gives up on
 * a response whose headers take more than five minutes, and the server sends
 * none until the answer is written — which a slow CPU reading a long prompt
 * with the larger model can take. The measurement should not decide how slow
 * a machine is allowed to be.
 */
function complete(port, predict, run) {
  const body = JSON.stringify({
    // A different first line per run, so nothing is served from a cache.
    prompt: `Run ${run}.\n${prompt()}`,
    n_predict: predict,
    temperature: 0,
    cache_prompt: false,
    // Always write the full count, so every run measures the same amount of
    // decoding rather than however much the model felt like saying.
    ignore_eos: true,
  });

  return new Promise((resolve, reject) => {
    const request = http.request(
      {
        host: '127.0.0.1',
        port,
        path: '/completion',
        method: 'POST',
        headers: { 'content-type': 'application/json', 'content-length': Buffer.byteLength(body) },
      },
      (response) => {
        let text = '';
        response.setEncoding('utf8');
        response.on('data', (chunk) => (text += chunk));
        response.on('end', () => {
          if (response.statusCode !== 200) {
            reject(new Error(`the server answered ${response.statusCode}: ${text}`));
            return;
          }

          try {
            resolve(JSON.parse(text).timings);
          } catch (cause) {
            reject(cause);
          }
        });
      },
    );

    request.on('error', reject);
    request.end(body);
  });
}

/**
 * The devices the engine can offload to, in its own words. An empty list means
 * every mode below ran on the CPU, whatever it is called.
 */
function listDevices(server) {
  const { variable, value } = loaderPath();
  const listed = execFileSync(server, ['--list-devices'], {
    cwd: LIBRARIES,
    env: { ...process.env, [variable]: value },
    encoding: 'utf8',
    stdio: ['ignore', 'pipe', 'pipe'],
  });

  return listed
    .split('\n')
    .map((line) => line.trim())
    .filter((line) => line !== '' && line !== 'Available devices:' && line !== '(none)');
}

async function measure(server, model, mode, { runs, predict }) {
  const port = await freePort();
  const { child, said } = start(server, model, port, mode.args);

  try {
    await waitUntilReady(port, child, said);

    // One discarded run: the first request after a start pays for things no
    // later one does, and it is the later ones a person lives with.
    await complete(port, predict, 0);

    const timings = [];
    for (let run = 1; run <= runs; run += 1) timings.push(await complete(port, predict, run));

    return {
      mode: mode.name,
      promptTokens: timings[0].prompt_n,
      prefill: median(timings.map((t) => t.prompt_per_second)),
      decode: median(timings.map((t) => t.predicted_per_second)),
      firstToken: median(timings.map((t) => t.prompt_ms)),
    };
  } finally {
    child.kill();
  }
}

async function main() {
  const settings = options(process.argv.slice(2));
  const server = findServer();
  const model = findModel(settings.model);
  const stamp = fs.readFileSync(path.join(BINARIES, '.build'), 'utf8').trim();

  console.log(`engine  ${stamp}`);
  console.log(`model   ${path.basename(model)}`);
  console.log(`runs    ${settings.runs} (median), ${settings.predict} tokens each`);

  const found = listDevices(server);
  console.log(`devices ${found.length === 0 ? 'none — the CPU only' : found.join('; ')}\n`);

  const results = [];
  for (const mode of MODES) {
    process.stdout.write(`measuring ${mode.name}…\n`);
    results.push(await measure(server, model, mode, settings));
  }

  const [gpu, cpu] = results;
  const row = (r) =>
    `| ${r.mode.padEnd(7)} | ${String(r.promptTokens).padStart(6)} | ${r.prefill.toFixed(1).padStart(8)} | ${r.decode.toFixed(1).padStart(7)} | ${Math.round(r.firstToken).toString().padStart(8)} |`;

  console.log('\n| mode    | prompt | prefill  | decode  | prefill  |');
  console.log('|         | tokens | tok/s    | tok/s   | ms       |');
  console.log('| ------- | ------ | -------- | ------- | -------- |');
  for (const result of results) console.log(row(result));
  console.log(
    `\ndefault vs cpu: prefill ×${(gpu.prefill / cpu.prefill).toFixed(2)}, decode ×${(gpu.decode / cpu.decode).toFixed(2)}`,
  );
}

if (process.argv[1] === fileURLToPath(import.meta.url)) {
  main().catch((cause) => {
    console.error(`\nCould not measure the engine: ${cause.message}`);
    process.exit(1);
  });
}
