import { useState, type FormEvent } from 'react';
import { KeyRound, X } from 'lucide-react';

import { Freshness } from '@/components/Freshness';
import { useSyncState } from '@/hooks/use-sync-state';
import { Button } from '@/components/ui/button';
import { Dots } from '@/components/ui/activity';
import { addLinearKey, disconnect, type Account } from '@/lib/integrations';

interface LinearKeyProps {
  accounts: Account[];
  onChanged: () => void;
}

/**
 * Linear, connected with a key the user pastes.
 *
 * No sign-in flow, because Linear issues personal API keys and Chief only ever
 * reads what is assigned to one person. The interface is therefore a field and
 * a button rather than a browser round trip.
 *
 * Two things this screen has to say out loud. The key is **never shown again**
 * once stored — it is a bearer credential, and a settings screen that prints it
 * hands somebody's Linear account to whoever is looking at the laptop. And a
 * personal key carries **read and write** access, because Linear issues no
 * read-only kind; leaving that implied would be the same omission as not saying
 * what GitHub's `repo` scope grants.
 */
export function LinearKey({ accounts, onChanged }: LinearKeyProps) {
  const { states } = useSyncState();
  const [key, setKey] = useState('');
  const [busy, setBusy] = useState<number | 'adding' | null>(null);
  const [error, setError] = useState<string | null>(null);
  const [connected, setConnected] = useState<string | null>(null);

  function connect(event: FormEvent) {
    event.preventDefault();
    if (key.trim() === '' || busy !== null) return;

    setBusy('adding');
    setError(null);
    setConnected(null);

    addLinearKey(key.trim())
      .then((account) => {
        setConnected(account.identity ?? 'that workspace');
        setKey('');
        onChanged();
      })
      .catch((cause: unknown) => {
        setError(cause instanceof Error ? cause.message : String(cause));
      })
      .finally(() => setBusy(null));
  }

  function forget(account: Account) {
    setBusy(account.id);
    setError(null);

    disconnect(account.id)
      .then(() => onChanged())
      .catch((cause: unknown) => {
        setError(cause instanceof Error ? cause.message : String(cause));
      })
      .finally(() => setBusy(null));
  }

  return (
    <>
      {accounts.length > 0 && (
        <ul className="mt-3.5 flex flex-col gap-2">
          {accounts.map((account) => {
            const name = account.label ?? account.identity ?? account.accountKey;

            return (
              <li
                key={account.id}
                className="flex items-center justify-between gap-3 rounded-md border border-border px-3 py-2"
              >
                <span className="flex min-w-0 flex-1 flex-col gap-1">
                  <span className="truncate text-sm">{name}</span>
                  {/* The card's own form below is the way back in, so no
                      reconnect button here: the key or the address is
                      re-entered where it was entered. */}
                  <Freshness state={states.get(account.id)} />
                </span>
                <Button
                  size="sm"
                  variant="ghost"
                  onClick={() => forget(account)}
                  disabled={busy !== null}
                  aria-label={`Disconnect ${name}`}
                >
                  {busy === account.id ? <Dots /> : <X aria-hidden />}
                  Disconnect
                </Button>
              </li>
            );
          })}
        </ul>
      )}

      <form className="mt-3.5 flex flex-col gap-2" onSubmit={connect}>
        <label className="micro text-muted-foreground" htmlFor="linear-key">
          Linear API key
        </label>
        <input
          id="linear-key"
          type="password"
          value={key}
          onChange={(event) => setKey(event.target.value)}
          placeholder="lin_api_…"
          spellCheck={false}
          autoComplete="off"
          className="w-full rounded-md border border-input bg-background px-3 py-2 font-mono text-[13px]"
        />

        <p className="mt-1 text-[13px] leading-snug text-muted-foreground">
          Make a <strong>personal API key</strong> in Linear under Settings → Security &amp; access.
          Linear issues no read-only kind, so it carries <strong>read and write</strong> access to
          everything you can see — Chief only ever reads with it, and stores it the way it stores a
          token. It is never shown again.
        </p>

        {error !== null && (
          <p className="text-[13px] leading-snug text-attention-text" role="alert">
            {error}
          </p>
        )}

        {busy === 'adding' && (
          <p className="flex items-center gap-2 micro text-verified-text" role="status">
            <Dots />
            Checking that key
          </p>
        )}

        {connected !== null && (
          <p className="micro text-verified-text" role="status">
            {`Connected as ${connected}`}
          </p>
        )}

        <div className="mt-1 flex">
          <Button type="submit" size="sm" disabled={key.trim() === '' || busy !== null}>
            <KeyRound aria-hidden />
            Connect
          </Button>
        </div>
      </form>
    </>
  );
}
