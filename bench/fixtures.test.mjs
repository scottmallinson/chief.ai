import { expect, test } from 'vitest';

import { cases } from './routing-fixtures.json';

const TOOLS = { github: 'fetch_github_prs', calendar: 'fetch_calendar', mail: 'fetch_recent_mail' };

test('every case has a unique id and a question', () => {
  expect(new Set(cases.map((c) => c.id)).size).toBe(cases.length);
  for (const c of cases) expect(c.question.trim().length, c.id).toBeGreaterThan(0);
});

test('the expected tool follows from the expected source', () => {
  for (const c of cases) {
    const want = c.not_connected ? null : (TOOLS[c.source] ?? null);
    expect(c.tool, `${c.id} ${c.question}`).toBe(want);
  }
});

test('a source marked not connected is the one the question needs', () => {
  for (const c of cases.filter((c) => c.not_connected)) {
    expect(c.connected[c.source], c.id).toBe(false);
    expect(c.tool, c.id).toBeNull();
  }
});

test('every case keeps the label and wording of each set it came from', () => {
  for (const c of cases) {
    expect(Object.keys(c.labels).sort(), c.id).toEqual(Object.keys(c.texts).sort());
    expect(Object.keys(c.labels).sort(), c.id).toEqual([...c.origins].sort());
  }
});

test('every group is present', () => {
  const groups = new Set(cases.map((c) => c.group));
  for (const g of [
    'source',
    'github-view',
    'calendar-range',
    'bench',
    'time-and-date',
    'not-connected',
  ]) {
    expect(groups.has(g), g).toBe(true);
  }
});

test('arguments are only given where the tool takes them', () => {
  for (const c of cases.filter((c) => c.args && !c.not_connected)) {
    expect(['fetch_github_prs', 'fetch_calendar'], c.id).toContain(c.tool);
  }
});
