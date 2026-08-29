import { useState } from 'react';
import { Save } from 'lucide-react';

import { Button } from '@/components/ui/button';
import { Dots, Sweep } from '@/components/ui/activity';
import { refineDraft, saveProposal, type Proposal } from '@/lib/proposals';

/**
 * The rewrites worth a button.
 *
 * Each is one the user would otherwise type, and typing it is the friction the
 * single-shot path exists to remove — this is the call people make repeatedly
 * while looking at the result, so it has to cost a click.
 */
const REWRITES = ['Make it less formal', 'Make it shorter', 'Make it warmer'];

interface DraftEditorProps {
  proposal: Proposal;
  onSaved: () => void;
}

/**
 * A draft, open for editing, in the drawer.
 *
 * The textarea is the truth here, not the file: a rewrite replaces what is in
 * the box, and Save writes the box back to disk. Nothing is written until Save,
 * so trying three rewrites and liking none of them costs nothing.
 */
export function DraftEditor({ proposal, onSaved }: DraftEditorProps) {
  const [body, setBody] = useState(proposal.body);
  const [busy, setBusy] = useState<string | null>(null);
  const [error, setError] = useState<string | null>(null);
  const [saved, setSaved] = useState(false);

  function rewrite(instruction: string) {
    setBusy(instruction);
    setError(null);
    setSaved(false);

    refineDraft(body, instruction)
      .then((rewritten) => setBody(rewritten))
      .catch((cause: unknown) => {
        // The draft in the box is untouched. Losing somebody's words because a
        // rewrite of them failed would be the worst thing this screen can do.
        setError(cause instanceof Error ? cause.message : String(cause));
      })
      .finally(() => setBusy(null));
  }

  function save() {
    setBusy('save');
    setError(null);

    saveProposal(proposal.path, body)
      .then(() => {
        setSaved(true);
        onSaved();
      })
      .catch((cause: unknown) => {
        setError(cause instanceof Error ? cause.message : String(cause));
      })
      .finally(() => setBusy(null));
  }

  const working = busy !== null;

  return (
    <div className="flex h-full flex-col">
      <div className="min-h-0 flex-1 overflow-y-auto px-7 py-6">
        <div className="flex max-w-[680px] flex-col gap-3">
          <div>
            <h3 className="text-[15px] font-semibold tracking-[-0.015em]">{proposal.title}</h3>
            <p className="mt-1 text-[13px] leading-snug text-muted-foreground">
              Chief has sent nothing. This is a file in your corpus, and it stays one.
            </p>
          </div>

          <textarea
            value={body}
            onChange={(event) => {
              setBody(event.target.value);
              setSaved(false);
            }}
            aria-label="Draft"
            rows={12}
            className="min-h-[220px] w-full resize-y rounded-md border border-input bg-background px-3 py-2 text-sm leading-relaxed"
          />

          <p className="font-mono text-[11px] break-all text-muted-foreground" data-selectable>
            {proposal.path}
          </p>

          {error !== null && (
            <p
              className="rounded-md border border-destructive bg-destructive-surface px-3.5 py-3 text-[13px] leading-snug text-destructive-text"
              role="alert"
            >
              {error}
            </p>
          )}

          {busy !== null && busy !== 'save' && (
            <p className="flex items-center gap-2 micro text-verified-text" role="status">
              <Sweep />
              local · Rewriting
            </p>
          )}

          {saved && (
            <p className="micro text-verified-text" role="status">
              Saved to your corpus
            </p>
          )}
        </div>
      </div>

      <div className="border-t border-border px-7 py-3.5">
        <div className="flex max-w-[680px] flex-col gap-2.5">
          {/* Chip-shaped, but not `Chip`: chips carry state, never actions. */}
          <div className="flex flex-wrap gap-1.5">
            {REWRITES.map((instruction) => (
              <button
                key={instruction}
                type="button"
                onClick={() => rewrite(instruction)}
                disabled={working}
                className="rounded-sm border border-input px-[9px] py-[3px] text-[11px] text-muted-foreground transition-colors duration-[120ms] ease-instrument hover:bg-accent hover:text-accent-foreground disabled:opacity-60"
              >
                {instruction}
              </button>
            ))}
          </div>

          <div className="flex items-center gap-2">
            <Button size="sm" onClick={save} disabled={working}>
              {busy === 'save' ? <Dots /> : <Save aria-hidden />}
              Save
            </Button>
          </div>
        </div>
      </div>
    </div>
  );
}
