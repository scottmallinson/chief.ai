# Chief test runbook

What Chief has to get right, written as three people's days, and how each scenario is checked.

Chief's job is to save a person's time by surfacing what they should look at before they have to
ask. So the scenarios are not about screens. Each one is a moment in somebody's day where Chief is
either right, late, stale or wrong, and the cost of the last three is a missed meeting or a
review nobody noticed.

## How to run it

```bash
pnpm runbook          # the automatic scenarios (Rust) and the UI day-rollover scenarios
pnpm verify           # everything CI runs
```

`pnpm runbook` is `cargo test scenarios` plus the `across midnight` tests in
`src/hooks/use-brief.test.ts`. The same midnight scenarios also run in the built app, in a real browser
with the clock under test control, as `e2e/midnight.spec.ts` (`pnpm test:e2e`). The manual scenarios at the end need a real machine; run them with
the `on-device-validation` skill before a release.

## How the automatic scenarios work

Chief reads deterministic services and has an on-device model write them up. So the services are
mocked and the model is mocked, and everything between them is real.

- Each scenario starts small HTTP servers that answer the way GitHub, Microsoft Graph and
  `llama-server` answer, and points Chief's real clients at them. No function is replaced.
- The servers answer by path, not by position, so adding a request to a pass breaks nothing.
- Graph honours `$top` and the requested time window, as the real service does. A mock that
  returns everything hides the cap on how many meetings are read.
- The model server keeps every prompt it was sent. Most scenarios assert on **what the model was
  told**, which is the part of Chief that decides whether the brief is right.
- A server can vanish (`Offline`), or promise a stream and hang up half way (`Torn`). That is how
  offline, flaky wifi and a power cut during writing are tested.
- A time can be passed in (`daily_brief_at`) so 23:59 and 00:01 are tested without waiting.

The code is `src-tauri/src/scenarios.rs`; each test is named after its ID below.

## Personas

### Engineering manager

Back-to-back meetings: technical, 1:1s, managerial. Needs to be right about the next one, and
cannot afford to open six tabs to find out.

| ID    | Scenario                                                                                 | Expected                                                                                                                                                                                                                           | Check  |
| ----- | ---------------------------------------------------------------------------------------- | ---------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------- | ------ |
| EM-01 | 14 meetings today, 08:30 to 16:30, brief written at 08:00                                | The model is told about the next three, in order, and the brief says "Showing the next 3 of 14 meetings still to come." It used to be told about all fourteen and wrote fourteen bullets.                                          | Auto   |
| EM-02 | Two reviews asked of me and one pull request of my own                                   | Reviews under "waiting on you", my own under its own heading. Sources name both.                                                                                                                                                   | Auto   |
| EM-03 | A 10:00 1:1 moves to 14:00                                                               | "What's on my calendar" shows 14:00 only. It showed both.                                                                                                                                                                          | Auto   |
| EM-04 | A meeting is cancelled                                                                   | It leaves the calendar answer. It stayed.                                                                                                                                                                                          | Auto   |
| EM-05 | On a train, no signal, "what's on my calendar"                                           | Answered from what was saved, with how old it is. No network call and no model call.                                                                                                                                               | Auto   |
| EM-06 | The Outlook sign-in expired overnight                                                    | The brief is still written from GitHub.                                                                                                                                                                                            | Auto   |
| EM-07 | Mail from direct reports, some read                                                      | Only unread mail reaches the model.                                                                                                                                                                                                | Auto   |
| EM-08 | Ask for the calendar after a 16-meeting day                                              | All 16 are listed.                                                                                                                                                                                                                 | Auto   |
| EM-09 | The calendar is unreachable, refused (503) or hangs up, on a later pass                  | Saved meetings stay. An outage is not a cancellation.                                                                                                                                                                              | Auto   |
| EM-10 | A note typed by hand, and a pass that clears cancelled meetings                          | The note is untouched.                                                                                                                                                                                                             | Auto   |
| EM-11 | Open the app at 08:50 and see the brief for the 09:00 meeting without asking             | The brief is waiting, written by the background pass, and names the 09:00 meeting.                                                                                                                                                 | Manual |
| EM-15 | Chief launched hidden at login; the window is opened while the engine is still loading   | The window opens on the brief, not on "Set up Chief". `SetupView` lets itself out when the engine that was loading at launch starts answering; after a first-run download it still waits to be told. Found by EM-11 on 2026-10-06. | Auto   |
| EM-12 | Brief quality: does a 3B model, given 14 meetings and 3 reviews, lead with the next one? | Time-bound items first, no invented names or numbers.                                                                                                                                                                              | Manual |
| EM-13 | The same day, brief rewritten at 14:10                                                   | The morning has gone: the model is told about Vendor call, Budget check-in and Design critique, and about none of the morning. `refresh_due` is false at 08:00 and true at 09:30.                                                  | Auto   |
| EM-14 | The same day, brief written at 19:00                                                     | Every meeting has finished, so the brief says nothing else is scheduled and the model is not called. Nothing is due a refresh.                                                                                                     | Auto   |

### Working parent

Household admin and school logistics, shared with a partner, around a full-time job. Mostly a
calendar and an inbox, often with no work tools connected at all.

| ID     | Scenario                                                                          | Expected                                                                                        | Check  |
| ------ | --------------------------------------------------------------------------------- | ----------------------------------------------------------------------------------------------- | ------ |
| PAR-01 | Family calendar and school mail only, no GitHub                                   | The brief is about the school run. No work headings. Sources are `calendar` and `inbox`.        | Auto   |
| PAR-02 | "INSET day — school closed" as an all-day event                                   | Read as a day-long fact, not a meeting at 00:00. The model was being told "00:00".              | Auto   |
| PAR-03 | Fresh install, nothing connected                                                  | Says nothing is connected and what to do. No model call, no invented day.                       | Auto   |
| PAR-04 | Emoji, accents and curly apostrophes in event names; then 400 of them             | Names survive untouched. A flood is cut to the budget and cannot panic on a character boundary. | Auto   |
| PAR-05 | The user's calendar and a partner's both hold the same pickup                     | Both rows are kept, one per account.                                                            | Auto   |
| PAR-06 | Subscribe to the partner's calendar by link, see the pickup in the brief          | The pickup appears once, at its time.                                                           | Manual |
| PAR-07 | A holiday week: an all-day event that started on Monday, asked about on Wednesday | Still shown on Wednesday. **Not yet covered, see known gaps.**                                  | Manual |

### Software engineer

A stand-up each day, occasional ad-hoc meetings, focused on shipping and coordinating.

| ID     | Scenario                                                                            | Expected                                                                                                              | Check  |
| ------ | ----------------------------------------------------------------------------------- | --------------------------------------------------------------------------------------------------------------------- | ------ |
| ENG-01 | "Draft my standup" after two merges yesterday                                       | The material names both pull requests by title. Gathering is not a model call.                                        | Auto   |
| ENG-02 | An issue assigned to me on GitHub                                                   | It is in the brief, and the source is "GitHub issues". It was fetched and thrown away.                                | Auto   |
| ENG-03 | GitHub rate-limits one of the searches                                              | The other searches still land. One bucket is lost, not the brief.                                                     | Auto   |
| ENG-04 | A pull request open at 09:00 and merged at 09:30                                    | One row that changes from open to merged. Running more passes adds nothing.                                           | Auto   |
| ENG-05 | "What did I ship today / this week", just after a merge                             | Answered from the log, naming the pull request, with no model call.                                                   | Auto   |
| ENG-06 | Ask the chat a question that needs a live GitHub read, offline                      | The model is handed a plain error to explain, with no credential in it. No crash.                                     | Auto   |
| ENG-07 | Stand-up at 09:30 and the draft reads as something to say aloud                     | Short, first person, no invented work.                                                                                | Manual |
| ENG-08 | "Draft my standup", asked of a model whose template rejects two user turns in a row | The facts and the question are one user turn, and no tools are offered. It was a 400 on Gemma and a 500 on Llama 3.2. | Auto   |
| ENG-09 | "Draft my standup" first thing on a Monday                                          | Friday's merges are in the material, not an empty log.                                                                | Auto   |

## Anyone: edges of the day, the network, the machine and the model

| ID     | Scenario                                                                                  | Expected                                                                                                   | Check  |
| ------ | ----------------------------------------------------------------------------------------- | ---------------------------------------------------------------------------------------------------------- | ------ |
| ANY-01 | A brief starts at 23:59:50 on 31 December                                                 | Filed under 31 December with that day's meetings, and told it is 23:59. Tomorrow starts un-briefed.        | Auto   |
| ANY-02 | The same machine at 00:01 on 1 January                                                    | Briefs the new day with the new day's meetings and none of yesterday's.                                    | Auto   |
| ANY-03 | "This week" and "last week" at Sunday 23:59:59 and Monday 00:00:00                        | Turn over exactly at midnight on Monday.                                                                   | Auto   |
| ANY-04 | Auckland (UTC+13), Los Angeles (UTC−8), London; meetings at 00:00 and 23:59               | Each lands in the user's own day, never the UTC one.                                                       | Auto   |
| ANY-05 | The model engine is not running                                                           | Error, no file, and the day is not marked briefed, so the next pass tries again.                           | Auto   |
| ANY-06 | The model answers with nothing                                                            | Not filed as the brief. It used to write an empty file and mark the day done.                              | Auto   |
| ANY-07 | The power goes while the model is writing                                                 | The half-answer is not the brief, and the day stays open. It used to keep it with an apology attached.     | Auto   |
| ANY-08 | Signed in, but offline, nothing saved yet                                                 | Says it could not reach anything. It said "nothing is connected".                                          | Auto   |
| ANY-09 | Offline, with a work log from earlier                                                     | The brief is still written from the log.                                                                   | Auto   |
| ANY-10 | Wake with no network, then the network returns                                            | First pass records an outage, not a revoked sign-in. The next pass recovers by itself.                     | Auto   |
| ANY-11 | Power loss while Chief writes a file                                                      | The file is whole or the old one, never torn. Nothing stray is left in the folder.                         | Auto   |
| ANY-12 | The user edits today's brief and the background pass runs again                           | Their edit is kept.                                                                                        | Auto   |
| ANY-13 | 60 meetings, 30 reviews, 30 own pull requests, 30 unread mails                            | One model call, and the prompt is inside the token budget.                                                 | Auto   |
| ANY-14 | Questions in each persona's words, including ones that merely mention a trigger word      | The ones that mean a stored read take the free route. The others reach the model.                          | Auto   |
| ANY-15 | The connection to GitHub is accepted and then dropped                                     | The brief is written from the calendar. The account shows an outage.                                       | Auto   |
| ANY-16 | The window is left open overnight (Chief lives in the tray) and shown the next day        | Heading and brief move to the new day, and the brief the background pass wrote is on screen.               | Auto   |
| ANY-17 | The same, while an earlier day is being read                                              | The reader is not moved.                                                                                   | Auto   |
| ANY-18 | The window is left open and visible across midnight                                       | Rolls over by itself.                                                                                      | Auto   |
| ANY-19 | Sleep the laptop across midnight with Chief hidden in the tray, then open the lid         | The window opens on the new day.                                                                           | Manual |
| ANY-20 | Pull the plug during a brief on a real machine                                            | Chief starts cleanly, the day is un-briefed, and one is written.                                           | Manual |
| ANY-21 | The night clocks change (spring forward and fall back)                                    | The day still has exactly one brief and the right meetings. Needs a real clock change.                     | Manual |
| ANY-22 | Wi-Fi off in the real app, then on                                                        | Settings shows an outage and recovers; no sign-in prompt.                                                  | Manual |
| ANY-23 | First launch on the oldest supported macOS                                                | Opens, sets up, answers.                                                                                   | Manual |
| ANY-24 | A brief is requested of Qwen3 on a machine that writes 8 tokens a second                  | The request switches the template's thinking off and carries a repeat penalty of 1.05, no stronger.        | Auto   |
| ANY-25 | A small model loops, listing the same items until the length limit (seen on 1B, 8 GB Mac) | Each line is kept once, the length-limit apology is dropped, and the request asks the model not to repeat. | Auto   |

## The model: what the benchmark graded, run by hand

The automatic scenarios mock the model, so they pass for every model and cannot choose between them.
These are the cases the 2026-10-02 benchmark on an Intel Mac mini (8 GB, two cores) graded, three runs
each. Run them against any model Chief ships, on the machine it will run on.

| ID     | Scenario                                                        | Expected                                                                  | Check  |
| ------ | --------------------------------------------------------------- | ------------------------------------------------------------------------- | ------ |
| MOD-01 | The same pull request appears on several lines of the material  | Each is listed once. No loop, and the answer ends without the length cap. | Manual |
| MOD-02 | A school-only day, no work tools connected                      | Only the school events. No invented meetings, names or numbers.           | Manual |
| MOD-03 | "Draft my standup" from a work log with two merges              | Written in words from the log. No tool call, no error from the engine.    | Manual |
| MOD-04 | A question that needs a live lookup (calendar, review requests) | One well-formed tool call whose arguments are values the tool accepts.    | Manual |
| MOD-05 | Speed on the target machine: cold and warm brief                | Tokens a second recorded for reading and writing, and resident memory.    | Manual |

## What the first pass found

Each of these scenarios was red against the code as it stood, for the reason in the second column, before its fix.

| Found by       | What was wrong                                                                                                           | Fix                                                                                                    |
| -------------- | ------------------------------------------------------------------------------------------------------------------------ | ------------------------------------------------------------------------------------------------------ |
| ANY-01, ANY-02 | A brief read the clock four times, the last after the model finished. A brief started at 23:59 was filed under tomorrow. | `daily_brief_at` reads the clock once.                                                                 |
| ANY-16–18      | The window worked out the date once when it opened. After a night in the tray it still said yesterday.                   | The date is re-read on focus, on becoming visible and at midnight; a reader of an earlier day is kept. |
| EM-01          | Only the first ten meetings of a day were read, so a busy afternoon was invisible. No warning.                           | Twenty. About 280 prompt tokens at the most, and sorted, de-duplicated and capped before the budget.   |
| EM-03, EM-04   | A meeting is keyed on its start time, so a move left the old row and a cancellation left the meeting.                    | A pass that read a calendar in full clears that account's rows for today that it no longer holds.      |
| ENG-09         | A Monday-morning stand-up read "this week", which starts at midnight, and reported nothing to report.                    | The last 7 days, so Friday is in.                                                                      |
| ENG-02         | GitHub issues were fetched and then overwritten by the Linear read. A wasted request, and the brief never listed them.   | Their own field and source name.                                                                       |
| PAR-02         | All-day events read as a meeting at 00:00.                                                                               | Graph's `isAllDay` is read, and the line says "all day".                                               |
| ANY-06         | An empty model answer was written as the brief and the day marked done.                                                  | Rejected, so the next pass tries again.                                                                |
| ANY-07         | An interrupted answer was kept with an apology attached and the day marked done.                                         | Rejected, so the next pass tries again.                                                                |
| ANY-25         | A looping 1B brief filed the same four pull requests repeatedly, ending in a length-limit apology.                       | Repeats removed; a repeat penalty is sent with brief requests only.                                    |
| ANY-08         | Connected but unreachable said "nothing is connected yet".                                                               | A separate message that says the network could not be reached.                                         |
| ANY-11         | Files were written in place, so a power cut could leave one empty or cut off.                                            | Written to a neighbour and renamed over the original.                                                  |

Each guard test was then checked by breaking the fix and watching it fail: the date taken from the
wall clock (ANY-01, ANY-02), the interruption check removed (ANY-07), an unreachable calendar read
as an empty one (EM-09), and the in-place write restored (ANY-11).

## Known gaps

Not fixed, and not hidden.

- **A multi-day all-day event on its second day (PAR-07).** Meetings are stored at their start, so
  an event that began on Monday is not in Wednesday's "what's on my calendar". The brief reads the
  calendar live and is not affected. Not yet shown by a test.
- **Daylight-saving days.** The day's bounds are computed from the local zone, and the code has a
  fallback for a missing midnight, but no scenario runs on a 23- or 25-hour day. That needs a time
  zone database, which this repository does not carry, or a real clock change (ANY-21).
- **A subscribed calendar.** Pasted-link calendars are read in full and are capped at twenty in the
  brief like the rest. They have no scenario here because that client insists on HTTPS.
- **Linear, Jira and Confluence.** Covered by their own module tests, not by a persona scenario.
- **Hitting the answer length limit.** A brief that reaches its 500-token ceiling is kept with a
  "cut short" note inside the file.

## Adding a scenario

1. Write it from the person's side first: what they did, what they needed.
2. Add a row above with the next ID in its group.
3. Write the test in `src-tauri/src/scenarios.rs`, named after the ID. Prefer asserting on what the
   model was told over asserting on what a stand-in model said back.
4. If it guards something ("never", "still", "does not clear"), break the thing it guards, watch
   the test fail, and put it back. See the `proving-a-guard-test` skill.
5. If the fix adds behaviour, add the scenarios for the behaviour in the same change.
