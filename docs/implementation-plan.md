# Chief — Implementation Plan

A synthesis of the **Enterprise Blueprint**, the **PRD** and **The Hybrid Model ("Action-Backed
Chat")**, reconciled against the code in this repository — including the work on
`feature/integration-roadmap-priority-5cb1bc` — and written to be executed autonomously.

**Revision 3.** Revision 1 was written against `main` and did not account for the integration
branch. Revision 2 reconciled with it. **All eight decisions are now taken, so nothing in this plan
is waiting on an answer** — with one exception that is a measurement rather than a decision (D8).
§6 records what changed and why.

---

## 1. Where the code actually is

Roadmap steps 1–6 shipped on `main`. **Step 7 has shipped on
`feature/integration-roadmap-priority-5cb1bc`** and is not yet merged. That branch also carries two
approved design documents which this plan defers to rather than restates. **They live on that
branch, so these two paths only resolve once it is merged:**

- `docs/superpowers/specs/2026-08-21-integration-roadmap-design.md`
  — primary-source, adversarially verified research into six providers. **It is the authority on
  provider viability.** Nothing in this plan overrides it.
- `docs/superpowers/plans/2026-08-22-integration-layer.md`
  — the task-by-task plan that produced step 7.

### Shipped on the branch (step 7 — generic integration layer)

| Capability                                                          | Where                                   |
| ------------------------------------------------------------------- | --------------------------------------- |
| PKCE (S256) verifier, challenge, state nonce                        | `src-tauri/src/oauth/pkce.rs`           |
| One-shot loopback listener on `127.0.0.1:0`                         | `src-tauri/src/oauth/loopback.rs`       |
| `Provider` trait — endpoints, client id, scopes, optional secret    | `src-tauri/src/oauth/mod.rs`            |
| Migration v3 — `integration_accounts`, many accounts per service    | `src-tauri/src/db.rs`                   |
| `work_logs.account_id` + rebuilt dedupe index                       | `src-tauri/src/db.rs`                   |
| `Session<P>` — renewal and retry-once, written once                 | `src-tauri/src/session.rs`              |
| Account-first commands (`start_login(service)`, `label_account`, …) | `src-tauri/src/connect.rs`              |
| Per-account daemon passes, one account's failure its own            | `src-tauri/src/daemon.rs`               |
| Settings listing every connected account, with labels               | `src/components/views/SettingsView.tsx` |
| Engine stderr captured and quoted rather than guessed at            | `src-tauri/src/engine.rs`               |

**Not built:** step 8 (Outlook), and everything in this plan from step 9 onward.

### Also on the branch, and being reversed

The branch merged `feature/refactor-to-gemma-3-1b`: `weights.rs` moved to **Gemma 3 1B Instruct
Q4_K_M**, `CONTEXT_SIZE` rose from 4096 to **8192**, and native tool calling was removed. See
**D1** — this is being undone.

### Where the source documents are stale, and the code is right

Recorded so an agent does not "fix" working code to match prose.

1. **The runtime.** Both documents target Ollama on `:11434`. Chief bundles `llama-server`
   (`b10545`) on `127.0.0.1:11435` speaking the OpenAI contract. **Do not migrate to Ollama.**
2. **PKCE for GitHub.** GitHub requires a client secret to exchange an authorization code; PKCE does
   not replace it. Chief uses the device flow, which needs none. The blueprint's guarantee is
   honoured — the mechanism it names is not the one that delivers it. Loopback PKCE is correct for
   every _other_ provider, and step 7 built it.
3. **"No dynamic agentic loop."** Chief built a hardcoded Rust tool registry — the blueprint's own
   prescribed alternative to MCP. The branch's spec measured the real cost: a Notion MCP toolset is
   17,161 tokens against a 4096 window. Chief's catalogue is one tool. The prohibition targets a cost
   Chief does not pay.

---

## 2. Decisions

All eight are now taken. Each records the conflict, the call, and what it costs.

### D1 — Llama 3.2 3B. Revert the Gemma migration. ✅

**Verified before acting, on two independent grounds.**

- llama.cpp's own [function-calling documentation](https://github.com/ggml-org/llama.cpp/blob/master/docs/function-calling.md)
  lists the models with native tool-call format support: Llama 3.1/3.3, **Llama 3.2**, Functionary,
  Hermes 2/3, Qwen 2.5, Mistral Nemo, Firefunction v2, Command R7B, DeepSeek R1. **Gemma 3 appears
  nowhere.** Gemma 2 is mapped to the "Generic" handler.
- The branch proves it empirically. `agent.rs` on the branch carries the comment _"Gemma 3's bundled
  chat template is text-only and rejects the OpenAI tools field"_, and the code replaces tool calling
  with `github_state_for()` — a keyword match on `"ship"`, `"merged"`, `"pull request"`, `"review"`,
  `"waiting"` that prefetches PRs and staples the JSON onto the user's message.

**One honest caveat.** llama.cpp _does_ have a Generic tool-call handler for unrecognised templates,
so "impossible" would be too strong — it costs more tokens and is less reliable, and on this branch
the attempt hit a hard rejection rather than falling back. The conclusion is unchanged: a model
listed as natively supported is the right substrate for an orchestrator Chief already has.

**What the revert touches** — this is the whole of it, and it should land as one commit:

| Action                                                          | Where                                                                                                   |
| --------------------------------------------------------------- | ------------------------------------------------------------------------------------------------------- |
| Model back to Llama 3.2 3B Instruct Q4_K_M                      | `weights.rs` — **keep the branch's commit-SHA pinning**, which is better than `main`'s `/resolve/main/` |
| Delete `github_state_for()` and the `native_tools` branch       | `agent.rs`                                                                                              |
| Delete `tools::prefetch_github_prs`                             | `tools.rs`                                                                                              |
| Restore `MODEL` in the orchestration tests to `DEFAULT_MODEL`   | `agent.rs`                                                                                              |
| Re-decide `CONTEXT_SIZE` (8192 on the branch) against the probe | `engine.rs` — see step 8                                                                                |

**Keep** — these are improvements that happen to have arrived alongside the migration and are not
Gemma-specific: the content-parts tolerance in `llama.rs` (`text_from_value`), the stderr capture,
the macOS 12 build fix, and the SHA-pinned weights URL. **Review** `fix(agent): merge adjacent turns
for Gemma templates` on its own merits — Llama does not require strict alternation, but the merge
also guards malformed input, so it may be worth keeping for its own sake rather than for Gemma's.

**Cost of the revert, stated plainly:** the download grows from ~0.8 GB to ~2.0 GB and resident
memory roughly doubles. That is the trade being made for a working tool loop, and it is what makes
step 8's tiering necessary rather than nice.

### D2 — Keep the tool loop, demote it below recipes, cap it with a test. ✅

Recipes become the primary path for everything Chief does on its own initiative. The loop remains the
fallback for open-ended typed questions. Cap: **6 tools / ~600 tokens**, asserted by a test that
fails when the catalogue grows past it. Enforced at step 13.

### D3 — The corpus lives in the user's own directory. ✅

Default `~/Chief/`, not a dotfile — every source document calls the corpus human-editable, and hiding
it contradicts that. Path stored in `settings`. All access through typed commands in `corpus.rs`;
**the renderer does not get `fs:*`**. Database and weights stay where they are.

### D4 — Approving sends. ⚠️ **This reverses an approved spec.**

The branch's approved spec lists under _Out of scope_: **"Writing to any connected service. Chief
reads."** D4 overrides that. It is the right product call — a proposed action the user must go and
perform by hand is a reminder, not an action — but it is a genuine change of the app's risk class and
it propagates further than it first appears. Consequences, all of which step 15 owns:

- **Scopes change on every provider.** GitHub already holds `repo` (write). Microsoft needs
  `Mail.Send`, with no restricted-scope tax. Slack needs `chat:write`, free on a bring-your-own
  internal app. Atlassian's write scopes sit on the same resource as its read scopes — requesting
  them **forfeits the read-only-is-enforceable-at-the-authorization-server property** the spec
  valued, which should be a conscious trade rather than a side effect.
- **A useful asymmetry in Gmail, found while checking this.** [`gmail.send` is a _sensitive_
  scope, not a restricted one](https://developers.google.com/workspace/gmail/api/auth/scopes) —
  so it needs Google verification but **no annual CASA assessment**, whereas `gmail.readonly` and
  `gmail.metadata` are both restricted and do. Sending Gmail is dramatically cheaper than reading
  it. This inverts the Gmail plan: see step 21.
- **The user needs enough to decide.** Named in the decision: for a GitHub action, surface whether
  the PR's checks have failed or are still running. `PullRequest` carries no such field today
  (`number`, `title`, `repository`, `state`, `draft`, `url`, `updated_at`, `merged_at`), so this is
  new work in `github.rs`.
- **Nothing sends without a distinct, deliberate act**, and every send is recorded locally with what
  was sent, where, and when. Design in step 15.

### D5 — Microsoft 365 primary, plus Zoom, Slack, Jira, Confluence, Google (Gmail/Calendar/Tasks) and Linear. ✅

The branch's research already covers Microsoft, Google, Atlassian, Slack and LinkedIn to primary
sources with an adversarial pass. **Zoom and Linear were never researched.** First-pass findings
below; both need the same treatment the other five got before their step begins.

- **Zoom — promising.** Zoom documents PKCE with _"a public client ID and no client secret, without
  the need for a backend server"_, the PKCE token exchange sends no secret, loopback redirects such
  as `http://127.0.0.1:8080/callback` are supported, **and there is a device flow**
  (`/oauth/devicecode`). On the auth axis this is as easy as GitHub. _Unverified:_ whether a
  user-created app needs admin approval to leave development, and what the marketplace terms say
  about distribution.
- **Linear — promising.** Linear's OAuth docs list `client_secret` as **optional** for the PKCE
  token exchange, so `client_id` + `code_verifier` suffices — a genuine public client. _Unverified:_
  whether loopback redirect URIs are accepted (not documented; needs a live probe), whether creating
  an OAuth application requires workspace admin rights, and whether personal API keys exist as a
  fallback.

**LinkedIn is not in the list, and the plan does not add it.** The branch's research found no
member-level read scope exists at any self-serve tier. If it is ever wanted it is an archive
importer, not an API integration.

### D6 — The chat drawer overlays the detail column. ✅

The canvas already specifies the 240px list pane. The Executive Feed becomes a new **"Today"** rail
destination; chat is demoted to a drawer that **overlays** the detail column rather than displacing
it.

The consequence worth naming: because it overlays, **the Feed neither reflows nor re-renders when
the drawer opens.** That is what makes `[Edit]` on a Proposed Action feel like opening a panel over
your work rather than rearranging it — and it gives step 12 a precise assertion, that the detail
column's measured width is unchanged between drawer-closed and drawer-open.

Width was not specified, and is a routine call now the harder half is settled. **Assumed, and
overridable:** 420px on wide windows, full-width below the point where 420px would leave the detail
column unreadable. The drawer uses the design system's **one shadow level** — this is what a single
elevation step exists for — over a scrim, with no gradient. It is a dialog: `role="dialog"`,
`aria-modal`, focus trapped on open and restored on close, Escape closes it. Under
`prefers-reduced-motion` it appears without sliding, consistent with the `.motion-loop` /
`.motion-still` pair already in `globals.css`.

### D7 — `CONTEXT_SIZE` per tier: 4096 light, 8192 standard. ✅

The branch runs 8192, chosen for a 1B model. With a 3B, the KV cache at 8192 is materially larger on
a machine with 3–4 GB of headroom, so the number is decided by the probe rather than fixed. **Light
tier 4096; standard tier 8192.** Step 8 owns it, and the argument-list test asserts the value
follows the tier rather than being hard-coded.

Note that this is the engine's window, not Chief's budget. The `ContextBudget` ceiling of ~2,400
tokens of injected context (§3) holds on **both** tiers: a larger window buys room for a longer
conversation and a longer answer, not a larger corpus injection, because prefill cost scales with
what is put in it.

### D8 — Verified by hand, once, on target hardware. ✅ _Acknowledged_

Chief reports nothing anywhere and CI runners are not representative. `chief doctor` measures prefill
tok/s, decode tok/s, resident memory and the resulting tier. CI asserts the harness runs and the
maths is right; it cannot assert the numbers.

**This is the one part of the plan an autonomous agent cannot complete.** Step 8 is therefore done
when the harness ships and its unit tests pass — not when the table in §3 is filled in. Filling that
table is a separate, manual act, and the plan does not block on it.

---

## 3. The governing constraint: the hardware budget

The blueprint's diagnosis — the constraint is context budget and token throughput, not parameter
count — is correct and deserves to be numerical.

**The target machine.** 16 GB RAM, integrated graphics, 4–8 cores, **CPU-only inference**, already
running Teams, a browser and an IDE. Realistic headroom for Chief is **3–4 GB**.

**Where the time goes.** CPU decode is memory-bandwidth-bound and scales with model size. CPU
prefill is compute-bound and scales with prompt length — a few thousand tokens is tens of seconds
before the first word. **A brief injecting 3,000 tokens of corpus therefore costs more wall-clock in
prefill than the whole answer costs in decode.** Prompt length, not model size, is the dominant cost
of a feature.

### Three levers, in order of value

1. **Cap the prompt.** A `ContextBudget` that refuses to build an over-budget prompt rather than
   letting the server silently truncate. Budget: **~2,400 tokens of injected context**.
2. **Pay prefill once.** llama.cpp caches the KV state of a prompt prefix. Assemble the system
   prompt, clock line and corpus block in a **stable order with the volatile parts last**, and the
   expensive prefix is reused across turns. The largest win available, and **none of the three source
   documents mentions it.** _First task of step 8: run `llama-server --help` against `b10545` and
   confirm which cache and threading flags exist. The pinned build is the authority, not this plan._
3. **Give the memory back when idle.** Biscotti runs inference out-of-process precisely so closing it
   reclaims memory in full. Chief already runs `llama-server` as a child process and does not use
   that property — after D1 the model holds ~2.4 GB for the life of the window. Stop the engine after
   an idle period; `ensure_running` is already idempotent and the setup screen's `loading` state
   already covers the restart.

### What must not be built

**Do not run an Overseer model alongside the main one.** Two servers means two resident models — the
exact thing the budget cannot afford, for a task that needs no model. Intent routing is slash
commands and a keyword match: deterministic Rust, zero tokens, zero megabytes. That is more faithful
to the blueprint's own thesis than the blueprint's own recommendation.

### Gates

| Gate                               | Threshold                                                       |
| ---------------------------------- | --------------------------------------------------------------- |
| Injected context, any prompt       | ≤ 2,400 tokens — enforced by `ContextBudget`, covered by a test |
| Tool catalogue                     | ≤ 6 tools / ~600 tokens (D2)                                    |
| Time to first token, warm prefix   | Recorded by `chief doctor` on target hardware (D8)              |
| Resident memory, idle              | Engine stopped — measured, not assumed                          |
| A daemon pass while the user waits | Yields — `Attention` already does this; extend to recipes       |

**Measured on target hardware — to be filled in once (D8):**

| Tier     | Model               | ctx  | Prefill tok/s | Decode tok/s | Resident  |
| -------- | ------------------- | ---- | ------------- | ------------ | --------- |
| Standard | Llama 3.2 3B Q4_K_M | 8192 | _pending_     | _pending_    | _pending_ |
| Light    | _1B-class fallback_ | 4096 | _pending_     | _pending_    | _pending_ |

---

## 4. The steps

Dependency-ordered, continuing the roadmap in CLAUDE.md. One reviewable pull request each; each
leaves the app working; build in order and stop for review.

Steps 8–15 are the **product spine** and are strictly ordered. Steps 16–21 are the **integration
fan-out**: once step 9 lands, they are near-identical in shape and may be reordered freely as
priorities change.

> **Step 7 — Generic integration layer.** ✅ Shipped on `feature/integration-roadmap-priority-5cb1bc`.
> Land that branch before starting step 8.

### Step 8 — Model revert, hardware probe, tiering, engine thrift

_First, because every prompt-design decision downstream depends on which model answers — and because
each step written against the keyword heuristic is a step that has to be unwritten._

**Delivers.** Llama 3.2 3B restored with native tool calling; Chief measures the machine it is on,
picks a model that fits, and stops holding memory it is not using.

**In scope.** The D1 revert exactly as tabulated. Then: a **catalogue** in `weights.rs` (standard 3B,
light 1B-class), each entry carrying URL, filename, quantisation, approximate resident size and a
`supports_tools` flag — keeping the GGUF magic check, `.part` resume and HTTPS-only redirect chain
untouched. A new `probe.rs` producing a `Tier`, pure from measurements so it is unit-testable without
a model. **Migration v4** extending `settings` with the tier and model id (v3 is taken — see §6).
`CONTEXT_SIZE` per tier (D7). Prompt-cache and thread flags in `engine.rs`, extending the existing
argument-list test. An idle stop. The tier shown in setup before a gigabyte is fetched.
`chief doctor` (D8).

**Verified by.** Tier selection, catalogue integrity and the argument list under `cargo test`; an
ignored model-backed probe test; **every orchestration test in `agent.rs` passing with native tools
again** — that is the proof the revert is complete.

**Risk.** Medium. The idle stop interacts with the daemon: a pass that wakes the engine every 30
minutes defeats the purpose. Either run passes while warm, or batch and accept the restart.

### Step 9 — Outlook Mail and Calendar

_Already specified in full. The primary workplace stack, and the step that proves the step-7
abstraction against its second provider._

**Delivers.** Chief reads the user's Outlook mail and calendar.

**In scope.** Exactly as the branch's spec sets out, and it should be followed rather than
re-derived: one multi-tenant registration with `signInAudience =
AzureADandPersonalMicrosoftAccount` against `/common`; scopes `Mail.Read Calendars.Read
offline_access User.Read`; `CHIEF_MICROSOFT_CLIENT_ID` overriding at run time; `127.0.0.1`
registered through `replyUrlsWithType`; delta-query polling; `AADSTS90094` as a first-class error
state reading _"your organisation requires an administrator to approve Chief"_.

**Note.** The device flow is **not** available — Entra blocks it by default and has since 1 July 2026. Loopback + PKCE is the only path, which is why step 7 built it.

**Risk.** Medium. Auth is solved; the org-consent wall is a product problem, not a code one, and the
ladder out of it needs no code change because `client_id` is already a column.

### Step 10 — The corpus layer and the context budget

_The storage substrate for everything after it. No LLM involvement, so it is fast to build and easy
to test._

**Delivers.** A folder of human-editable markdown Chief reads, writes, indexes and watches — and a
type that makes an over-budget prompt impossible to construct.

**In scope.** New `corpus.rs` owning the root (D3), path-traversal checked, with the COSTA directory
shape: `context/agents/<agent>/agent.json`, `briefs/`, `proposed/`, `1-1s/`, `meeting-notes/`,
`journal/`. A debounced watcher — Chief writes here too and must not react to itself. **Migration
v5** for `corpus_files` (path, mtime, size, estimated tokens, agent), so assembling a prompt is a
query rather than a disk walk. New `context.rs`: `ContextBudget`, the `slimtoken` minifier, and a
token estimator calibrated against the real tokenizer rather than guessed at four characters per
token.

**Verified by.** Traversal refused, watcher debounced, index consistent after external edits, budget
refuses an oversized assembly, minifier idempotent, estimator within 10% on both catalogue models.

**Risk.** Low-medium.

### Step 11 — The recipe engine and the first brief

_Proves the architecture end to end on data Chief already reaches._

**Delivers.** A daily brief generated in the background, written to the corpus as markdown.

**In scope.** New `recipe.rs` with a three-phase trait — **gather** (deterministic Rust, no model, no
planning), **assemble** (one prompt, through `ContextBudget`), **render** (exactly one LLM call, no
tools). One model call per recipe; if it needs two, it is two recipes. `Recipe::DailyBrief` over
GitHub PRs, the last 24 hours of `work_logs`, Outlook calendar and mail from step 9, and any
always-loaded agent files. A schedule in `daemon.rs` that survives a sleeping laptop: on wake, run
the pass that was missed, once, not the four that were.

**Verified by.** The pattern `daemon.rs::run_once` already establishes — in-memory database, stub
providers, stub engine. Assert one model call per brief, prompt within budget, same-day rerun
replaces rather than duplicates, failed render writes nothing.

**Risk.** Medium. The prompt is the product; a 3B model needs a rigid output shape or it writes an
essay.

### Step 12 — The Executive Feed and the chat drawer

_Makes step 11 visible._

**Delivers.** The dual-panel layout, as an addition to the Instrument shell rather than a
replacement.

**In scope.** The 240px list pane the canvas specifies; a **"Today"** destination rendering the brief
and recent activity; chat demoted to an **overlay** drawer (D6) that opens empty or pre-loaded, at one
shadow level over a scrim, as a focus-trapped `role="dialog"` that Escape closes; layout tests across
all three Playwright projects including the scrollbar-taking-space one.

**Verified by.** `pnpm verify:layout` — including the assertion D6 makes available: **the detail
column's measured width is identical with the drawer open and closed.** jsdom reports every height as
zero and cannot see any of this. Keyboard behaviour (focus trap, Escape, focus restored) is testable
in jsdom through the accessible surface and belongs there.

**Risk.** Low-medium.

### Step 13 — Deterministic intent routing

_The blueprint's "Overseer", as code instead of a second model._

**Delivers.** `/brief`, `/prep`, `/log` and a conservative set of natural-language triggers, routed in
Rust at zero token cost. **D2's cap is enforced here.**

**In scope.** New `intent.rs`: a table of pattern → recipe, slash commands first. Deliberately
conservative — a false positive silently replaces the user's question with a canned recipe, which is
worse than a miss. A matched intent streams its markdown over the existing `agent-stream` event; an
unmatched question falls through to the tool loop unchanged. A test asserting the catalogue stays
within 6 tools / ~600 tokens.

**Verified by.** Every pattern, plus the phrases that must _not_ match. Existing agent tests passing
untouched is the proof the fall-through is intact.

**Risk.** Low.

### Step 14 — Proposed Actions, drafted and reviewed

_The payoff, minus the sending. Split from step 15 deliberately: drafting and sending are different
risk classes and should not land together._

**Delivers.** Chief drafts the thing before you ask, and you review it. Nothing leaves the machine
yet.

**In scope.** **Migration v6** for `proposed_actions` (source, account_id, dedupe hash, status,
corpus path, created/acted timestamps) — COSTA's `task-action-state.json`, in the store Chief already
has. A daemon pass running a generation recipe for one pending item, copying COSTA's ordering
literally: _load org structure → load writing style → load item context → only then generate_. Body
to `proposed/YYYY-MM-DD-slug.md`; state in SQLite. Review cards in the feed. `[Edit]` opens the
drawer with the draft loaded. A dedicated single-shot transformation path for _"make this less
formal"_ — no tools, no history, no corpus: the cheapest call Chief makes and the one the user feels
most.

**Verified by.** Dedupe, one-per-pass, yielding under `Attention`, and a test that **no network call
occurs** in this step.

**Risk.** Medium-high. Quality depends on a populated corpus, which is step 22. If the order has to
slip, slip it that way.

### Step 15 — The send path (D4)

_A new risk class. It gets its own step, its own review, and its own tests._

**Delivers.** `[Approve]` performs the action against the connected service, with enough on the card
to make that decision safely.

**In scope.**

- **Write scopes**, per provider, requested only where a send is actually offered — and recorded in
  `integration_accounts.scopes`, which step 7 already stores. The Atlassian read-only forfeit (D4) is
  called out in the connect UI rather than buried.
- **Decision context on the card.** For GitHub: the PR's check runs — failed, pending, or passing —
  which needs new fields on `PullRequest` and a new call in `github.rs`. A card whose checks are
  failing or incomplete says so, prominently, in **attention** amber. The design system already
  reserves amber for _you are needed_, which is exactly this.
- **A send is a distinct act.** Never a default, never a keyboard-repeat away, and never batched.
  Show precisely what will be sent, to which account, and where it will land.
- **Migration v7** for a local `sent_actions` audit: what was sent, to which account, when, and the
  provider's identifier for the result. This is the only record that will exist, since Chief reports
  nothing anywhere.
- **Failure is legible.** A rejected token, a revoked scope, a 403 from an admin policy — each says
  what happened and what to do, not "request failed".

**Verified by.** Against stub servers: the correct endpoint and payload; nothing sent without the
explicit act; a failed send leaves the action re-sendable rather than half-done; the audit row is
written. **A test that a draft in the `proposed` state performs no network call** — the boundary
between steps 14 and 15, asserted rather than trusted.

**Risk.** **High.** This is the first code in Chief that changes something outside the machine.
Treat "undo is impossible for most of these" as a design input: prefer actions that are reversible
(a draft, a comment) over ones that are not, and say which is which on the card.

### Steps 16–21 — The integration fan-out

Near-identical in shape once step 9 lands: describe the provider, add its reads, add its tools, add
its recipes. Order is a priority call, not a dependency chain. **Zoom and Linear need the same
primary-source, adversarially verified research pass the other five got before their step begins.**

| Step | Integration                 | Mechanism                                                | Dominant cost                                                                        |
| ---- | --------------------------- | -------------------------------------------------------- | ------------------------------------------------------------------------------------ |
| 16   | **Zoom**                    | PKCE public client, loopback; device flow also available | Unverified: admin approval to leave development; distribution terms                  |
| 17   | **Linear**                  | PKCE, `client_secret` optional — a genuine public client | Unverified: loopback redirect support; admin rights to create the app                |
| 18   | **Google Calendar + Tasks** | Same machinery as step 9, reused                         | Verified domain, hosted privacy policy, demo video; 100-user cap until verified      |
| 19   | **Jira + Confluence**       | DCR-over-MCP, PKCE, no shipped secret                    | Undocumented registration endpoint; paid plan; deterministic tools only              |
| 20   | **Slack**                   | Bring-your-own internal app, REST                        | A distributed app is throttled to 1 req/min and 15 messages — BYO is strictly better |
| 21   | **Gmail**                   | **Send first, read later**                               | `gmail.send` is _sensitive_; reading is _restricted_ and triggers annual paid CASA   |

Two rules from the branch's spec that survive into every one of these:

- **Deterministic tools only.** Never a provider's natural-language search endpoint — that would send
  the user's typed question off the machine. The sidebar promises "Nothing you type leaves this
  machine" on every screen, and that promise is load-bearing.
- **Never declare the MCP `sampling` capability.** A server that can request sampling can run its own
  agentic loop on the user's local model. Client capabilities must be empty, asserted by a test.

### Step 22 — Bootstrapping the corpus

_COSTA's honest finding: stale data produces generic drafts. An empty corpus is step 14's failure
mode._

**Delivers.** A corpus with something in it on day one, derived from work Chief can already see.

**In scope.** `writing_style.md` extracted from the user's own PR descriptions, review comments and —
after step 9 — sent mail: one bounded call over a sample, producing **a draft the user edits**, never
applied silently. `team_structure.md` seeded from collaborators, frequent reviewers and recurring
meeting attendees. A first-run scaffold: empty files with good headings beat an empty folder.

**Verified by.** Extraction against stub data, and a test that it never overwrites a file the user
has edited — `mtime` against the step-10 index.

**Risk.** Low-medium. The privacy story needs stating plainly in the UI.

---

## 5. Out of scope

- **LinkedIn as an API integration.** No member-level read scope exists at any self-serve tier. If
  wanted, it is an archive importer.
- **Moving tokens to the OS keychain.** Step 7 built the `CredentialStore` seam; the move is not
  taken.
- **A general-purpose MCP client.** The context budget forbids it and only one provider would
  benefit. Atlassian gets a narrow module.
- **Embeddings.** `sqlite-vec` is in the stack table and unused. With a small structured corpus,
  path- and manifest-based retrieval is cheaper and more predictable — and an embedding model is
  another resident model on a machine with no room for one.
- **Local Whisper.** Biscotti is the reference if it is ever wanted.
- **Telemetry**, here as everywhere.

---

## 6. What changed, and when

| Revision 1 said                         | Revision 2 says                                          | Why                                                                                                                                            |
| --------------------------------------- | -------------------------------------------------------- | ---------------------------------------------------------------------------------------------------------------------------------------------- |
| Step 7 = hardware probe                 | Step 7 = generic integration layer, **shipped**          | The branch had already built and merged it                                                                                                     |
| Migrations v3, v4, v5                   | v4, v5, v6, v7                                           | **v3 is taken** by `integration_accounts` on the branch                                                                                        |
| D5: local calendar over Microsoft Graph | Outlook via loopback + PKCE, at step 9                   | The branch's research found Graph viable as a public client; the device flow is what Entra blocks, not the auth-code flow. Better research won |
| D4: copy-only, Chief sends nothing      | Approving sends, at step 15                              | User decision. Reverses the branch spec's "Chief reads"                                                                                        |
| Gemma 3 "would stop working"            | Confirmed, with the caveat that a Generic handler exists | Verified against llama.cpp's docs and the branch's own code                                                                                    |
| One linear sequence                     | Product spine (8–15) + reorderable fan-out (16–21)       | The integration steps are genuinely independent of each other                                                                                  |
| Five providers                          | Seven, adding Zoom and Linear                            | User decision. Both are unresearched and flagged as such                                                                                       |

---

## 7. Notes for the executing agent

1. **Read [CLAUDE.md](../CLAUDE.md) fully before the first commit.** It is binding.
2. **Read the branch's two design documents** before any integration step. They are primary-source
   researched and adversarially verified; do not re-derive or contradict them without equal evidence.
3. **Land `feature/integration-roadmap-priority-5cb1bc` before step 8.**
4. **Check the next free migration version before writing one.** Revision 1 got this wrong.
5. **One step per pull request. Stop for review.**
6. **`pnpm verify` before every push**, not `pnpm check`.
7. **Never `fetch` from the renderer.** ESLint blocks it, and the block is the point.
8. **Every new outbound host is a violation** unless it is loopback or a service the user connected.
   There is exactly one exception in the tree (`weights.rs`) and it stays exactly one.
9. **Test through the accessible surface**; anything layout-dependent goes in `e2e/`.
10. **Prefer extending a tested pattern to inventing one.** `run_once` taking a `Context` rather than
    an `AppHandle` is why a whole background pass is testable; recipes should be built the same way.
