import fs from 'node:fs';
import path from 'node:path';
import { fileURLToPath } from 'node:url';

import { describe, expect, it } from 'vitest';

import { PAGE, anchor, escape, parseChangelog, renderPage } from './changelog-page.mjs';

const ROOT = path.resolve(path.dirname(fileURLToPath(import.meta.url)), '..');

/** A changelog in the shape scripts/release.mjs writes. */
const changelog = (...lines) => ['# Changelog', '', ...lines, ''].join('\n');

const release = (version = '0.4.0', date = '2026-09-03') => [
  `## ${version} (${date})`,
  '',
  '### Features',
  '',
  '- **agent:** stream answers ([a1b2c3d](https://github.com/o/r/commit/a1b2c3d4e5f6))',
  '',
];

describe('parseChangelog', () => {
  it('reads a release, its sections and its changes', () => {
    const [entry] = parseChangelog(changelog(...release()));

    expect(entry).toMatchObject({ version: '0.4.0', date: '2026-09-03' });
    expect(entry.groups).toHaveLength(1);
    expect(entry.groups[0]).toMatchObject({ title: 'Features' });
    expect(entry.groups[0].changes[0]).toMatchObject({
      scope: 'agent',
      subject: 'stream answers',
      sha: 'a1b2c3d',
      breaking: false,
    });
  });

  it('reads a change with no scope and no commit link', () => {
    const [entry] = parseChangelog(
      changelog('## 0.1.0 (2026-01-01)', '', '### Fixes', '', '- tidy the shell'),
    );

    expect(entry.groups[0].changes[0]).toMatchObject({
      scope: null,
      subject: 'tidy the shell',
      sha: null,
      url: null,
    });
  });

  it('marks a breaking change, keeping the subject whole', () => {
    const [entry] = parseChangelog(
      changelog('## 0.5.0 (2026-01-01)', '', '### Features', '', '- **Breaking.** **db:** move it'),
    );

    expect(entry.groups[0].changes[0]).toMatchObject({
      breaking: true,
      scope: 'db',
      subject: 'move it',
    });
  });

  it('keeps every release in the order the changelog has them', () => {
    const entries = parseChangelog(
      changelog(...release('0.4.0', '2026-09-03'), ...release('0.3.0', '2026-08-29')),
    );

    expect(entries.map((entry) => entry.version)).toEqual(['0.4.0', '0.3.0']);
  });

  /* The guard. A parser that shrugged at a line it did not recognise would
     publish a release with entries silently missing, and the page would look
     perfectly fine. Deleting the two throws below leaves the rest of this file
     green, which is why these three are here rather than implied. */
  it('refuses a change it cannot read rather than dropping it', () => {
    const markdown = changelog('## 0.4.0 (2026-09-03)', '', '### Features', '', '- **unclosed');

    expect(() => parseChangelog(markdown)).toThrow(/cannot read/);
  });

  it('refuses a change that belongs to no section', () => {
    const markdown = changelog('## 0.4.0 (2026-09-03)', '', '- **agent:** stream answers');

    expect(() => parseChangelog(markdown)).toThrow(/outside any section/);
  });

  it('refuses prose where a release, a section or a change should be', () => {
    const markdown = changelog(...release(), 'Released on a Tuesday.');

    expect(() => parseChangelog(markdown)).toThrow(/not a heading or a change/);
  });

  it('refuses a changelog with no releases in it', () => {
    expect(() => parseChangelog('# Changelog\n')).toThrow(/no releases/);
  });
});

describe('anchor', () => {
  it('turns a version into something a URL can hold', () => {
    expect(anchor('0.4.0')).toBe('v0-4-0');
    expect(anchor('1.0.0-rc.1')).toBe('v1-0-0-rc-1');
  });
});

describe('escape', () => {
  it('escapes what would otherwise become markup', () => {
    expect(escape('<img src=x onerror="a">&')).toBe('&lt;img src=x onerror=&quot;a&quot;&gt;&amp;');
  });
});

describe('renderPage', () => {
  it('renders every change in the changelog, and its commit link', () => {
    const page = renderPage(changelog(...release('0.4.0'), ...release('0.3.0', '2026-08-29')));

    expect(page.match(/<li class="change">/g)).toHaveLength(2);
    expect(page).toContain('https://github.com/o/r/commit/a1b2c3d4e5f6');
    expect(page).toContain('id="v0-4-0"');
    expect(page).toContain('id="v0-3-0"');
  });

  it('marks only the newest release as the current one', () => {
    const page = renderPage(changelog(...release('0.4.0'), ...release('0.3.0', '2026-08-29')));

    expect(page.match(/current release/g)).toHaveLength(1);
    expect(page.indexOf('current release')).toBeLessThan(page.indexOf('id="v0-3-0"'));
  });

  it('carries the header, the footer and the changelog link they share', () => {
    const page = renderPage(changelog(...release()));

    expect(page).toContain('class="site-header"');
    expect(page).toContain('class="site-footer"');
    expect(page).toContain('<a href="changelog.html" aria-current="page">Changelog</a>');
  });

  it('escapes a subject rather than letting it reach the page as markup', () => {
    const page = renderPage(
      changelog('## 0.4.0 (2026-01-01)', '', '### Fixes', '', '- **ui:** stop <script> escaping'),
    );

    expect(page).toContain('stop &lt;script&gt; escaping');
    expect(page).not.toContain('<script> escaping');
  });
});

describe('the committed page', () => {
  /* The other half of `pnpm check:changelog`, so a changelog edited without
     the page being rebuilt fails here as well as in CI. */
  it('is what the changelog beside it says it should be', () => {
    const markdown = fs.readFileSync(path.join(ROOT, 'CHANGELOG.md'), 'utf8');

    expect(fs.readFileSync(path.join(ROOT, PAGE), 'utf8')).toBe(renderPage(markdown));
  });

  it('accounts for every entry in the changelog', () => {
    const markdown = fs.readFileSync(path.join(ROOT, 'CHANGELOG.md'), 'utf8');
    const entries = markdown.split('\n').filter((line) => line.startsWith('- ')).length;
    const page = fs.readFileSync(path.join(ROOT, PAGE), 'utf8');

    expect(page.match(/<li class="change">/g) ?? []).toHaveLength(entries);
  });
});
