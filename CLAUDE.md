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
pnpm fix                  # format + lint --fix + rustfmt — run this before `check`, not after it
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
- **The checks live in one file, and both workflows call it.** `.github/workflows/checks.yml` is
  a `workflow_call` workflow holding Frontend, Layout, Rust and the app build. CI calls it on a
  pull request; Release calls it before it bundles anything. That is what makes "a release depends
  on CI being green" true by construction — a release runs the same jobs against the very commit
  it is about to ship, rather than trusting that some earlier run on some other commit was green.
- **A draft pays for nothing.** `ci.yml`'s single job is skipped while the pull request is a
  draft, which is the cheapest saving available: the checks it calls include the two paid
  platforms, and nobody reads a red draft. The author runs `pnpm verify` instead — the same jobs
  in the same order, on the machine they are already at. Two details make it work rather than
  quietly break things. `ready_for_review` has to be added to the trigger's `types`, because it is
  not one of the defaults and `Checks / Complete` is a required check: without it a pull request
  marked ready would sit forever waiting for a run that never starts. And the guard tests
  `github.event_name` as well as the flag, because `workflow_dispatch` carries no `pull_request`
  object, `null == false` is false, and gating on the flag alone would disable the manual trigger.
- **Cancel superseded runs.** Pushing again to a pull request cancels the run it replaced.
- **Cache anything downloaded twice** — the pnpm store, the cargo registry and target directory,
  and Chromium for the layout tests.
- **`main` is not built.** CI runs on pull requests only. A branch has to be up to date with
  `main` before it can merge, so the tree a pull request proves green is the tree the merge
  produces; running the same jobs again on the merge commit would pay twice for an answer already
  given. Nothing at all runs on a push to `main`.
- **Build where nothing else is looking.** The app is built on macOS and Windows, and not on
  Linux: the Rust job already compiles the whole crate there, but `#[cfg(windows)]` code is
  compiled on Windows and nowhere else — `engine.rs:493` is where the last two fixes on `main`
  went. macOS bills at 10× a Linux runner and Windows at 2×, so this is most of what CI costs; it
  buys the only proof that the platform-conditional code compiles at all.
- **Chief ships macOS Apple silicon, macOS Intel and Windows.** There is no Linux bundle, and no
  Linux app build: that job was the only thing proving a link nobody installs. Linux is still
  where every cheap job runs — Frontend, Layout, Rust, commitlint — and the `linux-x64` engine is
  still fetched there, because Tauri's build script wants the sidecar on disk even for
  `cargo test`.
- **Don't build it at all when the change can't reach it.** The `changes` job spends a Linux
  minute working out whether a pull request touches `src-tauri/`, `scripts/`, the manifests,
  `.github/actions/` or `checks.yml`, and the app build is skipped when it does not. A change
  under `src/` is proved by the Frontend job's `pnpm build`; it cannot break platform-conditional
  Rust. Deliberately not all of `.github/workflows/`: `App build (macos-latest)` is the largest
  single line in this repository's bill, and rebuilding the app on two paid platforms says nothing
  about a change to `release.yml`, which bundles what is already built, or to `ci.yml`, which only
  decides who calls `checks.yml`.
- **Fail in seconds, not in minutes.** `setup-tauri` checks the engine is on disk under the name
  the target expects, immediately after fetching it. Tauri only notices a missing sidecar part-way
  through its build script, so the first release to reach bundling spent six macOS minutes
  compiling before saying the file was not there — on a runner billed at 10×, the most expensive
  possible way to learn that. A quarter of the runner time this repository spent in its first two
  weeks went on jobs that failed, most of it compiling before the failure.
- **The app build only runs where it answers something.** On a pull request it builds `--debug`,
  because the question is whether the platform-conditional code compiles and links, and
  optimisation is not part of that. A release skips it entirely: the bundle jobs compile the same
  two platforms straight afterwards, in the profile that actually ships.
- **A release is asked for, not triggered.** `workflow_dispatch` and nothing else. Three bundles,
  two of them macOS at 10×, make it the most expensive thing here — releasing on every merge spent
  that on each pull request separately. Releasing by hand lets several merges go out together, and
  lets a person choose when. It takes a `ref`, defaulting to `main`.
- **Compile a dependency once per target.** Release keys its cargo cache on the target rather than
  the runner — `shared-key: tauri-<target>` — so one release restores what the last one built.
  This matters most on macOS, where both bundles are built on one arm64 runner into different
  target directories: a single key had the two jobs overwriting each other with artifacts the
  other could not use. Pull request debug builds keep their own `tauri-dev-<platform>` caches, so
  a branch cannot evict what a release restores from. Both workflows set the same
  `CARGO_TERM_COLOR`, because rust-cache hashes every `CARGO_*` and `RUST*` variable into the key.
  Chief's own crates are never cached, only its dependencies.
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
  `llama-server-<target triple>` for `externalBin`, and its shared libraries in `lib/` for
  `bundle.resources`. The target is this machine's unless `CHIEF_ENGINE_TARGET` names another —
  which the release needs, because both macOS architectures are built on one arm64 runner and the
  Intel bundle has to link an Intel server. Getting this wrong does not degrade anything: Tauri
  stops with `resource path binaries/llama-server-x86_64-apple-darwin doesn't exist`. Only CPU builds — a binary that runs on a machine with no GPU and no AVX-512,
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
- **One slot, and an inactivity timeout rather than a deadline.** `--parallel 1` because Chief
  asks one question at a time: llama.cpp defaults to four slots and hands each request the least
  recently used one, so consecutive questions meet a cache belonging to some other conversation
  and `--cache-reuse` never pays. And the HTTP client uses `read_timeout`, not `timeout` — the
  latter bounds the whole exchange including the body, which on a streamed answer is a limit on
  how much the model may say rather than on how long it may stall.
- Two things llama.cpp makes unnecessary that Ollama needed. The **context window** is a launch flag
  (`--ctx-size`), not a per-request option, so no request can evict the weights and there is no
  keep-alive to negotiate — the server owns one model for its lifetime. And it **warms itself** as
  it starts, so there is no warm-up to schedule.
- `--jinja` is not optional: tool calling goes through the model's own chat template, which
  llama.cpp only applies in Jinja mode.

## Watching the corpus

`src-tauri/src/watcher.rs` keeps `corpus_files` in step with a folder the user edits themselves.

- Every command that touches the corpus already reindexes, so the index is correct **whenever Chief
  looks**. The watcher closes the gap between somebody saving a file and something else happening to
  trigger a scan. It is an optimisation, not a dependency: a machine where the watch could not be
  established is exactly as correct as one from before it existed.
- **`Debounce` is pure and takes an `Instant`**, so "a burst collapses into one reindex" is tested
  without sleeping through it. Saving one file produces several events — a write, a rename from a
  temporary file, an attribute change — and reindexing on each is a folder walk apiece.
- **A self-triggered loop is impossible, and needs no mechanism to prevent it.** The plan warned
  that Chief writes here too and a watcher might react to itself. A loop needs an edge from
  reindexing back to the filesystem; reindexing writes only to SQLite, and
  `reindexing_does_not_touch_the_corpus_at_all` asserts the folder is byte-identical afterwards.
- Only `.md` is watched, matching `Corpus::list`. An editor's swap files, `.DS_Store` and the
  numbered temporaries vim leaves behind would each otherwise be a reindex.
- `notify` is pulled with `default-features = false` and `macos_fsevent`. The backend is chosen per
  platform regardless — inotify on Linux, `ReadDirectoryChangesW` on Windows — so the feature only
  settles which of the two macOS backends is used.

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
- **"Waiting on you" is a different question from "your open work"**, and conflating them was a real
  wrong answer rather than a thin one. `github::Involvement` splits `author:@me` from
  `review-requested:@me`, and `assigned_issues` adds `is:issue assignee:@me`. The brief keeps them
  in separate buckets under separate headings, because a review somebody requested of you is a
  request and your own open pull request is not.
- **The catalogue is at 515 of D2's 600 tokens.** REC-41 added the `whose` parameter for 71 and
  recovered 33 by cutting redundancy from the description, so it cost 38 net. **85 tokens of
  headroom is roughly half a parameter**, so measure before adding anything — the number is in the
  PR each time it moves, not assumed.
- `agent::Attention` counts the questions the user is waiting on. The engine decodes one request at
  a time, so the daemon reads this and steps aside rather than putting a background summary ahead of
  a person.
- `src-tauri/src/tools.rs` holds the catalogue and the dispatcher. A tool failure — bad arguments, an
  unknown name — is reported back to the _model_ as an `error` payload, not raised to the user: it
  can then explain itself or try something else instead of collapsing the conversation.
- Add a tool by writing its schema in `catalog()` and its arm in `dispatch()`. Keep the two in step
  via a shared name constant.
- Errors are user-facing: an engine that is not running, or one still reading the weights, says so
  in those words rather than surfacing a transport error. A body that stops arriving is classified
  rather than stringified — it used to reach the screen as `error decoding response body`.
- **An interrupted answer is kept, not discarded.** If the stream stops part-way, whatever arrived
  is returned with a marker saying so, on the same principle as the length ceiling. Only prose
  survives: a half-delivered tool call is truncated JSON, and an error the engine itself reported
  in the stream is it saying the answer is void, so both still raise. Measured in the product, the
  alternative was hundreds of words the reader had already watched arrive being replaced by a red
  box.

## Routing without a model

`src-tauri/src/intent.rs` is the blueprint's "Overseer", as code rather than a second model. On a
machine where prefill runs at 18–34 tokens a second, the largest optimisation available is not
calling the model at all.

- `route` is pure and total: a question in, an `Intent` or `None` out. `/brief`, `/prep` and `/log`
  match on the first word; everything else matches the **whole normalised question** against a
  written list of phrases, never a keyword found somewhere inside one. That is what keeps "how do I
  brief a client?" and "a brief history of Rust" out of the brief, and it is why the list is long
  and dull rather than clever.
- **A false positive is the failure mode to design against**, because it silently replaces the
  user's question with a canned answer and never says it did. A miss only costs what the question
  cost before this module existed. `refuses_everything_that_merely_mentions_a_trigger_word` is the
  test that keeps this honest; add to it before adding a phrase.
- **A routed intent that finds nothing steps aside.** An answer of "you have nothing logged" is
  indistinguishable, to the reader, from Chief having misunderstood them — so `answer` returns
  `None` and the tool loop takes the question after all.
- `/brief` **reads** today's brief and never regenerates it, on the same principle as
  `todays_brief`: writing one is a model call, and this is meant to be the free way to read it.
- Routing happens in `ask_agent` **before the engine is started**, so a question answerable from
  this machine's own disk does not first wait for a couple of gigabytes of weights to be read.
- The tool catalogue's cap lives beside it, in `agent::tests::keeps_the_tool_catalogue_within_its_budget`:
  6 tools and 600 tokens. The tokens are the ceiling that bites — three tools already cost 477 — so
  the fourth tool is where it starts having an opinion. Raising either number is a decision about
  how much of every prompt is spent describing tools before the question is read.

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
  This is a deviation from RFC 8252 by GitHub rather than by Chief, and it is what GitHub's own `gh`
  does — `cli/oauth` tries the device flow first. Outlook is already the RFC 8252 flow, and GitHub
  joins it when GitHub supports public clients: REC-61, and §9 of the plan, hold the evidence.
- **Chief opens the verification page with the code already in it.** RFC 8628 has a field for that
  and GitHub does not send one, so `github::prefilled` builds the URL from a query parameter GitHub
  honours but does not document. That is why the code and the plain address stay on screen and the
  renderer falls back to the bare page: a prefill withdrawn costs a keystroke, not the sign-in.
- The client id comes from `src-tauri/src/oauth/registration.rs`, which is the one place either
  provider's registration is decided. It is **not a secret** — a device-flow client id and an Entra
  public-client id are both public by design — but it does have to exist, and neither GitHub nor
  Entra offers dynamic registration to mint one on the spot. So the only question is where it comes
  from, and there are **three layers, most deliberate first**: `CHIEF_GITHUB_CLIENT_ID` in the
  environment, then one the user pasted in Settings, then whatever the release baked in from the
  repository _variable_ of the same name. Because of the third, nobody installing Chief has to
  register anything; because of the second, a build that shipped without one is not a dead end, and
  an organisation can sign in against its own OAuth app or Entra registration without a rebuild.
- **The client id is shown back to the user**, unlike the Linear key or a calendar address. Those
  are hidden because holding one grants access; a client id grants nothing on its own, and the user
  cannot otherwise tell which registration they are signed in against.
- **A release cannot compile without one.** `build.rs` panics when `CHIEF_REQUIRE_CLIENT_ID` is
  set and `CHIEF_GITHUB_CLIENT_ID` is not; the release workflow sets the first on the bundle step.
  The workflow's older guard checks the repository _variable_ before anything is compiled, which is
  fast but proves only that the variable exists — this one runs inside the compilation, so nothing
  is left between it and the binary. It exists because 0.4.0 shipped a Windows build whose sign-in
  reported that it had no client id, and no mechanism between a set variable and that binary was
  ever identified.
- `option_env!` **is** tracked by cargo — rustc writes an `env-dep:` line into the dep-info and
  changing the id recompiles the crate, which was measured rather than assumed. The
  `cargo:rerun-if-env-changed` lines in `build.rs` are there for the reads the build script itself
  makes, which are not tracked that way.
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

## Proposed actions

`src-tauri/src/propose.rs` drafts the thing before the user asks; `proposed.rs` keeps what became
of it. The item is one of the user's own open pull requests that has been sitting, and the draft is
a short message asking for a review.

- **Nothing here sends anything, and that boundary is a test.**
  `never_reaches_the_network_to_draft` asserts every request the pass makes to GitHub is a `GET`,
  having first asserted the request list is not empty — a loop over nothing passes without checking
  anything, which would be a broken stub dressed up as proof.
- **The prompt is assembled in COSTA's order, literally**: org structure, then writing style, then
  the item. Who these people are frames how to address them, how the user writes frames the words,
  and the thing being written about arrives last. `reads_the_team_and_the_style_before_the_item`
  asserts the three appear in that order in what was actually sent.
- **One item per pass.** A draft is a model call, and a machine that woke to nine stale pull
  requests would spend minutes of engine on work nobody asked for while the user waits behind it.
  The pass yields to `Attention` between accounts and between items, like the daemon's own.
- **The model is asked before anything is written.** A refusal therefore leaves no half-written
  draft and no orphan row, because neither had been created yet — verified by moving the write
  earlier and watching the test fail.
- Dedupe is the unique index on `(source, account_id, dedupe_key)`, and it deliberately excludes
  `status`: a **dismissed** proposal has to keep occupying its slot, or the next pass drafts it
  again and dismissing it reads as having done nothing.
- The body is a file in the corpus and only its state is in SQLite, so a draft the user edited in
  their own editor is the one on screen. A proposal whose file has gone is dropped from the list —
  deleting the file is a reasonable way to say no.
- `refine_draft` is the **single-shot transformation path**: no tools, no history, no corpus, one
  message. It is the cheapest call Chief makes and the one the user feels most, because they make
  it repeatedly while looking at the result. `sends_nothing_but_the_instruction_and_the_draft`
  keeps it that way.

## Seeding the profile

`src-tauri/src/profile.rs` fills in `writing_style.md` and `team_structure.md` from the user's own
work. COSTA's finding is the reason it exists: an empty corpus produces generic drafts, so the
profile has to be seeded from something real before Proposed Actions is worth building.

Three rules, all about not taking something that is the user's:

- **Nothing runs without being asked.** `plan` says what would be read and what would be written, in
  those words, and returns having touched nothing; the Settings card shows it and waits. This is the
  first feature that reads the user's own writing in bulk to build a profile of it, and a profile
  assembled quietly would be the wrong way to do that however local it stays.
- **A file the user has touched is theirs.** `is_untouched` compares the file against the starter
  `ensure_shape` wrote — content, not mtime. mtime says _when_ a file was written and content says
  _what is in it_, and a git checkout, a Dropbox sync or a restored backup all move mtime without
  changing a byte. Judged on mtime Chief would refuse to seed a file nobody had opened.
- **One call over a sample.** Only the writing style needs a model. The team is _counted_ from
  recurring meeting attendees, because who somebody meets and how often is arithmetic, and asking a
  3B model to do it would be slower, less accurate, and would spend the one call this step is
  allowed. Reviewers and repository collaborators belong there too and are deliberately absent:
  GitHub's search response carries neither, so gathering them is one API call per pull request.

`github::PullRequest` carries `body` for this and nothing else. It is `#[serde(skip_serializing)]`
so it cannot leak into `as_tool_entries` — a list of pull request bodies would swamp the prompt
budget, and the model does not need one to say what is waiting.

## Calendars without a sign-in

`src-tauri/src/ical.rs` parses iCalendar; `src-tauri/src/calendar.rs` fetches it. Together they are
a calendar that needs no OAuth application, no registration and no administrator consent — the user
pastes the address their provider already publishes.

- **The address is a credential.** A subscription link grants read access to somebody's whole
  calendar to anyone holding it. It is stored in `integrations` beside the OAuth tokens, and
  **never logged, never put in an error, never shown once saved** — `calendar::Error` has four
  variants and not one of them carries the URL, which a test asserts.
- **The first outbound host the user chooses.** Everywhere else Chief talks to a constant. Here a
  person types it, so it takes the care `weights.rs` takes: HTTPS only, every redirect required to
  stay HTTPS, no other service's credential attached, no proxy. `webcal://` is rewritten rather
  than refused, because that is the scheme providers actually hand out.
- **No new tool.** Subscriptions merge into the same `microsoft::Event` the Outlook path produces,
  so a brief never knows which kind of calendar an entry came from. The catalogue stays at 3 tools:
  it is at 477 of D2's 600-token cap, so a fourth would break it.
- **`ical.rs` is pure**, which is where the mistakes are. Line folding, escaped text, `TZID` against
  the file's own `VTIMEZONE`, all-day dates, cancellations and `RRULE` expansion are all tested
  without a server.
- **What it does not support is written down at the top of the module** and should stay that way:
  `BYSETPOS`, `BYMONTHDAY`/`BYMONTH` selectors, and `RECURRENCE-ID` overrides. A rule it cannot
  read is treated as the one-off the event was defined as — showing a meeting on a day it does not
  happen is worse than missing a series.
- **A malformed fixture is a folded line.** A stray leading space in a test calendar is a
  continuation to RFC 5545, so it silently glues `SUMMARY` onto the line above and the parser gets
  blamed. `calendar_file()` is built from a list of lines for exactly this reason.

## Linear, read with a pasted key

`src-tauri/src/linear.rs` is the simplest integration Chief has, deliberately.

- **No OAuth at all.** Linear issues personal API keys, and Chief only reads what is assigned to one
  person, so there is no `Provider`, no PKCE, no loopback listener and no renewal — a header on one
  request. An OAuth application would be right only if Chief acted on behalf of a workspace.
- **The key is a credential and never comes back.** Stored beside the OAuth tokens, and like the
  calendar subscription address it is never logged, never in an error and never returned to the
  frontend. `linear::Error` carries no key, which a test asserts. The field is `type="password"`.
- **A personal key has no read-only kind**, so it carries read _and_ write. The Settings card says
  so out loud, for the same reason the `repo` scope should.
- **One GraphQL query**, asking for exactly the fields the brief renders. `viewer` comes back with
  the issues rather than from a second call: a key that cannot name its owner cannot read issues
  either, so one round trip both validates and reads.
- **Open means `completedAt` and `canceledAt` are null**, never a workflow state called "Done".
  Every workspace renames its states; none of them can rename those two fields.
- **GraphQL answers 200 with an `errors` array**, so status alone is not enough — `read` inspects
  the body and separates an authentication failure, which is the user's to fix, from anything else.

## Jira, over an MCP server that registers Chief at runtime

`src-tauri/src/atlassian.rs` is the odd one out, and the module docs carry the evidence because the
mechanism is undocumented.

- **Classic 3LO is impossible, not merely awkward.** Atlassian's discovery document offers
  `client_secret_basic` and `client_secret_post` and no `none`, so every 3LO client is confidential.
  PKCE does not rescue it: there it hardens the code exchange on top of client authentication rather
  than replacing it. A secret compiled into a distributed binary is not a secret.
- **The Remote MCP server runs a different authorization server, and that one takes public
  clients.** So Chief registers itself per installation against a `registration_endpoint` Atlassian
  does not document. Nothing is baked in and nothing is shipped. Re-verified 2026-09-08; the
  fetched facts and the date are in the module docs, because there is no page to cite.
- **The issuer now advertises `client_id_metadata_document_supported`**, the successor MCP's spec
  prefers over registration. It is worse here: a Client ID Metadata Document must be hosted at a
  stable HTTPS URL the client controls and that URL _is_ the client id — a permanent off-machine
  dependency for an app whose claim is that none of it exists off the user's machine. If
  registration is withdrawn, the fallback is the pasted API token, not this.
- **Registration happens per sign-in, after the loopback port is bound**, so the redirect URI
  registered names a port the process already holds. RFC 8252 §7.3 requires any port to be accepted
  on a loopback redirect; nothing says an undocumented endpoint follows that rule. The minted id is
  stored on the account, and `session.rs` now prefers `integration_accounts.client_id` over the
  configured registration — one OAuth client cannot exchange another's refresh token. That column
  had never been written before this.
- **Deterministic tools only, as an allowlist.** `searchAtlassian` and `fetchAtlassian` are the Rovo
  natural-language layer; calling either would send the user's typed question to Atlassian, and
  `Sidebar.tsx` promises on every screen that nothing they type leaves this machine. `DETERMINISTIC`
  is checked before a request is built, and the test proves the refusal costs no request by making a
  legitimate call afterwards — the first version passed with the check moved _after_ the send.
- **Client capabilities are empty**, asserted against the bytes that go out rather than against the
  constant. A server holding `sampling` can run its own agentic loop on the user's local model.
- **Read-only is enforced at the authorization server**, because Atlassian's write scopes sit on the
  same resource as its read scopes. Asking for none of them makes it a property of the grant rather
  than a promise about this code — strictly better than the `repo` scope Chief settles for on
  GitHub. D4 is where forfeiting it would be argued. **Confluence's scopes are deliberately not
  requested yet**: a consent screen naming access no code path uses is what `repo` is criticised for
  here, so the scope and the read land together.
- **Discovery is followed for its paths, not for where it points.** The issuer is compared as a whole
  origin against `auth.atlassian.com` before its metadata is fetched — scheme included, so
  `http://` is not the same host over a transport anyone can rewrite. One MCP server surveyed for
  the design spec published forged metadata naming an issuer it did not own.
- Jira issues join the brief beside Linear's under one heading, because they answer the same
  question — but they are two fields in `Gathered`, so a brief built from Jira alone does not report
  its source as Linear.
- **A failed request says which host and why.** The module shipped with ten call sites that threw
  the cause away with `map_err(|_| Error::Transport)`, so a name that does not resolve, a
  certificate that does not verify, a firewall and a 404 all reached the screen as "Atlassian could
  not be reached" — true, and useless to the person reading it or the next one debugging it.
  `Unreachable` now carries the host and the transport's own words (`because` walks the source
  chain, since reqwest's outer message is "error sending request for url" every time), `Answered`
  carries a status, and both are written to stderr so `tauri dev` shows them. The token is never in
  either: a `reqwest::Error` holds a URL and an I/O cause, never headers, and
  `a_failed_request_never_renders_the_token_it_carried` drives a real failing request with a real
  token rather than trusting that.
- **A certificate failure is its own error, because it is the one that was reported.** On the
  machine that hit it, corporate DNS resolved `mcp.atlassian.com` to `185.166.141.x` — not
  Atlassian's `104.192.142.x` — and Windows refused the substituted certificate with
  `CRYPT_E_NO_REVOCATION_CHECK`, while `api.github.com`, which was not redirected, answered 200.
  "Could not be reached" sends somebody to check their wifi; `Untrusted` names the certificate and
  says something on the network may be inspecting HTTPS. **Chief offers no way to skip
  verification and must not grow one** — an intercepted connection to a host holding somebody's
  Jira is exactly what verification is for. The classification is matched on the transport's text,
  since that is all a `reqwest::Error` offers, and it lives in a pure `Error::from_transport` so
  the branch is provable: the first version had a test for the matcher and a test for the wording
  and nothing joining them, and disabling the branch entirely left the suite green.

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
- **It is not only rendering.** On 2026-08-29 a regex lookbehind — `(?<!…)` — reached `main` past
  162 jsdom tests, 93 Chromium layout tests and a green CI, and opened as a white screen with
  nothing but `SyntaxError: Invalid regular expression: invalid group specifier name` in a console
  the user had to think to open. jsdom runs on V8 and Playwright runs on Chromium; both have
  supported lookbehind for years, and the WKWebView on the machine did not. A module-level `const`
  makes it fatal: the parse fails while the module graph evaluates, so React never mounts and one
  cosmetic helper takes the whole app down.
- **esbuild does not catch this, and setting `build.target` would not have.** Below its
  `safari16.4` target esbuild rewrites the literal to `new RegExp("…")`, which moves the same
  failure from parse time to module-evaluation time — the shipped bundle contained exactly that.
  The guard is therefore a lint rule (`no-restricted-syntax` in `eslint.config.js`), because this
  has to be caught in the editor rather than by a runtime nobody has.
- **The general rule: syntax the tests accept is not syntax the product accepts.** Prefer the
  boring construction. A feature that landed in Safari in the last few years — regex lookbehind and
  the `d`/`v` flags, `Object.hasOwn`, `structuredClone`, `Array.prototype.at`, `findLast`,
  `toSorted` — is worth avoiding for the same reason, and none of them are in the tree today.
- These are kept out of `pnpm check` because they need a build and a browser. CI runs them as the
  **Layout** job.
- **A stub answers in the shape the command actually returns.** The one that answers `[]` to
  everything is the most expensive shortcut in this repository: it has taken the whole screen down
  three separate times, because a view received a list where it expected an object and a render
  threw. Dispatch on the command name and return the real shape — `todays_brief` gives a brief or
  `null` and never a list; `profile_plan` gives an object with three arrays. Note that
  `answers[command] ?? []` turns a deliberate `null` back into a list, so dispatch on presence.
  Every one of those crashes was found by a test rather than by a user, which is the system working
  — but only because something rendered the real component.
- **A test that guards an invariant is not finished until you have watched it fail.** Caps,
  boundaries, "never overwrites", "a failure writes nothing" — these pass on the day they are
  written whatever they assert. Break the thing they guard, watch the failure, put it back, and put
  the failure message in the pull request. See the `proving-a-guard-test` skill; it exists because
  three of them lied in one afternoon.

## Working from Linear

Remaining work lives in the **Chief** project in Linear, not in this repository. Issues carry a
work type, acceptance criteria, and `blockedBy` relations where the plan's ordering is real.

**"Work on the next N Chief tasks autonomously"** means exactly this:

1. List issues in the `Chief` project carrying the **`agent-executable`** label.
2. Drop any whose `blockedBy` issues are not yet Done — fetch relations explicitly, they are not
   in the default response.
3. Sort what is left by priority, then by the dependency order among themselves.
4. Take the first N, and work each to its acceptance criteria: one branch and one pull request
   per issue, `main` protected, `Checks / Complete` green before merge.

**`agent-executable` is a claim about the work, not about its importance.** It means an agent can
finish the issue alone: no physical hardware, no third-party portal, no product decision, and no
irreversible action outside this machine. Several of the highest-priority issues do not carry it
and never will — validating the Windows build needs a Windows machine, registering an Entra
application needs somebody in the Azure portal. Do not reach for those because they sort higher.

Two issues are deliberately excluded and should stay excluded until a person says otherwise: the
brief-quality defect, which needs a product decision between three stated options rather than an
implementation; and the send path, which is the first code that changes something outside the
machine and is the wrong thing to build unattended.

If an issue turns out not to be finishable alone, stop and say why rather than guessing — and
remove the label, so the next pass does not pick it up again.

## Commits and pull requests

[Conventional Commits](https://www.conventionalcommits.org/) are enforced by commitlint on both the
commit message (via a git hook) and the PR title (via CI).

Two more guards run alongside it, because both catch mistakes that are silent until they are
expensive:

- `scripts/check-attribution.sh` runs from the `commit-msg` hook and **refuses a commit** that
  carries a co-author or tool-attribution trailer, or whose author or committer is not the owner.
  The documented fix is `--amend --reset-author`, which is tedious over a branch and impossible
  once merged — so the commit is refused at the last moment it is still free.
- `scripts/check-migrations.mjs` runs from `pre-commit` and from `pnpm check`, and refuses a
  duplicate or non-contiguous migration version. A duplicate version compiles, passes every test,
  and puts installed copies on a schema this code does not expect.

```
<type>(<scope>): <subject>
```

Types: `feat`, `fix`, `docs`, `style`, `refactor`, `perf`, `test`, `build`, `ci`, `chore`, `revert`.
Scopes: `agent`, `auth`, `corpus`, `db`, `daemon`, `integrations`, `ui`, `tauri`, `deps`, `ci`,
`repo`. The list is enforced by `commitlint.config.js`; a scope that is not on it fails the
`commit-msg` hook, so read it there rather than guessing a plausible-sounding one.

**Write the message to a file and commit with `-F`.** A body of several paragraphs is the norm here
and it will contain an apostrophe, a quotation mark or a backtick sooner rather than later — `-m`
with a shell-quoted string breaks on the first one, and the failure looks like git rejecting the
commit rather than like a quoting mistake.

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

## Dependency advisories

`SECURITY.md` holds the advisories that cannot be fixed by a bump, each with the reason and a date
to look again. Read it before investigating an open Dependabot alert — an alert sitting there with
no pull request behind it usually means Dependabot has no version to offer, not that anybody has
ignored it. Add to that list rather than re-deriving the analysis, and only after establishing
whether the package reaches a shipped artifact: `cargo tree -i <crate> --target <triple>` for the
three targets Chief bundles is what settles that for a Rust dependency, and it is what proves a
Linux-only crate is not in any of them.

## Releases

A release is cut deliberately, and it can carry several pull requests at once.
`.github/workflows/release.yml` is started by hand from the Actions tab. It runs the checks
first, and then one Linux job decides whether what has landed since the last tag is worth
releasing. If it is, that job _is_ the release: the new version is
written into the four files that carry it, `CHANGELOG.md` gains an entry, both are committed back
to `main` and tagged, and the three bundles — two macOS architectures and Windows — build and
publish against that tag.

`scripts/release.mjs` holds the decision, which is why it is a tested script rather than a heap of
YAML — a person chooses _when_ to release, but nothing between that click and a published release
is checked by hand. `pnpm test` covers it.

- **A `feat`, `fix`, `perf` or `revert` releases. Nothing else does.** A `docs`, `ci`, `chore`,
  `style`, `test` or `refactor` commit changes nothing a person can download, and a release is
  three bundles — two of them macOS at 10× a Linux runner, so on the order of 200 billed minutes.
  Those commits neither cause a release nor appear in one.
- **The version is derived, never chosen.** A `feat` is a minor and anything else releasable is a
  patch. A breaking change — `feat!:` or a `BREAKING CHANGE:` footer — is a major, except before
  1.0.0, where it is a minor: a project that is not finished should not be forced to call itself
  1.0 by its first breaking change.
- **Four files carry the version** — `package.json`, `src-tauri/tauri.conf.json`,
  `src-tauri/Cargo.toml` and `src-tauri/Cargo.lock` — and the script rewrites exactly one version
  string in each, failing if it finds none or several. A test asserts each pattern still matches
  its real file, so reformatting one of them breaks a test rather than a release.
- **The release commit starts nothing.** It is pushed with `GITHUB_TOKEN`, and GitHub deliberately
  raises no workflow runs for those. Nothing runs on a push to `main` anyway, but this also keeps
  the commit from tripping anything added there later.
- **Drafted, filled, then published.** The release is created as a draft so nobody is told about a
  release they cannot download; the `publish` job takes it out of draft once every bundle is
  attached. If that job never runs, the release sits there as a draft with its assets and one click
  finishes it. That is the failure this is shaped around.
- **The first release needs a starting point.** With no `v*` tag to measure from, the script reads
  from the `BASELINE` commit rather than summarising the entire history.
- **`main` is protected, and the release has to be let through it.** Merging needs the checks
  green and the branch up to date, so a red pull request cannot land. But the release commit goes
  straight to `main` by design, and a rule that simply forbids that would deadlock releasing
  entirely — so the ruleset grants a bypass to the GitHub Actions app, and to nothing else. If
  releasing ever fails on a protected-branch error, that bypass is the first thing to check.
- `CHANGELOG.md` is in `.prettierignore`. It is generated, and a formatting check failing on a
  release commit would block releasing entirely.

## Roadmap

**[`docs/implementation-plan.md`](docs/implementation-plan.md) is the roadmap.** It holds the build
order from step 8 to step 22, decisions D1–D8 with what each one costs, the hardware budget, and
§0's ledger of what is actually built. Steps 1–7 shipped before it was written.

It is **a living document, updated in the pull request that changes it** — see its §8. A step that
lands without moving its row in §0 has left the plan describing something that is no longer true,
which is the state it was in when it sat unmerged on a branch for seven steps.

Three things to do with it rather than around it:

- **Before starting a step**, read its section and §9's open questions. Something you are about to
  decide may already be recorded as decided, or as deliberately somebody else's call.
- **When the code and the plan disagree**, the code is usually right and the plan is stale — say so
  in the plan under a `— revision N` note rather than quietly changing the code to match prose.
- **Anything you find and do not fix** goes in §9, with the issue number if there is one.

Do not start a later step before the earlier one is reviewed and merged, unless the plan itself
says otherwise — it does for step 22, which comes before step 14 because an empty corpus is step
14's failure mode.
