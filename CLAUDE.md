# CLAUDE.md

Guidance for Claude Code (and any other agent or contributor) working in this repository.

## What this is

**Chief** is a privacy-first, on-device AI chief of staff: a desktop app that answers questions
about your work by reading your tools, keeping a local work log, and reasoning with a local LLM.

## The one non-negotiable rule

**Everything runs on the user's machine.** There is no cloud backend, no remote LLM, and no proxy
server. Concretely:

- Inference goes to a local Ollama instance at `http://localhost:11434`. Never to a hosted model.
- All persistence is a local SQLite database in the OS app-data directory.
- OAuth is PKCE, done from the desktop app itself. There is no server to exchange codes.
- The only outbound traffic permitted is (a) `localhost`, and (b) direct calls to a SaaS API the
  user has explicitly connected (e.g. `api.github.com`), made from Rust with that user's token.
- Telemetry, crash reporting and analytics are out of scope. Do not add them.

If a change would send user data anywhere else, it is wrong — stop and raise it instead.

## Stack

| Layer    | Choice                                                    |
| -------- | --------------------------------------------------------- |
| Shell    | Tauri v2                                                  |
| Frontend | React 18, TypeScript, Vite, Tailwind CSS v4, shadcn/ui    |
| Backend  | Rust                                                      |
| LLM      | `reqwest` → local Ollama (`/api/chat`, tool calling)      |
| Database | SQLite via `@tauri-apps/plugin-sql`                       |
| Vectors  | `sqlite-vec`, or cosine similarity in Rust for the MVP    |
| Auth     | Local PKCE OAuth via deep link (`chief://oauth/callback`) |

## Commands

```bash
pnpm install              # install (pnpm is the package manager — do not use npm/yarn)
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
```

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
src-tauri/               Rust backend
  src/lib.rs             Tauri builder — plugins and command registration
  src/main.rs            Desktop entry point
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

## Agent layer

`src-tauri/src/ollama.rs` is the only place that speaks HTTP to a model.

- [`Client`] **refuses any base URL that is not loopback**, so a misconfiguration cannot turn into a
  hosted model reading the user's work. Proxies are disabled on the client for the same reason.
- The `tools` array is modelled in Ollama's function-calling format and is omitted from the payload
  when empty.
- `src-tauri/src/agent.rs` owns the system prompt and drops any `system` turn sent by the renderer —
  how the agent is instructed is not the frontend's to change.
- `agent::respond` is the orchestration loop: ask, run any `tool_calls`, append each result as a
  `tool` message, repeat until the model answers in words. It is bounded by `MAX_TOOL_ROUNDS` so a
  model that will not stop calling tools cannot spin forever.
- Answers **stream**. `Client::chat_stream` reads Ollama's newline-delimited reply and `respond`
  forwards each piece to the window on the `agent-stream` event, tagged with the `requestId` the
  renderer generated. The finished answer is still returned from `ask_agent`, so a dropped event
  costs a frame and nothing more. A model this size writes at reading speed but takes tens of
  seconds to finish, and waiting for all of it before showing any of it is what made the app feel
  broken.
- An `Update` is one of three things: `delta` (append this), `restart` (what was shown turned out to
  be preamble to a tool call — discard it) or `tool` (a tool is running, so the wait has a reason to
  show).
- `ollama::Options` is sent with every request. `num_predict` is the ceiling on how long a question
  can take. `num_ctx` is deliberately **identical for every request Chief makes**, including the
  daemon's: Ollama loads a model per context size, so varying it evicts the copy already in memory.
  `keep_alive` holds the model there for half an hour, because otherwise the next question pays to
  read the weights off disk again.
- `agent::warm_up` loads the model while the window is still opening, so the first question does not
  pay for it either. It fails silently — on a fresh machine Ollama may not be installed, which is
  what the setup screen is for.
- `agent::Attention` counts the questions the user is waiting on. Ollama answers one request at a
  time per model, so the daemon reads this and steps aside rather than putting a background summary
  ahead of a person.
- `src-tauri/src/tools.rs` holds the catalogue and the dispatcher. A tool failure — bad arguments, an
  unknown name — is reported back to the _model_ as an `error` payload, not raised to the user: it
  can then explain itself or try something else instead of collapsing the conversation.
- Add a tool by writing its schema in `catalog()` and its arm in `dispatch()`. Keep the two in step
  via a shared name constant.
- Errors are user-facing: an unreachable Ollama or a missing model says what to run, rather than
  surfacing a transport error.

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
  be unreachable or Ollama may not be running — so it reports and tries again next time.
- Every pass is **idempotent**: entries carry the pull request's identifier in `external_id`, and
  the unique index added in migration v2 means the same merge is never logged twice. Entries the
  user writes by hand have no `external_id`, which is why that index is partial.
- Work already in the log is skipped _before_ the model is asked, so a caught-up pass costs nothing.
- A pass stops between items when `Attention` says the user is waiting on an answer. The rest keeps
  until the next pass; their question is worth more than the log being current.
- If summarising fails, nothing is written for that item. Writing an unsummarised row would mean it
  is never revisited, since the dedupe key would already be present.
- `run_once` takes a `Context` rather than an `AppHandle`, so a whole pass runs in tests against an
  in-memory database and stub GitHub and Ollama servers.

## First-run setup

`src-tauri/src/setup.rs` answers one question for the frontend: can this machine answer anything
yet? `check_readiness` reports whether Ollama is up (`/api/version`) and whether the model is
installed (`/api/tags`); `pull_model` downloads it (`/api/pull`), streaming newline-delimited
progress that is forwarded to the renderer as `model-pull-progress` events.

- `App` renders `SetupView` instead of the shell until both are true, with a "Skip for now" escape.
- Ollama is not bundled: it is a gigabyte-scale install with its own GPU runtimes and system
  service. Detecting it and offering the download is the honest trade.
- Ollama reports a failed pull _inside_ a 200 response, so the stream parser treats an `error` field
  as a failure.

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

## Roadmap

Build strictly in order, and stop for review at each step:

1. **Project scaffolding & UI foundation** — Tauri v2 + React + Vite, Tailwind, app shell. ✅
2. **SQLite local database** — `@tauri-apps/plugin-sql`, migrations for `work_logs` and
   `integrations`, Tauri commands to read/write the log. ✅
3. **Local LLM engine** — Rust service posting to Ollama `/api/chat`, `ask_agent` command. ✅
4. **Tool calling orchestrator** — `fetch_github_prs` schema, intercept `tool_calls`, feed results
   back to the model. ✅
5. **GitHub sign-in** — device flow from the desktop app, token stored in `integrations`, and
   `fetch_github_prs` reading real data. ✅
6. **Background daemon & work log** — periodic fetch, summarise locally, write to `work_logs`. ✅

Do not start a later step before the earlier one is reviewed and merged.
