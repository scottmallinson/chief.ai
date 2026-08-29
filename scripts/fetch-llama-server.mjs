// Fetch the llama.cpp server Chief bundles.
//
// The engine is part of the installation rather than something the user has to
// install, so `llama-server` and the shared libraries it loads have to be on
// disk before Tauri builds a bundle. This script puts them in
// `src-tauri/binaries/`, where `externalBin` and `resources` pick them up.
//
// It is idempotent: a second run with the same pinned build does nothing.
//
// Only CPU builds are fetched. That is the point of the move to llama.cpp — one
// binary that runs on a machine with no GPU and no AVX-512, picking the best
// instruction set it finds at run time.

import { createWriteStream } from 'node:fs';
import { execFileSync } from 'node:child_process';
import { Readable } from 'node:stream';
import { pipeline } from 'node:stream/promises';
import fs from 'node:fs/promises';
import os from 'node:os';
import path from 'node:path';
import process from 'node:process';
import { fileURLToPath } from 'node:url';

/** The llama.cpp build Chief ships. Bump deliberately, not automatically. */
const BUILD = 'b10545';

const RELEASE = `https://github.com/ggml-org/llama.cpp/releases/download/${BUILD}`;

const ROOT = path.resolve(path.dirname(fileURLToPath(import.meta.url)), '..');
const BINARIES = path.join(ROOT, 'src-tauri', 'binaries');
const LIBRARIES = path.join(BINARIES, 'lib');
/** Records which build is already unpacked, so a rerun can do nothing. */
const STAMP = path.join(BINARIES, '.build');

/** The CPU-only asset for each platform we build for. */
const ASSETS = {
  'linux-x64': `llama-${BUILD}-bin-ubuntu-x64.tar.gz`,
  'linux-arm64': `llama-${BUILD}-bin-ubuntu-arm64.tar.gz`,
  'darwin-x64': `llama-${BUILD}-bin-macos-x64.tar.gz`,
  'darwin-arm64': `llama-${BUILD}-bin-macos-arm64.tar.gz`,
  'win32-x64': `llama-${BUILD}-bin-win-cpu-x64.zip`,
  'win32-arm64': `llama-${BUILD}-bin-win-cpu-arm64.zip`,
};

/**
 * The Rust triples we can fetch an engine for, and the asset key each one
 * wants.
 *
 * A build that cross-compiles asks for a triple that is not this machine's:
 * macOS builds both Apple architectures on one arm64 runner, and the Intel
 * half needs the Intel server beside it. Without this the sidecar is named
 * after the host and Tauri stops with `resource path
 * binaries/llama-server-x86_64-apple-darwin doesn't exist` — which is exactly
 * how the first release of this app failed.
 */
const TRIPLES = {
  'x86_64-apple-darwin': 'darwin-x64',
  'aarch64-apple-darwin': 'darwin-arm64',
  'x86_64-unknown-linux-gnu': 'linux-x64',
  'aarch64-unknown-linux-gnu': 'linux-arm64',
  'x86_64-pc-windows-msvc': 'win32-x64',
  'aarch64-pc-windows-msvc': 'win32-arm64',
};

/** What counts as a shared library the server needs beside it. */
const LIBRARY_PATTERN = /(\.so(\.\d+)*|\.dylib|\.dll|\.metal)$/i;

/**
 * macOS 12's Accelerate framework lacks the ILP64 symbol in the prebuilt.
 *
 * Only ever true for the machine doing the building: the fallback compiles
 * llama.cpp here, so it cannot produce a server for a platform this is not.
 */
function needsMacos12Fallback(target, host) {
  if (target !== host) return false;
  if (process.platform !== 'darwin' || process.arch !== 'x64') return false;

  const version = execFileSync('sw_vers', ['-productVersion'], { encoding: 'utf8' });
  return Number.parseInt(version.trim(), 10) < 13;
}

/**
 * The Rust host triple, which is the suffix Tauri expects on a sidecar. Asking
 * rustc is the only way to be right about it on every machine.
 */
function hostTriple() {
  const report = execFileSync('rustc', ['-vV'], { encoding: 'utf8' });
  const host = /^host:\s*(\S+)$/m.exec(report);

  if (host === null) throw new Error('could not read the host triple from `rustc -vV`');

  return host[1];
}

async function exists(target) {
  try {
    await fs.access(target);
    return true;
  } catch {
    return false;
  }
}

/** Has this exact build already been unpacked? */
async function alreadyHere(sidecar, stamp) {
  if (!(await exists(sidecar))) return false;

  const stamped = await fs.readFile(STAMP, 'utf8').catch(() => '');

  return stamped.trim() === stamp;
}

async function download(url, destination) {
  const response = await fetch(url, { redirect: 'follow' });

  if (!response.ok || response.body === null) {
    throw new Error(`could not download ${url}: HTTP ${response.status}`);
  }

  await pipeline(Readable.fromWeb(response.body), createWriteStream(destination));
}

/** Build a macOS 12-compatible server without the unavailable Accelerate BLAS backend. */
async function buildMacos12Server(scratch) {
  const sourceArchive = path.join(scratch, `${BUILD}.tar.gz`);
  await download(
    `https://github.com/ggml-org/llama.cpp/archive/refs/tags/${BUILD}.tar.gz`,
    sourceArchive,
  );

  const source = path.join(scratch, 'source');
  await fs.mkdir(source, { recursive: true });
  unpack(sourceArchive, source);

  const sourceRoot = (await fs.readdir(source, { withFileTypes: true })).find((entry) =>
    entry.isDirectory(),
  );
  if (sourceRoot === undefined) throw new Error(`${BUILD} source archive is empty`);

  const build = path.join(scratch, 'build');
  execFileSync(
    'cmake',
    [
      '-S',
      path.join(source, sourceRoot.name),
      '-B',
      build,
      '-DCMAKE_BUILD_TYPE=Release',
      '-DGGML_BLAS=OFF',
      '-DGGML_METAL=OFF',
      '-DLLAMA_BUILD_UI=OFF',
      '-DLLAMA_BUILD_SERVER=ON',
      '-DLLAMA_BUILD_TESTS=OFF',
      '-DLLAMA_BUILD_EXAMPLES=OFF',
    ],
    { stdio: 'inherit' },
  );
  execFileSync('cmake', ['--build', build, '--target', 'llama-server', '--parallel', '1'], {
    stdio: 'inherit',
  });

  return path.join(build, 'bin');
}

/**
 * Unpack an archive. `tar` reads both gzipped tarballs and zips: the zip case
 * is Windows only, where `tar.exe` is bsdtar and handles them.
 */
function unpack(archive, into) {
  execFileSync('tar', ['-xf', archive, '-C', into], { stdio: 'inherit' });
}

/**
 * Every file in `directory`, however deeply nested the archive turns out to be.
 *
 * Symlinks count. The release ships each library under its real name and its
 * SONAME — `libllama.so.0` pointing at `libllama.so.0.1.2` — and the SONAME is
 * the one `llama-server` asks the loader for, so dropping the links would leave
 * the server unable to start.
 */
async function walk(directory) {
  const entries = await fs.readdir(directory, { withFileTypes: true });
  const found = [];

  for (const entry of entries) {
    const full = path.join(directory, entry.name);
    if (entry.isDirectory()) found.push(...(await walk(full)));
    else if (entry.isFile() || entry.isSymbolicLink()) found.push(full);
  }

  return found;
}

async function main() {
  const host = hostTriple();
  // `CHIEF_ENGINE_TARGET` is how a cross-compiling build asks for the engine
  // that matches what it is about to link, rather than the one that matches
  // the runner it is standing on.
  const target = process.env.CHIEF_ENGINE_TARGET?.trim() || host;
  const platform = TRIPLES[target];

  if (platform === undefined) {
    throw new Error(
      `Chief has no llama.cpp CPU build for ${target}. It bundles: ${Object.keys(TRIPLES).join(', ')}.`,
    );
  }

  const serverName = target.includes('windows') ? 'llama-server.exe' : 'llama-server';
  const fallback = needsMacos12Fallback(target, host);
  const stamp = fallback ? `${BUILD}-macos12-no-blas-${target}` : `${BUILD}-${target}`;
  const asset = ASSETS[platform];

  const sidecar = path.join(BINARIES, `llama-server-${target}${path.extname(serverName)}`);

  if (await alreadyHere(sidecar, stamp)) {
    console.log(`llama.cpp ${BUILD} is already here.`);
    return;
  }

  const scratch = await fs.mkdtemp(path.join(os.tmpdir(), 'chief-llama-'));

  try {
    let files;
    if (fallback) {
      console.log(`Building llama.cpp ${BUILD} without BLAS for macOS 12 or older…`);
      files = await walk(await buildMacos12Server(scratch));
    } else {
      const archive = path.join(scratch, asset);
      console.log(`Fetching llama.cpp ${BUILD} for ${platform}…`);
      await download(`${RELEASE}/${asset}`, archive);

      const unpacked = path.join(scratch, 'unpacked');
      await fs.mkdir(unpacked, { recursive: true });
      unpack(archive, unpacked);
      files = await walk(unpacked);
    }
    const server = files.find((file) => path.basename(file) === serverName);

    if (server === undefined) {
      throw new Error(`${asset} does not contain ${serverName}`);
    }

    // Start clean: a stale library from an older build would be found first and
    // fail to load against the new server.
    await fs.rm(LIBRARIES, { recursive: true, force: true });
    await fs.mkdir(LIBRARIES, { recursive: true });

    await fs.copyFile(server, sidecar);
    await fs.chmod(sidecar, 0o755);

    // Copied rather than linked: `copyFile` follows the symlink, so every name
    // the loader might ask for is a real file. Bundling never has to reason
    // about links, at the cost of a few megabytes beside a model of two
    // gigabytes.
    let libraries = 0;
    for (const file of files.filter((file) => LIBRARY_PATTERN.test(file))) {
      await fs.copyFile(file, path.join(LIBRARIES, path.basename(file)));
      libraries += 1;
    }

    await fs.writeFile(STAMP, `${stamp}\n`);

    console.log(
      `Installed ${path.relative(ROOT, sidecar)} and ${libraries} libraries into ${path.relative(ROOT, LIBRARIES)}.`,
    );
  } finally {
    await fs.rm(scratch, { recursive: true, force: true });
  }
}

main().catch((cause) => {
  console.error(`\nCould not fetch the llama.cpp server: ${cause.message}`);
  process.exit(1);
});
