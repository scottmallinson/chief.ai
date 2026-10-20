# Mac re-score of batch 5 (20 Oct 2026)

Scott asked for this on 20 Oct 11:37Z. It runs no engine and no model: it only re-scores the answers batch 5 already
recorded with the scorer fixed at `f835468` (a bold label's next word and "N days ago" are no longer scored as invented).

## Steps

1. In a clean worktree of `claude/project-thread-krmkq8` (not your working tree, not `stash@{0}`), at the branch head, which
   contains `f835468`: `pnpm install`. Do not push, open a PR or commit. No need to quit Claude Desktop (nothing loads a model).
2. Copy `~/chief-bench-batch5` to `~/chief-bench-batch5-rescored` first. Never score in place; the original stays as it was.
3. For every run folder under `default/`, `router/` and `model-writes/` in the copy:
   `pnpm bench --score-only <that folder>`. Then rebuild the report tables the way batch 5 did (`report.mjs`).
4. Write `~/chief-bench-batch5-rescored/comparison.md` in the same layout as batch 5's, and put a short table at the top of what
   changed against the original: per model and variant, G3 and the tool-fixture pass rates, before and after.
   Expected: Gemma's G3 failure (5/50 in default and router) disappears, and Qwen3.5 4B's routed tool-open-prs and tool-reviews rise.
   Report anything else that moves, and anything that was expected to move and did not.
5. Leave `rating-combined/` alone. Message the thread when done.
