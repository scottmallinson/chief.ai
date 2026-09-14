import { useState, type FormEvent } from 'react';
import { KeyRound } from 'lucide-react';

import { Button } from '@/components/ui/button';
import { Dots } from '@/components/ui/activity';
import { addAtlassianToken } from '@/lib/integrations';

interface AtlassianTokenProps {
  /**
   * Refresh the card this sits in.
   *
   * A token connection lands in `integration_accounts` under the same service
   * as a browser sign-in, so the card above already lists it — this component
   * renders no accounts of its own, it just says when there is a new one.
   */
  onChanged: () => void;
}

/**
 * Jira and Confluence with a token the user pastes.
 *
 * **The other way in.** Signing in through Atlassian's MCP server needs
 * nothing but a browser click, and an administrator can switch that server
 * off — Rovo is a paid add-on, and a network that rewrites TLS on
 * `*.atlassian.com` takes it away just as completely. When that has happened
 * no amount of retrying helps, and this is the way through.
 *
 * Folded away, because it is the second answer to a question the button above
 * usually answers. Pasting a credential is strictly worse than a sign-in that
 * mints one: the token cannot be scoped, cannot be narrowed to read-only, and
 * is revoked from Atlassian rather than from here.
 *
 * Three fields rather than one, because Atlassian's Basic auth is
 * `email:token` and the site is the user's own — a token is worthless without
 * knowing which Jira to send it to. The site takes a bare name, so `acme` is
 * enough.
 */
export function AtlassianToken({ onChanged }: AtlassianTokenProps) {
  const [showing, setShowing] = useState(false);
  const [site, setSite] = useState('');
  const [email, setEmail] = useState('');
  const [token, setToken] = useState('');
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState<string | null>(null);
  const [connected, setConnected] = useState<string | null>(null);

  const ready = site.trim() !== '' && email.trim() !== '' && token.trim() !== '';

  function connect(event: FormEvent) {
    event.preventDefault();
    if (!ready || busy) return;

    setBusy(true);
    setError(null);
    setConnected(null);

    addAtlassianToken(site.trim(), email.trim(), token.trim())
      .then((account) => {
        setConnected(account.accountKey);
        // The token is cleared and the site is not: a second account on the
        // same site is a different person, and retyping the site to add one
        // would be asking for something Chief already knows.
        setToken('');
        onChanged();
      })
      .catch((cause: unknown) => {
        setError(cause instanceof Error ? cause.message : String(cause));
      })
      .finally(() => setBusy(false));
  }

  return (
    <div className="mt-4 border-t border-border pt-3.5">
      <div className="flex items-center justify-between gap-3">
        <p className="flex min-w-0 flex-wrap items-center gap-x-2 gap-y-1 micro text-muted-foreground">
          <span>Another way in</span>
          <span aria-hidden>·</span>
          <span>API token</span>
        </p>
        <Button
          variant="ghost"
          size="sm"
          aria-expanded={showing}
          onClick={() => setShowing((was) => !was)}
        >
          {showing ? 'Hide' : 'Use a token'}
        </Button>
      </div>

      {showing && (
        <>
          <p className="mt-2.5 text-[13px] leading-snug text-muted-foreground">
            Signing in above goes through Atlassian&rsquo;s MCP server, which an administrator can
            switch off — it is part of Rovo. If that sign-in will not complete, an API token reaches
            Jira and Confluence directly instead.
          </p>

          <form className="mt-3 flex flex-col gap-2" onSubmit={connect}>
            <label className="micro text-muted-foreground" htmlFor="atlassian-site">
              Your Atlassian site
            </label>
            <input
              id="atlassian-site"
              type="text"
              value={site}
              onChange={(event) => setSite(event.target.value)}
              placeholder="acme"
              spellCheck={false}
              autoComplete="off"
              className="w-full rounded-md border border-input bg-background px-3 py-2 font-mono text-[13px]"
            />

            <label className="mt-1 micro text-muted-foreground" htmlFor="atlassian-email">
              The email the token belongs to
            </label>
            <input
              id="atlassian-email"
              type="email"
              value={email}
              onChange={(event) => setEmail(event.target.value)}
              placeholder="you@example.com"
              spellCheck={false}
              autoComplete="off"
              className="w-full rounded-md border border-input bg-background px-3 py-2 font-mono text-[13px]"
            />

            <label className="mt-1 micro text-muted-foreground" htmlFor="atlassian-token">
              API token
            </label>
            <input
              id="atlassian-token"
              type="password"
              value={token}
              onChange={(event) => setToken(event.target.value)}
              spellCheck={false}
              autoComplete="off"
              className="w-full rounded-md border border-input bg-background px-3 py-2 font-mono text-[13px]"
            />

            <p className="mt-1 text-[13px] leading-snug text-muted-foreground">
              Make an <strong>API token</strong> at{' '}
              <span className="font-mono text-[12px]">id.atlassian.com</span> under Security → API
              tokens. Atlassian issues no read-only kind, so it carries{' '}
              <strong>everything you can already see</strong> in Jira and Confluence — Chief only
              ever reads with it. It is stored on this machine and never shown again. The site can
              be just your organisation&rsquo;s name.
            </p>

            {error !== null && (
              <p className="text-[13px] leading-snug text-attention-text" role="alert">
                {error}
              </p>
            )}

            {busy && (
              <p className="flex items-center gap-2 micro text-muted-foreground" role="status">
                <Dots />
                Checking that token
              </p>
            )}

            {connected !== null && (
              <p className="micro text-verified-text" role="status">
                {`Connected to ${connected}`}
              </p>
            )}

            <div className="mt-1">
              <Button type="submit" size="sm" disabled={!ready || busy}>
                <KeyRound aria-hidden />
                Connect with a token
              </Button>
            </div>
          </form>
        </>
      )}
    </div>
  );
}
