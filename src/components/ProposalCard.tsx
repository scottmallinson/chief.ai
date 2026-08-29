import { useState } from 'react';
import { PencilLine, X } from 'lucide-react';

import { Button } from '@/components/ui/button';
import { Chip } from '@/components/ui/chip';
import { Dots } from '@/components/ui/activity';
import { dismissProposal, type Proposal } from '@/lib/proposals';

interface ProposalCardProps {
  proposal: Proposal;
  /** Open this draft in the drawer, loaded and ready to change. */
  onEdit: (proposal: Proposal) => void;
  onDismissed: (id: number) => void;
}

/**
 * The draft itself, without the heading Chief writes at the top of the file.
 *
 * That heading says what the draft is and that nothing was sent — which the
 * card also says, in the card's own words. Showing both would read as Chief
 * repeating itself, and the file needs it because a file has no card around it.
 */
function bodyOf(markdown: string): string {
  return markdown
    .split('\n')
    .filter((line) => !line.startsWith('#'))
    .join('\n')
    .replace(/^Chief drafted this[^\n]*\n?/m, '')
    .replace(/^https?:\/\/\S+\n?/m, '')
    .trim();
}

/**
 * One thing Chief noticed and drafted for, waiting to be reviewed.
 *
 * The chip says **Not sent** rather than a status, because that is the single
 * fact a reader needs about anything on this screen: drafting and sending are
 * separate steps, and until the second one exists nothing here has left the
 * machine. Amber would be wrong — nothing is required of the user — so it is
 * the quiet tone.
 */
export function ProposalCard({ proposal, onEdit, onDismissed }: ProposalCardProps) {
  const [dismissing, setDismissing] = useState(false);
  const [error, setError] = useState<string | null>(null);

  function dismiss() {
    setDismissing(true);
    setError(null);

    dismissProposal(proposal.id)
      .then(() => onDismissed(proposal.id))
      .catch((cause: unknown) => {
        // The card stays. A draft that vanished without being dismissed would
        // be indistinguishable from one that was.
        setError(cause instanceof Error ? cause.message : String(cause));
        setDismissing(false);
      });
  }

  return (
    <li className="rounded-lg border border-border bg-card p-4">
      <div className="flex items-start justify-between gap-4">
        <h3 className="min-w-0 flex-1 text-[15px] font-semibold tracking-[-0.015em]">
          {proposal.title}
        </h3>
        <Chip tone="quiet">Not sent</Chip>
      </div>

      <p className="mt-2.5 text-sm leading-relaxed whitespace-pre-wrap" data-selectable>
        {bodyOf(proposal.body)}
      </p>

      <p className="mt-2.5 font-mono text-[11px] break-all text-muted-foreground" data-selectable>
        {proposal.path}
      </p>

      {error !== null && (
        <p className="mt-2.5 text-[13px] leading-snug text-attention-text" role="alert">
          {error}
        </p>
      )}

      <div className="mt-3.5 flex flex-wrap gap-2">
        <Button size="sm" variant="outline" onClick={() => onEdit(proposal)}>
          <PencilLine aria-hidden />
          Edit
        </Button>
        <Button size="sm" variant="ghost" onClick={dismiss} disabled={dismissing}>
          {dismissing ? <Dots /> : <X aria-hidden />}
          Dismiss
        </Button>
      </div>
    </li>
  );
}
