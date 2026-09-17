import fs from 'node:fs';
import os from 'node:os';
import path from 'node:path';

import { describe, expect, it } from 'vitest';

import {
  VERSIONED,
  bumpVersion,
  changesTheApp,
  decideBump,
  parseCommits,
  renderEntry,
  replaceOnce,
  writeChangelog,
  writeVersion,
} from './release.mjs';

/** Build the `git log` output shape this parses, so tests read like commits. */
const log = (...commits) =>
  commits
    .map(([subject, body = '', sha = 'a'.repeat(40)]) => `${sha}\x00${subject}\x00${body}`)
    .join('\x1e');

describe('parseCommits', () => {
  it('reads the type, scope and subject', () => {
    const [commit] = parseCommits(log(['feat(agent): stream answers']));

    expect(commit).toMatchObject({ type: 'feat', scope: 'agent', subject: 'stream answers' });
  });

  it('marks a commit breaking from the ! shorthand', () => {
    expect(parseCommits(log(['feat(db)!: drop the old table']))[0].breaking).toBe(true);
  });

  it('marks a commit breaking from the footer', () => {
    const raw = log(['feat(db): drop the old table', 'BREAKING CHANGE: the schema moved']);

    expect(parseCommits(raw)[0].breaking).toBe(true);
  });

  it('keeps a commit that is not conventional, without a type', () => {
    expect(parseCommits(log(['tidied things up']))[0]).toMatchObject({
      type: null,
      breaking: false,
    });
  });
});

describe('changesTheApp', () => {
  /** What `main` does with the commits before it decides anything. */
  const app = (raw) => parseCommits(raw).filter(changesTheApp);

  it('drops a site commit, whatever its type', () => {
    const raw = log(
      ['feat(website): publish the changelog as a page'],
      ['fix(website): fit the header on a phone'],
    );

    expect(app(raw)).toEqual([]);
  });

  it('keeps every scope that is the app', () => {
    const raw = log(
      ['feat(agent): stream answers'],
      ['fix(ui): keep a row inside its card'],
      ['fix: something unscoped'],
    );

    expect(app(raw)).toHaveLength(3);
  });

  it('leaves a release with nothing to release when only the site changed', () => {
    const raw = log(['feat(website): add the marketing site'], ['docs(repo): write it down']);

    expect(decideBump(app(raw), '0.5.0')).toBeNull();
  });

  it('does not let a site feature turn a fix release into a minor one', () => {
    const raw = log(
      ['feat(website): add a changelog page'],
      ['fix(agent): stop the brief echoing dates'],
    );

    expect(decideBump(app(raw), '0.5.0')).toBe('patch');
  });

  it('writes no changelog line for a site commit', () => {
    const raw = log(['feat(website): add a changelog page'], ['feat(agent): read Jira']);
    const entry = renderEntry('0.6.0', app(raw), { date: '2026-09-15', repository: null });

    expect(entry).toContain('read Jira');
    expect(entry).not.toContain('changelog page');
  });
});

describe('decideBump', () => {
  it('does not release for docs, ci or chore alone', () => {
    const commits = parseCommits(
      log(['docs(repo): explain the daemon'], ['ci(ci): cache chromium']),
    );

    expect(decideBump(commits, '0.1.0')).toBeNull();
  });

  it('does not release when nothing is conventional', () => {
    expect(decideBump(parseCommits(log(['wip'])), '0.1.0')).toBeNull();
  });

  it('bumps the patch for a fix', () => {
    expect(decideBump(parseCommits(log(['fix(ui): keep the window still'])), '0.1.0')).toBe(
      'patch',
    );
  });

  it('bumps the minor for a feature, whatever else is alongside it', () => {
    const commits = parseCommits(log(['fix(ui): a fix'], ['feat(agent): a feature']));

    expect(decideBump(commits, '0.1.0')).toBe('minor');
  });

  it('bumps the minor, not the major, for a breaking change before 1.0.0', () => {
    expect(decideBump(parseCommits(log(['feat(db)!: new schema'])), '0.4.2')).toBe('minor');
  });

  it('bumps the major for a breaking change once 1.0.0 has shipped', () => {
    expect(decideBump(parseCommits(log(['feat(db)!: new schema'])), '1.4.2')).toBe('major');
  });
});

describe('bumpVersion', () => {
  it.each([
    ['0.1.0', 'patch', '0.1.1'],
    ['0.1.9', 'minor', '0.2.0'],
    ['1.2.3', 'major', '2.0.0'],
  ])('bumps %s by %s to %s', (from, bump, expected) => {
    expect(bumpVersion(from, bump)).toBe(expected);
  });

  it('refuses a version it cannot read', () => {
    expect(() => bumpVersion('not-a-version', 'patch')).toThrow(/cannot|not a version/i);
  });
});

describe('renderEntry', () => {
  const commits = parseCommits(
    log(
      ['feat(agent): stream answers', '', 'b'.repeat(40)],
      ['fix(ui): stop the jump'],
      ['ci(ci): cheaper'],
    ),
  );
  const entry = renderEntry('0.2.0', commits, {
    date: '2026-08-27',
    repository: 'https://example.test/o/r',
  });

  it('titles the release', () => {
    expect(entry).toContain('## 0.2.0 (2026-08-27)');
  });

  it('groups the releasable types under headings', () => {
    expect(entry).toContain('### Features');
    expect(entry).toContain('### Fixes');
  });

  it('leaves out types that have no section', () => {
    expect(entry).not.toContain('cheaper');
  });

  it('links each commit to itself', () => {
    expect(entry).toContain('([bbbbbbb](https://example.test/o/r/commit/' + 'b'.repeat(40) + '))');
  });
});

describe('replaceOnce', () => {
  const pattern = /^ {2}"version": "[^"]+",$/m;

  it('replaces the one version it finds', () => {
    expect(replaceOnce('{\n  "version": "0.1.0",\n}', pattern, '9.9.9', 'x')).toContain('"9.9.9"');
  });

  it('refuses a file with no version', () => {
    expect(() => replaceOnce('{}', pattern, '9.9.9', 'x')).toThrow(/found 0/);
  });

  it('refuses a file with more than one', () => {
    const twice = '{\n  "version": "0.1.0",\n  "version": "0.1.0",\n}';

    expect(() => replaceOnce(twice, pattern, '9.9.9', 'x')).toThrow(/found 2/);
  });
});

// The patterns are only useful if they still match the real files. This is the
// test that fails when one of them is reformatted or renamed.
describe('the files that carry the version', () => {
  it.each(VERSIONED.map(({ file, pattern }) => [file, pattern]))(
    // The pattern is in the title because `website/index.html` carries two of
    // them, and two tests called the same thing tell you nothing about which
    // one broke.
    'has exactly one version in %s matching %s',
    (file, pattern) => {
      const contents = fs.readFileSync(path.join(process.cwd(), file), 'utf8');
      const found = contents.match(new RegExp(pattern.source, `${pattern.flags}g`)) ?? [];

      expect(found).toHaveLength(1);
    },
  );
});

// The patterns matching is not the same claim as the release actually writing
// them. `website/index.html`'s two fallbacks sat at 0.4.0 through two releases
// precisely because nothing asserted the end of this path, so this drives the
// real files through `writeVersion` and reads the result back.
describe('writeVersion', () => {
  /** A throwaway tree holding the real versioned files at their real paths. */
  const stage = () => {
    const dir = fs.mkdtempSync(path.join(os.tmpdir(), 'chief-version-'));

    for (const { file } of VERSIONED) {
      const target = path.join(dir, file);
      fs.mkdirSync(path.dirname(target), { recursive: true });
      fs.copyFileSync(path.join(process.cwd(), file), target);
    }

    return dir;
  };

  it('writes the new version into every file that carries one', () => {
    const dir = stage();
    writeVersion('9.9.9', dir);

    for (const { file, pattern } of new Map(VERSIONED.map((v) => [v.file, v])).values()) {
      const contents = fs.readFileSync(path.join(dir, file), 'utf8');
      expect(contents, `${file} was not stamped`).toContain('9.9.9');
      expect(pattern.test(contents), `${file} no longer matches its own pattern`).toBe(true);
    }
  });

  it('leaves no stale version behind in the site the visitor reads', () => {
    const dir = stage();
    const before = fs.readFileSync(path.join(dir, 'website/index.html'), 'utf8');

    // The fixture has to actually carry a version, or this passes by vacuum.
    expect(before).toMatch(/Version \d+\.\d+\.\d+/);

    writeVersion('9.9.9', dir);
    const after = fs.readFileSync(path.join(dir, 'website/index.html'), 'utf8');

    // Both fallbacks, named separately: one of them being right is how this
    // was wrong before.
    expect(after).toContain('data-download-note>Version 9.9.9');
    expect(after).toContain('<span data-version>9.9.9</span>');

    // And nothing anywhere in the page still claims an older one.
    expect(after.match(/Version \d+\.\d+\.\d+|<span data-version>\d+\.\d+\.\d+/g)).toEqual([
      'Version 9.9.9',
      '<span data-version>9.9.9',
    ]);
  });
});

describe('writeChangelog', () => {
  const read = (dir) => fs.readFileSync(path.join(dir, 'CHANGELOG.md'), 'utf8');
  const entry = (version) => `## ${version} (2026-08-27)\n\n### Fixes\n\n- **ui:** a fix\n`;

  it('starts a changelog, with one blank line and a trailing newline', () => {
    const dir = fs.mkdtempSync(path.join(os.tmpdir(), 'chief-changelog-'));

    writeChangelog(entry('0.1.1'), dir);

    expect(read(dir)).toBe(
      `# Changelog\n\n## 0.1.1 (2026-08-27)\n\n### Fixes\n\n- **ui:** a fix\n`,
    );
  });

  it('puts a later release above an earlier one, and never doubles a blank line', () => {
    const dir = fs.mkdtempSync(path.join(os.tmpdir(), 'chief-changelog-'));

    writeChangelog(entry('0.1.1'), dir);
    writeChangelog(entry('0.2.0'), dir);

    const contents = read(dir);

    expect(contents.indexOf('0.2.0')).toBeLessThan(contents.indexOf('0.1.1'));
    expect(contents).not.toMatch(/\n{3}/);
    expect(contents.endsWith('\n')).toBe(true);
  });
});
