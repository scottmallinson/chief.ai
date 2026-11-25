// Turns a TAWOS slice (see extract-tawos.mjs) into what Jira's search endpoint returns, plus an
// answer key worked out in code from that same JSON, so a benchmark can say what is true without a
// person or a model deciding.
//
// Three things are done to the real data, and each is here because a fixture without it would be
// wrong or unsafe rather than merely untidy:
//
// 1. **Rebuilt as of the day.** TAWOS keeps an issue's final state. A board on a past day is
//    rebuilt from the change log: the status is where the last move on or before the day left it.
// 2. **Shifted by whole weeks.** Every date moves by the same number of days, a multiple of seven so
//    "this week" is still a week with the same weekdays, so the day lands on `today`.
// 3. **People replaced.** The slice holds ids only; every id becomes an invented name from a fixed
//    list. Nothing real is carried over, and `NAMES` is the only place a name can come from.
//
// Nothing here calls a model or the network. The output feeds the same reader Chief uses for Jira
// (`atlassian::issues_at`), so what is emitted is the REST shape, not a simplification of it.

export const SCHEMA_VERSION = 1;

/** Invented, and deliberately not names of anybody in the source projects. */
export const NAMES = [
  'Avery Quinn',
  'Maya Okafor',
  'Tomas Lindqvist',
  'Priya Ramanathan',
  'Jonas Whitlock',
  'Noor Haddad',
  'Elena Marchetti',
  'Kwame Boateng',
  'Sofia Delacroix',
  'Ravi Chandrasekar',
];

const DAY = 86_400_000;
const WEEK = 7 * DAY;
/** How far back a finished issue still counts as this week's news. */
export const RECENT = 14 * DAY;

/** Status name to Jira's own three categories. Every site renames statuses; the categories stay. */
export const CATEGORY = {
  open: 'new',
  'to do': 'new',
  new: 'new',
  backlog: 'new',
  reopened: 'new',
  'needs triage': 'new',
  triage: 'new',
  'in progress': 'indeterminate',
  'in review': 'indeterminate',
  'in code review': 'indeterminate',
  'in pr': 'indeterminate',
  'under review': 'indeterminate',
  'needs review': 'indeterminate',
  'needs reproduction': 'indeterminate',
  'needs feedback': 'indeterminate',
  'needs more info': 'indeterminate',
  'needs info': 'indeterminate',
  blocked: 'indeterminate',
  'waiting (blocked)': 'indeterminate',
  'debugging with submitter': 'indeterminate',
  testing: 'indeterminate',
  'in testing': 'indeterminate',
  'ready for work': 'new',
  'ready to merge': 'indeterminate',
  'waiting for customer': 'indeterminate',
  'waiting for user input': 'indeterminate',
  'waiting for review': 'indeterminate',
  // Project-specific names. The category is a judgement made here, not Jira's own: the dump keeps
  // status names and drops the category, so each name was read as what it says about the work.
  scoping: 'indeterminate',
  investigating: 'indeterminate',
  designing: 'indeterminate',
  'needs verification': 'indeterminate',
  'needs scope': 'new',
  'needs design': 'new',
  'needs scope approval': 'indeterminate',
  'needs design approval': 'indeterminate',
  'waiting for reporter': 'indeterminate',
  accepted: 'new',
  reviewable: 'indeterminate',
  'ready to review': 'indeterminate',
  'waiting for infra': 'indeterminate',
  resolved: 'done',
  closed: 'done',
  done: 'done',
  fixed: 'done',
  complete: 'done',
  completed: 'done',
  "won't fix": 'done',
  'won’t fix': 'done',
  "won't do": 'done',
  duplicate: 'done',
  invalid: 'done',
};

export function categoryOf(status) {
  const key = String(status ?? '')
    .trim()
    .toLowerCase();
  const found = CATEGORY[key];
  if (!found)
    throw new Error(`no category for status ${JSON.stringify(status)}; add it to CATEGORY`);
  return found;
}

/**
 * TAWOS stores a title the way a CSV cell is written: wrapped in double quotes, with any quote in
 * it doubled. Read it back as the title it was.
 */
export function title(raw) {
  const t = String(raw ?? '').trim();
  const quoted = t.length >= 2 && t.startsWith('"') && t.endsWith('"');
  return (quoted ? t.slice(1, -1) : t).replaceAll('""', '"');
}

/** Milliseconds since the epoch for a TAWOS timestamp (`YYYY-MM-DD HH:MM:SS`, UTC). */
export const ms = (s) => Date.parse(`${s.replace(' ', 'T')}Z`);

/**
 * One issue's status moves, oldest first. A "move" from a status to itself is the workflow being
 * edited, not work moving; left in, Chief would report "moved Open to Open", which nobody did.
 */
export function movesOf(list) {
  return list.filter((c) => c.from !== c.to).sort((a, b) => a.at.localeCompare(b.at));
}

/**
 * The status an issue had at the end of the day `T` (milliseconds), or `null` if it did not exist.
 * It is where the last move on or before the day left it; with none, where the first move after the
 * day started from; with no moves at all, the status it still has.
 */
export function statusAt(issue, moves, firstMoveAfter, T) {
  if (ms(issue.created) > T) return null;
  const made = moves.filter((c) => ms(c.at) <= T);
  if (made.length) return made.at(-1).to;
  return firstMoveAfter ? firstMoveAfter.from : issue.finalStatus;
}

/**
 * When an issue reached a finished status, as the change log says. Not TAWOS's `Resolution_Date`:
 * the Mesos project resolved thousands of issues in one sweep in 2018 and stamped them all with
 * that date, while their status had said Resolved for a year. One clock is used for both, because
 * a fixture that called an issue open and resolved at once would be asking Chief to guess.
 */
export function finishedAt(moves, T) {
  const made = moves.filter((c) => ms(c.at) <= T);
  const last = made.at(-1);
  return last && categoryOf(last.to) === 'done' ? ms(last.at) : null;
}

const iso = (t) => `${new Date(t).toISOString().slice(0, 19)}.000+0000`;
const day = (t) => new Date(t).toISOString().slice(0, 10);

/**
 * @param slice a TAWOS slice
 * @param target the date the slice's day should become, `YYYY-MM-DD`
 * @returns `{ today, me, search, answer }`
 */
export function convert(slice, target) {
  const asof = ms(`${slice.source.asof} 23:59:59`);
  const weeks = Math.round((ms(`${target} 23:59:59`) - asof) / WEEK);
  const shift = weeks * WEEK;
  const T = asof;

  // Invented names, handed out in order of first appearance so the same slice always gets the same
  // people. The person whose board it is comes first.
  const people = new Map();
  const nameOf = (id) => {
    if (!people.has(id)) {
      if (people.size >= NAMES.length) throw new Error('more people than invented names');
      people.set(id, NAMES[people.size]);
    }
    return people.get(id);
  };
  nameOf(slice.me);

  const byId = new Map(slice.issues.map((i) => [i.id, i]));
  const sprint = new Map(slice.sprints.map((s) => [s.id, s]));

  const moves = new Map();
  for (const c of slice.changes.filter((c) => c.field === 'status')) {
    (moves.get(c.issue) ?? moves.set(c.issue, []).get(c.issue)).push(c);
  }
  for (const [id, list] of moves) moves.set(id, movesOf(list));
  const firstAfter = new Map(slice.firstMoveAfter.map((c) => [c.issue, c]));
  const status_ = (issue) =>
    statusAt(issue, moves.get(issue.id) ?? [], firstAfter.get(issue.id), T);

  const statusObject = (name) => ({ name, statusCategory: { key: categoryOf(name) } });

  // A block is "A is blocked by B". The dump records each link from both ends, so both are read
  // and the pair is kept once.
  const blockedBy = new Map();
  for (const l of slice.links) {
    const blockerWords = /block/i.test(l.name) || /block/i.test(l.words);
    if (!blockerWords) continue;
    let blocked;
    let blocker;
    if (/^blocks$/i.test(l.words.trim()) && l.direction === 'OUTBOUND')
      [blocker, blocked] = [l.issue, l.target];
    else if (/is blocked by/i.test(l.words) && l.direction === 'INBOUND')
      [blocked, blocker] = [l.issue, l.target];
    else continue;
    const a = byId.get(blocked);
    const b = byId.get(blocker);
    if (!a || !b || ms(a.created) > T || ms(b.created) > T) continue;
    const list = blockedBy.get(blocked) ?? blockedBy.set(blocked, []).get(blocked);
    if (!list.includes(blocker)) list.push(blocker);
  }

  const issues = [];
  for (const issue of slice.issues.filter((i) => i.board)) {
    const status = status_(issue);
    if (status === null) continue;
    const category = categoryOf(status);
    const own = (moves.get(issue.id) ?? []).filter((c) => ms(c.at) <= T);
    const finished = finishedAt(own, T);
    // On the board if still open, or finished in the fortnight before the day.
    if (category === 'done' && !(finished && finished > T - RECENT)) continue;
    const linkChanges = slice.changes.filter(
      (c) =>
        c.issue === issue.id && c.field === 'link' && /is blocked by/i.test(c.to) && ms(c.at) <= T,
    );

    const events = [
      ms(issue.created),
      ...own.map((c) => ms(c.at)),
      ...linkChanges.map((c) => ms(c.at)),
    ];
    const histories = [
      ...own.map((c) => ({
        at: ms(c.at),
        item: { field: 'status', fromString: c.from, toString: c.to },
      })),
      ...linkChanges.map((c) => ({ at: ms(c.at), item: { field: 'Link', toString: c.to } })),
    ]
      .sort((a, b) => a.at - b.at)
      .map((h) => ({ created: iso(h.at + shift), items: [h.item] }));

    const resolved = category === 'done' ? iso(finished + shift) : null;
    const sp = sprint.get(issue.sprint);

    const links = (blockedBy.get(issue.id) ?? []).map((id) => {
      const b = byId.get(id);
      return {
        type: { name: 'Blocks', inward: 'is blocked by', outward: 'blocks' },
        inwardIssue: { key: b.key, fields: { status: statusObject(status_(b)) } },
      };
    });

    issues.push({
      key: issue.key,
      fields: {
        summary: title(issue.title),
        status: statusObject(status),
        resolutiondate: resolved,
        priority: { name: issue.priority ?? 'Medium' },
        duedate: sp?.end ? day(ms(sp.end) + shift) : null,
        updated: iso(Math.max(...events) + shift),
        issuelinks: links,
        assignee: { displayName: nameOf(issue.assignee) },
        labels: [],
      },
      changelog: { histories },
    });
  }

  issues.sort((a, b) => a.key.localeCompare(b.key, 'en', { numeric: true }));
  const today = day(T + shift);
  const search = { issues };
  return {
    schema_version: SCHEMA_VERSION,
    source: slice.source,
    today,
    me: nameOf(slice.me),
    search,
    answer: recompute(search, today),
  };
}

/**
 * What is true about a search result on a given day, worked out from the JSON alone. It is the
 * answer key, and it is deliberately not Chief's code: a benchmark that asked Chief what is true
 * would pass whatever Chief believed.
 */
export function recompute(search, today) {
  // Seven days ahead is the report's own "due soon": it says `Overdue or due this week` and means
  // anything due on or before the same weekday next week.
  const week = day(Date.parse(`${today}T00:00:00Z`) + 7 * DAY);
  const key = (i) => i.key;
  const cat = (i) => i.fields.status.statusCategory.key;
  const open = search.issues.filter((i) => cat(i) !== 'done');
  const done = search.issues.filter((i) => cat(i) === 'done');
  const blockers = (i) =>
    i.fields.issuelinks
      .filter((l) => l.inwardIssue.fields.status.statusCategory.key !== 'done')
      .map((l) => l.inwardIssue.key);
  const sorted = (list) =>
    list.map(key).sort((a, b) => a.localeCompare(b, 'en', { numeric: true }));

  // Where `/items` puts each open issue, first rule that applies: blocked, then due (or overdue)
  // within a week, then in progress, then not started. A section shows twelve lines and counts the
  // rest, so `count` is how many belong and `keys` is all of them, not only those shown.
  const due = (i) => i.fields.duedate && i.fields.duedate <= week;
  const section = (pick) => {
    const keys = sorted(open.filter(pick));
    return { count: keys.length, keys };
  };
  const isBlocked = (i) => blockers(i).length > 0;
  const sections = {
    Blocked: section(isBlocked),
    'Overdue or due this week': section((i) => !isBlocked(i) && due(i)),
    'In progress': section((i) => !isBlocked(i) && !due(i) && cat(i) === 'indeterminate'),
    'Not started': section((i) => !isBlocked(i) && !due(i) && cat(i) !== 'indeterminate'),
  };

  return {
    open: sorted(open),
    not_started: sorted(open.filter((i) => cat(i) === 'new')),
    in_progress: sorted(open.filter((i) => cat(i) === 'indeterminate')),
    finished: sorted(done),
    blocked: Object.fromEntries(
      open.filter((i) => blockers(i).length).map((i) => [i.key, blockers(i)]),
    ),
    overdue: sorted(open.filter((i) => i.fields.duedate && i.fields.duedate < today)),
    due_this_week: sorted(
      open.filter((i) => i.fields.duedate && i.fields.duedate >= today && i.fields.duedate <= week),
    ),
    no_due_date: sorted(open.filter((i) => !i.fields.duedate)),
    sections,
    people: [...new Set(search.issues.map((i) => i.fields.assignee.displayName))].sort(),
  };
}

/**
 * The same fixture on another day: every date moves by the same exact number of days, so the
 * day it describes becomes `today` and everything keeps its distance from it. The answer key is
 * therefore the same, which is checked rather than assumed (`recompute` on the result).
 *
 * A benchmark needs this because Chief reads the clock of the machine it runs on, and "overdue" is
 * a statement about that clock. Whole weeks (see `convert`) would leave the weekday right and the
 * date wrong; here the date is what has to be right.
 */
export function retarget(fixture, today) {
  const days = Math.round(
    (Date.parse(`${today}T00:00:00Z`) - Date.parse(`${fixture.today}T00:00:00Z`)) / DAY,
  );
  const moved = (iso) => (iso ? new Date(Date.parse(iso) + days * DAY).toISOString() : iso);
  const datetime = (iso) => (iso ? `${moved(iso).slice(0, 19)}.000+0000` : iso);
  const date = (d) => (d ? day(Date.parse(`${d}T00:00:00Z`) + days * DAY) : d);

  const out = structuredClone(fixture);
  out.today = today;
  for (const issue of out.search.issues) {
    issue.fields.duedate = date(issue.fields.duedate);
    issue.fields.updated = datetime(issue.fields.updated);
    issue.fields.resolutiondate = datetime(issue.fields.resolutiondate);
    for (const history of issue.changelog.histories) history.created = datetime(history.created);
  }
  return out;
}
