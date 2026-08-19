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
- `src-tauri/src/tools.rs` holds the catalogue and the dispatcher. A tool failure — bad arguments, an
  unknown name — is reported back to the _model_ as an `error` payload, not raised to the user: it
  can then explain itself or try something else instead of collapsing the conversation.
- Add a tool by writing its schema in `catalog()` and its arm in `dispatch()`. Keep the two in step
  via a shared name constant.
- Errors are user-facing: an unreachable Ollama or a missing model says what to run, rather than
  surfacing a transport error.

## Integrations

`src-tauri/src/github.rs` is the only code allowed to reach a host that is not this machine, and
only because the user connected the account. Requests carry the user's own token and go straight
from here to GitHub.

- Sign-in uses the **device flow**, not authorization code + PKCE. GitHub still requires a client
  secret to exchange an authorization code — PKCE protects the code but does not replace the secret,
  and GitHub "does not distinguish between public and confidential clients". A secret shipped inside
  a desktop binary is not a secret, so the device flow, which needs none, is the only honest option.
- The client id comes from `CHIEF_GITHUB_CLIENT_ID` at run time, falling back to build time. It is
  not a secret. Without it, sign-in fails with a message saying what to do.
- `Client::against` — the constructor that points at another host — is `#[cfg(test)]`, so a release
  build cannot be aimed anywhere but GitHub.
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
- If summarising fails, nothing is written for that item. Writing an unsummarised row would mean it
  is never revisited, since the dedupe key would already be present.
- `run_once` takes a `Context` rather than an `AppHandle`, so a whole pass runs in tests against an
  in-memory database and stub GitHub and Ollama servers.

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

## Commits and pull requests

[Conventional Commits](https://www.conventionalcommits.org/) are enforced by commitlint on both the
commit message (via a git hook) and the PR title (via CI).

```
<type>(<scope>): <subject>
```

Types: `feat`, `fix`, `docs`, `style`, `refactor`, `perf`, `test`, `build`, `ci`, `chore`, `revert`.
Scopes: `agent`, `auth`, `db`, `daemon`, `integrations`, `ui`, `tauri`, `deps`, `ci`, `repo`.

Keep commits well-scoped — one logical change each, with the frontend and backend halves of a single
feature together. Commits are authored by the repository owner; do not add co-author or
tool-attribution trailers.

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
