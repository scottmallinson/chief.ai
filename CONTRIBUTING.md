# Contributing

Contributions are welcome — bug reports, fixes, integrations, documentation, and the awkward
questions that turn into either. This file is everything you need to get a change from a clone to a
merge, and it tries to be honest about the parts that are unusual here.

One thing to know before you read any further: **Chief runs entirely on the user's machine, and
that is not negotiable.** A change that would send a user's data anywhere else will not be accepted,
however good it is otherwise. [The privacy bar](#the-privacy-bar) says what that means in practice,
and it is worth reading before you build something substantial.

## Before you start

**Open an issue first for anything larger than a fix.** Not as paperwork — the roadmap is kept
privately (see [Where the roadmap lives](#where-the-roadmap-lives)), so a feature you build in
isolation may already be designed, deliberately deferred, or blocked on something you cannot see.
An issue costs you a paragraph and can save you a weekend.

Fixes, tests, documentation and anything clearly self-contained need no issue. Open the pull
request.

If you want something to pick up, the [issues list](https://github.com/scottmallinson/chief.ai/issues)
is the public queue. Anything labelled `good first issue` or `help wanted` is known to be
self-contained and is not waiting on a decision.

## Setup

```bash
corepack enable          # pnpm is the package manager — not npm, not yarn
pnpm install
pnpm tauri:dev           # fetches the llama.cpp engine, then runs the app
```

Platform prerequisites are in the [README](README.md#requirements). `pnpm dev` runs the frontend
alone in a browser at http://localhost:1420, which is enough for most UI work and needs no Rust
toolchain at all.

**First run downloads a model.** The app walks you through it; the weights are a couple of
gigabytes and land in your OS app-data directory, not in the repository. If you already run a
`llama-server` yourself, point Chief at it with `CHIEF_LLAMA_BASE_URL` (loopback addresses only)
and it will use that instead of starting its own.

**You do not need an OAuth app** unless you are working on GitHub sign-in specifically. Released
builds have a client id compiled in; a build from source does not, so sign-in will tell you it has
none. `.env.example` explains how to register your own — it takes two minutes and there is no
secret involved.

## Before you push

One command runs everything CI runs, in the same order, quickest first:

```bash
pnpm verify
```

That takes a while the first time. While you are working, run the job that covers what you touched:

| Command                | Covers                                        | Roughly                    |
| ---------------------- | --------------------------------------------- | -------------------------- |
| `pnpm verify:commits`  | Your commit messages                          | instant                    |
| `pnpm verify:frontend` | Format, lint, typecheck, unit tests, bundle   | ~1 min                     |
| `pnpm verify:layout`   | Playwright layout tests, in a real browser    | ~35 s                      |
| `pnpm verify:rust`     | `rustfmt`, clippy with warnings denied, tests | ~4 min                     |
| `pnpm verify:app`      | The desktop app compiles and links            | ~7 min cold, far less warm |

`pnpm fix` formats and auto-fixes most of what `verify:frontend` would complain about. Run it
before `verify`, not after.

Two things `pnpm verify` cannot cover, so do not be surprised by them:

- **It builds for your machine only.** The Rust is portable; the WebKitGTK, WebView2 and WKWebView
  differences are not. CI builds on macOS and Windows for exactly this reason.
- **The pull request title is linted by CI rather than by your hooks.** `verify:commits` checks the
  commit messages the title is usually taken from, which is close but not the same thing.

Git hooks do a subset of this automatically: `pre-commit` runs `lint-staged` (ESLint, Prettier and
`rustfmt` over staged files) and checks the migration versions, and `commit-msg` validates the
message. They are installed by `pnpm install`.

## Commit messages

[Conventional Commits](https://www.conventionalcommits.org/), enforced by commitlint on the commit
message locally and on the pull request title in CI:

```
<type>(<scope>): <subject>
```

**Types:** `feat`, `fix`, `docs`, `style`, `refactor`, `perf`, `test`, `build`, `ci`, `chore`,
`revert`.

**Scopes** — the list is enforced, so a plausible-sounding scope that is not on it will fail the
hook:

| Scope          | What it covers                                           |
| -------------- | -------------------------------------------------------- |
| `agent`        | The llama.cpp client, the prompt, tool-calling           |
| `auth`         | OAuth flows and credentials                              |
| `corpus`       | The markdown corpus, its index, recipes and drafts       |
| `db`           | SQLite schema, migrations, queries                       |
| `daemon`       | The background work-log daemon                           |
| `integrations` | GitHub, Linear, Atlassian, calendars, and other sources  |
| `ui`           | React components, styling, layout                        |
| `tauri`        | The Tauri shell, capabilities, plugins, config           |
| `deps`         | Dependency bumps                                         |
| `ci`           | Workflows and automation                                 |
| `repo`         | Tooling, config, docs, meta                              |
| `website`      | The marketing site in `website/` — see the warning below |

**The type and the scope are not paperwork — they decide whether your change ships.** A `feat`,
`fix`, `perf` or `revert` merged to `main` cuts a release; everything else does not. And a change
to the site under `website/` must carry the `website` scope, whatever its type, because the release
script drops those commits: the site is deployed from `main` the moment it lands and has no version
to bump, so a site change scoped `ui` would tag a version and build three installers identical to
the last three.

Examples:

```
feat(db): add work_logs and integrations migrations
fix(agent): keep tool call ids stable across the response loop
fix(website): fit the header on a phone
chore(deps): bump tauri to 2.2.7
```

Keep each commit to one logical change, with the frontend and backend halves of a feature together.
Write the message to a file and commit with `-F` if the body runs to paragraphs — `-m` with a
shell-quoted string breaks on the first apostrophe and the failure looks like git rejecting the
commit rather than like a quoting mistake.

**Your commits stay yours.** You author them under your own name and email, and you keep the
copyright in your contribution under the [MIT licence](LICENSE) the project ships under. The one
thing the `commit-msg` hook refuses is a trailer crediting a _tool_ for the work — a
`Claude-Session:` line, a `Generated with …` footer, a `Co-Authored-By:` naming an AI assistant.
A human co-author is fine. The message describes the change, not what typed it.

## Pull requests

- **Fork the repository and branch from `main`.** Push to your fork and open the pull request from
  there; you do not need write access to this repository for anything.
- **A draft runs no CI.** That is deliberate: the slowest of the checks are two desktop app builds,
  and nobody reads a red draft. Run `pnpm verify` locally while the work is in progress, and **mark
  the pull request ready for review to get a full run**. `Checks / Complete` is a required check, so
  it has to report before the change can merge.
- **The title must be a valid conventional commit**, because CI lints it. It is not what decides the
  release, though — see below.
- **Your individual commits are what land on `main`.** A pull request is merged with a merge commit
  rather than squashed, so every commit on your branch keeps its own subject in the history. That
  is what `scripts/release.mjs` reads: it collects commits since the last tag with `--no-merges`, so
  it sees yours and never sees the pull request title. **A `chore` title does not stop a `feat`
  commit on the branch from releasing.** Get the type right on each commit, not just on the title.
- **Fill in the template**, including the privacy checklist. It is three boxes and it is the
  cheapest place to catch the one mistake this project cannot accept.
- **Push again rather than force-pushing over review history** where you reasonably can. It keeps a
  reviewer's comments anchored to the lines they were about.
- CI cancels superseded runs, so pushing again while a run is in flight costs nothing.

### What review looks for

Beyond "does it work":

- **Where the network call lives.** Never `fetch` from the renderer — ESLint blocks it. Outbound
  requests belong in Rust, behind a Tauri command, and only to `localhost` or a service the user
  explicitly connected.
- **Migrations are append-only.** Once a version has shipped, add a new `Migration` rather than
  editing an existing one, or installed copies drift from the schema. `scripts/check-migrations.mjs`
  refuses a duplicate or non-contiguous version.
- **A test that guards an invariant has been watched to fail.** Caps, boundaries, "never
  overwrites", "no network call", "nothing was written" — these pass on the day they are written
  whatever they assert. Break the thing the test guards, watch it fail, put it back, and put the
  failure message in the pull request. Three of them turned out to be asserting nothing at all.
- **Layout claims are measured in a browser.** jsdom reports every height as zero, so anything
  depending on layout belongs in `e2e/` rather than in a unit test.
- **The design system is a system.** Five colours, five type sizes, four radii, four things that may
  move. Changing one of those numbers is a change to the system rather than to one screen; say so
  in the pull request and expect it to be discussed on those terms.

Architecture notes, the reasoning behind most of the unusual decisions, and the conventions above
in much more detail live in [CLAUDE.md](CLAUDE.md). It is written for whoever is working in the
repository next, human or agent, and it is the best single explanation of why the code looks the
way it does.

### Where the roadmap lives

The build order, the open questions and the ledger of what is actually finished are kept in a
private tracker rather than in this repository. That is a deliberate split: a roadmap and a list of
half-finished ideas are working notes for whoever is building Chief, and a public repository is the
wrong place to publish them as though they were commitments.

The practical consequence for you is that **the issues list is the public queue, and an issue is the
way to find out whether something is already planned.** Ask. The answer is cheap to give and it is
not a brush-off.

## Releases

You do not cut one, and you do not need to ask for one. Merging a `feat`, `fix`, `perf` or `revert`
to `main` releases it: the version is derived from the commit types, the changelog is written, the
tag is pushed, and the installers are built and published. A `docs`, `ci`, `chore`, `style`, `test`
or `refactor` merge releases nothing, because it changes nothing anybody can download.

Do not bump versions, edit `CHANGELOG.md`, or push tags by hand. All three are outputs of
releasing, and a pull request that touches them will conflict with the release that would have
written them. `website/changelog.html` is generated too — edit `CHANGELOG.md`'s source of truth,
which is your commit message, or the generator in `scripts/`.

## The privacy bar

The whole claim of this project is that your work never leaves your machine, so this is the one area
where a pull request is refused on principle rather than on quality:

- **No hosted model, no relay, no proxy server.** Inference goes to the bundled `llama.cpp` server
  on loopback. The client refuses any base URL that is not loopback, and that refusal is a test.
- **No telemetry, analytics or crash reporting.** Not opt-in, not anonymised, not behind a flag.
- **Outbound traffic is `localhost`, a SaaS API the user explicitly connected (with that user's own
  token, from Rust), and the one-off model-weights download from Hugging Face** — which carries no
  token, no cookie and nothing about the user, and lives in `src-tauri/src/weights.rs` and nowhere
  else.
- **Persistence is the local SQLite database.** New state goes there, through a typed Tauri command;
  the renderer is not granted arbitrary SQL.

If a change you want to make seems to need one of those things, open an issue instead of a pull
request. There is usually another way, and working it out is more interesting than the workaround.

## Security

Do not open a public issue for a vulnerability. [SECURITY.md](SECURITY.md) has the private
reporting route, and the list of dependency advisories that are accepted deliberately — read it
before investigating an open Dependabot alert, because an alert with no pull request behind it
usually means there is no version to bump to.

## Code of conduct

By participating you agree to the [Code of Conduct](CODE_OF_CONDUCT.md).

## Troubleshooting

**`resource path binaries/llama-server-… doesn't exist`** — the engine is not on disk for the
target being built. `pnpm engine:fetch` fetches it; it is idempotent.

**A Tauri build fails with `No space left on device` part-way through a link.** A debug build of the
crate is several gigabytes and a release build more. `cargo clean --manifest-path src-tauri/Cargo.toml -p Chief`
frees most of it without discarding the dependency build.

**Compilation exhausts memory on a small machine.** Cap the parallelism for that build rather than
for the repository: `CARGO_BUILD_JOBS=1 pnpm verify:rust`.

**The app opens to a white screen.** Open the webview devtools and read the console. The usual cause
is syntax the test runners accept and the shipped webview does not — jsdom runs on V8 and Playwright
on Chromium, and neither is the WKWebView or WebView2 the app actually ships in. Prefer the boring
construction; `eslint.config.js` blocks the specific case that reached `main` this way.

**Ports 1420 or 11435 are already in use.** Both survive an unclean exit:
`lsof -ti:1420 -ti:11435 | xargs kill -9`.
