import { readFileSync, readdirSync } from 'node:fs';
import { dirname, join } from 'node:path';
import { fileURLToPath } from 'node:url';

import { expect, test } from 'vitest';

import { build } from './build-fixtures.mjs';
import {
  CATEGORY,
  NAMES,
  RECENT,
  categoryOf,
  convert,
  finishedAt,
  movesOf,
  recompute,
  retarget,
  statusAt,
  title,
} from './to-jira.mjs';

const here = dirname(fileURLToPath(import.meta.url));
const DAY = 86_400_000;

// A board on Friday 2019-02-15, to be moved to Friday 2026-11-20.
const issue = (over) => ({
  id: 1,
  key: 'P-1',
  title: '"A title"',
  type: 'Bug',
  priority: 'Major',
  finalStatus: 'Closed',
  created: '2019-01-01 10:00:00',
  resolved: null,
  updated: null,
  assignee: 900,
  sprint: null,
  board: true,
  ...over,
});
const slice = (over = {}) => ({
  source: { asof: '2019-02-15', project: { key: 'P', name: 'P' } },
  me: 900,
  issues: [issue({})],
  changes: [],
  firstMoveAfter: [],
  links: [],
  sprints: [],
  ...over,
});
const move = (id, at, from, to) => ({ issue: id, field: 'status', at, from, to });

test('a title is read back out of the way TAWOS wrote it', () => {
  expect(title('"Add ""test"" connection"')).toBe('Add "test" connection');
  expect(title('plain')).toBe('plain');
  expect(title('"')).toBe('"');
});

test('the status is where the last move on or before the day left it', () => {
  const T = Date.parse('2019-02-15T23:59:59Z');
  const i = issue({});
  const moves = movesOf([
    move(1, '2019-01-05 09:00:00', 'Open', 'In Progress'),
    move(1, '2019-02-10 09:00:00', 'In Progress', 'Closed'),
    move(1, '2019-02-16 09:00:00', 'Closed', 'Open'),
  ]);
  expect(statusAt(i, moves, undefined, T)).toBe('Closed');
  expect(statusAt(i, moves.slice(0, 1), undefined, T)).toBe('In Progress');
});

test('with no move before the day, an issue is where its first later move started', () => {
  const T = Date.parse('2019-02-15T23:59:59Z');
  const later = move(1, '2019-03-01 09:00:00', 'Open', 'Closed');
  expect(statusAt(issue({}), [later], later, T)).toBe('Open');
  expect(statusAt(issue({ finalStatus: 'Open' }), [], undefined, T)).toBe('Open');
});

test('an issue that did not exist yet is not on the board', () => {
  const T = Date.parse('2019-02-15T23:59:59Z');
  expect(statusAt(issue({ created: '2019-02-16 00:00:01' }), [], undefined, T)).toBeNull();
});

test('a move from a status to itself is not work moving', () => {
  expect(movesOf([move(1, '2019-01-05 09:00:00', 'Open', 'Open')])).toEqual([]);
});

test('finished is the day the status said so, not the resolution date', () => {
  const T = Date.parse('2019-02-15T23:59:59Z');
  const moves = movesOf([move(1, '2018-06-01 09:00:00', 'Open', 'Resolved')]);
  expect(finishedAt(moves, T)).toBe(Date.parse('2018-06-01T09:00:00Z'));
  expect(
    finishedAt(movesOf([move(1, '2019-01-05 09:00:00', 'Open', 'In Progress')]), T),
  ).toBeNull();
});

test('a status the table does not know stops the build instead of being guessed', () => {
  expect(() => categoryOf('Gnarly')).toThrow(/no category/);
  expect(categoryOf('closed')).toBe('done');
  for (const k of Object.keys(CATEGORY))
    expect(['new', 'indeterminate', 'done']).toContain(CATEGORY[k]);
});

test('dates move by whole weeks, so the weekday survives, and the day lands on the target', () => {
  const out = convert(
    slice({
      changes: [move(1, '2019-02-12 09:30:00', 'Open', 'In Progress')],
      issues: [issue({ finalStatus: 'In Progress' })],
    }),
    '2026-11-23',
  );
  expect(out.today).toBe('2026-11-20'); // the Friday, not the Monday asked for
  const created = out.search.issues[0].changelog.histories[0].created;
  expect(created).toBe('2026-11-17T09:30:00.000+0000');
  expect(new Date(created).getUTCDay()).toBe(new Date('2019-02-12T09:30:00Z').getUTCDay());
});

test('every person is an invented name, the owner of the board first', () => {
  const out = convert(
    slice({
      me: 900,
      issues: [
        issue({ id: 1, assignee: 900, finalStatus: 'Open' }),
        issue({ id: 2, key: 'P-2', assignee: 777, finalStatus: 'Open' }),
      ],
    }),
    '2026-11-23',
  );
  expect(out.me).toBe(NAMES[0]);
  const named = out.search.issues.map((i) => i.fields.assignee.displayName);
  expect(named).toEqual([NAMES[0], NAMES[1]]);
  expect(JSON.stringify(out)).not.toMatch(/\b(900|777)\b/);
});

test('more people than invented names is an error, not a reused name', () => {
  const many = Array.from({ length: NAMES.length + 1 }, (_, n) =>
    issue({ id: n + 1, key: `P-${n + 1}`, assignee: 1000 + n, finalStatus: 'Open' }),
  );
  expect(() => convert(slice({ me: 1000, issues: many }), '2026-11-23')).toThrow(/invented names/);
});

test('an issue is blocked once, whichever end of the link the dump recorded', () => {
  const issues = [
    issue({ id: 1, key: 'P-1', finalStatus: 'Open' }),
    issue({ id: 2, key: 'P-2', finalStatus: 'Open', board: false }),
  ];
  const from = (links) =>
    convert(slice({ issues, links }), '2026-11-23').search.issues[0].fields.issuelinks;
  const mirrored = [
    { issue: 2, name: 'Blocks', words: 'blocks', direction: 'OUTBOUND', target: 1 },
    { issue: 1, name: 'Blocks', words: 'is blocked by', direction: 'INBOUND', target: 2 },
  ];
  expect(from(mirrored).map((l) => l.inwardIssue.key)).toEqual(['P-2']);
  expect(from([mirrored[1]]).map((l) => l.inwardIssue.key)).toEqual(['P-2']);
  expect(from([{ ...mirrored[0], name: 'Relates', words: 'relates to' }])).toEqual([]);
});

test('a finished blocker is not reported as blocking', () => {
  const search = {
    issues: [
      {
        key: 'P-1',
        fields: {
          status: { statusCategory: { key: 'new' } },
          duedate: null,
          assignee: { displayName: 'A' },
          issuelinks: [
            {
              inwardIssue: { key: 'P-2', fields: { status: { statusCategory: { key: 'done' } } } },
            },
            {
              inwardIssue: {
                key: 'P-3',
                fields: { status: { statusCategory: { key: 'indeterminate' } } },
              },
            },
          ],
        },
      },
    ],
  };
  expect(recompute(search, '2026-11-20').blocked).toEqual({ 'P-1': ['P-3'] });
});

test('overdue and due this week have exact edges', () => {
  const at = (duedate) => ({
    key: `k-${duedate}`,
    fields: {
      status: { statusCategory: { key: 'new' } },
      duedate,
      assignee: { displayName: 'A' },
      issuelinks: [],
    },
  });
  const search = {
    issues: ['2026-11-19', '2026-11-20', '2026-11-27', '2026-11-28', null].map(at),
  };
  const a = recompute(search, '2026-11-20');
  expect(a.overdue).toEqual(['k-2026-11-19']);
  expect(a.due_this_week).toEqual(['k-2026-11-20', 'k-2026-11-27']);
  expect(a.no_due_date).toEqual(['k-null']);
});

test('finished work counts only if it was finished in the fortnight before the day', () => {
  const day = (n) =>
    new Date(Date.parse('2019-02-15T12:00:00Z') - n * DAY)
      .toISOString()
      .slice(0, 19)
      .replace('T', ' ');
  const issues = [
    issue({ id: 1, key: 'P-1' }),
    issue({ id: 2, key: 'P-2' }),
    issue({ id: 3, key: 'P-3' }),
  ];
  const changes = [
    move(1, day(3), 'Open', 'Closed'),
    move(2, day(RECENT / DAY + 1), 'Open', 'Closed'),
    move(3, day(60), 'Open', 'Closed'),
  ];
  const out = convert(slice({ issues, changes }), '2026-11-23');
  expect(out.answer.finished).toEqual(['P-1']);
});

test('a resolution date that disagrees with the status does not make an issue open and finished', () => {
  const changes = [move(1, '2018-06-01 09:00:00', 'Open', 'Closed')];
  const out = convert(
    slice({ changes, issues: [issue({ resolved: '2019-03-01 00:00:00' })] }),
    '2026-11-23',
  );
  expect(out.search.issues).toEqual([]); // finished long ago on the status clock
});

// --- the committed fixtures ----------------------------------------------------------------

const fixtures = readdirSync(join(here, 'fixtures')).filter((f) => f.endsWith('.json'));
const read = (f) => JSON.parse(readFileSync(join(here, 'fixtures', f), 'utf8'));

test('the fixtures are what the converter writes from the slices', () => {
  const built = build();
  expect(Object.keys(built).sort()).toEqual([...fixtures].sort());
  for (const [name, text] of Object.entries(built)) {
    expect(readFileSync(join(here, 'fixtures', name), 'utf8'), name).toBe(text);
  }
});

test('every answer key is the one worked out from the JSON beside it', () => {
  for (const f of fixtures) {
    const fx = read(f);
    expect(fx.answer, f).toEqual(recompute(fx.search, fx.today));
  }
});

test('a fixture carries no free text beyond a title, and no name outside the invented list', () => {
  for (const f of fixtures) {
    const fx = read(f);
    for (const i of fx.search.issues) {
      expect(Object.keys(i.fields).sort(), i.key).toEqual([
        'assignee',
        'duedate',
        'issuelinks',
        'labels',
        'priority',
        'resolutiondate',
        'status',
        'summary',
        'updated',
      ]);
      expect(NAMES, i.key).toContain(i.fields.assignee.displayName);
    }
  }
});

test('nothing in a fixture is dated after the day it describes', () => {
  for (const f of fixtures) {
    const fx = read(f);
    const end = Date.parse(`${fx.today}T23:59:59Z`);
    for (const i of fx.search.issues) {
      expect(Date.parse(i.fields.updated), i.key).toBeLessThanOrEqual(end);
      for (const h of i.changelog.histories)
        expect(Date.parse(h.created), i.key).toBeLessThanOrEqual(end);
    }
  }
});

test('the board is the one person it says it is, and there is something on it to ask about', () => {
  for (const f of fixtures) {
    const fx = read(f);
    expect(fx.answer.people, f).toEqual([fx.me]);
    expect(fx.answer.open.length, f).toBeGreaterThan(0);
  }
});

test('a fixture moved to another day says the same things about that day', () => {
  for (const f of fixtures) {
    const fx = read(f);
    for (const today of ['2026-12-02', '2027-03-15', '2026-11-01']) {
      const moved = retarget(fx, today);
      expect(moved.today).toBe(today);
      expect(recompute(moved.search, today), `${f} on ${today}`).toEqual(fx.answer);
    }
  }
});

test('retarget moves every date and leaves everything else alone', () => {
  const fx = read('jira-mesos.json');
  const days = Math.round((Date.parse('2026-12-02') - Date.parse(fx.today)) / DAY);
  const moved = retarget(fx, '2026-12-02');
  const was = fx.search.issues.find((i) => i.fields.duedate);
  const now = moved.search.issues.find((i) => i.key === was.key);
  expect(Date.parse(now.fields.duedate) - Date.parse(was.fields.duedate)).toBe(days * DAY);
  expect(Date.parse(now.fields.updated) - Date.parse(was.fields.updated)).toBe(days * DAY);
  expect(now.fields.summary).toBe(was.fields.summary);
  expect(fx.today).not.toBe('2026-12-02'); // the original is untouched
});
