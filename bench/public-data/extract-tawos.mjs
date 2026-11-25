// Cuts a small, readable slice out of the TAWOS dump: one person's board in one project as it stood
// on one past day. Run it once, by hand; the slice it writes is what gets committed, and the
// converter (`to-jira.mjs`) works from that, never from the 4 GB dump.
//
//   node bench/public-data/extract-tawos.mjs <TAWOS.sql> --project COMPASS --asof 2019-02-15 \
//        --out bench/public-data/slices/compass.json
//
// TAWOS is Apache-2.0 (https://github.com/solar-group/tawos, doi 10.5522/04/21308124). It stores
// each issue's *final* state, so "what was open that day" is rebuilt from the change log by the
// converter. Here we only choose who and what to keep, and keep as little as will do:
//
// - the busiest assignee on the day, so a board is a person's, as `/items` reads it;
// - their issues open on the day (status rebuilt from the change log), and finished in the fortnight before it;
// - the issues those link to, so a blocker has a status;
// - titles, never descriptions or comments, and no author names. TAWOS replaced people with ids;
//   the converter replaces those with invented names.

import { writeFileSync } from 'node:fs';
import { parseArgs } from 'node:util';

import { rows } from './mysql-dump.mjs';
import { RECENT, categoryOf, finishedAt, movesOf, statusAt } from './to-jira.mjs';

const DAY = 86_400_000;
// RECENT (the fortnight a finished issue stays on the board) lives in the converter, because both
// halves must agree on it.
const at = (s) => (s ? Date.parse(`${s.replace(' ', 'T')}Z`) : null);

const { values, positionals } = parseArgs({
  allowPositionals: true,
  options: {
    project: { type: 'string' },
    asof: { type: 'string' },
    out: { type: 'string' },
    assignee: { type: 'string' },
    survey: { type: 'boolean', default: false },
  },
});
const [dump] = positionals;
if (!dump || !values.project || !values.asof || (!values.out && !values.survey)) {
  console.error('usage: extract-tawos.mjs <TAWOS.sql> --project KEY --asof YYYY-MM-DD --out file');
  process.exit(2);
}
const T = at(`${values.asof} 23:59:59`);

// Pass 1: the project, then every issue in it.
let project;
for await (const r of rows(dump, 'Project'))
  if (r[1] === values.project) project = { id: r[0], key: r[1], name: r[2] };
if (!project) throw new Error(`no project ${values.project}`);

const all = new Map();
for await (const r of rows(dump, 'Issue')) {
  if (r[28] !== project.id) continue;
  all.set(r[0], {
    id: r[0],
    key: r[2],
    title: r[4],
    type: r[8],
    priority: r[9],
    finalStatus: r[10],
    created: r[12],
    resolved: r[14],
    updated: r[15],
    assignee: r[27],
    sprint: r[29],
  });
}

// Pass 2, the only read of the change log: every status move and link change of every issue in the
// project, because who is busy on the day is only known once each status has been rebuilt.
const moves = new Map();
const linkChanges = [];
for await (const r of rows(dump, 'Change_Log')) {
  const issue = r[9];
  if (!all.has(issue)) continue;
  if (r[1] === 'status')
    (moves.get(issue) ?? moves.set(issue, []).get(issue)).push({
      issue,
      field: 'status',
      at: r[7],
      from: r[4],
      to: r[5],
    });
  else if (r[1] === 'Link' && at(r[7]) <= T)
    linkChanges.push({ issue, field: 'link', at: r[7], to: r[5] });
}
for (const [id, list] of moves) moves.set(id, movesOf(list));

// Every status the project ever used, checked at once, so a table that is missing three names says
// so in one run and not in three.
const seen = new Set([...all.values()].map((i) => i.finalStatus));
for (const list of moves.values()) for (const c of list) seen.add(c.from).add(c.to);
const unknown = [...seen].filter((name) => {
  try {
    categoryOf(name);
    return false;
  } catch {
    return true;
  }
});
if (unknown.length)
  throw new Error(
    `statuses with no category in to-jira.mjs: ${unknown.map((n) => JSON.stringify(n)).join(', ')}`,
  );

const firstAfter = (id) => (moves.get(id) ?? []).find((c) => at(c.at) > T);
const state = (i) => {
  const status = statusAt(i, moves.get(i.id) ?? [], firstAfter(i.id), T);
  if (status === null) return null;
  const done = categoryOf(status) === 'done';
  const finished = done
    ? finishedAt(
        (moves.get(i.id) ?? []).filter((c) => at(c.at) <= T),
        T,
      )
    : null;
  return { open: !done, finished };
};
const onBoard = (i) => {
  const s = state(i);
  return s && i.assignee && (s.open || (s.finished && s.finished > T - RECENT));
};

// --survey: say who has a board worth cutting on that day, and write nothing.
if (values.survey) {
  const idsOfProject = new Set(all.keys());
  const blockedBy = new Map();
  const releasedBy = new Map();
  for await (const r of rows(dump, 'Issue_Link')) {
    if (!idsOfProject.has(r[1]) || !idsOfProject.has(r[5])) continue;
    let blocked;
    let blocker;
    if (/^blocks$/i.test(r[3].trim()) && r[4] === 'OUTBOUND') [blocker, blocked] = [r[1], r[5]];
    else if (/is blocked by/i.test(r[3]) && r[4] === 'INBOUND') [blocked, blocker] = [r[1], r[5]];
    else continue;
    if (state(all.get(blocker))?.open && state(all.get(blocked))?.open)
      blockedBy.set(blocked, new Set([...(blockedBy.get(blocked) ?? []), blocker]));
  }
  const people = new Map();
  for (const i of all.values()) {
    if (!state(i)?.open || !i.assignee) continue;
    const p = people.get(i.assignee) ?? { open: 0, blocked: 0, released: 0 };
    p.open++;
    if (blockedBy.has(i.id)) p.blocked++;
    if (releasedBy.has(i.id)) p.released++;
    people.set(i.assignee, p);
  }
  const top = [...people.entries()]
    .filter(([, p]) => p.blocked)
    .sort((a, b) => b[1].blocked - a[1].blocked);
  console.log(
    `${project.key} ${values.asof}:`,
    top
      .slice(0, 6)
      .map(([id, p]) => `#${id} open ${p.open} blocked ${p.blocked} released ${p.released}`)
      .join(' | '),
  );
  process.exit(0);
}

// The busiest assignee among issues created in the 180 days before the day, so the board is a
// current one and not a person's whole history.
const load = new Map();
for (const i of all.values()) {
  if (state(i)?.open && i.assignee && at(i.created) >= T - 180 * DAY)
    load.set(i.assignee, (load.get(i.assignee) ?? 0) + 1);
}
const me = values.assignee
  ? Number(values.assignee)
  : ([...load.entries()].sort((a, b) => b[1] - a[1] || a[0] - b[0])[0] ?? [])[0];
if (me === undefined) throw new Error('nobody had open work on that day');

const mine = [...all.values()].filter((i) => i.assignee === me && onBoard(i));
const ids = new Set(mine.map((i) => i.id));

// Links between issues, and the issues a board's blockers are.
const links = [];
for await (const r of rows(dump, 'Issue_Link')) {
  if (ids.has(r[1]) || ids.has(r[5]))
    links.push({ issue: r[1], name: r[2], words: r[3], direction: r[4], target: r[5] });
}
const blockers = (l) => /block/i.test(l.name) || /block/i.test(l.words);
const neighbours = new Set();
for (const l of links.filter(blockers))
  for (const id of [l.issue, l.target]) if (!ids.has(id)) neighbours.add(id);
const linked = [...neighbours].map((id) => all.get(id)).filter(Boolean);
const keep = new Set([...ids, ...linked.map((i) => i.id)]);

// Only what the converter reads is kept. Moves after the day are dropped, except the first, which
// says where an issue started when it has no earlier move.
const changes = [];
const firstMoveAfter = [];
for (const id of keep) {
  for (const c of moves.get(id) ?? []) if (at(c.at) <= T) changes.push(c);
  const next = firstAfter(id);
  if (next) firstMoveAfter.push(next);
}
changes.push(...linkChanges.filter((c) => keep.has(c.issue)));

const sprints = new Map();
for await (const r of rows(dump, 'Sprint')) {
  if (r[8] === project.id) sprints.set(r[0], { id: r[0], name: r[2], start: r[4], end: r[5] });
}
const used = new Set([...keep].map((id) => all.get(id)?.sprint).filter(Boolean));

const slice = {
  source: {
    dataset: 'TAWOS v1.1',
    doi: '10.5522/04/21308124',
    licence: 'Apache-2.0',
    project: { key: project.key, name: project.name },
    asof: values.asof,
  },
  me,
  issues: [...keep].map((id) => all.get(id)).map((i) => ({ ...i, board: ids.has(i.id) })),
  changes,
  firstMoveAfter,
  links: links.filter((l) => keep.has(l.issue) && keep.has(l.target)),
  sprints: [...used].map((id) => sprints.get(id)).filter(Boolean),
};
writeFileSync(values.out, `${JSON.stringify(slice, null, 1)}\n`);
console.log(
  `${project.key}: assignee #${me}, ${mine.length} on the board, ${linked.length} linked, ${changes.length} changes`,
);
