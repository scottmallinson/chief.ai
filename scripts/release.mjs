// Work out the next release from the commits, and write it into the files that
// carry the version.
//
// This runs on every push to `main`. Releases are not gated on a pull request:
// if what landed is worth releasing, it is released. So the decision this file
// makes is the decision — there is no human between it and a published
// release, which is why the rules below are stated rather than implied, and
// why the pieces that decide a version number are exported and tested.

import { execFileSync } from 'node:child_process';
import fs from 'node:fs';
import path from 'node:path';
import process from 'node:process';
import { fileURLToPath } from 'node:url';

import { buildPage } from './changelog-page.mjs';

const ROOT = path.resolve(path.dirname(fileURLToPath(import.meta.url)), '..');

/**
 * Where to start when no release tag exists yet. Without this the first
 * release would summarise the whole history of the repository.
 */
const BASELINE = '3997c5beb576512ad3fa5d9d9fcfb77db7abd295';

/**
 * The types that are worth releasing for. A `docs`, `ci`, `chore`, `style`,
 * `test` or `refactor` commit changes nothing a user can download, and a
 * release costs four bundles — two of them on a runner that bills at ten times
 * Linux. Those commits neither cause a release nor appear in one: the
 * changelog is what changed for the person installing it, and `SECTIONS` below
 * is the whole of it.
 */
const RELEASABLE = new Set(['feat', 'fix', 'perf', 'revert']);

/**
 * Scopes that are not the application.
 *
 * The changelog documents what changed for the person who installed Chief, and
 * the marketing site is not something they installed — it is deployed by Vercel
 * from `main` the moment a change lands there, on its own, with no version
 * number and nothing to download. A `feat(website)` in the changelog therefore
 * describes a change the reader cannot have received, and worse, it releases:
 * it bumps the minor, writes five files, tags, and builds three bundles that
 * are byte-identical to the last three.
 *
 * So a site commit neither releases nor appears in a release, on exactly the
 * same footing as a `docs` or a `chore` — the difference is only that this one
 * is decided by the scope rather than by the type, because a site change is
 * still a `feat` or a `fix` of the site.
 *
 * This is a claim the author makes, not one inferred from the files: a site
 * change legitimately touches `playwright.config.ts`, `.prettierignore` or a
 * workflow, so "every path is under `website/`" would have missed three of the
 * four site commits in 0.5.0 and is not a rule worth having. `website` is on
 * commitlint's `scope-enum`, so the claim is checked when it is made.
 */
const OFF_APP_SCOPES = new Set(['website']);

/** Whether a commit changed the thing a user installs. */
export function changesTheApp(commit) {
  return !OFF_APP_SCOPES.has(commit.scope);
}

/** How each type is titled in the changelog, in the order they appear. */
const SECTIONS = [
  ['feat', 'Features'],
  ['fix', 'Fixes'],
  ['perf', 'Performance'],
  ['revert', 'Reverts'],
];

const HEADER = /^(?<type>[a-z]+)(?:\((?<scope>[^)]*)\))?(?<breaking>!)?: (?<subject>.+)$/;

/** Split `git log` output back into commits. */
export function parseCommits(raw) {
  return raw
    .split('\x1e')
    .map((record) => record.trim())
    .filter(Boolean)
    .map((record) => {
      const [sha = '', subject = '', body = ''] = record.split('\x00');
      const match = HEADER.exec(subject.trim());

      if (match === null) return { sha, subject: subject.trim(), type: null, breaking: false };

      return {
        sha,
        type: match.groups.type,
        scope: match.groups.scope ?? null,
        subject: match.groups.subject,
        // Either spelling counts: the `!` shorthand, or the footer in the body.
        breaking: Boolean(match.groups.breaking) || /^BREAKING[ -]CHANGE:/m.test(body),
      };
    });
}

/**
 * Which part of the version a set of commits moves, or `null` for no release.
 *
 * Before 1.0.0 a breaking change bumps the minor rather than the major: a
 * project that is not finished should not be forced to call itself 1.0 by its
 * first breaking change.
 */
export function decideBump(commits, current) {
  const releasable = commits.filter((commit) => commit.breaking || RELEASABLE.has(commit.type));

  if (releasable.length === 0) return null;

  const preMajor = Number(current.split('.')[0]) === 0;

  if (releasable.some((commit) => commit.breaking)) return preMajor ? 'minor' : 'major';
  if (releasable.some((commit) => commit.type === 'feat')) return 'minor';

  return 'patch';
}

/** Apply a bump to a semver string. */
export function bumpVersion(version, bump) {
  const [major, minor, patch] = version.split('.').map(Number);

  if ([major, minor, patch].some((part) => !Number.isInteger(part))) {
    throw new Error(`not a version this can bump: ${version}`);
  }

  if (bump === 'major') return `${major + 1}.0.0`;
  if (bump === 'minor') return `${major}.${minor + 1}.0`;
  if (bump === 'patch') return `${major}.${minor}.${patch + 1}`;

  throw new Error(`unknown bump: ${bump}`);
}

/** The changelog entry for one release, newest section first. */
export function renderEntry(version, commits, { date, repository }) {
  const lines = [`## ${version} (${date})`];

  for (const [type, title] of SECTIONS) {
    const matching = commits.filter((commit) => commit.type === type);

    if (matching.length === 0) continue;

    lines.push('', `### ${title}`, '');

    for (const commit of matching) {
      const scope = commit.scope ? `**${commit.scope}:** ` : '';
      const breaking = commit.breaking ? '**Breaking.** ' : '';
      const link = repository
        ? ` ([${commit.sha.slice(0, 7)}](${repository}/commit/${commit.sha}))`
        : '';

      lines.push(`- ${breaking}${scope}${commit.subject}${link}`);
    }
  }

  return `${lines.join('\n')}\n`;
}

/**
 * Replace exactly one version string in a file. Anything else — no match, or
 * more than one — is a file that has changed shape, and is worth stopping for
 * rather than guessing at.
 */
export function replaceOnce(contents, pattern, version, what) {
  const matches = contents.match(new RegExp(pattern.source, `${pattern.flags}g`)) ?? [];

  if (matches.length !== 1) {
    throw new Error(`expected one version in ${what}, found ${matches.length}`);
  }

  return contents.replace(pattern, (line) =>
    line.replace(/\d+\.\d+\.\d+(?:[-+][0-9A-Za-z.-]+)?/, version),
  );
}

/** Every file that carries the version, and how to find it in each. */
export const VERSIONED = [
  { file: 'package.json', pattern: /^ {2}"version": "[^"]+",$/m },
  { file: 'src-tauri/tauri.conf.json', pattern: /^ {2}"version": "[^"]+",$/m },
  { file: 'src-tauri/Cargo.toml', pattern: /^version = "[^"]+"$/m },
  // The lockfile records the workspace member's own version. Cargo rewrites it
  // at build time anyway, but leaving it stale means every later build shows a
  // dirty tree.
  { file: 'src-tauri/Cargo.lock', pattern: /^name = "Chief"\nversion = "[^"]+"$/m },
  // The website builds its download URLs from this constant. GitHub's release
  // assets are version-stamped, so `releases/latest/download/` cannot resolve
  // them — and resolving them through the API instead would hand every visitor's
  // IP address to GitHub on page load. Stamping it here costs the site no
  // request at all.
  { file: 'website/support.js', pattern: /^const VERSION = '[^']+';$/m },
];

export function writeVersion(version, root = ROOT) {
  for (const { file, pattern } of VERSIONED) {
    const target = path.join(root, file);
    const contents = fs.readFileSync(target, 'utf8');

    fs.writeFileSync(target, replaceOnce(contents, pattern, version, file));
  }
}

export function writeChangelog(entry, root = ROOT) {
  const target = path.join(root, 'CHANGELOG.md');
  const existing = fs.existsSync(target) ? fs.readFileSync(target, 'utf8') : '# Changelog\n';
  const break_ = existing.indexOf('\n');
  const heading = (break_ === -1 ? existing : existing.slice(0, break_)).trim();
  const earlier = (break_ === -1 ? '' : existing.slice(break_ + 1)).trim();

  // Heading, this release, then everything that was already there — kept
  // opaque, so an entry written by an older version of this script is left
  // exactly as it was.
  fs.writeFileSync(target, `${[heading, entry.trim(), earlier].filter(Boolean).join('\n\n')}\n`);
}

function git(...args) {
  return execFileSync('git', args, { cwd: ROOT, encoding: 'utf8' }).trim();
}

/** The last release tag, or the baseline commit if nothing has been released. */
function since() {
  try {
    // stderr is swallowed: with no tags yet `describe` says so loudly, and
    // that is the ordinary first-release case rather than a problem.
    return execFileSync('git', ['describe', '--tags', '--abbrev=0', '--match', 'v*'], {
      cwd: ROOT,
      encoding: 'utf8',
      stdio: ['ignore', 'pipe', 'ignore'],
    }).trim();
  } catch {
    return BASELINE;
  }
}

function main() {
  const write = process.argv.includes('--write');
  const from = since();
  const commits = parseCommits(
    git('log', '--format=%H%x00%s%x00%b%x1e', '--no-merges', `${from}..HEAD`),
  );
  // The site is deployed by Vercel on its own and carries no version, so its
  // commits are dropped before anything else looks at them: they decide no
  // bump and they write no changelog line. See `OFF_APP_SCOPES`.
  const app = commits.filter(changesTheApp);
  const current = JSON.parse(fs.readFileSync(path.join(ROOT, 'package.json'), 'utf8')).version;
  const bump = decideBump(app, current);

  if (bump === null) {
    console.log(`Nothing to release: no feat, fix, perf or revert to the app since ${from}.`);
    report({ releasing: 'false' });
    return;
  }

  const version = bumpVersion(current, bump);
  const entry = renderEntry(version, app, {
    date: new Date().toISOString().slice(0, 10),
    repository:
      process.env.GITHUB_SERVER_URL &&
      process.env.GITHUB_REPOSITORY &&
      `${process.env.GITHUB_SERVER_URL}/${process.env.GITHUB_REPOSITORY}`,
  });

  console.log(
    `${current} → ${version} (${bump}, from ${app.length} of ${commits.length} commits since ${from})`,
  );
  console.log();
  console.log(entry);

  if (write) {
    writeVersion(version);
    writeChangelog(entry);
    // The website's changelog page is generated from the markdown, so it is
    // written in the same breath rather than left for somebody to notice.
    buildPage();
  }

  report({ releasing: 'true', version, tag: `v${version}`, bump });

  if (process.env.GITHUB_OUTPUT) {
    fs.appendFileSync(process.env.GITHUB_OUTPUT, `notes<<CHIEF_EOF\n${entry}\nCHIEF_EOF\n`);
  }
}

function report(outputs) {
  if (!process.env.GITHUB_OUTPUT) return;

  for (const [key, value] of Object.entries(outputs)) {
    fs.appendFileSync(process.env.GITHUB_OUTPUT, `${key}=${value}\n`);
  }
}

if (process.argv[1] === fileURLToPath(import.meta.url)) main();
