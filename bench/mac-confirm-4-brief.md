# Mac confirmation 4: brief for the Mac session (19 Oct 2026)

Scott asked for one short on-device confirmation of the three fixes in 43a67e6 (a brief that is only
"[Cut short]" is now an error and a cut-off brief is written from the material; attendees a title already
names are not named twice; "my open PRs" routes at margin 0.045). Run the code below; change no code.

## Code

- Repository `chief.ai`, branch `claude/project-thread-krmkq8`, the head of the branch (43a67e6 plus this file).
  Clean worktree (not your working tree, not `stash@{0}`), `pnpm install`, `pnpm engine:fetch`. Do not push, open a PR or commit.
- `git status` clean; every run's `env.json` must show the same commit.

## Models

Only `qwen3-1.7b` and `llama-3.2-3b` (`--only` each).

## Runs (3 repeats, `--machine mac-mini-2014`, `--judge none`, under `~/chief-bench-confirm4/`)

1. `pnpm bench --only qwen3-1.7b --only llama-3.2-3b --repeats 3 --machine mac-mini-2014 --judge none --out ~/chief-bench-confirm4/default`
2. Router: the same plus `--router-server <llama-server from the router run> --router-model <EmbeddingGemma 2 q8 gguf used before>`, `--out ~/chief-bench-confirm4/router`.

## Rules

- One benchmark at a time. Quit Claude Desktop before the runs; reopen it afterwards and confirm it is running before saying so.
- Same commit, clean tree. Record the prompt length beside any speed number.

## Report

`~/chief-bench-confirm4/comparison.md`: per model and run the gates (G1 to G6), G3 count, by-code/fell-back counts, and for the router
run how many of `tool-open-prs`, `tool-reviews`, `tool-tomorrow` were routed (request has no `tools` field) and any wrong view. Check
`em-01` answers for a doubled name ("with Priya with Priya"). Message the thread when done.
