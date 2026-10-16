import { useCallback, useEffect, useState, type FormEvent } from 'react';
import { Pencil, Plus, Trash2 } from 'lucide-react';

import { Button } from '@/components/ui/button';
import {
  deleteFact,
  describeFact,
  KINDS,
  listFacts,
  saveFact,
  type Fact,
  type FactInput,
} from '@/lib/facts';

const EMPTY: FactInput = { about: '', what: 'reviews', scope: '', value: '' };

const MONTHS = ['Jan', 'Feb', 'Mar', 'Apr', 'May', 'Jun', 'Jul', 'Aug', 'Sep', 'Oct', 'Nov', 'Dec'];

/** `2026-10-12` as `12 Oct`. Written out rather than localised, so it reads the same everywhere. */
function dated(seenOn: string): string {
  const [, month, day] = seenOn.split('-');
  const name = MONTHS[Number(month) - 1];
  return name === undefined || day === undefined ? seenOn : `${Number(day)} ${name}`;
}

/**
 * What Chief knows about how the user writes and who reviews their work, as
 * rows they can read, correct and delete.
 *
 * A row Chief counted says where and when. One the user edits becomes theirs
 * and no recount touches it; one they delete stays deleted. That is the whole
 * contract, and it is why this replaces a hand-edited file: every line here can
 * say when it was last true.
 */
export function FactsEditor() {
  const [facts, setFacts] = useState<Fact[] | null>(null);
  const [editing, setEditing] = useState<{ id: number | null; input: FactInput } | null>(null);
  const [error, setError] = useState<string | null>(null);

  const load = useCallback(() => {
    listFacts()
      .then(setFacts)
      .catch((cause: unknown) => {
        setError(cause instanceof Error ? cause.message : String(cause));
      });
  }, []);

  useEffect(() => load(), [load]);

  const kind = KINDS.find((one) => one.what === editing?.input.what) ?? KINDS[0];

  function save(event: FormEvent) {
    event.preventDefault();
    if (editing === null || kind === undefined) return;

    const input: FactInput = {
      ...editing.input,
      about: kind.about === 'you' ? 'you' : editing.input.about.trim().replace(/^@/, ''),
    };

    setError(null);
    saveFact(editing.id, input)
      .then(() => {
        setEditing(null);
        load();
      })
      .catch((cause: unknown) => {
        setError(cause instanceof Error ? cause.message : String(cause));
      });
  }

  function remove(fact: Fact) {
    setError(null);
    deleteFact(fact.id)
      .then(load)
      .catch((cause: unknown) => {
        setError(cause instanceof Error ? cause.message : String(cause));
      });
  }

  function edit(fact: Fact) {
    setEditing({
      id: fact.id,
      input: { about: fact.about, what: fact.what, scope: fact.scope, value: fact.value },
    });
  }

  const rows = facts ?? [];
  const editable = (fact: Fact) => KINDS.some((one) => one.what === fact.what);
  const groups = [
    { title: 'How you write', rows: rows.filter((fact) => fact.about === 'you') },
    { title: 'People', rows: rows.filter((fact) => fact.about !== 'you') },
  ];

  const missing =
    editing === null ||
    kind === undefined ||
    (kind.about === 'person' && editing.input.about.trim() === '') ||
    (kind.needs.includes('scope') && editing.input.scope.trim() === '') ||
    (kind.needs.includes('value') && editing.input.value.trim() === '');

  return (
    <div className="mt-5 border-t border-border pt-4">
      <h3 className="micro text-muted-foreground">What Chief knows</h3>

      {facts !== null && rows.length === 0 && (
        <p className="mt-2 text-[13px] leading-snug text-muted-foreground">
          Nothing yet. Count it from your work above, or add a fact yourself.
        </p>
      )}

      {groups
        .filter((group) => group.rows.length > 0)
        .map((group) => (
          <div key={group.title} className="mt-3">
            <h4 className="text-[12px] text-muted-foreground">{group.title}</h4>
            <ul className="mt-1.5 flex flex-col gap-1.5">
              {group.rows.map((fact) => (
                <li
                  key={fact.id}
                  className="flex items-center justify-between gap-3 rounded-md border border-border px-3 py-1.5"
                >
                  <span className="min-w-0 flex-1 text-[13px] leading-snug">
                    {describeFact(fact)}
                    <span className="ml-2 font-mono text-[11px] text-muted-foreground">
                      {fact.origin === 'yours' ? 'yours' : `${fact.source} · ${dated(fact.seenOn)}`}
                    </span>
                  </span>
                  <span className="flex shrink-0 gap-1">
                    {editable(fact) && (
                      <Button
                        variant="ghost"
                        size="sm"
                        aria-label={`Edit: ${describeFact(fact)}`}
                        onClick={() => edit(fact)}
                      >
                        <Pencil aria-hidden />
                      </Button>
                    )}
                    <Button
                      variant="ghost"
                      size="sm"
                      aria-label={`Remove: ${describeFact(fact)}`}
                      onClick={() => remove(fact)}
                    >
                      <Trash2 aria-hidden />
                    </Button>
                  </span>
                </li>
              ))}
            </ul>
          </div>
        ))}

      {editing !== null && kind !== undefined && (
        <form className="mt-3 flex flex-col gap-2" onSubmit={save}>
          <label className="micro text-muted-foreground" htmlFor="fact-kind">
            What
          </label>
          <select
            id="fact-kind"
            value={editing.input.what}
            onChange={(event) =>
              setEditing({ ...editing, input: { ...editing.input, what: event.target.value } })
            }
            className="w-full rounded-md border border-input bg-background px-3 py-2 text-sm"
          >
            {KINDS.map((one) => (
              <option key={one.what} value={one.what}>
                {one.label}
              </option>
            ))}
          </select>

          {kind.about === 'person' && (
            <>
              <label className="micro text-muted-foreground" htmlFor="fact-about">
                GitHub handle
              </label>
              <input
                id="fact-about"
                type="text"
                value={editing.input.about}
                spellCheck={false}
                autoComplete="off"
                onChange={(event) =>
                  setEditing({ ...editing, input: { ...editing.input, about: event.target.value } })
                }
                placeholder="priya-n"
                className="w-full rounded-md border border-input bg-background px-3 py-2 font-mono text-[13px]"
              />
            </>
          )}

          {kind.needs.includes('scope') && (
            <>
              <label className="micro text-muted-foreground" htmlFor="fact-scope">
                Repository
              </label>
              <input
                id="fact-scope"
                type="text"
                value={editing.input.scope}
                spellCheck={false}
                autoComplete="off"
                onChange={(event) =>
                  setEditing({ ...editing, input: { ...editing.input, scope: event.target.value } })
                }
                placeholder="owner/name"
                className="w-full rounded-md border border-input bg-background px-3 py-2 font-mono text-[13px]"
              />
            </>
          )}

          {kind.needs.includes('value') && (
            <>
              <label className="micro text-muted-foreground" htmlFor="fact-value">
                Value
              </label>
              <input
                id="fact-value"
                type="text"
                value={editing.input.value}
                onChange={(event) =>
                  setEditing({ ...editing, input: { ...editing.input, value: event.target.value } })
                }
                className="w-full rounded-md border border-input bg-background px-3 py-2 text-sm"
              />
            </>
          )}

          <div className="mt-1 flex gap-2">
            <Button type="submit" size="sm" disabled={missing}>
              Save
            </Button>
            <Button type="button" variant="ghost" size="sm" onClick={() => setEditing(null)}>
              Cancel
            </Button>
          </div>
        </form>
      )}

      {error !== null && (
        <p className="mt-3 text-[13px] leading-snug text-attention-text" role="alert">
          {error}
        </p>
      )}

      {editing === null && (
        <div className="mt-3 flex">
          <Button
            variant="outline"
            size="sm"
            onClick={() => setEditing({ id: null, input: EMPTY })}
          >
            <Plus aria-hidden />
            Add a fact
          </Button>
        </div>
      )}
    </div>
  );
}
