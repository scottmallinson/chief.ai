# Public-data fixtures

Evaluation fixtures made from real, public data, in the shape the real API returns, with an answer
key worked out by code. **They are for evaluation only. Nothing here is training data, and none of it
should be mixed into any.**

| Folder      | What it is                                                                     |
| ----------- | ------------------------------------------------------------------------------ |
| `slices/`   | A small cut of the source, as extracted. The only thing committed from it.     |
| `fixtures/` | What the converter writes: a Jira search result and its answer key. Generated. |

## The Jira boards

Three boards from [TAWOS](https://github.com/solar-group/tawos) v1.1 (Tawosi, Al-Subaihin, Moussa,
Sarro, MSR 2022; doi 10.5522/04/21308124; Apache-2.0), each one person's work on one past day:

| Fixture   | Project                   | Open | Finished that fortnight | Blocked |
| --------- | ------------------------- | ---- | ----------------------- | ------- |
| `compass` | MongoDB Compass (COMPASS) | 30   | 3                       | 0       |
| `be`      | Hyperledger Explorer (BE) | 22   | 0                       | 1       |
| `mesos`   | Apache Mesos (MESOS)      | 11   | 1                       | 2       |

The projects keep their own open-source licences; the titles are theirs. Titles are the only free
text kept: no descriptions, no comments, no author names.

What the converter does to the real data, and why:

1. **Rebuilds the board as of the day.** TAWOS keeps each issue's final state, so the status on the
   day is rebuilt from the change log. Resolution dates are not used: Mesos stamped thousands of
   issues with one sweep date while their status had said Resolved for a year.
2. **Moves every date by whole weeks**, so the weekdays survive and the day lands on a Monday to
   Friday close to `2026-11-23`. Each fixture says its own `today`.
3. **Replaces every person** with an invented name from a fixed list (`NAMES`). The source is already
   pseudonymised to ids; none of those ids reach the output.
4. **Writes the answer key from the JSON, not from Chief**: open, not started, in progress,
   finished, blocked and by what, overdue, due this week, no due date. A benchmark that asked Chief
   what is true would pass whatever Chief believed.

Two limits to keep in mind. A due date comes from the end of the issue's sprint, the nearest thing
the dataset has. And the status categories (new, in progress, done) are a judgement made in
`to-jira.mjs`, because the dump keeps status names and drops Jira's categories; an unknown name
stops the build rather than being guessed.

None of the three boards has a blocker that was already finished on the day, so "blocked, but not any
more" is covered by a unit test of the answer key and not by these fixtures.

These are open-source bug trackers, not a manager's sprint board. They are good for "is it true"
checks and weaker for judging "what should I prioritise".

## Commands

```bash
node bench/public-data/build-fixtures.mjs            # rewrite fixtures/ from slices/
node bench/public-data/build-fixtures.mjs --check    # fail if they differ
node bench/public-data/extract-tawos.mjs TAWOS.sql --project MESOS --asof 2018-06-15 \
     --assignee 3374 --out bench/public-data/slices/mesos.json
node bench/public-data/extract-tawos.mjs TAWOS.sql --project MESOS --asof 2018-06-15 --survey
```

The dump is a 4 GB MySQL file at the DOI above (a 640 MB zip). It is read as a stream, about four
minutes for a project. `--survey` lists who had a board worth cutting on a day, with how many of
their issues were blocked. The slices are committed, so nobody needs the dump to run the tests.

`pnpm test` checks that the fixtures are what the converter writes, that each answer key matches the
JSON beside it, and (in Rust, `atlassian::tests`) that Chief's own Jira reader reads every fixture
the way the key says.

## The Jira suite

Seven fixtures play these boards through Chief's real code, with a mock Jira server standing where
Atlassian is. Run it with `pnpm bench --suite jira` (it never runs with the main fixtures, and has
its own version, `JIRA_SUITE_VERSION` in `bench/run.mjs`).

| Fixture                         | Asks                                   | The key says                                   |
| ------------------------------- | -------------------------------------- | ---------------------------------------------- |
| `jira-items-{compass,be,mesos}` | `/items`, worded by the model          | every issue under its own heading, with counts |
| `jira-ask-blocked-mesos`        | "What is blocked, and by what?"        | two issue and blocker pairs, on one line each  |
| `jira-ask-blocked-none-compass` | "Is anything of mine blocked?"         | says none; names nothing it should not         |
| `jira-ask-overdue-compass`      | "Which of my Jira issues are overdue?" | the three that are, and no other               |
| `jira-ask-progress-compass`     | "Which am I actively working on?"      | the two that are, and no other                 |

How a run works: `bench/run.mjs` moves every board so the day it describes is today on the machine
under test (Chief reads that machine's clock, and "overdue" is a claim about it), writes the moved
copies into the run folder, and the Rust runner serves them. The jobs the mock answers are the two
Chief asks: assigned and open, and finished since a date.

The checks are in `bench/score.mjs`: an issue key the board does not hold is an invented issue;
a real one named for the wrong reason is a wrong statement; a person's name is always invented;
and each issue must sit under the heading the report's own rules give it. Any of those fails gate
G7, and a model that fails a gate is not ranked. Part of an `/items` answer is written in code with
no model (the headline, a section of one line), so that text is passed to the scorer as material
and is not counted as invented.

The test that Chief reads the boards as the key says (`atlassian::tests`) is separate from the
suite: it needs no model, and runs in CI.
