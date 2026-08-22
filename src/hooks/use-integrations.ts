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

type Status = 'loading' | 'idle' | 'awaiting-user' | 'working';

interface UseIntegrations {
  accounts: Account[];
  accountsFor: (service: string) => Account[];
  login: DeviceLogin | null;
  /** Which service is being connected, so only that card shows the code. */
  connecting: string | null;
  status: Status;
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

  const settle = useCallback((work: Promise<Account[]>) => {
    setError(null);
    setStatus('working');

    work
      .then((current) => {
        setAccounts(current);
        setStatus('idle');
      })
      .catch((cause: unknown) => {
        setError(describe(cause));
        setStatus('idle');
      });
  }, []);

  const disconnect = useCallback((accountId: number) => settle(forget(accountId)), [settle]);

  const rename = useCallback(
    (accountId: number, label: string | null) => settle(labelAccount(accountId, label)),
    [settle],
  );

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
    error,
    connect,
    disconnect,
    rename,
  };
}
