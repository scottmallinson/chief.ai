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
   `integrations`, Tauri commands to read/write the log.
3. **Local LLM engine** — Rust service posting to Ollama `/api/chat`, `ask_agent` command.
4. **Tool calling orchestrator** — `fetch_github_prs` schema, intercept `tool_calls`, feed results
   back to the model.
5. **Local PKCE OAuth** — GitHub authorization code + PKCE from the desktop app, token stored in
   `integrations`.
6. **Background daemon & work log** — periodic fetch, summarise locally, write to `work_logs`.

Do not start a later step before the earlier one is reviewed and merged.
