---
name: on-device-validation
description: Use when validating Chief by running it — a manual test pass, measuring engine throughput, timing the idle stop, or checking a migration against the live database. Carries the preconditions that make a measurement mean anything, and the ways this app is expensive to get wrong.
---

# Validating Chief on the machine it runs on

Chief's test suite proves the parts. Running it proves the product, and everything
below was learned by running it and getting a wrong answer first.

## Before anything: the two expensive mistakes

**Migrating is a one-way door.** Launching a branch applies its migrations to the
live database in place, with no down path — and `main` will then refuse to start,
because sqlx's migrator runs with `ignore_missing: false` and returns
`VersionMissing` for the versions it does not know. That failure happens in
SQL-plugin setup, so `Builder::build()` fails and `lib.rs` panics: **no window, no
message, no obvious cause.** Anyone reviewing a branch will switch back to `main`
sooner or later.

Back up before the first launch of a branch that adds migrations:

The app-data directory is the OS's, so the path depends where you are running. The bundle
identifier is `com.scottmallinson.chief` on all three:

| Platform | Directory                                                    |
| -------- | ------------------------------------------------------------ |
| macOS    | `~/Library/Application Support/com.scottmallinson.chief`     |
| Linux    | `~/.config/com.scottmallinson.chief`                         |
| Windows  | `%APPDATA%\com.scottmallinson.chief`                         |

```bash
D="$HOME/Library/Application Support/com.scottmallinson.chief"   # macOS
# D="$HOME/.config/com.scottmallinson.chief"                     # Linux
B="$HOME/chief-db-backup-$(date +%Y%m%d-%H%M%S)"                 # read the clock once
mkdir -p "$B" && cp -a "$D"/chief.db* "$B/"
```

**Back up the database, not the app-data directory.** That directory also holds
gigabytes of model weights. The three `chief.db*` files are tens of kilobytes and
are all that migrating touches.

**Do not disconnect an integration to test an empty state.** Reconnecting costs a
device-flow round trip. Delete the row, test, and restore it from the backup —
the token comes back byte for byte.

## Preconditions for any measurement

**No stray `llama-server`.** Two timing runs were invalidated by an engine with
`ppid 1` that predated the app — orphaned by an earlier hot reload, and answering
on the port while the app under test believed it had started its own. Chief does
not kill a server it did not start, deliberately, because one you are running
yourself looks identical.

```bash
pgrep -lf '[l]lama-server'   # expect nothing, or a parent you recognise
ps -o ppid= -p "$(pgrep -f '[l]lama-server' | head -1)"
```

**Freeze the tree.** `pnpm tauri:dev` hot-reloads on save. Editing anything during
a timed run restarts the app and orphans its engine, which is how the strays above
were created. Measure on a tree you are not touching.

**Ports.** 1420 (vite) and 11435 (engine) survive an unclean exit and block the
next launch:

```bash
lsof -ti:1420 -ti:11435 | xargs kill -9 2>/dev/null
```

## Reading engine throughput

`llama-server` reports its own timings; they are the measurement, not a stopwatch.
Timestamps are `MM.SS.mmm.uuu` from process start.

```
prompt eval time = 39606.67 ms / 881 tokens ( 44.96 ms per token, 22.24 tokens per second)
       eval time =  3237.38 ms /  20 tokens (170.39 ms per token,  5.87 tokens per second)
```

Prefill is the first line, decode the second. **Always record the context length
alongside them** — decode degrades sharply with context, measured at 7× between a
short prompt and ~2,900 tokens on one machine. A throughput figure without a
context length is not comparable to anything.

Slot selection matters too:

```
slot get_availabl: id 2 | selected slot by LRU, t_last = -1      <- cold, no reuse
slot get_availabl: id 2 | selected slot by LCP similarity, f_sim_best = 0.976   <- reusing
```

`t_last = -1` means the slot had never been used, so the whole prompt was
prefilled from scratch.

## Reaching what has no interface

Several commands have no button. Open devtools in the Chief window and:

```js
await window.__TAURI_INTERNALS__.invoke('generate_brief');
```

DevTools prints the expression before the promise settles, so an `undefined`
followed by the real value is normal and not a failure.

## Recording the outcome

Keep a log per pass, outside the repository or excluded from it. **In a worktree,
`info/exclude` lives in the common git dir**, not the per-worktree one:

```bash
echo 'RUNBOOK-VALIDATION.md' >> "$(git rev-parse --git-common-dir)/info/exclude"
```

Prettier still checks untracked files, so either format the log or keep it outside
the tree.

For every scenario record the verdict *and the evidence* — the actual output, the
row count, the log line. "Passed" without evidence is not a result, and a scenario
that cannot fail should be struck from the runbook rather than ticked.

## When a scenario is wrong

Some scenarios cannot prove what they claim. One expected the word "ship" to stop
fetching pull requests after a keyword heuristic was deleted; the small model calls
the tool for "how many shipping containers fit on a Panamax vessel?" as well, so
the scenario passes either way. **Correct the runbook and say so** rather than
recording a pass. The removal was provable by code — the function was gone.
