// Rebuilds the committed fixtures from the committed slices. No dump, no network.
//
//   node bench/public-data/build-fixtures.mjs          # write
//   node bench/public-data/build-fixtures.mjs --check  # fail if a fixture is not what this would write
//
// The check is run by the tests, so a fixture edited by hand, or a converter changed without
// rebuilding, is found by `pnpm test` and not by a benchmark that quietly measured something else.

import { readFileSync, readdirSync, writeFileSync, mkdirSync } from 'node:fs';
import { dirname, join } from 'node:path';
import { fileURLToPath } from 'node:url';

import { convert } from './to-jira.mjs';

const here = dirname(fileURLToPath(import.meta.url));
/** The day every slice is moved to: a Monday, so "this week" is a whole working week ahead. */
export const TODAY = '2026-11-23';

export function build() {
  const out = {};
  for (const file of readdirSync(join(here, 'slices'))
    .filter((f) => f.endsWith('.json'))
    .sort()) {
    const slice = JSON.parse(readFileSync(join(here, 'slices', file), 'utf8'));
    out[`jira-${file}`] = `${JSON.stringify(convert(slice, TODAY), null, 1)}\n`;
  }
  return out;
}

if (process.argv[1] === fileURLToPath(import.meta.url)) {
  const dir = join(here, 'fixtures');
  const built = build();
  if (process.argv.includes('--check')) {
    const stale = Object.entries(built).filter(([name, text]) => {
      try {
        return readFileSync(join(dir, name), 'utf8') !== text;
      } catch {
        return true;
      }
    });
    if (stale.length) {
      console.error(`stale: ${stale.map(([n]) => n).join(', ')}`);
      process.exit(1);
    }
  } else {
    mkdirSync(dir, { recursive: true });
    for (const [name, text] of Object.entries(built)) writeFileSync(join(dir, name), text);
  }
}
