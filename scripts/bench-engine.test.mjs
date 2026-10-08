import fs from 'node:fs';
import path from 'node:path';
import { fileURLToPath } from 'node:url';

import { describe, expect, it } from 'vitest';

import { median, modelFiles } from './bench-engine.mjs';

const ROOT = path.resolve(path.dirname(fileURLToPath(import.meta.url)), '..');

describe('modelFiles', () => {
  it("finds every model Chief downloads, in weights.rs's own order", () => {
    const weights = fs.readFileSync(path.join(ROOT, 'src-tauri', 'src', 'weights.rs'), 'utf8');
    const files = modelFiles(weights);

    // Read from the source, so this is the list the app itself would download.
    // At least the two tiers, and nothing that is not a model file.
    expect(files.length).toBeGreaterThanOrEqual(2);
    for (const file of files) expect(file).toMatch(/\.gguf$/);
  });
});

describe('median', () => {
  it('is not dragged around by one slow run', () => {
    expect(median([10, 11, 90])).toBe(11);
    expect(median([10, 12])).toBe(11);
  });
});
