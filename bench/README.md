# Model benchmark

Runs every enabled model in `models.json` against the same commit, the same fixtures and the same
engine flags Chief ships with, then writes a report a person can audit. The design, the research
behind it and the open questions are in the plan (`benchmarks/model-benchmark-plan.md` in the
project files); this file is how to run it.

```bash
pnpm engine:fetch                       # once: the bundled llama-server
pnpm bench                              # every enabled model, 5 repeats, no AI judge
pnpm bench --only qwen3-1.7b            # one model
pnpm bench --fixture em-12 --repeats 1  # one fixture, quick
pnpm bench --judge api                  # Haiku 5.5 via ANTHROPIC_API_KEY, capped by --max-spend (default $10)
pnpm bench --judge claude-code          # your own `claude -p`; fine for a dry run, not for decisions
pnpm bench --judge file                 # write judge-requests.json; answer it with judgements.json
pnpm bench --score-only <folder>        # re-score and re-report a finished run (and fold in ratings.json)
pnpm bench --machine mac-mini-2014      # say that this is the target machine
```

Weights are downloaded into `bench/.models` (or `CHIEF_BENCH_MODELS_DIR`) and results go to
`bench-results/<date>-<commit>-<machine>/`. Neither is committed.

## What a run writes

| File              | For                                                                          |
| ----------------- | ---------------------------------------------------------------------------- |
| `report.md`       | Reading. Fixed section order so two reports line up.                         |
| `results.json`    | Machines. Versioned by `schema_version`.                                     |
| `scores.csv`      | Spreadsheets: one row per model and fixture.                                 |
| `../trend.csv`    | One row per model per run, appended, for charts over time.                   |
| `answers/`        | Every answer of every repeat, verbatim.                                      |
| `human-eval.html` | Blind rating page. Download `ratings.json`, drop it in the folder, re-score. |
| `<model>.jsonl`   | The raw record: exact requests, streamed responses, timings, memory.         |
| `env.json`        | Commit, dirty-tree hash, machine, engine build, weight hashes.               |

## How a model is judged

1. **Gates** (pass or fail, decided before any score): starts and answers, fits in 8 GB with no
   swapping, invents nothing on the quiet days, finishes without circling, tool calls are valid.
   A model that fails one is not ranked.
2. **Deterministic checks** per fixture (`score.mjs`): facts kept, nothing invented, order,
   bullet and word caps, stand-up format, tool choice and arguments.
3. **AI judge** (`judge.mjs`): a pinned model and prompt, shown only the synthetic fixture and the
   answer. Pointwise marks for one model; order-swapped pairwise verdicts for two or more.
4. **Human rating**: the same answers, names hidden.
5. **Ranking** (`stats.mjs`): Bradley-Terry with a bootstrap over fixtures, reported as rank bands.

Speed is only ranked on the target machine (`--machine mac-mini-2014`). Anywhere else it is shown,
labelled, and left out of the score.

## Rules that keep it honest

- The judge only ever sees the runbook's synthetic fixtures. Nothing from a real machine.
- The Rust runner (`src-tauri/src/scenarios/bench.rs`) uses Chief's own prompts, tools and engine
  arguments; it replaces only the data sources with the runbook's mocks. If it drifts from the app,
  the benchmark measures something Chief does not do.
- A change to a fixture, a key, a weight or the judge prompt changes the ruler. Bump
  `FIXTURE_VERSION` in `run.mjs` or the judge prompt version when you do, and do not compare across.
- The invention check is a heuristic for times, numbers and names. It is why the judge and the
  human ratings exist.
