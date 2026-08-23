import { useCallback, useEffect, useState } from 'react';
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
  disconnect: (accountId: number) => void;
  rename: (accountId: number, label: string | null) => void;
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
    setError(null);
    setConnecting(service);
    setStatus('working');

    startLogin(service)
      .then(async (started) => {
        setLogin(started);
        setStatus('awaiting-user');

        // Best effort: the code is on screen either way.
        await openUrl(started.verificationUri).catch(() => undefined);

        const current = await finishLogin(service);
        setAccounts(current);
        setLogin(null);
        setConnecting(null);
        setStatus('idle');
      })
      .catch((cause: unknown) => {
        setError(describe(cause));
        setLogin(null);
        setConnecting(null);
        setStatus('idle');
      });
  }, []);

  const disconnect = useCallback((accountId: number) => {
    setError(null);
    setDisconnecting((current) => [...current, accountId]);

    forget(accountId)
      .then((current) => setAccounts(current))
      .catch((cause: unknown) => setError(describe(cause)))
      .finally(() => setDisconnecting((current) => current.filter((id) => id !== accountId)));
  }, []);

  const rename = useCallback((accountId: number, label: string | null) => {
    setError(null);

    // Deliberately ungated: a name is one column, the command answers with the
    // authoritative list, and a rename landing beside a disconnect is settled
    // by whichever answers last. Nothing has to wait on it, so nothing does.
    labelAccount(accountId, label)
      .then((current) => setAccounts(current))
      .catch((cause: unknown) => setError(describe(cause)));
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
    disconnect,
    rename,
  };
}
