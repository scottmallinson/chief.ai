import { useCallback, useEffect, useRef, useState } from 'react';
import { openUrl } from '@tauri-apps/plugin-opener';

import {
  accountName,
  connections,
  disconnect as forget,
  finishLogin,
  labelAccount,
  startLogin,
  type Account,
  type Login,
} from '@/lib/integrations';

/**
 * Where sign-in stands, and nothing else. Work on an account that is already
 * connected is tracked per account rather than here: naming one is not a
 * reason to freeze the screen, and a gate that covered both disabled the
 * button the user was in the middle of clicking.
 */
type Status = 'loading' | 'idle' | 'awaiting-user' | 'working';

/**
 * Why a sign-in was started, which decides what its outcome means.
 *
 * Coming back as the account already connected is the ordinary, wanted result
 * of `again` — the user pressed Reconnect on that very account. It is the
 * *failure* of `another`: they asked for a second account and got the first
 * one back, because the device flow authorises whoever the browser is signed
 * in as. Same outcome, opposite meanings, so the intent has to be carried in.
 */
export type Intent = 'first' | 'another' | 'again';

/**
 * Everything the settings screen needs to show and change the connections.
 *
 * Exported because the screen reads it **once** and hands it to each card:
 * every card calling the hook itself meant one read of the table per card and
 * one copy of the answer per card, which disagreed after an action until
 * whichever card owned the change reloaded (REC-42).
 */
/**
 * Something to say, and whose card should say it.
 *
 * `service` is `null` when it belongs to the screen rather than to one card —
 * the initial read of the connections failing, which is nobody's card in
 * particular and everybody's problem.
 */
interface Owned {
  service: string | null;
  message: string;
}

export interface UseIntegrations {
  accounts: Account[];
  accountsFor: (service: string) => Account[];
  login: Login | null;
  /** Which service is being connected, so only that card shows the code. */
  connecting: string | null;
  status: Status;
  /** Accounts being forgotten, so a row cannot be asked to go twice. */
  disconnecting: readonly number[];
  /**
   * What went wrong on this service's card, if anything.
   *
   * **Asked per service, not read flat.** When each card had its own hook, an
   * error raised by one was visible only on that one, because the others knew
   * nothing about it. One shared hook makes a flat `error` show on every card
   * at once — which is how REC-42's refactor first announced itself, as three
   * `role="alert"` nodes where a test wanted one. So every error is stored
   * with the card it belongs to, and a card asks for its own.
   */
  errorFor: (service: string) => string | null;
  /**
   * Something that happened and is worth saying, but is not a failure — a
   * sign-in that reconnected the account already there rather than adding one.
   *
   * Per service for the same reason as [`errorFor`].
   */
  noticeFor: (service: string) => string | null;
  connect: (service: string, intent?: Intent) => void;
  /** Stop waiting on a sign-in the user has walked away from. */
  cancel: () => void;
  disconnect: (accountId: number) => void;
  /** Resolves false when the write was refused, so the field can go back. */
  rename: (accountId: number, label: string | null) => Promise<boolean>;
  /** Read the connections again — after something added one outside this hook. */
  reload: () => void;
}

function describe(cause: unknown): string {
  return cause instanceof Error ? cause.message : String(cause);
}

/** Drive sign-in and account management from the settings screen. */
export function useIntegrations(): UseIntegrations {
  const [accounts, setAccounts] = useState<Account[]>([]);
  const [login, setLogin] = useState<Login | null>(null);
  const [connecting, setConnecting] = useState<string | null>(null);
  const [status, setStatus] = useState<Status>('loading');
  const [disconnecting, setDisconnecting] = useState<readonly number[]>([]);
  const [error, setError] = useState<Owned | null>(null);
  const [notice, setNotice] = useState<Owned | null>(null);

  // The accounts as they now stand, readable from a callback that does not
  // depend on them. `disconnect` and `rename` take an account id and have to
  // say which card their failure belongs to, and neither is worth rebuilding
  // on every change to the list.
  const known = useRef<Account[]>([]);

  // Which sign-in attempt is the live one. Giving up moves it on, so the poll
  // still running in Rust answers a number nobody is waiting for and is
  // dropped rather than connecting an account behind the user.
  const attempt = useRef(0);

  useEffect(() => {
    known.current = accounts;
  }, [accounts]);

  /** Whose card an account's failure belongs to. */
  const serviceOf = useCallback(
    (accountId: number) =>
      known.current.find((account) => account.id === accountId)?.service ?? null,
    [],
  );

  const reload = useCallback(() => {
    connections()
      .then(setAccounts)
      .catch((cause: unknown) => setError({ service: null, message: describe(cause) }));
  }, []);

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
        // No service: the screen could not read the connections at all, which
        // is every card's problem rather than one card's.
        setError({ service: null, message: describe(cause) });
        setStatus('idle');
      });

    return () => {
      cancelled = true;
    };
  }, []);

  const connect = useCallback((service: string, intent: Intent = 'first') => {
    const mine = ++attempt.current;

    setError(null);
    setNotice(null);
    setConnecting(service);
    setStatus('working');

    startLogin(service)
      .then(async (started) => {
        if (attempt.current !== mine) return;
        setLogin(started);
        setStatus('awaiting-user');

        // Best effort for a device code, which is on screen either way. For a
        // browser sign-in it is the whole flow — but the URL is also shown, so
        // a blocked opener leaves the user something to click rather than a
        // dead dialog.
        //
        // The device code opens the page that already has the code in it, so
        // the flow is click, authorise, done rather than click, read a code,
        // type a code, authorise, done. `??` rather than a truthiness check
        // and rather than nothing at all: the field is built by Rust from a
        // prefill GitHub does not document, and an older backend or a removed
        // prefill has to leave the plain page working.
        const destination =
          started.kind === 'device'
            ? (started.verificationUriComplete ?? started.verificationUri)
            : started.url;
        await openUrl(destination).catch(() => undefined);

        const finished = await finishLogin(service);
        if (attempt.current !== mine) return;
        setAccounts(finished.accounts);
        // Asked for a second account and given back the first one. Nothing
        // failed — the sign-in worked — so this is not an error, but saying
        // nothing leaves an unchanged list as the only report, and the button
        // reads as broken. Naming who signed in is the whole message: it is
        // what tells the user their browser is the thing to change.
        if (intent === 'another' && finished.reconnected) {
          setNotice({
            service,
            message: `That signed in as ${accountName(finished.account)}, which was already connected, so no account was added. Sign-in uses whoever your browser is signed in to — sign out there, or use a private window, then try again.`,
          });
        }
        setLogin(null);
        setConnecting(null);
        setStatus('idle');
      })
      .catch((cause: unknown) => {
        if (attempt.current !== mine) return;
        // Named here rather than read off `connecting`, which is cleared on
        // the very next line: by the time anything renders it is null, and the
        // error would belong to nobody.
        setError({ service, message: describe(cause) });
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
    setNotice(null);
    setStatus('idle');
  }, []);

  const disconnect = useCallback(
    (accountId: number) => {
      setError(null);
      setDisconnecting((current) => [...current, accountId]);

      forget(accountId)
        .then((current) => setAccounts(current))
        .catch((cause: unknown) =>
          setError({ service: serviceOf(accountId), message: describe(cause) }),
        )
        .finally(() => setDisconnecting((current) => current.filter((id) => id !== accountId)));
    },
    [serviceOf],
  );

  const rename = useCallback(
    (accountId: number, label: string | null): Promise<boolean> => {
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
          setError({ service: serviceOf(accountId), message: describe(cause) });
          return false;
        },
      );
    },
    [serviceOf],
  );

  const accountsFor = useCallback(
    (service: string) => accounts.filter((account) => account.service === service),
    [accounts],
  );

  /**
   * Shown on this card when it is this card's, and on every card when it
   * belongs to none of them — which is what each card did for itself when
   * each card had its own failing read.
   */
  const mine = (owned: Owned | null, service: string) =>
    owned !== null && (owned.service === null || owned.service === service) ? owned.message : null;

  const errorFor = useCallback((service: string) => mine(error, service), [error]);
  const noticeFor = useCallback((service: string) => mine(notice, service), [notice]);

  return {
    accounts,
    accountsFor,
    reload,
    login,
    connecting,
    status,
    disconnecting,
    errorFor,
    noticeFor,
    connect,
    cancel,
    disconnect,
    rename,
  };
}
