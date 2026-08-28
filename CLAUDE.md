# CLAUDE.md

Guidance for Claude Code (and any other agent or contributor) working in this repository.

## What this is

**Chief** is a privacy-first, on-device AI chief of staff: a desktop app that answers questions
about your work by reading your tools, keeping a local work log, and reasoning with a local LLM.

## The one non-negotiable rule

**Everything runs on the user's machine.** There is no cloud backend, no remote LLM, and no proxy
server. Concretely:

- Inference goes to a llama.cpp server Chief bundles, starts and stops itself, on loopback at
  `http://127.0.0.1:11435`. Never to a hosted model.
- All persistence is a local SQLite database in the OS app-data directory.
- OAuth is PKCE, done from the desktop app itself. There is no server to exchange codes.
- The only outbound traffic permitted is (a) `localhost`, (b) direct calls to a SaaS API the user has
  explicitly connected (e.g. `api.github.com`), made from Rust with that user's token, and (c) the
  one-off download of the model weights from `huggingface.co`, which carries no token, no cookie and
  nothing about the user. That third one lives in `src-tauri/src/weights.rs` and nowhere else.
- Telemetry, crash reporting and analytics are out of scope. Do not add them.

If a change would send user data anywhere else, it is wrong — stop and raise it instead.

## Stack

| Layer    | Choice                                                    |
| -------- | --------------------------------------------------------- |
| Shell    | Tauri v2                                                  |
| Frontend | React 18, TypeScript, Vite, Tailwind CSS v4, shadcn/ui    |
| Backend  | Rust                                                      |
| LLM      | Bundled `llama-server` (llama.cpp), OpenAI chat API       |
| Database | SQLite via `@tauri-apps/plugin-sql`                       |
| Vectors  | `sqlite-vec`, or cosine similarity in Rust for the MVP    |
| Auth     | Local PKCE OAuth via deep link (`chief://oauth/callback`) |

## Commands

```bash
pnpm install              # install (pnpm is the package manager — do not use npm/yarn)
pnpm engine:fetch         # download the bundled llama.cpp server for this machine
pnpm tauri:dev            # run the desktop app with hot reload
pnpm dev                  # run the frontend alone in a browser
pnpm check                # format:check + lint + typecheck + test — run before every commit
pnpm test:watch           # vitest in watch mode
pnpm test:e2e             # layout tests in a real browser (builds first; not part of `check`)
pnpm build                # typecheck + build the frontend bundle
pnpm tauri build --no-bundle   # compile the desktop binary without packaging installers
pnpm rust:fmt             # cargo fmt
pnpm rust:lint            # cargo clippy -D warnings
pnpm rust:test            # cargo test
pnpm verify               # everything CI runs, on this machine — before every push
```

### Verifying without CI

`pnpm verify` is one command per CI job, in the same order, cheapest first, so a branch can be
taken as far as CI would take it without a runner. Each job can be run on its own while working on
that part:

| Command                | CI job               | Roughly                    |
| ---------------------- | -------------------- | -------------------------- |
| `pnpm verify:commits`  | Conventional Commits | instant                    |
| `pnpm verify:frontend` | Frontend             | ~1 min                     |
| `pnpm verify:layout`   | Layout               | ~35 s                      |
| `pnpm verify:rust`     | Rust                 | ~4 min                     |
| `pnpm verify:app`      | App build            | ~7 min cold, far less warm |

Two things it cannot cover. `verify:app` builds for **this** machine only, so the platforms you are
not sitting at are unverified until CI builds there — the Rust is portable but the
WebKitGTK/WebView2/WKWebView differences are not. And the PR _title_ is linted by CI rather than by
commitlint here; `verify:commits` checks the commit messages the title is usually taken from.

Anything that builds or runs the desktop app needs `llama-server` on disk first, which is why
`tauri:dev`, `tauri:build` and `verify:app` all run `pnpm engine:fetch`. It is idempotent — a rerun
with the pinned build does nothing — and CI runs it as part of setting a job up.

### What CI does with its minutes

Runner time is the one cost this project has, so the workflows are written to spend it once.

- The setup every job repeats lives in `.github/actions/`, not in each job.
  `setup-node` is corepack, Node with a pnpm store cache, and `pnpm install`; `setup-tauri` is that
  plus the WebKitGTK toolchain on Linux, a Rust toolchain, a cargo cache and the llama.cpp engine.
  A new step that more than one job needs belongs in one of those two.
- **Cancel superseded runs, keep `main`.** Pushing again to a pull request cancels the run it
  replaced; a run on `main` is the record that a merged commit is good, so it is left to finish.
- **Cache anything downloaded twice** — the pnpm store, the cargo registry and target directory,
  and Chromium for the layout tests.
- **Build where nothing else is looking.** A pull request builds the app on macOS and Windows, and
  not on Linux: the Rust job already compiles the whole crate there, but `#[cfg(windows)]` code is
  compiled on Windows and nowhere else — `engine.rs:493` is where the last two fixes on `main`
  went. `main` adds Linux, where the app build is the only job that links a release profile
  against WebKitGTK. macOS bills at 10× a Linux runner and Windows at 2×, so this is most of what
  CI costs; it buys the only proof that the platform-conditional code compiles at all.
- **Don't build it at all when the change can't reach it.** The `changes` job spends a Linux
  minute working out whether a pull request touches `src-tauri/`, `scripts/`, the manifests or CI
  itself, and the app build is skipped when it does not. A change under `src/` is proved by the
  Frontend job's `pnpm build`; it cannot break platform-conditional Rust. Everything that is not a
  pull request builds unconditionally.
- **A pull request builds `--debug`.** It has one question to answer — does this compile and link
  on a platform nothing else compiles it on — and optimisation is not part of it. The release
  profile is proved on `main` and again when a release is cut.
- **Compile a dependency once per platform.** CI's app build and Release share one cargo cache per
  platform — `shared-key: tauri-<platform>` — so cutting a release restores what `main` already
  built instead of starting from nothing. Two things keep that working: both workflows set the
  same `CARGO_TERM_COLOR`, because rust-cache hashes every `CARGO_*` and `RUST*` variable into the
  key; and the debug builds pull-requests do are kept in a separate `tauri-dev-<platform>` cache,
  so a branch cannot evict what a release restores from. Chief's own crates are never cached, only
  its dependencies.
- **One dependency pull request a month, not twenty.** Every bump touches a lockfile, which is
  exactly what makes the app build run, so Dependabot groups minor and patch updates per ecosystem
  and runs monthly. Majors stay on their own — a batch that has to be reverted for one breaking
  change takes the rest with it — and security updates ignore the schedule entirely.

Building the desktop app on Linux needs the WebKitGTK toolchain:

```bash
sudo apt-get install -y libwebkit2gtk-4.1-dev build-essential curl wget file \
  libxdo-dev libssl-dev libayatana-appindicator3-dev librsvg2-dev patchelf
```

## Layout

```
src/                     React frontend
  components/            Layout, Sidebar, and view components
    ui/                  shadcn/ui primitives (generated — edit sparingly)
    views/               One component per top-level destination
  lib/                   Shared helpers (cn, navigation model)
  styles/globals.css     Tailwind entry point and design tokens
  test/setup.ts          Vitest + Testing Library setup
e2e/                     Layout tests driven through a real browser
scripts/                 Build-time tooling (fetching the llama.cpp engine)
src-tauri/               Rust backend
  src/lib.rs             Tauri builder — plugins and command registration
  src/main.rs            Desktop entry point
  binaries/              The fetched llama-server and its libraries (gitignored)
  capabilities/          Tauri permission scopes
  tauri.conf.json        Window, bundle and CSP configuration
```

## Data layer

The SQL plugin owns the connection pool for `sqlite:chief.db`, which resolves to a file in the OS
app-config directory. `plugins.sql.preload` in `tauri.conf.json` makes the plugin connect and
migrate at startup, so the pool is ready before the first command runs.

- Migrations live in `src-tauri/src/db.rs` and are **append-only**: once a version has shipped, add
  a new `Migration` rather than editing an existing one, or installed copies drift from the schema.
- `db::pool(&app)` borrows the plugin's pool. Never open a second pool on the same file.
- Query functions take `&SqlitePool` so they can be tested against an in-memory database; the
  `#[tauri::command]` wrappers stay thin. See `src-tauri/src/work_log.rs` for the pattern.
- The renderer is **not** granted the `sql:*` permissions, so it cannot run arbitrary SQL. All
  database access goes through typed commands. If direct queries are ever wanted, add `sql:default`
  to `src-tauri/capabilities/default.json` — deliberately, not by reflex.

## The engine

`src-tauri/src/engine.rs` owns the `llama-server` process. Chief ships llama.cpp rather than asking
the user to install a runtime, which is the whole reason the engine is a module rather than a URL.

- `scripts/fetch-llama-server.mjs` puts a pinned CPU build in `src-tauri/binaries/`:
  `llama-server-<host triple>` for `externalBin`, and its shared libraries in `lib/` for
  `bundle.resources`. Only CPU builds — a binary that runs on a machine with no GPU and no AVX-512,
  picking the best instruction set it finds at run time, is the point.
- `Engine::discover` finds that binary next to the app executable (where Tauri puts a sidecar, in
  both a release install and `tauri dev`), then a `llama-server` on `PATH`, then gives up and says
  so through the setup screen.
- `llama-server` finds its libraries by `$ORIGIN`, which works where the sidecar and the resources
  land in the same directory — Linux and Windows — but not on macOS, where the bundle splits
  `Contents/MacOS` from `Contents/Resources`. `library_dirs` therefore sets the platform's loader
  path explicitly. A debug build also looks in `src-tauri/binaries/lib`, so the source tree works
  before anything has been bundled.
- `ensure_running` is idempotent and never starts a second server: a health check comes first, so a
  `tauri dev` reload or a server the user is running themselves is left alone.
  `CHIEF_LLAMA_BASE_URL` points Chief at somebody else's server, and marks it not-ours to start or
  kill.
- The process is killed on `RunEvent::Exit`. Nothing else would stop it, and a resident model holds
  a couple of gigabytes after the window has gone.
- The server's **stderr is read rather than inherited**, and the last twenty lines are kept. A
  server that dies on the way up is quoted, not guessed at: Chief used to report every early exit as
  the model not fitting in this machine's memory, and on macOS 12 — where the bundled build wants a
  LAPACK symbol that arrived in 13.3 — the real answer was on that stream the whole time, going to a
  terminal nobody installing the app ever sees. Every line read is written straight back out, so
  `tauri dev` still shows the model loading.
- Two things llama.cpp makes unnecessary that Ollama needed. The **context window** is a launch flag
  (`--ctx-size`), not a per-request option, so no request can evict the weights and there is no
  keep-alive to negotiate — the server owns one model for its lifetime. And it **warms itself** as
  it starts, so there is no warm-up to schedule.
- `--jinja` is not optional: tool calling goes through the model's own chat template, which
  llama.cpp only applies in Jinja mode.

## Agent layer

`src-tauri/src/llama.rs` is the only place that speaks HTTP to a model.

- [`Client`] **refuses any base URL that is not loopback**, so a misconfiguration cannot turn into a
  hosted model reading the user's work. Proxies are disabled on the client for the same reason.
- `llama-server` speaks the OpenAI chat completions contract, so that is what this module models:
  `POST /v1/chat/completions`, tool calls whose `arguments` are a JSON **string**, and results
  returned under the `tool_call_id` that asked for them. The `tools` array is omitted from the
  payload when empty.
- Two shapes have to be tolerated on the way in, because llama.cpp emits both: a `content` of `null`
  where the model said nothing but a tool call, and `arguments` as an object rather than a string on
  some template paths. Both are normalised at the edge so nothing downstream has to care.
- `src-tauri/src/agent.rs` owns the system prompt and drops any `system` turn sent by the renderer —
  how the agent is instructed is not the frontend's to change.
- `agent::respond` is the orchestration loop: ask, run any `tool_calls`, append each result as a
  `tool` message, repeat until the model answers in words. It is bounded by `MAX_TOOL_ROUNDS` so a
  model that will not stop calling tools cannot spin forever.
- Answers **stream**. `Client::chat_stream` reads the server-sent events `llama-server` writes and
  `respond` forwards each piece to the window on the `agent-stream` event, tagged with the
  `requestId` the renderer generated. The finished answer is still returned from `ask_agent`, so a
  dropped event costs a frame and nothing more. A model this size writes at reading speed but takes
  tens of seconds to finish, and waiting for all of it before showing any of it is what made the app
  feel broken.
- A streamed **tool call arrives in fragments**: the name once, then the arguments a few characters
  at a time, with `index` saying which call each fragment belongs to. `PartialToolCalls` folds them
  back together, which is why a server that sends a whole call in one event and one that dribbles it
  out both work.
- An `Update` is one of three things: `delta` (append this), `restart` (what was shown turned out to
  be preamble to a tool call — discard it) or `tool` (a tool is running, so the wait has a reason to
  show).
- `llama::Options` rides in the request body, OpenAI-style. `max_tokens` is the ceiling on how long
  a question can take; `temperature` is low because these answers are about what the tools returned.
  Nothing a request carries can change how the engine itself is running.
- `agent::Attention` counts the questions the user is waiting on. The engine decodes one request at
  a time, so the daemon reads this and steps aside rather than putting a background summary ahead of
  a person.
- `src-tauri/src/tools.rs` holds the catalogue and the dispatcher. A tool failure — bad arguments, an
  unknown name — is reported back to the _model_ as an `error` payload, not raised to the user: it
  can then explain itself or try something else instead of collapsing the conversation.
- Add a tool by writing its schema in `catalog()` and its arm in `dispatch()`. Keep the two in step
  via a shared name constant.
- Errors are user-facing: an engine that is not running, or one still reading the weights, says so
  in those words rather than surfacing a transport error.

## Telling the model what day it is

`src-tauri/src/clock.rs` prefixes every question with the current local date and time.

- A model has no clock: asked what shipped "last week" it answers against whenever its training data
  ended. The prompt therefore states today's date and spells out the ranges — this week, last week,
  the last 7 days, this month — because a 3B model does not do date arithmetic reliably.
- The clock read is `Local`, not UTC: "today" means the user's today.
- It is worked out per question, so an app left open overnight does not still think it is yesterday.
- `describe` is generic over the time zone, so the ranges are tested at a fixed offset rather than
  against whatever clock the test machine keeps.

## Integrations

`src-tauri/src/github.rs` is the only code allowed to reach a host that is not this machine, and
only because the user connected the account. Requests carry the user's own token and go straight
from here to GitHub.

- Sign-in uses the **device flow**, not authorization code + PKCE. GitHub still requires a client
  secret to exchange an authorization code — PKCE protects the code but does not replace the secret,
  and GitHub "does not distinguish between public and confidential clients". A secret shipped inside
  a desktop binary is not a secret, so the device flow, which needs none, is the only honest option.
- The client id comes from `CHIEF_GITHUB_CLIENT_ID` at run time, falling back to build time. It is
  **not a secret** — a device-flow client id is public by design, which is why release builds bake
  one in from the repository _variable_ of the same name and nobody installing Chief has to register
  anything. The run-time override exists for development against your own OAuth app.
- `Client::against` — the constructor that points at another host — is `#[cfg(test)]`, so a release
  build cannot be aimed anywhere but GitHub.
- Chief requests the `repo` scope. That is read _and_ write across public and private
  repositories, which is broader than it needs — but GitHub gives OAuth apps no read-only scope for
  private repositories, and `repo:status` grants no pull request access at all. A GitHub App with
  fine-grained permissions is the way to narrow this.
- Expiring user access tokens are supported. `src-tauri/src/session.rs` renews a rejected token and
  retries once, storing the rotated pair. GitHub requires a client secret to refresh _unless_ the
  token came from the device flow — which is how Chief signs in, so no secret is involved.
- Every GitHub read goes through `Session`, not `Client` directly, so renewal is not something each
  caller has to remember.
- Tokens live in `integrations`, one row per service; reconnecting replaces the row. They are stored
  as plain text in the local database, protected by the OS user account rather than by encryption.
  Moving them to the OS keychain would be a genuine improvement.
- `tools::Context` carries the pool and the GitHub client, so tools are testable against an
  in-memory database and a stub server rather than a live Tauri app.

## Background daemon

`src-tauri/src/daemon.rs` keeps the work log current: ask GitHub what the user merged, ask the local
model to turn each merge into a one-sentence achievement, and write it to `work_logs`.

- It runs shortly after launch and then on an interval. A failing pass is never fatal — GitHub may
  be unreachable or the engine may still be loading — so it reports and tries again next time.
- Every pass is **idempotent**: entries carry the pull request's identifier in `external_id`, and
  the unique index added in migration v2 means the same merge is never logged twice. Entries the
  user writes by hand have no `external_id`, which is why that index is partial.
- Work already in the log is skipped _before_ the model is asked, so a caught-up pass costs nothing.
- A pass stops between items when `Attention` says the user is waiting on an answer. The rest keeps
  until the next pass; their question is worth more than the log being current.
- If summarising fails, nothing is written for that item. Writing an unsummarised row would mean it
  is never revisited, since the dedupe key would already be present.
- `run_once` takes a `Context` rather than an `AppHandle`, so a whole pass runs in tests against an
  in-memory database and stub GitHub and engine servers.

## First-run setup

`src-tauri/src/setup.rs` answers one question for the frontend: can this machine answer anything
yet? `check_readiness` reports whether the weights are on disk and what `/health` says — `ready`,
`loading`, or `down`. `download_model` fetches the weights and then starts the engine on them,
forwarding progress to the renderer as `model-download-progress` events; `start_engine` starts one
and waits until it can answer.

- `App` renders `SetupView` instead of the shell until the model is here and the engine is ready,
  with a "Skip for now" escape.
- There is only one thing left to install, because the engine ships with the app. That is the whole
  benefit of llama.cpp over Ollama here: a gigabyte-scale runtime with its own GPU stack and system
  service could not be bundled, and a 30 MB CPU server can.
- `loading` is a state, not a failure. The setup screen shows it as "Starting…" and re-checks itself
  until it resolves, because reading a couple of gigabytes takes a few seconds and telling the user
  it failed would be wrong.

## Model weights

`src-tauri/src/weights.rs` owns the GGUF file: where it belongs, and how to get it the first time.

- The download is **the only outbound request in the app that is not to a host the user connected**.
  It goes to one pinned HTTPS URL on `huggingface.co` with no token, no cookie and no body. Every
  redirect hop is required to stay on HTTPS.
- It **resumes**. Two gigabytes is too much to throw away because a connection dropped, so a partial
  download lands in a `.part` file beside the destination and a retry sends a `Range` header. A host
  that ignores the range answers 200 rather than 206, and the part file is started over rather than
  appended to.
- The finished file is checked for GGUF's magic bytes before it is renamed into place. A login wall,
  an error page or a truncated transfer saved under the model's name would otherwise be handed to
  the engine, which would fail in a much less obvious way.
- Progress is reported every few megabytes, not every chunk: a progress event per packet is
  thousands a second and tells the user nothing more.

## The design system

**Chief design system 1.0 — "Instrument".** Software that will be read by a security team should
look like an instrument, not a campaign. The whole system lives in `src/styles/globals.css` plus a
handful of components; nothing here is decorative, so a change to one of these numbers is a change
to the system rather than to one screen.

- **Four principles.** On-device is the headline, stated in words rather than implied by a padlock.
  Declarative, never chatty. Instrument, not poster — flat fields, hairline rules, one shadow level,
  no gradients. Always show the work.
- **Five colours.** Graphite `#14171A`, paper `#F4F5F3`, slate `#2C5C7A`, verified `#3F6B4F`,
  attention `#B8761F`. Grey does everything else. Both token sets are complete and carry equal
  weight; `index.html` ships with `class="dark"` on `<html>`, and removing it gives the light one.
  Amber is the only colour that changes role between themes: on paper it fills and `#8A5510` carries
  the text, on graphite `#D9903A` does both — which is why every signal colour has a `-surface` and
  a `-text` token beside it. Chips are always a tinted fill with dark text, never solid colour
  behind 11px type.
- **Two faces, bundled.** Instrument Sans for everything, IBM Plex Mono for machine facts — ports,
  models, codes, paths, timestamps, counts, never prose. They come from `@fontsource*` packages and
  are served from the app itself: the CSP allows `font-src 'self'`, and a design system that phones
  a font CDN on launch would break the one rule this app has. An e2e test asserts the app fetches
  nothing off its own origin.
- **Sizes are fixed.** Type is 10, 12, 14, 16, 20 — nothing between and nothing above, so the
  largest type in the product is a 20px headline in sentence case. The only uppercase is the 10px
  mono micro-label (`.micro`) at 0.1em tracking. Radius is 4 chips / 6 buttons and fields / 8 cards
  / 12 windows, which falls out of `--radius: 0.5rem`. No pills.
- **The shell** is a 56px icon rail, a 48px header, and a detail measure that stops at 680px. Rail
  items are 32px tiles with 15px icons and no labels — the name arrives as a tooltip after 500ms,
  which is a CSS transition delay rather than the platform's own `title` timing. The header says
  where the data is on every screen. `e2e/shell.spec.ts` measures all of this in a real browser,
  because jsdom reports every height as zero.
- **Four things may move, and only while work is in flight** (`src/components/ui/activity.tsx`).
  The mark turns half a revolution over 2.4s once a question is dispatched; a 3px slate hairline
  sweeps every 1.4s while a tool runs, captioned with the step actually running; a 1px caret sits at
  the live end of the text while it is written; three dots mark a control being waited on.
  Indicators are slate or green, never amber — amber means _you_ are needed, and a machine working
  is not that. Only the model download knows a total, so it is the only bar that fills.
- **A slow machine says so.** Past three seconds the caption gains elapsed time; past fifteen it
  gains a sentence saying nothing has stalled. Silence is the thing to avoid.
- **`prefers-reduced-motion` swaps every loop for a static dot and the same caption**, via the
  `.motion-loop` / `.motion-still` pair in `globals.css`. It is CSS rather than React so no
  component has to know about it.

The design canvas this was built from also specifies three platform title bars and a 240px list
pane between the rail and the detail. Neither is here: the title bar is still the platform's own,
and the list pane needs per-view content the app does not have yet. Both are additions to the
shell rather than changes to it.

## Conventions

**TypeScript**

- Strict mode, `verbatimModuleSyntax`, `exactOptionalPropertyTypes`. Import types with
  `import { type Foo }`.
- Import app code through the `@/` alias, not deep relative paths.
- Components are named exports; `App` is the one default export.
- Never call `fetch` from the renderer — ESLint blocks it. Network access lives in Rust behind a
  Tauri command.

**Rust**

- `cargo clippy -D warnings` is enforced in CI; keep it clean rather than sprinkling `#[allow]`.
- Tauri commands return `Result<T, String>` (or a typed error) so the frontend can surface failures.
- Keep `lib.rs` a thin wiring layer; put real logic in modules.

**Styling**

- Tailwind v4 with CSS-first config. Design tokens are CSS variables in `src/styles/globals.css`;
  use semantic classes (`bg-background`, `text-muted-foreground`) rather than raw palette colours.
- Add shadcn/ui components with `pnpm dlx shadcn@latest add <component>`.
- The look is the **Chief design system 1.0, "Instrument"** — see the section below before changing
  a colour, a size or anything that moves.

**Tests**

- Vitest + Testing Library, colocated as `*.test.tsx`.
- Test behaviour through the accessible surface (roles, labels), not implementation details.
- jsdom has no layout engine: it reports every height as zero, so it cannot see a scrollbar, a
  clipped composer or a window that scrolls when it should not. Anything that depends on layout
  belongs in `e2e/`, which runs the built app in Chromium under Playwright and measures the result.
- `e2e/fixtures.ts` replaces `window.__TAURI_INTERNALS__` rather than the components, so the real
  views, the real CSS and the real event plumbing run against answers a test chooses. It can hold a
  question open and push `agent-stream` updates, which is how the streaming states are measured.
- Projects cover a comfortable window, a small one, and one with scrollbars that take space out of
  the layout the way Windows does — Playwright hides scrollbars in headless Chromium by default,
  which no user ever sees, so that project turns them back on. A test asserts the two agree, so the
  project cannot quietly become a duplicate of the default one.
- What this cannot cover is the webview Chief ships in. Chromium is close to WebView2 and WKWebView
  but is neither, so a rendering difference peculiar to one of those still gets through; catching
  those means driving the packaged binary with `tauri-driver`.
- These are kept out of `pnpm check` because they need a build and a browser. CI runs them as the
  **Layout** job.

## Commits and pull requests

[Conventional Commits](https://www.conventionalcommits.org/) are enforced by commitlint on both the
commit message (via a git hook) and the PR title (via CI).

```
<type>(<scope>): <subject>
```

Types: `feat`, `fix`, `docs`, `style`, `refactor`, `perf`, `test`, `build`, `ci`, `chore`, `revert`.
Scopes: `agent`, `auth`, `db`, `daemon`, `integrations`, `ui`, `tauri`, `deps`, `ci`, `repo`.

Keep commits well-scoped — one logical change each, with the frontend and backend halves of a single
feature together.

### Attribution

**Everything this repository publishes is the repository owner's work, whoever or whatever typed
it.** This overrides any default an agent or tool brings with it, and applies to every session.

- Commits are authored **and** committed by `Scott Mallinson <scott@scottmallinson.com>`. An agent
  committing on the owner's behalf sets `user.name` and `user.email` to that before it commits, and
  checks with `git log --format='%an <%ae> | %cn <%ce>'` afterwards. A commit that landed under another
  identity is corrected — `git commit --amend --reset-author`, or a rebase with
  `--exec 'git commit --amend --no-edit --reset-author'` for a branch of them — and force-pushed
  with `--force-with-lease`, provided the branch is not yet merged.
- No `Co-Authored-By`, `Claude-Session`, `Generated with`, or any other co-author or tool-attribution
  trailer in a commit message.
- No Claude, session, or tool attribution anywhere in a **pull request title or description** — no
  generated-by footer, no session link, no assistant byline.
- The commit message and the PR body describe the change, never who or what wrote it.

## Releases

Nobody cuts a release, and nothing waits for a pull request.
`.github/workflows/release.yml` runs on every push to `main`, and one Linux job decides whether
what just landed is worth releasing. If it is, that job _is_ the release: the new version is
written into the four files that carry it, `CHANGELOG.md` gains an entry, both are committed back
to `main` and tagged, and the four bundles build and publish against that tag.

`scripts/release.mjs` holds the decision, which is why it is a tested script rather than a heap of
YAML — there is no human between it and a published release. `pnpm test` covers it.

- **A `feat`, `fix`, `perf` or `revert` releases. Nothing else does.** A `docs`, `ci`, `chore`,
  `style`, `test` or `refactor` commit changes nothing a person can download, and a release is four
  bundles — two of them macOS at 10× a Linux runner, so on the order of 200 billed minutes. Those
  commits neither cause a release nor appear in one.
- **The version is derived, never chosen.** A `feat` is a minor and anything else releasable is a
  patch. A breaking change — `feat!:` or a `BREAKING CHANGE:` footer — is a major, except before
  1.0.0, where it is a minor: a project that is not finished should not be forced to call itself
  1.0 by its first breaking change.
- **Four files carry the version** — `package.json`, `src-tauri/tauri.conf.json`,
  `src-tauri/Cargo.toml` and `src-tauri/Cargo.lock` — and the script rewrites exactly one version
  string in each, failing if it finds none or several. A test asserts each pattern still matches
  its real file, so reformatting one of them breaks a test rather than a release.
- **The release commit starts nothing.** It is pushed with `GITHUB_TOKEN`, and GitHub deliberately
  raises no workflow runs for those — so it cannot loop back into this workflow, and it does not
  spend another full CI matrix on `main`.
- **Drafted, filled, then published.** The release is created as a draft so nobody is told about a
  release they cannot download; the `publish` job takes it out of draft once every bundle is
  attached. If that job never runs, the release sits there as a draft with its assets and one click
  finishes it. That is the failure this is shaped around.
- **The first release needs a starting point.** With no `v*` tag to measure from, the script reads
  from the `BASELINE` commit rather than summarising the entire history.
- A branch protection rule that forbids pushing to `main` would stop this: the release commit goes
  straight to `main`, by design.
- `CHANGELOG.md` is in `.prettierignore`. It is generated, and a formatting check failing on a
  release commit would block releasing entirely.

## Roadmap

Build strictly in order, and stop for review at each step:

1. **Project scaffolding & UI foundation** — Tauri v2 + React + Vite, Tailwind, app shell. ✅
2. **SQLite local database** — `@tauri-apps/plugin-sql`, migrations for `work_logs` and
   `integrations`, Tauri commands to read/write the log. ✅
3. **Local LLM engine** — bundled `llama-server`, Rust client posting to `/v1/chat/completions`,
   `ask_agent` command. ✅
4. **Tool calling orchestrator** — `fetch_github_prs` schema, intercept `tool_calls`, feed results
   back to the model. ✅
5. **GitHub sign-in** — device flow from the desktop app, token stored in `integrations`, and
   `fetch_github_prs` reading real data. ✅
6. **Background daemon & work log** — periodic fetch, summarise locally, write to `work_logs`. ✅

Do not start a later step before the earlier one is reviewed and merged.
