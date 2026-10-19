# Mac batch 5: the equal comparison of every model (19 Oct 2026)

Scott asked on 19 Oct 10:21Z for "a comprehensive test across all models so that they can all be compared equally",
with Qwen3.5 4B included. Run it from the code below; change no code.

## Code

- Repository `chief.ai`, branch `claude/project-thread-krmkq8`, the head of the branch when you start. Clean worktree
  (not your working tree, not `stash@{0}`), `pnpm install`, `pnpm engine:fetch`. Do not push, open a PR or commit.
- `git status` clean and every `env.json` showing the same commit. The scorer now reads "3:00 PM" as the "15:00" in the material.

## Models (all eight in `bench/models.json`, every one run the same way)

qwen3-1.7b, llama-3.2-3b, llama-3.2-1b, qwen3-4b-instruct-2507, qwen3.5-4b, minicpm5-2b, gemma-4-e2b, lfm2.5-2.6b.
Run each model separately with its own `--only` (`bench/run.mjs` keeps only the last `--only`). Same order for both
variants. No other models.

## Variants (5 repeats, `--machine mac-mini-2014`, `--judge none`, under `~/chief-bench-batch5/`)

1. `default/<model>`: `pnpm bench --only <model> --repeats 5 --machine mac-mini-2014 --judge none --out ~/chief-bench-batch5/default/<model>`
2. `router/<model>`: the same plus `--router-server <llama-server from the router run> --router-model <EmbeddingGemma 2 q8 gguf used before>`.
3. `model-writes/<model>`: `CHIEF_BENCH_SHORT_BRIEF=0` with `--fixture em-` and then `--fixture par-`, `quiet-`, `loop-` (one `--out` sub-folder each).
   This is what a short day looks like when the model writes it, for the comparison with code-written briefs.

Order: all of 1, then all of 2, then all of 3. Record peak memory, swap, and the prompt length beside any speed number.

## Rules (Scott's)

- One benchmark at a time. Quit Claude Desktop before the first run and reopen it after the last; confirm it is running before saying so.
- A judge script or run where every call failed is a failure. Do not use the subscription judge.
- Every model is held to the same gates (G1 to G6); report a model that fails to load rather than skipping it.

## Report

`~/chief-bench-batch5/comparison.md`: one table, models as rows, columns for each gate, G3 count, by-code and fell-back counts,
tool-call validity, tool-fixture pass (default against router), median finish, peak memory and swap, once per variant. Note what
each model is best and worst at. Copy each run's `human-eval.html` into `~/chief-bench-batch5/rating/`. Message the thread when done.
