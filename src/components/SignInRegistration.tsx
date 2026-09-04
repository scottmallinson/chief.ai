import { useCallback, useEffect, useState, type FormEvent, type ReactNode } from 'react';
import { KeyRound } from 'lucide-react';

import { Button } from '@/components/ui/button';
import { Dots } from '@/components/ui/activity';
import {
  clearSignInRegistration,
  setSignInRegistration,
  signInRegistrations,
  type Registration,
} from '@/lib/registration';

interface SignInRegistrationProps {
  /** The value stored in `integration_accounts.service`. */
  service: string;
  /** What this service is called on screen. */
  name: string;
  /** Where the user makes a registration of their own, in a sentence. */
  where: ReactNode;
}

/** What to say about the id in use, given where it came from. */
function provenance(registration: Registration): string {
  switch (registration.source) {
    case 'environment':
      return 'From this machine’s environment';
    case 'stored':
      return 'Your own registration';
    case 'builtIn':
      return 'Built into this release';
    case 'missing':
      return 'None';
  }
}

/**
 * Which OAuth registration a service signs in with, and how to change it.
 *
 * **The defect REC-60 is about.** A release that shipped without a client id
 * compiled in told the user to set an environment variable, which is not a
 * thing somebody who double-clicked an installer can do — so sign-in was
 * simply over, with no way forward from inside the app. The same dead end met
 * an organisation that would rather sign in against its own OAuth app than
 * approve Chief's, and Outlook, which has no built-in registration at all
 * while Chief's Entra application is still somebody's portal work.
 *
 * So the id is a field. It is shown rather than hidden, unlike the Linear key
 * beside it: a client id grants nothing on its own, and hiding it would leave
 * the user unable to tell which registration they are signed in against.
 *
 * Folded away when there is one and it works, because the answer is then
 * "nothing to do here" and this is not what the card is for. Open, and stated
 * as the thing in the way, when there is not.
 */
export function SignInRegistration({ service, name, where }: SignInRegistrationProps) {
  const [registration, setRegistration] = useState<Registration | null>(null);
  const [showing, setShowing] = useState(false);
  const [clientId, setClientId] = useState('');
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState<string | null>(null);
  const [saved, setSaved] = useState(false);

  const take = useCallback(
    (all: Registration[]) => {
      setRegistration(all.find((one) => one.service === service) ?? null);
    },
    [service],
  );

  useEffect(() => {
    let cancelled = false;

    signInRegistrations()
      .then((all) => {
        if (!cancelled) take(all);
      })
      // A registration that cannot be read is not worth taking the settings
      // screen down for: the connect button reports the same problem, in the
      // words of the sign-in that failed.
      .catch(() => undefined);

    return () => {
      cancelled = true;
    };
  }, [take]);

  function save(event: FormEvent) {
    event.preventDefault();
    if (clientId.trim() === '' || busy) return;

    setBusy(true);
    setError(null);
    setSaved(false);

    setSignInRegistration(service, clientId.trim())
      .then((all) => {
        take(all);
        setClientId('');
        setSaved(true);
      })
      .catch((cause: unknown) => {
        setError(cause instanceof Error ? cause.message : String(cause));
      })
      .finally(() => setBusy(false));
  }

  function reset() {
    setBusy(true);
    setError(null);
    setSaved(false);

    clearSignInRegistration(service)
      .then(take)
      .catch((cause: unknown) => {
        setError(cause instanceof Error ? cause.message : String(cause));
      })
      .finally(() => setBusy(false));
  }

  if (registration === null) return null;

  const missing = registration.source === 'missing';
  const open = missing || showing;
  const fieldId = `${service}-client-id`;

  return (
    <div className="mt-4 border-t border-border pt-3.5">
      {missing ? (
        <p className="text-[13px] leading-snug text-attention-text" role="status">
          {`Chief has no ${name} client id, so signing in is not possible yet. Paste one below.`}
        </p>
      ) : (
        <div className="flex items-center justify-between gap-3">
          <p className="flex min-w-0 flex-wrap items-center gap-x-2 gap-y-1 micro text-muted-foreground">
            <span>Sign-in registration</span>
            <span aria-hidden>·</span>
            <span>{provenance(registration)}</span>
          </p>
          <Button
            variant="ghost"
            size="sm"
            aria-expanded={showing}
            onClick={() => setShowing((was) => !was)}
          >
            {showing ? 'Hide' : 'Change'}
          </Button>
        </div>
      )}

      {open && (
        <>
          {registration.clientId !== null && (
            <p className="mt-2.5 flex flex-wrap items-baseline gap-x-2 gap-y-1">
              <span className="micro text-muted-foreground">Client id</span>
              <span className="font-mono text-[13px] break-all" data-selectable>
                {registration.clientId}
              </span>
            </p>
          )}

          {registration.overriddenByEnvironment && (
            <p className="mt-2 text-[13px] leading-snug text-muted-foreground">
              An environment variable is setting this, and it wins over anything stored here.
            </p>
          )}

          <form className="mt-3 flex flex-col gap-2" onSubmit={save}>
            <label className="micro text-muted-foreground" htmlFor={fieldId}>
              {`${name} client id`}
            </label>
            <input
              id={fieldId}
              type="text"
              value={clientId}
              onChange={(event) => setClientId(event.target.value)}
              spellCheck={false}
              autoComplete="off"
              className="w-full rounded-md border border-input bg-background px-3 py-2 font-mono text-[13px]"
            />

            <p className="mt-1 text-[13px] leading-snug text-muted-foreground">{where}</p>

            {error !== null && (
              <p className="text-[13px] leading-snug text-attention-text" role="alert">
                {error}
              </p>
            )}

            {saved && (
              <p className="micro text-verified-text" role="status">
                Saved. Sign in again to use it.
              </p>
            )}

            <div className="mt-1 flex items-center gap-2">
              <Button type="submit" size="sm" disabled={clientId.trim() === '' || busy}>
                {busy ? <Dots /> : <KeyRound aria-hidden />}
                Save
              </Button>
              {registration.source === 'stored' && registration.hasBuiltIn && (
                <Button type="button" variant="outline" size="sm" disabled={busy} onClick={reset}>
                  Use the built-in one
                </Button>
              )}
            </div>
          </form>
        </>
      )}
    </div>
  );
}
