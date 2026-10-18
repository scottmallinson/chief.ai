# Mac batch 3: brief for the Mac session (18 Oct 2026)

Scott approved this batch on 18 Oct 16:05Z ("go ahead with the single Mac batch. Include all
identified models, their variants and combinations"). Run it from the code below; change no code.

## Code

- Repository `chief.ai`, branch `claude/project-thread-krmkq8`, **the head of the branch**, which differs from d5f6a93 only by this file (pushed). Check it
  out in a clean worktree (not your working tree, not `stash@{0}`), `pnpm install`, `pnpm engine:fetch`
  (pin b11454). Do not push, open a PR or commit.
- `git status` must be clean and `env.json` of every run must show that head commit. Same commit for every run.
- What is new in it: briefs of three lines or fewer are written in code; the final answer check;
  fixture set v2; `--reasoning-budget 0` on every server (LFM2.5 reasons otherwise; the bench
  starts its server the same way); `lfm2.5-2.6b` is in `bench/models.json`.

## Models (all enabled in `bench/models.json`, 8)

qwen3-1.7b, llama-3.2-3b, llama-3.2-1b, qwen3-4b-instruct-2507, qwen3.5-4b, minicpm5-2b,
gemma-4-e2b, lfm2.5-2.6b. The LFM2.5 file (LFM2.5-2.6B-Q4_K_M, 1.67 GB, from
huggingface.co/LiquidAI/LFM2.5-2.6B-GGUF, revision `main`) downloads on first use; its sha256 is
written to env.json. Do not add or drop models. Do not run Gemma 1B or Phi-4-mini.

## Variants (each a separate `--out` folder under `~/chief-bench-batch3/`, 3 repeats, `--machine mac-mini-2014`, `--judge none`)

1. **default** (`B-default`): `pnpm bench --repeats 3 --machine mac-mini-2014 --out ~/chief-bench-batch3/B-default`
2. **router** (`C-router`): same plus `--router-server <the llama-server from the router run> --router-model <EmbeddingGemma 2 q8 gguf used on 17/18 Oct>`.
3. **model writes every brief** (`A-model-writes`): `CHIEF_BENCH_SHORT_BRIEF=0`. Briefs are the only thing this changes, so run
   only those fixtures: for each of `par-`, `quiet-`, `em-`, `loop-` pass `--fixture <that>` (one invocation per substring, same `--out`
   parent with a sub-folder each).
4. **thinking left on, LFM2.5 only** (`D-lfm-thinking`): `CHIEF_ENGINE_THINKING=on pnpm bench --only lfm2.5-2.6b --fixture quiet- --repeats 1`. Expected to hit the length ceiling; it measures what switching it off buys.

Order: B, then C, then A, then D. Preceded by re-scoring (cheap, no engine): `pnpm bench --score-only` on the 17 Oct control, 17 Oct three-way and 18 Oct run-2 folders, copied first, so their G3 numbers are corrected for the old scorer bug.

## Rules (Scott's, from earlier batches)

- One benchmark at a time. Quit Claude Desktop before each long run and reopen it after; confirm it is running before saying so.
- A judge script or run where every call failed is a failure, not a pass. Do not use the subscription judge.
- Same commit, clean tree, Mac mini only. Record the prompt length beside any speed number.

## Report

Write `~/chief-bench-batch3/comparison.md`: per model and variant the gates (G1 to G6), G3 count, blocked/by-code/fell-back counts from
the `check` field, tool-call validity, median finish, peak memory. Then the same models in the 18 Oct run-2 for comparison, noting the v2
fixture set makes those not strictly comparable. State any model that fails to load. Message the thread when done.
