import { useCallback, useEffect, useState } from 'react';
import { openUrl } from '@tauri-apps/plugin-opener';

import {
  connections,
  disconnectAccount,
  finishLogin,
  GITHUB,
  startLogin,
  type Account,
  type DeviceLogin,
} from '@/lib/integrations';

type Status = 'loading' | 'idle' | 'awaiting-user' | 'working';

interface UseGithub {
  /** The connected GitHub account, or null when there is none. */
  account: Account | null;
  login: DeviceLogin | null;
  status: Status;
  error: string | null;
  connect: () => void;
  disconnect: () => void;
}

function describe(cause: unknown): string {
  return cause instanceof Error ? cause.message : String(cause);
}

/**
 * The backend answers with every account it holds. Settings shows one GitHub
 * connection today, so this picks the first one out rather than making the
 * screen understand a list it has nowhere to put yet.
 */
function github(accounts: Account[]): Account | null {
  return accounts.find((account) => account.service === GITHUB) ?? null;
}

/** Drive the GitHub device-flow sign-in from the settings screen. */
export function useGithub(): UseGithub {
  const [account, setAccount] = useState<Account | null>(null);
  const [login, setLogin] = useState<DeviceLogin | null>(null);
  const [status, setStatus] = useState<Status>('loading');
  const [error, setError] = useState<string | null>(null);

  useEffect(() => {
    let cancelled = false;

    connections()
      .then((accounts) => {
        if (cancelled) return;
        setAccount(github(accounts));
        setStatus('idle');
      })
      .catch((cause: unknown) => {
        if (cancelled) return;
        setError(describe(cause));
        setStatus('idle');
      });

    return () => {
      cancelled = true;
    };
  }, []);

  const connect = useCallback(() => {
    setError(null);
    setStatus('working');

    startLogin(GITHUB)
      .then(async (started) => {
        setLogin(started);
        setStatus('awaiting-user');

        // Best effort: the code is on screen either way.
        await openUrl(started.verificationUri).catch(() => undefined);

        setAccount(github(await finishLogin(GITHUB)));
        setLogin(null);
        setStatus('idle');
      })
      .catch((cause: unknown) => {
        setError(describe(cause));
        setLogin(null);
        setStatus('idle');
      });
  }, []);

  const disconnect = useCallback(() => {
    // Nothing to forget, and the command is keyed on an account id we would
    // not have.
    if (account === null) return;

    setError(null);
    setStatus('working');

    disconnectAccount(account.id)
      .then((accounts) => {
        setAccount(github(accounts));
        setStatus('idle');
      })
      .catch((cause: unknown) => {
        setError(describe(cause));
        setStatus('idle');
      });
  }, [account]);

  return { account, login, status, error, connect, disconnect };
}
