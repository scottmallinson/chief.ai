// Render CHANGELOG.md as the website's changelog page.
//
// The changelog is written by scripts/release.mjs, from the commits, at release
// time. This turns that markdown into `website/changelog.html` so the site
// shows the same history in the same design as the rest of it, rather than
// sending somebody to a raw file on GitHub.
//
// It is a generator rather than a page anybody edits, and `--check` is how that
// stays true: `pnpm check` fails if the committed page is not what this script
// would write from the markdown beside it, so the two cannot drift.
//
// The parse is deliberately strict. Silently dropping a line it did not
// recognise would publish a release with entries missing and say nothing, so
// anything unexpected stops the build instead.

import fs from 'node:fs';
import path from 'node:path';
import process from 'node:process';
import { fileURLToPath } from 'node:url';

const ROOT = path.resolve(path.dirname(fileURLToPath(import.meta.url)), '..');

/** Where the page is written, relative to the repository root. */
export const PAGE = 'website/changelog.html';

/** `## 0.4.0 (2026-09-03)` */
const RELEASE = /^## (?<version>\d+\.\d+\.\d+(?:[-+][0-9A-Za-z.-]+)?) \((?<date>[\d-]+)\)$/;

/** `### Features` */
const GROUP = /^### (?<title>.+)$/;

/** `- **Breaking.** **agent:** subject ([31cd9d3](https://…/commit/31cd9d3…))` */
const CHANGE =
  /^- (?:\*\*Breaking\.\*\* )?(?:\*\*(?<scope>[^*]+):\*\* )?(?<subject>.+?)(?: \(\[(?<sha>[0-9a-f]{7,40})\]\((?<url>[^)]+)\)\))?$/;

const BREAKING = /^- \*\*Breaking\.\*\* /;

export function escape(text) {
  return text
    .replace(/&/g, '&amp;')
    .replace(/</g, '&lt;')
    .replace(/>/g, '&gt;')
    .replace(/"/g, '&quot;');
}

/**
 * Read the changelog into releases, each with its groups of changes.
 *
 * Throws on anything it cannot place. A changelog entry that does not render
 * is a release the page does not report, which is worse than a failed build.
 */
export function parseChangelog(markdown) {
  const releases = [];
  let release = null;
  let group = null;

  for (const [index, raw] of markdown.split('\n').entries()) {
    const line = raw.trimEnd();

    if (line === '') continue;
    if (line.startsWith('# ')) continue;

    const heading = RELEASE.exec(line);
    if (heading !== null) {
      release = { version: heading.groups.version, date: heading.groups.date, groups: [] };
      group = null;
      releases.push(release);
      continue;
    }

    const where = `CHANGELOG.md line ${index + 1}`;

    if (release === null) throw new Error(`${where}: content before the first release heading`);

    const titled = GROUP.exec(line);
    if (titled !== null) {
      group = { title: titled.groups.title, changes: [] };
      release.groups.push(group);
      continue;
    }

    if (!line.startsWith('- ')) throw new Error(`${where}: not a heading or a change: ${line}`);
    if (group === null) throw new Error(`${where}: a change outside any section: ${line}`);

    const change = CHANGE.exec(line);
    if (change === null) throw new Error(`${where}: a change this cannot read: ${line}`);

    // The only markdown scripts/release.mjs emits inside a change is the bold
    // scope and the bold `Breaking.` marker, both of which the pattern above
    // has already taken off. Anything still holding `**` is emphasis this
    // renderer does not understand, and letting it through would put literal
    // asterisks on the page.
    if (change.groups.subject.includes('**')) {
      throw new Error(`${where}: a change this cannot read: ${line}`);
    }

    group.changes.push({
      breaking: BREAKING.test(line),
      scope: change.groups.scope ?? null,
      subject: change.groups.subject,
      sha: change.groups.sha ?? null,
      url: change.groups.url ?? null,
    });
  }

  if (releases.length === 0) throw new Error('CHANGELOG.md holds no releases');

  return releases;
}

/** `0.4.0` → `v0-4-0`, so a release can be linked to. */
export function anchor(version) {
  return `v${version.replace(/[^0-9A-Za-z]+/g, '-')}`;
}

function renderChange(change) {
  const parts = ['                <li class="change">'];

  if (change.breaking)
    parts.push('                  <span class="change-breaking">breaking</span>');
  if (change.scope !== null) {
    parts.push(`                  <span class="change-scope mono">${escape(change.scope)}</span>`);
  }

  parts.push(`                  <span class="change-subject">${escape(change.subject)}</span>`);

  if (change.sha !== null && change.url !== null) {
    parts.push(
      `                  <a class="change-sha mono" href="${escape(change.url)}">${escape(
        change.sha.slice(0, 7),
      )}</a>`,
    );
  }

  parts.push('                </li>');

  return parts.join('\n');
}

function renderRelease(release, { latest }) {
  const groups = release.groups.map(
    (group) => `            <div class="release-group">
              <p class="micro">${escape(group.title)}</p>
              <ul class="change-list">
${group.changes.map(renderChange).join('\n')}
              </ul>
            </div>`,
  );

  return `        <section class="release" id="${anchor(release.version)}">
          <div class="release-mark">
            <p class="release-version">${escape(release.version)}</p>
            <p class="release-date mono">${escape(release.date)}</p>
${latest ? '            <p class="release-latest">current release</p>\n' : ''}          </div>
          <div class="release-changes">
${groups.join('\n')}
          </div>
        </section>`;
}

/** The whole page, header and footer included, ready to be written to disk. */
export function renderPage(markdown) {
  const releases = parseChangelog(markdown);
  const current = releases[0].version;

  return `<!doctype html>
<html lang="en">
  <head>
    <meta charset="utf-8" />
    <meta name="viewport" content="width=device-width, initial-scale=1" />
    <title>Changelog — Chief</title>
    <meta
      name="description"
      content="Every release of Chief, and what changed in it. Written from the commits at release time."
    />
    <meta name="theme-color" content="#F4F5F3" />
    <link rel="icon" href="favicon.svg" type="image/svg+xml" />
    <link rel="stylesheet" href="styles.css" />
    <script src="support.js" defer></script>
  </head>
  <body>
    <!--
      Generated by scripts/changelog-page.mjs from CHANGELOG.md. Do not edit
      this file: \`pnpm check\` compares it against what that script writes, and
      an edit here would be overwritten by the next release anyway.
    -->
    <svg class="sprite" aria-hidden="true">
      <defs>
        <symbol
          id="i-shield-check"
          viewBox="0 0 24 24"
          fill="none"
          stroke="currentColor"
          stroke-width="2"
          stroke-linecap="round"
          stroke-linejoin="round"
        >
          <path
            d="M20 13c0 5-3.5 7.5-7.66 8.95a1 1 0 0 1-.67-.01C7.5 20.5 4 18 4 13V6a1 1 0 0 1 1-1c2 0 4.5-1.2 6.24-2.72a1.17 1.17 0 0 1 1.52 0C14.51 3.81 17 5 19 5a1 1 0 0 1 1 1z"
          />
          <path d="m9 12 2 2 4-4" />
        </symbol>
        <symbol
          id="i-download"
          viewBox="0 0 24 24"
          fill="none"
          stroke="currentColor"
          stroke-width="2"
          stroke-linecap="round"
          stroke-linejoin="round"
        >
          <path d="M21 15v4a2 2 0 0 1-2 2H5a2 2 0 0 1-2-2v-4" />
          <polyline points="7 10 12 15 17 10" />
          <line x1="12" x2="12" y1="15" y2="3" />
        </symbol>
        <symbol id="i-github" viewBox="0 0 24 24" fill="currentColor">
          <path
            d="M12 .5C5.37.5 0 5.87 0 12.5c0 5.3 3.44 9.8 8.2 11.39.6.11.82-.26.82-.58v-2.03c-3.34.73-4.04-1.61-4.04-1.61-.55-1.39-1.34-1.76-1.34-1.76-1.09-.75.08-.73.08-.73 1.2.08 1.84 1.24 1.84 1.24 1.07 1.84 2.81 1.31 3.5 1 .11-.78.42-1.31.76-1.61-2.67-.3-5.47-1.34-5.47-5.95 0-1.31.47-2.38 1.24-3.22-.12-.31-.54-1.53.12-3.18 0 0 1.01-.32 3.3 1.23a11.5 11.5 0 0 1 6 0c2.29-1.55 3.3-1.23 3.3-1.23.66 1.65.24 2.87.12 3.18.77.84 1.24 1.91 1.24 3.22 0 4.62-2.81 5.64-5.49 5.94.43.37.82 1.1.82 2.22v3.29c0 .32.21.7.83.58A12.01 12.01 0 0 0 24 12.5C24 5.87 18.63.5 12 .5z"
          />
        </symbol>
      </defs>
    </svg>

    <header class="site-header">
      <div class="shell">
        <a class="wordmark" href="index.html">
          <svg width="26" height="26" viewBox="0 0 32 32" aria-hidden="true">
            <rect width="32" height="32" rx="8" fill="#14171A" />
            <path
              d="M21.5 11.3a7 7 0 1 0 0 9.4"
              fill="none"
              stroke="#F4F5F3"
              stroke-width="3.2"
              stroke-linecap="round"
            />
          </svg>
          <p>Chief</p>
        </a>
        <nav class="site-nav">
          <a href="security.html">Security</a>
          <a href="docs.html">Docs</a>
          <a class="nav-external" href="https://github.com/scottmallinson/chief.ai">
            <svg width="15" height="15" aria-hidden="true"><use href="#i-github" /></svg>
            <span class="nav-label">GitHub</span>
          </a>
          <a class="btn btn-primary btn-sm" href="index.html#get">
            <svg width="15" height="15" aria-hidden="true"><use href="#i-download" /></svg>
            Download
          </a>
        </nav>
      </div>
    </header>

    <main class="shell page">
      <p class="micro">changelog</p>
      <h1>What changed, release by release.</h1>
      <p class="page-sub">
        Every release of Chief since the first one. The list is written from the commits at release
        time rather than by hand, so it says what actually shipped — and a release only happens when
        something a person can download has changed. Version
        <span data-version>${escape(current)}</span> is current.
      </p>

      <div class="changelog">
${releases.map((release, index) => renderRelease(release, { latest: index === 0 })).join('\n')}
      </div>

      <div class="note mt-24">
        <p>
          Builds are attached to each release on GitHub. Chief has no updater and no update check —
          nothing on your machine asks us whether a new version exists, because there is nobody to
          ask. <a href="https://github.com/scottmallinson/chief.ai/releases">Watch the releases</a>
          if you want to know.
        </p>
      </div>
    </main>

    <footer class="site-footer">
      <div class="shell footer-top">
        <div>
          <div class="wordmark">
            <svg width="24" height="24" viewBox="0 0 32 32" aria-hidden="true">
              <rect width="32" height="32" rx="8" fill="#14171A" />
              <path
                d="M21.5 11.3a7 7 0 1 0 0 9.4"
                fill="none"
                stroke="#F4F5F3"
                stroke-width="3.4"
                stroke-linecap="round"
              />
            </svg>
            <p>Chief</p>
          </div>
          <p class="footer-blurb">
            An on-device chief of staff for people whose work log is nobody else’s business.
          </p>
        </div>
        <div class="footer-cols">
          <div class="footer-col">
            <p class="micro">product</p>
            <a href="index.html#get">Download</a>
            <a href="docs.html">Docs</a>
            <a href="changelog.html" aria-current="page">Changelog</a>
          </div>
          <div class="footer-col">
            <p class="micro">trust</p>
            <a href="security.html">Security</a>
            <a href="security.html#collect">What it doesn’t collect</a>
          </div>
          <div class="footer-col">
            <p class="micro">source</p>
            <a href="https://github.com/scottmallinson/chief.ai">Source on GitHub</a>
            <a href="https://github.com/scottmallinson/chief.ai/releases">Releases</a>
          </div>
        </div>
      </div>
      <div class="footer-base">
        <div class="shell">
          <p class="mono">© 2026 Chief · no accounts, no telemetry</p>
          <p class="footer-badge">
            <svg width="14" height="14" aria-hidden="true"><use href="#i-shield-check" /></svg>
            on-device only
          </p>
        </div>
      </div>
    </footer>
  </body>
</html>
`;
}

/** Write the page from the changelog beside it. Returns what was written. */
export function buildPage(root = ROOT) {
  const page = renderPage(fs.readFileSync(path.join(root, 'CHANGELOG.md'), 'utf8'));

  fs.writeFileSync(path.join(root, PAGE), page);

  return page;
}

function main() {
  const check = process.argv.includes('--check');
  const wanted = renderPage(fs.readFileSync(path.join(ROOT, 'CHANGELOG.md'), 'utf8'));
  const target = path.join(ROOT, PAGE);

  if (!check) {
    fs.writeFileSync(target, wanted);
    console.log(`Wrote ${PAGE} from CHANGELOG.md.`);
    return;
  }

  const existing = fs.existsSync(target) ? fs.readFileSync(target, 'utf8') : '';

  if (existing !== wanted) {
    console.error(
      `${PAGE} is not what CHANGELOG.md says it should be. Run \`pnpm changelog\` and commit the result.`,
    );
    process.exit(1);
  }

  console.log(`${PAGE} is in step with CHANGELOG.md.`);
}

if (process.argv[1] === fileURLToPath(import.meta.url)) main();
