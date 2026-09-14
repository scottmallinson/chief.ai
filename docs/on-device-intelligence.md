# Can Chief use the operating system's own intelligence?

**REC-31.** Apple ships an on-device language model inside macOS; Microsoft ships one inside
Windows. Both are free, both are already on the user's machine, and Chief currently downloads two
gigabytes of its own to do the same job. The question is whether Chief should use theirs.

Researched 2026-09-14. Everything below is dated, because both platforms moved twice while this
was being written and one of them is mid-replacement right now.

---

## The answer

**Not as a replacement for the engine. Yes, eventually, for three specific calls — and not yet.**

The finding that decides it is not about either API. It is that **neither covers every machine
Chief ships to**, so `llama-server` has to stay regardless. That turns "use the OS model" from
_replacing_ a dependency into _adding_ one: a second and third inference backend beside the one
that still has to work everywhere.

Chief has already refused a smaller version of this. `adapter.rs` records the refusal of
`supports_native_tools()` — "a capability flag would be a permanent branch in the prompt path
standing in for a problem that turned out not to exist". Two OS backends is that branch, three
times over, standing in for a download.

There is a narrower version that is worth doing, argued in [§6](#6-the-narrow-version-that-is-worth-doing).

---

## 1. What the two platforms offer

|                   | **Apple Foundation Models**                                      | **Windows AI / Phi Silica**                                  |
| ----------------- | ---------------------------------------------------------------- | ------------------------------------------------------------ |
| Model             | ~3B on-device, powers Apple Intelligence                         | Phi Silica, **being removed** (see §5)                       |
| API               | Swift, `SystemLanguageModel`                                     | WinRT, `Microsoft.Windows.AI.Text.LanguageModel`             |
| Prompt shape      | Messages, session-based                                          | **A string** — `GenerateResponseAsync(prompt)`               |
| Tool calling      | ✅ Native                                                        | ❌ Not offered                                               |
| Structured output | ✅ Guided generation (`@Generable`)                              | Fixed "Text Intelligence Skills" only                        |
| Context window    | **4,096 tokens**, input _and_ output                             | Not published; prompt compression on NPU only                |
| Streaming         | ✅                                                               | ✅                                                           |
| Cost              | Free, no request limits on-device                                | Free                                                         |
| Availability gate | Apple Intelligence enabled, sufficient battery, not in Game Mode | Copilot+ NPU, or RTX 30+/RX 9060+ GPU, or CPU                |
| Distribution      | Model ships with the OS                                          | NPU: preinstalled. GPU: **several-GB download** on first use |

Both keep inference on the device. Apple's Private Cloud Compute is a **separate, larger model
that the app must ask for by name** — the on-device path never silently escalates to it. That
matters: Chief's one non-negotiable rule would be broken by a backend that could quietly send a
user's work log off the machine, and neither of these does.

## 2. Four constraints, measured against Chief

### 2.1 Tool calling — Windows fails this outright

Chief's agent loop is `ask → run any tool_calls → append each result as a tool message → repeat`.
`adapter.rs` is explicit about why the prompt is a message list and never a rendered string:

> The specification's `format_prompt(system, context, query) -> String` would break tool calling
> outright: Chief speaks the OpenAI chat-completions contract to `llama-server --jinja`, and a tool
> call goes through the _model's own_ chat template. A flat string bypasses the template, which is
> the one thing that must not happen — so the return type is the guard.

Windows' API is `GenerateResponseAsync(string prompt)`. It is exactly the shape `adapter.rs`
refuses, and it offers no tool calling to compensate. **The Windows model cannot serve Chief's
agent loop at all**, today or after the replacement, unless Microsoft adds function calling.

Apple's can: tool calling is native, and guided generation is a stronger constraint mechanism than
Chief has today.

### 2.2 Context window — Apple's is Chief's Light tier, exactly

Apple's on-device model has **4,096 tokens for input and output together**, and throws
`GenerationError.exceededContextWindowSize` when that is passed.

Chief runs 8192 on Standard and **4096 on Light** (`probe.rs`). So Apple's window is not a
disqualifier — it is precisely what Chief already gives a Light-tier machine. But it is a ceiling
that cannot be raised, on hardware that would otherwise have qualified for Standard: an M4 with
32 GB is a Standard machine by Chief's own thresholds, and the OS model would hold it at Light.

That is the wrong direction. The whole point of the tier is that a better machine gets a better
answer.

### 2.3 Coverage — the one that settles it

| Platform | Reaches                                               | Does not reach                                    |
| -------- | ----------------------------------------------------- | ------------------------------------------------- |
| Apple    | Apple Silicon with Apple Intelligence **turned on**   | Intel Macs — which Chief still ships a bundle for |
| Windows  | Copilot+ NPU, RTX 30+/RX 9060+ with Developer Mode on | Everything else; **not available in China**       |

Chief ships macOS Apple silicon, macOS Intel and Windows. The Intel bundle alone means the Apple
path can never be the only path. On Windows the GPU route needs Developer Mode enabled and a
manufacturer beta driver, which is not a thing somebody who double-clicked an installer will do.

So `llama-server` stays. Every OS backend is additive.

### 2.4 Content filtering — a behaviour difference, not a blocker

Windows applies content moderation with configurable severity (`ContentFilterOptions`). Chief reads
a person's own work log and their own writing. A filter that refuses to summarise a pull request
because of its language is a failure mode Chief does not have today, and one the user could not
diagnose. It is configurable, so it is manageable — but it is a thing to set deliberately rather
than inherit.

## 3. Reachability from Tauri

Both are reachable from Rust. Neither is free.

**Apple** is Swift-only — there is no Objective-C interface. The established pattern is a small
Swift file compiled at build time by `build.rs` via `xcrun swiftc`, exporting `@_cdecl` C-ABI
functions that Rust calls through `extern "C"`. Community crates already do this
(`fm-bindings`, `rusty_foundationmodels`, `foundation-models`). It adds a Swift toolchain
requirement to the macOS build.

**Windows** needs the Windows App SDK, not just inbox WinRT: `microsoft/windows-app-rs` deploys the
bootstrapper from `build.rs` with `bootstrap::deploy::to_output_dir()` and initialises it at runtime
with `bootstrap::initialize()`. Phi Silica additionally sits behind a **Limited Access Feature**
token, requested from Microsoft through a form — a third-party portal, which is exactly the kind of
dependency that makes an issue not finishable by an agent.

## 4. What it would actually buy

Honestly accounted, against what Chief does now:

| Claimed benefit                  | What it is really worth                                                                  |
| -------------------------------- | ---------------------------------------------------------------------------------------- |
| No 2 GB download                 | **Real, on the covered subset.** The biggest first-run cost, gone — for some users       |
| NPU acceleration, better battery | **Real on Copilot+.** Chief's engine is a CPU build by deliberate choice                 |
| No model to bundle or update     | Partly — the sidecar still ships for everyone else                                       |
| Better quality                   | **Unproven, and unlikely to matter.** D9 means read questions never reach a model at all |

That last row is the one that deflates this. **Chief's architecture is built around not calling the
model.** `intent.rs` answers every recognised read question from SQLite; `ingest.rs` derives work-log
rows deterministically and its module doc says so in those words — "Nothing here asks the model
anything." A faster model makes a shrinking fraction of interactions faster.

There are exactly **five** model call sites in the app:

| Site             | Shape                                                  |
| ---------------- | ------------------------------------------------------ |
| `agent.rs:372`   | Tool loop — messages, tools, streaming                 |
| `agent.rs:468`   | Closing call — messages                                |
| `propose.rs:220` | Draft a proposal — **one user message, no tools**      |
| `propose.rs:274` | Refine a draft — **one user message, no tools**        |
| `profile.rs:224` | Extract writing style — **one user message, no tools** |

## 5. The Windows timing problem

**Phi Silica is being removed.** Microsoft's own documentation carries the schedule:

- **Early October 2026** — standalone sideloadable Aion Instruct package for testing
- **October 2026** — Aion Instruct to Windows Insiders; both models present, chosen by a Controlled Feature Rollout
- **November 2026** — Aion Instruct to retail, **and Phi Silica is removed**

Aion Instruct runs on CPU, GPU or NPU without a dedicated GPU, and **needs no Limited Access
Feature token** — both improvements. But it is six weeks from retail as of this writing, and its API
surface and capabilities are not yet settled enough to design against.

**Building anything on Phi Silica now would be building on something with a removal date.** That
alone defers the Windows side past November 2026, independent of every other argument here.

## 6. The narrow version that is worth doing

Three of Chief's five model calls — `propose.rs:220`, `propose.rs:274`, `profile.rs:224` — are
**one user message in, prose out, no tools, no history**. `refine_draft` is described in `CLAUDE.md`
as "the cheapest call Chief makes and the one the user feels most, because they make it repeatedly
while looking at the result."

Those three are exactly the shape both OS APIs serve well, and they fit inside 4,096 tokens
comfortably. They need no tool calling, so Windows' string API is no longer a disqualifier. And
they are the calls where latency is felt directly by somebody watching.

This is the version worth building, because it asks the OS model to do only what it is good at and
leaves the agent loop — the part that needs tools, a large window, and a chat template — on
`llama-server` where it already works.

### The seam already exists

`adapter.rs` is the seam between how a question is answered and which model is loaded. It currently
has one axis (`ExecutionStrategy`). A second — _which backend serves this call_ — belongs beside
it, decided per call site rather than per machine, with `llama-server` as the fallback that is
always correct.

Note what this deliberately is **not**: a capability flag on the model. `adapter.rs` refused that
twice, and the refusal holds here. The call site knows whether it needs tools; the model does not
need to be asked.

## 7. Recommendation

|                                           |                                                                                                                        |
| ----------------------------------------- | ---------------------------------------------------------------------------------------------------------------------- |
| **Now**                                   | Nothing. Do not build against Phi Silica six weeks before its removal                                                  |
| **After Aion Instruct ships** (Nov 2026+) | Re-read this section. Confirm its API shape and whether it offers tool calling                                         |
| **Then, if worth it**                     | A `SingleShot` backend seam beside `adapter.rs`, serving the three no-tool call sites only, `llama-server` as fallback |
| **Never, on this evidence**               | The agent loop on an OS model. Windows cannot, and Apple would cap a Standard machine at Light                         |

Sequenced against the plan, this sits after step 15 (D4, the send path) rather than before it: the
send path is the one step left for a person and changes what the product does; this changes only how
fast part of it runs, on some machines.

## 8. What would change this answer

Written down so the re-read is cheap rather than a fresh investigation:

- **Apple raises the on-device context window past 4,096.** The Standard-tier cap is the sharpest
  objection to the Apple path, and it is one number.
- **Microsoft's Aion Instruct offers function calling.** That would make the Windows model able to
  serve the agent loop for the first time.
- **Chief's model calls grow past five, or the tool loop stops being the expensive one.** The
  accounting in §4 rests on D9 keeping most questions away from the model.
- **Apple Intelligence becomes on by default and Intel Macs leave the bundle list.** That would make
  the Apple path near-universal for macOS, which is the coverage objection.

## Sources

- [Foundation Models — Apple Developer Documentation](https://developer.apple.com/documentation/foundationmodels)
- [TN3193: Managing the on-device foundation model's context window](https://developer.apple.com/documentation/technotes/tn3193-managing-the-on-device-foundation-model-s-context-window)
- [Meet the Foundation Models framework — WWDC25](https://developer.apple.com/videos/play/wwdc2025/286/)
- [Updates to Apple's On-Device and Server Foundation Language Models](https://machinelearning.apple.com/research/apple-foundation-models-2025-updates)
- [Get started with Phi Silica in the Windows App SDK](https://learn.microsoft.com/en-us/windows/ai/apis/phi-silica)
- [What are Windows AI APIs?](https://learn.microsoft.com/en-us/windows/ai/apis/)
- [Choose your Windows AI solution](https://learn.microsoft.com/en-us/windows/ai/windows-ai-comparison)
- [microsoft/windows-app-rs — Rust for the Windows App SDK](https://github.com/microsoft/windows-app-rs)
- [microsoft/windows-rs — Rust for Windows](https://github.com/microsoft/windows-rs)
- [fm-bindings — Rust bindings for Apple's Foundation Models](https://github.com/remdalm/fm-bindings)
