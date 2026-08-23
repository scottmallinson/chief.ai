import type { ReactNode } from 'react';
import { Github } from 'lucide-react';

import { Button } from '@/components/ui/button';
import { Chip } from '@/components/ui/chip';
import { Dots } from '@/components/ui/activity';
import { useElapsed } from '@/hooks/use-elapsed';
import { useIntegrations } from '@/hooks/use-integrations';
import { accountName, GITHUB, type Account } from '@/lib/integrations';

interface SettingsSectionProps {
  title: string;
  description: string;
  /** The chip on the right: state, never an action. */
  state: ReactNode;
  children?: ReactNode;
}

function SettingsSection({ title, description, state, children }: SettingsSectionProps) {
  return (
    <section className="rounded-lg border border-border bg-card p-4">
      <div className="flex items-start justify-between gap-4">
        <div className="min-w-0 flex-1">
          <h2 className="text-base font-semibold tracking-[-0.015em]">{title}</h2>
          <p className="mt-1 text-sm leading-relaxed text-muted-foreground">{description}</p>
        </div>
        <span className="shrink-0">{state}</span>
      </div>
      {children}
    </section>
  );
}

const connectedFormat = new Intl.DateTimeFormat(undefined, { dateStyle: 'medium' });

/** m:ss. A code with fifteen minutes on it reads as a clock, not a number. */
function countdown(seconds: number): string {
  return `${Math.floor(seconds / 60)}:${String(seconds % 60).padStart(2, '0')}`;
}

/** One connected account: what it is called, since when, and how to remove it. */
function ConnectedAccount({
  account,
  onRename,
  onDisconnect,
  leaving,
}: {
  account: Account;
  onRename: (accountId: number, label: string | null) => void;
  onDisconnect: (accountId: number) => void;
  /** This account is being forgotten. Nothing else on the screen cares. */
  leaving: boolean;
}) {
  const since = new Date(account.connectedAt);
  const identity = account.identity ?? account.accountKey;

  // Committed on blur rather than per keystroke: naming an account is not
  // worth a database write per character.
  const commit = (value: string) => {
    const label = value.trim() === '' ? null : value.trim();

    if (label !== account.label) {
      onRename(account.id, label);
    }
  };

  return (
    <div className="flex items-center justify-between gap-4 border-t border-border py-3">
      <div className="min-w-0 flex-1">
        <input
          type="text"
          aria-label={`Name for ${identity}`}
          defaultValue={account.label ?? ''}
          placeholder={identity}
          onBlur={(event) => commit(event.target.value)}
          className="w-full rounded-md bg-transparent text-sm font-medium placeholder:font-normal placeholder:text-muted-foreground"
        />
        <p className="mt-0.5 micro text-muted-foreground">
          {Number.isNaN(since.getTime())
            ? 'Connected'
            : `Connected ${connectedFormat.format(since)}`}
        </p>
      </div>
      <Button
        variant="outline"
        size="sm"
        disabled={leaving}
        onClick={() => onDisconnect(account.id)}
      >
        Disconnect {accountName(account)}
      </Button>
    </div>
  );
}

function GithubIntegration() {
  const {
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
  } = useIntegrations();

  const accounts = accountsFor(GITHUB);
  // Only the sign-in flow blocks, and only the sign-in button: a browser tab
  // the user has not come back from is no reason another account cannot be
  // renamed or removed.
  const signingIn = status === 'working' || status === 'awaiting-user';
  const showCode = login !== null && connecting === GITHUB;

  // The user is the one being waited on here, and their code does not last
  // forever, so the wait is stated as what is left of it rather than as an
  // indicator that could run all afternoon.
  const waited = useElapsed(showCode);
  const remaining = login === null ? 0 : Math.max(0, login.expiresIn - waited);

  return (
    <SettingsSection
      title="GitHub"
      description="Lets Chief read your pull requests. Sign-in happens in your browser and the token is stored only on this machine."
      state={
        status === 'loading' ? (
          <Chip tone="quiet">Checking</Chip>
        ) : accounts.length === 0 ? (
          <Chip tone="quiet">Not connected</Chip>
        ) : (
          <Chip tone="verified" dot>
            {accounts.length === 1 ? 'Connected' : `${accounts.length} accounts`}
          </Chip>
        )
      }
    >
      {accounts.length > 0 && (
        <div className="mt-3">
          {accounts.map((account) => (
            <ConnectedAccount
              key={account.id}
              account={account}
              onRename={rename}
              onDisconnect={disconnect}
              leaving={disconnecting.includes(account.id)}
            />
          ))}
        </div>
      )}

      {showCode && (
        <div className="mt-4 rounded-md border border-border p-4" role="status">
          <p className="text-sm">
            Enter this code at{' '}
            <span className="font-mono text-[13px]" data-selectable>
              {login.verificationUri}
            </span>
          </p>
          <p className="mt-2 font-mono text-xl tracking-[0.2em]" data-selectable>
            {login.userCode}
          </p>
          {remaining > 0 ? (
            <p className="mt-2.5 flex items-center gap-2 micro text-muted-foreground">
              <Dots />
              Waiting for you to finish in the browser · {countdown(remaining)} left
            </p>
          ) : (
            <p className="mt-2.5 micro text-attention-text">
              This code has expired. Start again to get another.
            </p>
          )}
          <div className="mt-3">
            <Button variant="outline" size="sm" onClick={cancel}>
              Cancel
            </Button>
          </div>
        </div>
      )}

      {error !== null && (
        <p
          className="mt-4 rounded-md border border-destructive bg-destructive-surface px-3.5 py-3 text-[13px] leading-snug text-destructive-text"
          role="alert"
        >
          {error}
        </p>
      )}

      <div className="mt-4">
        <Button
          size="sm"
          variant={accounts.length > 0 ? 'outline' : 'default'}
          onClick={() => connect(GITHUB)}
          disabled={signingIn || status === 'loading'}
        >
          <Github aria-hidden />
          {accounts.length > 0 ? 'Add another GitHub account' : 'Connect GitHub'}
        </Button>
      </div>
    </SettingsSection>
  );
}

/** Configuration surface for the model, integrations and local data. */
export function SettingsView() {
  return (
    <div className="h-full overflow-y-auto">
      <div className="px-7 py-6">
        <div className="flex max-w-[680px] flex-col gap-3">
          <SettingsSection
            title="Local model"
            description="Chief runs llama.cpp itself, on a loopback address only this machine can reach. The engine ships with the app and stops when you close it; the client refuses any address that is not local."
            state={<Chip tone="machine">llama-3.2-3b-instruct</Chip>}
          />
          <GithubIntegration />
          <SettingsSection
            title="Local data"
            description="Your work log and integration tokens live in a SQLite file inside this app's config directory. Nothing is synchronised anywhere."
            state={
              <Chip tone="verified" dot>
                On this machine
              </Chip>
            }
          />
        </div>
      </div>
    </div>
  );
}
