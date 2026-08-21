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

/** What counts as a shared library the server needs beside it. */
const LIBRARY_PATTERN = /(\.so(\.\d+)*|\.dylib|\.dll|\.metal)$/i;

const serverName = process.platform === 'win32' ? 'llama-server.exe' : 'llama-server';

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
async function alreadyHere(sidecar) {
  if (!(await exists(sidecar))) return false;

  const stamped = await fs.readFile(STAMP, 'utf8').catch(() => '');

  return stamped.trim() === BUILD;
}

async function download(url, destination) {
  const response = await fetch(url, { redirect: 'follow' });

  if (!response.ok || response.body === null) {
    throw new Error(`could not download ${url}: HTTP ${response.status}`);
  }

  await pipeline(Readable.fromWeb(response.body), createWriteStream(destination));
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
  const platform = `${process.platform}-${process.arch}`;
  const asset = ASSETS[platform];

  if (asset === undefined) {
    throw new Error(
      `there is no llama.cpp CPU build for ${platform}. Chief bundles: ${Object.keys(ASSETS).join(', ')}.`,
    );
  }

  const sidecar = path.join(BINARIES, `llama-server-${hostTriple()}${path.extname(serverName)}`);

  if (await alreadyHere(sidecar)) {
    console.log(`llama.cpp ${BUILD} is already here.`);
    return;
  }

  const scratch = await fs.mkdtemp(path.join(os.tmpdir(), 'chief-llama-'));

  try {
    const archive = path.join(scratch, asset);
    console.log(`Fetching llama.cpp ${BUILD} for ${platform}…`);
    await download(`${RELEASE}/${asset}`, archive);

    const unpacked = path.join(scratch, 'unpacked');
    await fs.mkdir(unpacked, { recursive: true });
    unpack(archive, unpacked);

    const files = await walk(unpacked);
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

    await fs.writeFile(STAMP, `${BUILD}\n`);

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
