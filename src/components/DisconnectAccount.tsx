import { useState } from 'react';

import { Button } from '@/components/ui/button';
import { Dots } from '@/components/ui/activity';
import { accountData, type AccountData } from '@/lib/integrations';

interface DisconnectAccountProps {
  accountId: number;
  /** What this account is called, for the button's accessible name. */
  name: string;
  /** The word for removing it: providers call it different things. */
  verb: string;
  /** This account is being forgotten. Nothing else on the screen cares. */
  leaving: boolean;
  onConfirm: (accountId: number) => void;
}

/** What is about to go, in the fewest words that are still specific. */
function inventory(data: AccountData): string {
  const parts: string[] = [];

  if (data.entries > 0) {
    parts.push(`${data.entries} work log ${data.entries === 1 ? 'entry' : 'entries'}`);
  }

  if (data.proposals > 0) {
    parts.push(`${data.proposals} ${data.proposals === 1 ? 'draft' : 'drafts'}`);
  }

  if (parts.length === 0) return 'This account has left nothing on this machine.';

  return `${parts.join(' and ')} will be deleted from this machine.`;
}

/**
 * Unlinking an account, with what it costs stated first.
 *
 * Disconnecting deletes the account's work log entries, its drafts and their
 * markdown along with the credential — because that is what the word means to
 * somebody reading it, in an app whose one promise is about where their data
 * lives. It is also irreversible and removes something they may not know is
 * there, so it is confirmed, with counts, and the counts are read from the
 * database rather than guessed at.
 *
 * The confirmation is the row rather than a dialog. Nothing here is modal:
 * "Instrument, not poster", and a question about one account has no business
 * taking the screen.
 */
export function DisconnectAccount({
  accountId,
  name,
  verb,
  leaving,
  onConfirm,
}: DisconnectAccountProps) {
  const [asking, setAsking] = useState<AccountData | null>(null);
  const [counting, setCounting] = useState(false);

  function ask() {
    setCounting(true);

    accountData(accountId)
      // A count Chief could not read is not a reason to refuse to disconnect,
      // and it is not a reason to claim there is nothing there either — so the
      // confirmation still appears, saying only what is certain.
      .then((data) => setAsking(data))
      .catch(() => setAsking({ entries: 0, proposals: 0 }))
      .finally(() => setCounting(false));
  }

  if (asking !== null) {
    return (
      <div className="flex shrink-0 flex-col items-end gap-1.5" role="group" aria-label={name}>
        <p className="text-right text-[13px] leading-snug text-muted-foreground">
          {inventory(asking)}
        </p>
        <div className="flex items-center gap-2">
          <Button variant="outline" size="sm" onClick={() => setAsking(null)}>
            Cancel
          </Button>
          <Button
            variant="outline"
            size="sm"
            disabled={leaving}
            // Named for the account, so it is unambiguous read aloud and
            // cannot be confused with the button that opened it — which
            // carries the provider's own word and the same name.
            aria-label={`Delete ${name} and everything it stored`}
            onClick={() => {
              setAsking(null);
              onConfirm(accountId);
            }}
          >
            <span className="text-destructive-text">Delete</span>
          </Button>
        </div>
      </div>
    );
  }

  return (
    <Button variant="outline" size="sm" disabled={leaving || counting} onClick={ask}>
      {counting && <Dots />}
      {/* Bounded and truncated, so a long name ends in an ellipsis rather
          than pushing the field that names it down to nothing. The button
          keeps its full accessible name either way. */}
      <span className="max-w-[9rem] truncate">
        {verb} {name}
      </span>
    </Button>
  );
}
