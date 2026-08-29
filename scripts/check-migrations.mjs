// Migrations are append-only, and their versions have to be unique.
//
// This exists because the failure is silent and expensive. A duplicate version
// does not fail to compile and does not fail a test: sqlx applies whichever it
// meets first, and every installed copy that already ran the other one is now
// on a schema the code does not expect. There is no down path.
//
// It caught a real mistake: an implementation plan written against a branch at
// v2 allocated v3 for its first new table, while the branch it was reconciled
// against had already taken v3. Both were correct in isolation.
//
// Also checks the versions are contiguous from 1, so a gap — which usually
// means a migration was deleted rather than superseded — is caught in the same
// pass.

import fs from 'node:fs';
import path from 'node:path';
import process from 'node:process';
import { fileURLToPath } from 'node:url';

const ROOT = path.resolve(path.dirname(fileURLToPath(import.meta.url)), '..');
const SOURCE = path.join(ROOT, 'src-tauri', 'src', 'db.rs');

/** `version: 4,` inside a `Migration { ... }` literal. */
const VERSION = /^\s*version:\s*(\d+)\s*,/gm;

function fail(message) {
  console.error(`\nMigrations: ${message}\n`);
  process.exit(1);
}

const source = fs.readFileSync(SOURCE, 'utf8');
const versions = [...source.matchAll(VERSION)].map((match) => Number(match[1]));

if (versions.length === 0) {
  fail(`no migrations found in ${path.relative(ROOT, SOURCE)} — has the shape changed?`);
}

const seen = new Set();
const duplicates = new Set();

for (const version of versions) {
  if (seen.has(version)) duplicates.add(version);
  seen.add(version);
}

if (duplicates.size > 0) {
  fail(
    `version ${[...duplicates].join(', ')} is used more than once. ` +
      'Migrations are append-only: take the next free version rather than reusing one, ' +
      'or an installed copy ends up on a schema this code does not expect.',
  );
}

const ordered = [...versions].sort((a, b) => a - b);
const expected = ordered.map((_, index) => index + 1);

if (ordered.join() !== expected.join()) {
  fail(
    `versions are ${ordered.join(', ')} but should run 1..${ordered.length} without gaps. ` +
      'A gap usually means a migration was deleted rather than superseded.',
  );
}

console.log(`Migrations: ${versions.length} versions, 1..${versions.length}, no duplicates.`);
