import { useCallback, useEffect, useState, type ReactNode } from 'react';
import { Github } from 'lucide-react';

import { Button } from '@/components/ui/button';
import { Chip } from '@/components/ui/chip';
import { runDoctor, type Report } from '@/lib/doctor';
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

/**
 * How long a name may be. A row is a field beside a button inside a 680px
 * measure, and a name with no end to it takes the field down to nothing.
 */
const NAME_LIMIT = 40;

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
  onRename: (accountId: number, label: string | null) => Promise<boolean>;
  onDisconnect: (accountId: number) => void;
  /** This account is being forgotten. Nothing else on the screen cares. */
  leaving: boolean;
}) {
  const since = new Date(account.connectedAt);
  const identity = account.identity ?? account.accountKey;
  const [name, setName] = useState(account.label ?? '');

  // Committed on blur rather than per keystroke: naming an account is not
  // worth a database write per character.
  const commit = (value: string) => {
    const label = value.trim() === '' ? null : value.trim();

    if (label === account.label) return;

    void onRename(account.id, label).then((saved) => {
      // A name the database refused is not the name. Putting the stored one
      // back keeps the field honest about what Chief holds, and stops every
      // later blur re-firing a write that has already been refused once.
      if (!saved) setName(account.label ?? '');
    });
  };

  return (
    <div className="flex items-center justify-between gap-4 border-t border-border py-3">
      <div className="min-w-0 flex-1">
        <input
          type="text"
          aria-label={`Name for ${identity}`}
          value={name}
          placeholder={identity}
          maxLength={NAME_LIMIT}
          onChange={(event) => setName(event.target.value)}
          onBlur={(event) => commit(event.target.value)}
          className="w-full rounded-md border border-input bg-background px-2 py-1 text-sm font-medium transition-colors duration-[120ms] ease-instrument placeholder:font-normal placeholder:text-muted-foreground hover:border-ring"
        />
        <p className="mt-1 px-[9px] micro text-muted-foreground">
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
        {/* Bounded and truncated, so a long name ends in an ellipsis rather
            than pushing the field that names it down to nothing. The button
            keeps its full accessible name either way. */}
        <span className="max-w-[9rem] truncate">Disconnect {accountName(account)}</span>
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
/**
 * The engine, and the model it is actually serving.
 *
 * The name was written into this file, which made it wrong the moment there
 * was more than one model to run.
 */
function LocalModel() {
  const [model, setModel] = useState<string | null>(null);

  useEffect(() => {
    void runDoctor(false)
      .then((report) => setModel(report.model))
      .catch(() => setModel(null));
  }, []);

  return (
    <SettingsSection
      title="Local model"
      description="Chief runs llama.cpp itself, on a loopback address only this machine can reach. The engine ships with the app and stops when nothing is using it; the client refuses any address that is not local."
      state={model ? <Chip tone="machine">{model}</Chip> : <Chip tone="quiet">Checking</Chip>}
    />
  );
}

/** Machine facts read in the units a person thinks in. */
function gigabytes(mebibytes: number): string {
  return `${(mebibytes / 1024).toFixed(1)} GB`;
}

/** Seconds, to one decimal place, from milliseconds. */
function seconds(ms: number): string {
  return `${(ms / 1000).toFixed(1)}s`;
}

/**
 * What Chief worked out about this machine, and what it measured.
 *
 * Opens with whatever was recorded last rather than spending a generation on
 * every visit. "Measure again" is the only thing here that costs anything, and
 * it says so.
 */
function ThisMachine() {
  const [report, setReport] = useState<Report | null>(null);
  const [measuring, setMeasuring] = useState(false);
  const [problem, setProblem] = useState<string | null>(null);

  const load = useCallback(async (remeasure: boolean) => {
    setProblem(null);
    if (remeasure) setMeasuring(true);

    try {
      setReport(await runDoctor(remeasure));
    } catch (error) {
      setProblem(error instanceof Error ? error.message : String(error));
    } finally {
      setMeasuring(false);
    }
  }, []);

  useEffect(() => {
    void load(false);
  }, [load]);

  const measurement = report?.measurement ?? null;

  return (
    <SettingsSection
      title="This machine"
      description="Chief looked at the memory and cores here and chose a model that fits. Nothing about this leaves the machine."
      state={
        report ? <Chip tone="machine">{report.tier}</Chip> : <Chip tone="quiet">Checking</Chip>
      }
    >
      {report && (
        <dl className="mt-3 grid grid-cols-[max-content_minmax(0,1fr)] gap-x-4 gap-y-1 text-sm">
          <dt className="pt-1 micro text-muted-foreground">Memory</dt>
          <dd className="font-mono text-[13px]">{gigabytes(report.memoryMb)}</dd>
          <dt className="pt-1 micro text-muted-foreground">Cores</dt>
          <dd className="font-mono text-[13px]">{report.cores}</dd>
          <dt className="pt-1 micro text-muted-foreground">Window</dt>
          <dd className="font-mono text-[13px]">{report.contextSize} tokens</dd>
          {measurement && (
            <>
              <dt className="pt-1 micro text-muted-foreground">First reply</dt>
              <dd className="font-mono text-[13px]">{seconds(measurement.firstTokenMs)}</dd>
              <dt className="pt-1 micro text-muted-foreground">Writing</dt>
              <dd className="font-mono text-[13px]">
                {Math.round(report.charactersPerSecond ?? 0)} chars/s
              </dd>
            </>
          )}
        </dl>
      )}

      {!measurement && report && (
        <p className="mt-3 text-sm text-muted-foreground">This machine has not been timed yet.</p>
      )}

      {problem && <p className="mt-3 text-sm text-attention-text">{problem}</p>}

      <Button
        size="sm"
        variant="secondary"
        className="mt-3"
        onClick={() => void load(true)}
        disabled={measuring}
      >
        {measuring ? <Dots /> : null}
        {measuring ? 'Measuring…' : 'Measure again'}
      </Button>
    </SettingsSection>
  );
}

export function SettingsView() {
  return (
    <div className="h-full overflow-y-auto">
      <div className="px-7 py-6">
        <div className="flex max-w-[680px] flex-col gap-3">
          <LocalModel />
          <ThisMachine />
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
