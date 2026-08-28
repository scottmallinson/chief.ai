import { useCallback, useEffect, useRef, useState } from 'react';
import { openUrl } from '@tauri-apps/plugin-opener';

import {
  connections,
  disconnect as forget,
  finishLogin,
  labelAccount,
  startLogin,
  type Account,
  type DeviceLogin,
} from '@/lib/integrations';

/**
 * Where sign-in stands, and nothing else. Work on an account that is already
 * connected is tracked per account rather than here: naming one is not a
 * reason to freeze the screen, and a gate that covered both disabled the
 * button the user was in the middle of clicking.
 */
type Status = 'loading' | 'idle' | 'awaiting-user' | 'working';

interface UseIntegrations {
  accounts: Account[];
  accountsFor: (service: string) => Account[];
  login: DeviceLogin | null;
  /** Which service is being connected, so only that card shows the code. */
  connecting: string | null;
  status: Status;
  /** Accounts being forgotten, so a row cannot be asked to go twice. */
  disconnecting: readonly number[];
  error: string | null;
  connect: (service: string) => void;
  /** Stop waiting on a sign-in the user has walked away from. */
  cancel: () => void;
  disconnect: (accountId: number) => void;
  /** Resolves false when the write was refused, so the field can go back. */
  rename: (accountId: number, label: string | null) => Promise<boolean>;
}

function describe(cause: unknown): string {
  return cause instanceof Error ? cause.message : String(cause);
}

/** Drive sign-in and account management from the settings screen. */
export function useIntegrations(): UseIntegrations {
  const [accounts, setAccounts] = useState<Account[]>([]);
  const [login, setLogin] = useState<DeviceLogin | null>(null);
  const [connecting, setConnecting] = useState<string | null>(null);
  const [status, setStatus] = useState<Status>('loading');
  const [disconnecting, setDisconnecting] = useState<readonly number[]>([]);
  const [error, setError] = useState<string | null>(null);

  // Which sign-in attempt is the live one. Giving up moves it on, so the poll
  // still running in Rust answers a number nobody is waiting for and is
  // dropped rather than connecting an account behind the user.
  const attempt = useRef(0);

  useEffect(() => {
    let cancelled = false;

    connections()
      .then((current) => {
        if (cancelled) return;
        setAccounts(current);
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

  const connect = useCallback((service: string) => {
    const mine = ++attempt.current;

    setError(null);
    setConnecting(service);
    setStatus('working');

    startLogin(service)
      .then(async (started) => {
        if (attempt.current !== mine) return;
        setLogin(started);
        setStatus('awaiting-user');

        // Best effort: the code is on screen either way.
        await openUrl(started.verificationUri).catch(() => undefined);

        const current = await finishLogin(service);
        if (attempt.current !== mine) return;
        setAccounts(current);
        setLogin(null);
        setConnecting(null);
        setStatus('idle');
      })
      .catch((cause: unknown) => {
        if (attempt.current !== mine) return;
        setError(describe(cause));
        setLogin(null);
        setConnecting(null);
        setStatus('idle');
      });
  }, []);

  const cancel = useCallback(() => {
    // GitHub's device code lives for fifteen minutes and the poll in Rust runs
    // until it expires. There is nothing to call off — the answer is simply no
    // longer wanted — so the waiting ends here, and at once.
    attempt.current += 1;
    setLogin(null);
    setConnecting(null);
    setError(null);
    setStatus('idle');
  }, []);

  const disconnect = useCallback((accountId: number) => {
    setError(null);
    setDisconnecting((current) => [...current, accountId]);

    forget(accountId)
      .then((current) => setAccounts(current))
      .catch((cause: unknown) => setError(describe(cause)))
      .finally(() => setDisconnecting((current) => current.filter((id) => id !== accountId)));
  }, []);

  const rename = useCallback((accountId: number, label: string | null): Promise<boolean> => {
    setError(null);

    // Deliberately ungated: a name is one column, the command answers with the
    // authoritative list, and a rename landing beside a disconnect is settled
    // by whichever answers last. Nothing has to wait on it, so nothing does.
    // Whether it landed is still answered, because the field it came from has
    // to put the stored name back if it did not.
    return labelAccount(accountId, label).then(
      (current) => {
        setAccounts(current);
        return true;
      },
      (cause: unknown) => {
        setError(describe(cause));
        return false;
      },
    );
  }, []);

  const accountsFor = useCallback(
    (service: string) => accounts.filter((account) => account.service === service),
    [accounts],
  );

  return {
    accounts,
    accountsFor,
    login,
    connecting,
    status,
    disconnecting,
    error,
    connect,
    cancel,
    disconnect,
    rename,
  };
}
