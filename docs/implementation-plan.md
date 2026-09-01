# Chief — Implementation Plan

A synthesis of the **Enterprise Blueprint**, the **PRD** and **The Hybrid Model ("Action-Backed
Chat")**, reconciled against the code in this repository — including the work on
`feature/integration-roadmap-priority-5cb1bc` — and written to be executed autonomously.

**Revision 6.** Revision 1 was written against `main` and did not account for the integration
branch. Revision 2 reconciled with it. Revision 3 took all eight decisions. Revision 4 was the first to
live on `main`. Revision 5 was the first written in the same pull request as the code it
describes. **Revision 6 reconciles the plan with the Decoupled Local Engine specification** — see
§0 for the ledger, §2's D9 for the decision it turns on, and §6 for what changed.

> **This is a living document.** It is updated in the same pull request as the work it describes,
> not afterwards. §8 says how. A plan that records what was intended, and never what happened, is
> a historical artefact — and this one was load-bearing for seven steps while sitting unmerged on
> a branch, which is the failure §8 exists to prevent.

---

## 0. Status ledger

The current state of every step. **Update this table in the pull request that changes it.**

| Step  | What                                   | State                          | Issue            | Landed as |
| ----- | -------------------------------------- | ------------------------------ | ---------------- | --------- |
| 1–6   | Scaffolding through work log           | Shipped                        | —                | pre-plan  |
| 7     | Generic integration layer              | Shipped                        | —                | pre-plan  |
| 8     | Model revert, probe, tiering, thrift   | Shipped                        | —                | —         |
| 9     | Outlook Mail and Calendar              | Shipped                        | —                | —         |
| 10    | Corpus layer and context budget        | Shipped                        | REC-10 (watcher) | #57       |
| 11    | Recipe engine and the first brief      | Shipped                        | —                | —         |
| 12    | Executive Feed and the chat drawer     | Shipped                        | REC-14           | #52       |
| 13    | Deterministic intent routing           | Shipped                        | REC-15           | #53       |
| 14    | Proposed Actions, drafted              | Shipped                        | REC-17           | #55       |
| 15    | The send path                          | **Not started — deliberately** | REC-18           | —         |
| 16–21 | Integration fan-out                    | Not started                    | —                | —         |
| 22    | Bootstrapping the corpus               | Shipped                        | REC-16           | #54       |
| —     | Calendar by .ics subscription          | Shipped                        | REC-39           | #60       |
| —     | Linear by pasted API key               | Shipped                        | REC-40           | #61       |
| —     | "What is waiting on me?"               | Shipped                        | REC-41           | #62       |
| —     | DLE — structured log, FTS5, sync state | Not started                    | DLE-0            | —         |
| —     | DLE — zero-network read path           | Not started                    | DLE-1            | —         |
| —     | DLE — deterministic ingestion          | Not started                    | DLE-2            | —         |
| —     | DLE — freshness in the interface       | Not started                    | DLE-3            | —         |
| —     | DLE — unlinking deletes its data       | Not started                    | DLE-4            | —         |
| —     | DLE — local directory permissions      | Not started                    | DLE-5            | —         |
| —     | DLE — model adapter seam               | Not started                    | DLE-6            | —         |
| —     | DLE — prompt truncation and TTFT       | Not started                    | DLE-7            | —         |
| —     | DLE — monthly journal roll-up          | Not started                    | DLE-8            | —         |
| —     | DLE — provenance footers               | Not started                    | DLE-9            | —         |
| —     | DLE — action links from the feed       | Not started                    | DLE-10           | —         |
| —     | DLE — model-swap and concurrency       | Not started                    | DLE-11           | —         |

**Step 22 was built before step 14**, out of the numbered order and on the plan's own advice: an
empty corpus is step 14's failure mode, and a draft written against seven empty starter files is
the generic output that step exists to avoid.

### Decisions, as built

| Decision | State                          | Note                                                        |
| -------- | ------------------------------ | ----------------------------------------------------------- |
| D1       | Taken, **premise corrected**   | See D1: `supports_tools` was never built and was not needed |
| D2       | Taken and **enforced by test** | `agent::tests::keeps_the_tool_catalogue_within_its_budget`  |
| D3       | Taken, built                   | `corpus.rs`                                                 |
| D4       | Taken, **not yet built**       | Step 15. The one step left for a person                     |
| D5       | Taken, partially built         | Microsoft shipped; the rest of the fan-out is not started   |
| D6       | Taken, built, **measured**     | `e2e/feed.spec.ts` asserts the detail width does not change |
| D7       | Taken, built                   | `engine.rs`                                                 |
| D8       | Acknowledged, **still open**   | A measurement on real hardware. See §9                      |
| D9       | Taken, **not yet built**       | Reads from local storage. DLE-0 through DLE-11              |

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

#### What was actually built, and where the premise was wrong — revision 4

The revert happened and the model is Llama 3.2 3B. **Two things this section specified were not
built, and should not be:**

- **`supports_tools` per catalogue entry.** `weights.rs` has a two-model `CATALOGUE` and no such
  flag. Nothing reads one.
- **Disabling the tool loop on the Light tier.** The loop runs on both tiers.

**The premise was wrong.** This section assumed a 1B-class model could not be trusted with the tool
loop. Measured, it can: the 1B tool-called correctly on 3 of 3 attempts with valid arguments when
the tool fitted the question. What it does badly is call a tool for a question that needed none —
which is a _precision_ problem, not a capability one, and a flag saying "this model cannot use
tools" would have been the wrong fix for it.

**What was built instead** is degradation on engine rejection: `agent::is_unusable_tool_output`
recognises llama.cpp answering 500 because the model's tool output did not fit the template's
grammar, and retries the same question without tools rather than failing the conversation. That
handles the real failure — a generation that came out malformed — rather than a whole tier of model
being pre-emptively distrusted.

The precision problem is answered elsewhere and better: **step 13's intent router** takes the
questions that should never have reached a tool at all, at zero token cost.

#### Proposed a second time, and refused again — revision 6

The DLE specification asks for a `ModelAdapter` carrying `supports_native_tools()`. That is this
flag, under a different name, and it is refused for the reasons above rather than re-argued.
Recorded here so it is not proposed a third time.

Its sibling, `ExecutionStrategy::GrammarConstrainedJson`, is refused on a different ground: **it
would have no users.** `weights.rs`'s catalogue holds two models and both are Llama, which
llama.cpp lists as natively tool-calling. A code path no catalogue entry reaches cannot be tested
against anything real, and a test that only exercises a stub is how a guard comes to assert
nothing. Build it on the day a model that needs it enters the catalogue.

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

### D9 — A read question is answered from local storage, never from a live API. ✅

The Decoupled Local Engine specification's first tier is right, and Chief does not meet it today.
`intent::prep` — _"what are my meetings?"_, which is the specification's own example of a read —
calls Microsoft Graph while the user waits (`intent.rs:145-158`), and all three tools in the
catalogue reach the network (`tools.rs:219-236`). So **every read question that falls through the
router leaves the machine**, and its latency is a third party's to decide.

The decision: the daemon ingests into `work_logs`; a read question is answered from an FTS5 index
over that table; the model, when it is called at all, sees only what SQLite returned. Reads become
sub-second and work offline.

**What it costs, stated plainly. An answer is only as fresh as the last sync.** Chief used to be
wrong slowly; it can now be stale quickly, and staleness is invisible in a way a spinner is not.
That is the whole reason `sync_state` is surfaced in the interface (DLE-3) rather than kept as an
implementation detail — the trade is only honest if the user can see it.

Three consequences worth naming before they are discovered:

- **The tool catalogue's budget stops binding.** Tools that fetch become tools that query, or stop
  being tools at all. The 515-of-600 figure in §9 is measured against the current three and will
  move; measure it again rather than assuming which way.
- **Two ceilings, and neither absorbs the other.** `context::RETRIEVAL_CEILING` (300 tokens) governs
  the snippet a read question injects; `context::DEFAULT_CEILING` (2,400) still governs recipe and
  brief assembly. Collapsing them to one number would gut the brief — see §3.
- **The write path is untouched.** D4 and step 15 are unaffected: an action still reaches the
  network, deliberately and once, and that boundary stays exactly where it is.

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

### Three numbers the DLE specification asked for, corrected — revision 6

Recorded here rather than argued again each time somebody proposes them.

**Time to first token is not a number you can pick.** The specification asks for **≤ 1.5s at ≤ 2,000
prompt tokens** on CPU prefill. At the 18–34 tokens a second this section describes, 2,000 tokens is
**60 to 110 seconds**; even 300 tokens is 9 to 17. The target is off by roughly two orders of
magnitude against a cold prefill, and it is only meaningful for the **volatile suffix past a cached
prefix** — which is lever 2 above, and the reason lever 2 is worth building. So the gate reads
_"≤ 1.5s for the suffix on a warm prefix"_, it is filled in by `chief doctor` on real hardware, and
**CI never asserts it**: a runner cannot answer that question, which is the whole of D8.

**Resident memory is chosen, not capped.** The specification asks to "cap runtime allocations" to
≤ 4 GB. llama.cpp has no such cap; the number is the model plus its KV cache, and both are decided
before the server starts. Llama 3.2 3B is 28 layers × 8 KV heads × 128 head dim, so **112 KiB per
token** of f16 KV — **0.875 GiB at ctx 8192**, on top of ~2.0 GB of weights, ≈ **2.9 GB**. That fits
under 4 GB with less room than the specification implies. The three levers that actually move it are
the tier (`probe.rs`), `--ctx-size` (D7) and **KV quantisation** (`--cache-type-k/v q8_0`, roughly
halving the KV term), which Chief does not currently use. Enforcement is selection plus measurement.

**Two ceilings, not one.** The specification asks for ≤ 300 tokens of injected context. That is
right for a read question and wrong for a brief. Measured on its own example line, a row carrying an
inline GitHub link costs 34–38 tokens — **the URL alone is 13–15 of them** — so 300 tokens buys 8 or
9 rows. Drop the URL, which the model has no use for and which the interface renders from
`work_logs.url` anyway, and the same row costs 14–16 tokens: **19 to 21 rows in the same 300**. Eight
rows is not an answer to "what did I ship this week?"; twenty is. So:

| Ceiling                      | Value | Governs                                           |
| ---------------------------- | ----- | ------------------------------------------------- |
| `context::RETRIEVAL_CEILING` | 300   | the FTS5 snippet a read question injects (new)    |
| `context::DEFAULT_CEILING`   | 2,400 | recipe and brief assembly (unchanged, D7)         |
| assembled prompt             | 2,000 | hard truncation of **chat history**, oldest first |

One caveat that belongs beside the 300 rather than in a footnote: it is measured with
`context::BYTES_PER_TOKEN`, which is 3 and still uncalibrated (§9). The gate therefore has to use
the **same estimator as the truncator**, so it is at least self-consistent, and be written against
the estimate rather than a true token count.

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

**Built — revision 4.** `probe.rs`, `weights.rs`'s `CATALOGUE`, the tier in setup and the idle stop
all shipped. The daemon risk was answered by holding the engine's `working()` guard for the whole
pass rather than just its start, because a pass is one model call per merged pull request and then
the brief, which on a small model runs past the idle timeout.

Two things this step promised are **still open**:

- **`supports_tools` in the catalogue** — not built, and correctly so. See D1.
- **The `chief doctor` measurements.** The command exists; the numbers this document was to record
  are still blank, because they are a measurement on real hardware rather than something an agent
  can derive. Tracked as D8 in §9.

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

**Complete — revision 5.** The watcher landed as REC-10. `watcher.rs` holds a pure `Debounce`
driven by an injected `Instant`, so a burst collapsing into one reindex is tested without sleeping
through it, and `notify` supplies the events.

One thing this step's warning got slightly wrong, and it is worth recording rather than quietly
building around. The plan says a watcher "must not react to itself" because Chief writes here too,
implying a mechanism for telling Chief's writes from a person's. **No such mechanism is needed.**
The storm it fears is a cycle, and a cycle needs an edge from reindexing back to the filesystem —
reindexing writes to SQLite and leaves the folder byte-identical, so there is nothing to close the
loop however many events arrive. `reindexing_does_not_touch_the_corpus_at_all` asserts exactly that.
What Chief's own writes cost is one reindex per burst, the same as anybody else's, and the debounce
is what makes it one rather than several.

The estimator is still 3 bytes per token and still uncalibrated. See §9.

**Previously, revision 4 said:** `corpus.rs`, migration v5's `corpus_files`, `context.rs`
with its budget and minifier all shipped. **The debounced watcher was not built**, and there is no
`notify` dependency in `Cargo.toml`.

The gap is smaller than it looks and larger than it sounds. Every command that touches the corpus
reindexes, so the index is correct _whenever Chief looks_ — what is missing is noticing a file the
user edited in their own editor before something else triggers a scan. Tracked as **REC-10**.

The estimator is **3 bytes per token** (`context::BYTES_PER_TOKEN`), not the four this step warned
against guessing, and not yet calibrated against the real tokenizer. That calibration is unclaimed
work, not a decision.

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

### The Decoupled Local Engine — D9, as twelve issues

_Not a step. The spine's steps are strictly ordered because each one's substrate is the one before
it; these are deliberately not, and the shape is the point._

**Delivers.** D9: a read question answered from SQLite, in the time a query takes, with the network
untouched.

**One issue blocks, eleven do not.** The single genuine coupling in this work is the schema, so it
is confined to **DLE-0** — migration **v8** (the next free version; v7 is `proposed_actions`), plus
the seams the rest fill in: `retrieval::search`, `work_log::upsert`, and the `sync_state` accessors.
`scripts/check-migrations.mjs` requires contiguous versions, so two branches each writing v8 would
collide on merge — which is exactly why there is only one. After DLE-0 lands, each remaining issue
owns its own module and the only shared file is `lib.rs`, which gains one registration line each.

| Issue     | Owns                                                    | Note                                      |
| --------- | ------------------------------------------------------- | ----------------------------------------- |
| **DLE-0** | `db.rs`, `retrieval.rs`, `sync_state.rs`, `work_log.rs` | Blocks the other eleven                   |
| DLE-1     | `intent.rs`, `retrieval.rs`, `context.rs`               | The zero-network read path                |
| DLE-2     | `ingest.rs`, `daemon.rs`, `settings.rs`                 | Deterministic minification, no model call |
| DLE-3     | `sync_state.rs`, `SettingsView.tsx`, `Layout.tsx`       | D9's cost, made visible                   |
| DLE-4     | `connect.rs`, `integrations.rs`, `proposed.rs`          | Unlinking deletes its data                |
| DLE-5     | `perms.rs`, `corpus.rs`                                 | `0700` on the corpus **and** app config   |
| DLE-6     | `adapter.rs`                                            | The seam, not the capability flag         |
| DLE-7     | `llama.rs`, `engine.rs`, `probe.rs`                     | Truncation; TTFT measured, not asserted   |
| DLE-8     | `journal.rs`                                            | Roll up, delete nothing                   |
| DLE-9     | `ChatView.tsx`, `agent.rs`                              | **blockedBy DLE-1** — the one edge left   |
| DLE-10    | `TodayView.tsx`, `work_log.rs`                          | Links from `work_logs.url`, no model      |
| DLE-11    | `db.rs` tests, `weights.rs` tests                       | Model-swap isolation; WAL concurrency     |

Three mechanics make that table true rather than aspirational. A command lives in the module that
owns it, never in `connect.rs`, which DLE-4 rewrites. DLE-8's monthly pass is a `journal::run_once`
taking a `Context`, following `daemon::run_once`, so `daemon.rs` gains a call and not a body. And
DLE-2 removes the model call from ingestion, which is what lets the cadence move at all — a pass
that wakes the engine every 15 minutes against a 10-minute `IDLE_TIMEOUT` would mean the engine
never idles, which is §3's third lever undone.

**In scope, against what is already built.** Migration v8 is **additive**: `content` and `account_id`
stay, and `external_id` stays nullable behind the partial index migration v3 rebuilt
(`(source, account_id, external_id) WHERE external_id IS NOT NULL`). The specification's
`external_id TEXT UNIQUE NOT NULL` would break both a hand-written entry and a second account on the
same service. `sync_state` is keyed on `account_id`, not on `source`, for the same reason: a person
with a work and a personal GitHub has two. The FTS5 table is external-content over `work_logs` with
Porter stemming, and its migration **must** end in
`INSERT INTO work_logs_fts(work_logs_fts) VALUES('rebuild')` — without it every existing install
searches an empty index and returns nothing while looking perfectly healthy.

**Verified by.** Guard tests, every one of them through the `proving-a-guard-test` skill: the
rebuild, the ceiling, the "no non-loopback request", the "nothing was deleted", the "no duplicate
row". Two of the specification's own five acceptance criteria do not survive contact and are
restated rather than copied. **"0 outgoing HTTP requests" is impossible** — every answer talks to
`127.0.0.1:11435` — so it reads _no request to a non-loopback host_, built the way
`propose::never_reaches_the_network_to_draft` is, with the request list asserted non-empty before it
is asserted clean. **The stemming test fails as written**: FTS5 `MATCH` is AND across terms, so
`"login refactor"` against _"Refactored OAuth handler"_ matches `refactor` and not `login`, and
returns nothing on a correct implementation. It becomes a real stem pair, and the AND-versus-OR
choice is written down instead of inherited.

**Risk.** Medium. The schema is live and populated, which is why the migration is one issue and
carries `risk:high`. The rest is additive.

**Two things the specification asked for and this does not build.** Neither is a scoping accident.
**Pruning `work_logs` after 30 days** deletes the only copy of the user's history — "what did I ship
last quarter?" stops working, and a hand-written entry cannot be re-fetched — so DLE-8 writes the
monthly roll-up and keeps every row; whether pruning is ever wanted is in §9. **Battery-aware
polling** (15 minutes on AC, 45 on battery) needs a new crate and platform-conditional code, which
CLAUDE.md records as where the last two bugs on `main` came from, and Tauri raises no sleep event
to pause on; the cadence becomes a `settings` value defaulting to 30 minutes, a missed pass is
detected from monotonic-versus-wall-clock drift, and the AC/battery split is in §9.

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

### Revision 3 → revision 4

| Revision 3 said                              | Revision 4 says                               | Why                                                                                               |
| -------------------------------------------- | --------------------------------------------- | ------------------------------------------------------------------------------------------------- |
| Lives on a feature branch                    | Lives on `main`, and is updated as work lands | It was load-bearing for seven steps while unmerged and unversioned. REC-11                        |
| D1: `supports_tools` flag, no tools on Light | Neither built, and the premise was wrong      | The 1B tool-calls correctly (3/3) when a tool fits. The real problem is precision, not capability |
| Step 10: a debounced watcher                 | Not built; tracked as REC-10                  | Reindex-on-command covers the correctness; the watcher is the optimisation                        |
| Step 8: `chief doctor` measurements          | Command built, numbers still blank            | A measurement on real hardware, not something an agent can derive. See §9                         |
| Step 14 after step 22 in number order        | Step 22 built **first**                       | An empty corpus is step 14's failure mode. The plan said so; the order followed the advice        |
| The catalogue costs ~882 tokens              | 477 across 3 tools, measured                  | Measured on `main` at step 13. The 6-tool/600-token cap still binds, at the fourth tool           |

### Revision 4 → revision 5

| Revision 4 said                            | Revision 5 says                                    | Why                                                                                    |
| ------------------------------------------ | -------------------------------------------------- | -------------------------------------------------------------------------------------- |
| Step 10 partial; watcher tracked as REC-10 | Step 10 complete                                   | The watcher landed. `watcher.rs`, `notify`, a pure debounce                            |
| A watcher must not react to Chief's writes | It cannot loop, and needs no mechanism to avoid it | Reindexing writes only to SQLite, so the cycle has no closing edge. Asserted by a test |

### Revision 5 → revision 6

Prompted by the Decoupled Local Engine specification, reconciled against the code rather than
adopted.

| Revision 5 said                             | Revision 6 says                                                     | Why                                                                                                                                           |
| ------------------------------------------- | ------------------------------------------------------------------- | --------------------------------------------------------------------------------------------------------------------------------------------- |
| Eight decisions                             | Nine. **D9** — a read is answered from local storage                | The specification's first tier is right, and Chief does not meet it: `intent::prep` calls Graph while the user waits                          |
| One injected-context ceiling of 2,400       | Two, named separately: 300 retrieval, 2,400 assembly                | 300 is right for a read row and would gut the brief. Measured: dropping the URL takes a row from 34–38 tokens to 14–16, so 300 buys 20 not 8  |
| —                                           | TTFT is a **warm-prefix** measurement, never a CI assertion         | ≤1.5s at 2,000 prompt tokens is 60–110s at this machine's prefill rate. The target only means anything past a cached prefix. D8               |
| —                                           | Resident memory is chosen, not capped                               | llama.cpp has no allocation cap. 112 KiB/token of KV → 0.875 GiB at ctx 8192 on ~2.0 GB of weights. KV quantisation is the unused lever       |
| D1: `supports_tools` was refused once       | Refused again, and `GrammarConstrainedJson` with it                 | The specification proposes the same flag as `supports_native_tools`. Both catalogue models are Llama, so the grammar path would have no users |
| Steps are strictly ordered within the spine | The DLE work is twelve issues, one of which blocks the other eleven | The only real coupling is migration v8. Confining it to one issue is what lets the rest be worked at once                                     |
| Daemon interval fixed at 30 minutes         | A `settings` value, default 30. AC/battery split deferred           | A 15-minute pass fights the 10-minute `IDLE_TIMEOUT`; battery state needs a crate and platform code. §9                                       |
| —                                           | Old work is rolled up, never pruned                                 | The specification deletes rows after 30 days. That is the only copy, and a hand-written entry cannot be re-fetched. User decision             |

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

---

## 8. Keeping this document alive

This plan is **updated in the pull request that changes it**, never afterwards and never in a
sweep. Revision 3 sat on a branch while seven steps were built from it, which is exactly how a plan
stops describing the thing it planned.

What a step's pull request must do here:

1. **Move its row in §0.** State, issue, and the pull request it landed as.
2. **Record what was built instead**, wherever the plan said something different. Add a
   `— revision N` note under the section rather than rewriting it: what was intended and what
   happened are both worth having, and deleting the first hides why the second was necessary.
3. **Add a row to §6** if a decision or a premise changed, saying what and why.
4. **Add anything new to §9** — an issue raised in passing, a decision that surfaced and was not
   taken, a measurement that is still blank.
5. **Bump the revision number** in the header when §6 gains a row.

What this document is _not_: a design system, a changelog, or a status report for anybody outside
the repository. Prose is fine. Tables are fine because they parse cleanly. It is written to be read
by whoever picks up the next step, including an agent with no memory of this one.

`CHANGELOG.md` records what shipped to users. This records **what was decided, what was built
instead, and what is still open** — which is the part a released binary cannot tell you.

---

## 9. Open questions and unclaimed work

Everything known to be outstanding. **A step's pull request adds to this; a step's pull request
removes from it.**

### Decisions waiting on a person

| What                              | Why it is not an agent's call                                                                              | Where  |
| --------------------------------- | ---------------------------------------------------------------------------------------------------------- | ------ |
| **The send path (D4)**            | The first code that changes something outside this machine. Deliberately unattended-proof                  | REC-18 |
| **Slash command discoverability** | Three defensible designs, and a question underneath about whether a routed answer should say it was routed | REC-33 |
| **Brief quality**                 | Needs a product decision between three stated options, not an implementation                               | —      |

### Measurements nobody has taken

| What                                | Blocked on                                                                                                                                    | Where   |
| ----------------------------------- | --------------------------------------------------------------------------------------------------------------------------------------------- | ------- |
| **D8 — hardware verification**      | Real target hardware. `chief doctor` exists; the numbers do not                                                                               | D8      |
| **Token estimator calibration**     | The real tokenizer. Currently 3 bytes per token, uncalibrated                                                                                 | Step 10 |
| **Windows build validation**        | A Windows machine. CI compiles it; nobody has run it                                                                                          | —       |
| **Nothing ever opens the real app** | `tauri-driver` against the packaged binary. Every test runs in V8 or Chromium, so syntax the shipped WebView rejects reaches `main` — one did | REC-34  |

### Unclaimed engineering

| What                                   | Note                                                                                                                                      | Where       |
| -------------------------------------- | ----------------------------------------------------------------------------------------------------------------------------------------- | ----------- |
| **Tokens in the OS keychain**          | Stored as plain text, protected by the OS user account                                                                                    | —           |
| **Reviewers in `team_structure.md`**   | GitHub's search response carries none, so it is a call per pull request                                                                   | REC-16      |
| **`glib` GHSA-wrw7-89jp-8q8g**         | Accepted, not fixed. Linux-only and unreachable from a shipped build                                                                      | SECURITY.md |
| **AC/battery polling cadence**         | Needs a battery crate and platform-conditional code. Cadence is a `settings` value meanwhile                                              | DLE-2       |
| **Whether `work_logs` is ever pruned** | DLE-8 rolls up and keeps every row. Pruning is a product call and needs evidence the table is a problem                                   | DLE-8       |
| **`busy_timeout` through the plugin**  | `tauri-plugin-sql` owns the pool and `busy_timeout` is per-connection, so it may not be reachable without patching. `sqlx` defaults to 5s | DLE-11      |
| **FTS5 query semantics**               | OR-joined and `bm25()`-ranked, chosen because a read question is a recall problem. Revisit against real usage                             | DLE-0       |

### Settled during implementation, recorded so it is not re-litigated

- **A file the user edited is judged by content, not mtime** (`profile::is_untouched`). mtime moves
  on a checkout, a sync or a restored backup without a byte changing.
- **A routed intent that finds nothing steps aside** rather than answering "you have nothing",
  because to the reader those are indistinguishable from a misunderstanding.
- **`/brief` reads today's brief and never regenerates it.** Writing one is a model call.
- **A dismissed proposal keeps its dedupe slot**, or the next pass drafts it again.
- **A calendar subscription address is a credential, not a setting.** It grants read access to a
  whole calendar to anyone holding it, so it is stored like a token and never displayed.
- **A new integration does not get a new tool.** The catalogue is at 515 of D2's 600-token cap, so
  the fourth breaks it. New sources feed the recipes and the router, which cost nothing per turn.
- **A pasted key or URL is a credential, not a setting.** Both Linear and the calendar store one the
  way an OAuth token is stored, and neither is ever shown again.
- **"Open" is never a workflow state name.** Linear workspaces rename theirs; `completedAt` and
  `canceledAt` being null is the question that survives it.
- **"Waiting on you" and "your open work" are different questions.** Chief answered the first with
  the second for as long as the composer had been offering it. They are separate buckets now.
- **The catalogue is at 515 of 600 tokens.** Measure before touching it; 85 is about half a
  parameter, and the number goes in the pull request every time it moves.
- **The corpus watcher needs no way to tell Chief's writes from a person's.** Reindexing writes only
  to SQLite, so a self-triggered loop has no closing edge. The debounce is the whole mechanism.
- **The watcher is an optimisation, not a dependency.** Reindex-on-command still runs, so a machine
  where the watch could not be established is exactly as correct as one from before it existed.
- **The URL is the expensive half of a retrieved row, and the model has no use for it.** 13–15 of a
  row's 34–38 tokens. It is stripped from the injected context and re-attached by Rust from
  `work_logs.url` afterwards, which also means a link cannot be hallucinated.
- **A provenance footer is written by Rust, never by the model.** A footer the model composes is a
  footer it can get wrong, which is the one thing a provenance footer may not be.
- **`purge` keys on `account_id`, never on `source`.** Migration v3 made the account the unit;
  unlinking one of two GitHub accounts must not take the other's history with it.
- **`0700` on the corpus is not enough on its own.** The corpus holds markdown; the database and the
  OAuth tokens live in the app-config directory, and that is the one the specification omitted.
- **An external-content FTS5 migration ends in `'rebuild'`.** Without it the index is empty on every
  existing install and search returns nothing while looking healthy.
