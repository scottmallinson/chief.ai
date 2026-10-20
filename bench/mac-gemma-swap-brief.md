# Mac Gemma swap check (20 Oct 2026)

Scott asked on 20 Oct 11:48Z: "one more round of Gemma benchmark tests to see if the swap issue arises or if it was a
temporary issue with the Mac environment. Ensure Desktop and other apps are quit to save memory." Batch 5 had Gemma 4 E2B
swap on one run in default and one in router (G2); it is the only gate stopping it from being ranked.

## Code

Branch `claude/project-thread-krmkq8`, the head when you start (contains `f835468` and `ea7a36f`). Clean worktree (not your
working tree, not `stash@{0}`), `pnpm install`, `pnpm engine:fetch`. Do not push, open a PR or commit. Every `env.json` must show the same commit.

## Quiet the machine first

- Quit Claude Desktop, VS Code, Chrome, Plexamp and anything else you can; list what is still running in the report.
- Write `sysctl vm.swapusage` and `memory_pressure | head -3` to `~/chief-bench-gemma-swap/before.txt`. If swap used is above
  500 MB, say so; a restart is Scott's call, not yours.
- Record `sysctl vm.swapusage` again after every run, in `swap.log` beside the run name.

## Runs (Gemma 4 E2B only, `--only gemma-4-e2b`, 5 repeats, `--machine mac-mini-2014`, `--judge none`, under `~/chief-bench-gemma-swap/`)

One at a time, in this order: `default-1`, `router-1`, `default-2`, `router-2`. The router runs add `--router-server <llama-server from the router run> --router-model <EmbeddingGemma 2 q8 gguf used before>`.
Two of each tells you whether a swap, if one comes back, is repeatable and in the same fixture.

## Report

`~/chief-bench-gemma-swap/comparison.md`: per run, G2 (how many runs swapped, which fixtures, the swap-used figure before and
after each run), peak resident memory, and every other gate; then batch 5's Gemma rows beside them. Say plainly whether the
swap came back, whether it came back in the same fixture, and whether background memory at the start was lower than batch 5's
889 MB. Reopen Claude Desktop and the other apps you quit when the last run ends; confirm it is running before saying so.
Message the thread when done.
