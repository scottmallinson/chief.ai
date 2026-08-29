import { useCallback, useEffect, useState } from 'react';

import { listProposals, type Proposal } from '@/lib/proposals';

interface UseProposals {
  proposals: Proposal[];
  /** Read them again — after a save, or after the daemon has drafted another. */
  reload: () => void;
  /** Take one off the screen once it has actually been dismissed. */
  forget: (id: number) => void;
}

/**
 * The drafts Chief has prepared.
 *
 * A failure leaves the list as it was rather than emptying it: these are the
 * only record on screen that Chief did anything overnight, and blanking them
 * because one read failed would read as the work having been lost.
 */
export function useProposals(): UseProposals {
  const [proposals, setProposals] = useState<Proposal[]>([]);

  const reload = useCallback(() => {
    listProposals()
      .then(setProposals)
      .catch(() => undefined);
  }, []);

  useEffect(() => reload(), [reload]);

  const forget = useCallback((id: number) => {
    setProposals((current) => current.filter((proposal) => proposal.id !== id));
  }, []);

  return { proposals, reload, forget };
}
