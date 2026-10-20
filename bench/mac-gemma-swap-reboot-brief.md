# Mac Gemma swap check from a fresh reboot (for a fresh CLI session)

Self-contained: you have no earlier context. Scott triggers this himself, later, after restarting the Mac mini. Do exactly this and
change no code.

## Why

Gemma 4 E2B is the top-scoring model in the benchmark (about 96) but is not ranked, because the swap gate (G2) fails a model
for a single run that swapped. In batch 5 and in a repeat on 20 Oct, 3 of 600 Gemma runs swapped by about 12 MB, but swap
already stood at about 990 MB before any run began, since macOS never hands swap back until a restart. This run answers whether
Gemma swaps at all when swap starts near 0 and almost nothing else is running.

## Before you start (Scott does this)

Restart the Mac. Log in, open one Terminal and start this CLI session in it. Open nothing else: no Claude Desktop, VS Code, Chrome, Plexamp or Mail.
This CLI session is itself a process that uses memory; that cannot be avoided, so record it.

## Code

Repository `chief.ai`, branch `claude/project-thread-krmkq8`, the head when you start (it contains the scorer fixes). Make a clean
worktree (not the working tree, not `stash@{0}`): `git fetch`, `git worktree add ~/Projects/chief.ai-gemma-reboot origin/claude/project-thread-krmkq8`,
`pnpm install`, `pnpm engine:fetch`. Do not push, open a PR or commit. `git status` must be clean and every `env.json` must show the same commit.

## Record the starting state

Before anything else, write to `~/chief-bench-gemma-reboot/before.txt`: `sysctl vm.swapusage`, `memory_pressure | head -5`,
`uptime`, and `ps -axo rss,comm | sort -nr | head -15`. If swap used is above 200 MB, say so in the report: the reboot did not
give a clean start. Do not restart the Mac yourself.

## Runs (Gemma 4 E2B only, `--only gemma-4-e2b`, 5 repeats, `--machine mac-mini-2014`, `--judge none`, under `~/chief-bench-gemma-reboot/`)

One at a time, in this order: `default-1`, `router-1`, `default-2`, `router-2`. The router runs add
`--router-server <llama-server from the router run, ~/chief-router-engine> --router-model <the EmbeddingGemma 2 q8 gguf used before, embeddinggemma-2-Q8_0.gguf>`.
Run them as `pnpm bench --only gemma-4-e2b --repeats 5 --machine mac-mini-2014 --judge none --out ~/chief-bench-gemma-reboot/<name>` (add the router flags for router runs).
Every 15 s during the runs, append `date`, `sysctl vm.swapusage` and the `vm_stat` page-out count to `swap-samples.log`; every minute,
the top five processes by resident memory to `top-procs.log`.

## Report

`~/chief-bench-gemma-reboot/comparison.md`: per run, G2 (how many runs swapped, which fixtures, swap used before and after), peak resident memory, and
every other gate; the swap figure at the start of each run; and a comparison with the earlier rounds (batch 5: 1 swapped run in each of
default and router; 20 Oct rerun: 0, 0 and 1 of 120). Say plainly whether Gemma swapped at all from a near-zero start, and if it did,
whether in the same fixture as before (eng-standup-empty). Also report Gemma's tool-call validity on tool-reviews in every run, which
varied (100%, 96%, 88%) between rounds. When the last run ends, message the thread if you can, and tell Scott in your final reply.
