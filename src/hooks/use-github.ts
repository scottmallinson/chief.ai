import { useCallback, useEffect, useState } from 'react';
import { openUrl } from '@tauri-apps/plugin-opener';

import {
  disconnectGithub,
  finishGithubLogin,
  githubConnection,
  startGithubLogin,
  type Connection,
  type DeviceLogin,
} from '@/lib/integrations';

type Status = 'loading' | 'idle' | 'awaiting-user' | 'working';

interface UseGithub {
  connection: Connection | null;
  login: DeviceLogin | null;
  status: Status;
  error: string | null;
  connect: () => void;
  disconnect: () => void;
}

function describe(cause: unknown): string {
  return cause instanceof Error ? cause.message : String(cause);
}

/** Drive the GitHub device-flow sign-in from the settings screen. */
export function useGithub(): UseGithub {
  const [connection, setConnection] = useState<Connection | null>(null);
  const [login, setLogin] = useState<DeviceLogin | null>(null);
  const [status, setStatus] = useState<Status>('loading');
  const [error, setError] = useState<string | null>(null);

  useEffect(() => {
    let cancelled = false;

    githubConnection()
      .then((current) => {
        if (cancelled) return;
        setConnection(current);
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

    startGithubLogin()
      .then(async (started) => {
        setLogin(started);
        setStatus('awaiting-user');

        // Best effort: the code is on screen either way.
        await openUrl(started.verificationUri).catch(() => undefined);

        const current = await finishGithubLogin();
        setConnection(current);
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
    setError(null);
    setStatus('working');

    disconnectGithub()
      .then((current) => {
        setConnection(current);
        setStatus('idle');
      })
      .catch((cause: unknown) => {
        setError(describe(cause));
        setStatus('idle');
      });
  }, []);

  return { connection, login, status, error, connect, disconnect };
}
