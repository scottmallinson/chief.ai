import { Button } from '@/components/ui/button';
import { Chip } from '@/components/ui/chip';
import { describe, type SyncState } from '@/lib/sync-state';

interface FreshnessProps {
  /** Undefined when this account has never been read. */
  state: SyncState | undefined;
  /**
   * How to sign in again, for a credential Chief can no longer use. Left out
   * where the card's own form is the way back — a pasted key or a calendar
   * address is re-entered where it was entered.
   */
  onReconnect?: (() => void) | undefined;
}

/**
 * When an account was last read, and whether it still works.
 *
 * **This is what makes D9 honest.** Once a read question is answered from
 * `work_logs` rather than from the service, the answer is only as good as the
 * last sync — and staleness is invisible in a way a spinner is not, so it has
 * to be said out loud.
 *
 * A timestamp is a machine fact and takes the mono face, never the prose one.
 *
 * **Amber only for `authRequired`.** The design system reserves it for *you are
 * needed*, and only a revoked credential is that: a host that could not be
 * reached is Chief's problem and will be tried again, so it reads as a quiet
 * line rather than a chip. Making both amber is how this becomes an alarm every
 * time somebody closes their laptop, at which point nobody reads either.
 */
export function Freshness({ state, onReconnect }: FreshnessProps) {
  const words = describe(state);

  if (state?.status === 'authRequired') {
    return (
      <span className="inline-flex items-center gap-2">
        <Chip tone="attention" data-testid="freshness">
          {words}
        </Chip>
        {onReconnect !== undefined && (
          <Button variant="outline" size="sm" onClick={onReconnect}>
            Reconnect
          </Button>
        )}
      </span>
    );
  }

  return (
    <span className="micro whitespace-nowrap text-muted-foreground" data-testid="freshness">
      {words}
    </span>
  );
}
