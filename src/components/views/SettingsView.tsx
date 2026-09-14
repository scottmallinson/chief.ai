import { useCallback, useEffect, useState, type ReactNode } from 'react';
import { FolderOpen, Github, Mail, RefreshCw, SquareKanban } from 'lucide-react';
import { revealItemInDir } from '@tauri-apps/plugin-opener';

import { Button } from '@/components/ui/button';
import { Chip } from '@/components/ui/chip';
import { DisconnectAccount } from '@/components/DisconnectAccount';
import { Freshness } from '@/components/Freshness';
import { corpusLocation, type CorpusLocation } from '@/lib/corpus';
import { ProfileBootstrap } from '@/components/ProfileBootstrap';
import { CalendarSubscriptions } from '@/components/CalendarSubscriptions';
import { AtlassianToken } from '@/components/AtlassianToken';
import { LinearKey } from '@/components/LinearKey';
import { SignInRegistration } from '@/components/SignInRegistration';
import { runDoctor, type Report } from '@/lib/doctor';
import { Dots } from '@/components/ui/activity';
import { useElapsed } from '@/hooks/use-elapsed';
import { useIntegrations } from '@/hooks/use-integrations';
import { useSyncState } from '@/hooks/use-sync-state';
import type { SyncState } from '@/lib/sync-state';
import {
  accountName,
  CALENDAR,
  GITHUB,
  ATLASSIAN,
  LINEAR,
  MICROSOFT,
  type Account,
} from '@/lib/integrations';

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
  sync,
  onReconnect,
}: {
  account: Account;
  onRename: (accountId: number, label: string | null) => Promise<boolean>;
  onDisconnect: (accountId: number) => void;
  /** This account is being forgotten. Nothing else on the screen cares. */
  leaving: boolean;
  /** How fresh it is, or undefined if it has never been read. */
  sync: SyncState | undefined;
  onReconnect: () => void;
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
    <div className="border-t border-border py-3">
      {/* The field and the button are one line, so they are one flex row.
          Centring the button against the whole column instead — the field
          with its caption under it — put it half a caption low, which is
          visible at a glance and was 8px. */}
      <div className="flex items-center gap-4">
        <input
          type="text"
          aria-label={`Name for ${identity}`}
          value={name}
          placeholder={identity}
          maxLength={NAME_LIMIT}
          onChange={(event) => setName(event.target.value)}
          onBlur={(event) => commit(event.target.value)}
          className="min-w-0 flex-1 rounded-md border border-input bg-background px-2 py-1 text-sm font-medium transition-colors duration-[120ms] ease-instrument placeholder:font-normal placeholder:text-muted-foreground hover:border-ring"
        />
        <DisconnectAccount
          accountId={account.id}
          name={accountName(account)}
          verb="Disconnect"
          leaving={leaving}
          onConfirm={onDisconnect}
        />
      </div>
      <p className="mt-1 flex flex-wrap items-center gap-x-2 gap-y-1 px-[9px] micro text-muted-foreground">
        <span>
          {Number.isNaN(since.getTime())
            ? 'Connected'
            : `Connected ${connectedFormat.format(since)}`}
        </span>
        <span aria-hidden>·</span>
        <Freshness state={sync} onReconnect={onReconnect} />
      </p>
    </div>
  );
}

/** One provider's row: how it signs in, and which accounts are connected. */
interface IntegrationProps {
  /** The value stored in `integration_accounts.service`. */
  service: string;
  title: string;
  description: string;
  /** What the connect button says when nothing is connected yet. */
  connectLabel: string;
  /** What it says when adding a second account. */
  addLabel: string;
  icon: ReactNode;
  /**
   * Where the user makes a registration of their own, in a sentence.
   *
   * Optional, because not every provider has one to make. Atlassian registers
   * Chief at sign-in and the client belongs to that account, so there is no id
   * to paste and offering a field for one would be offering a setting that
   * does nothing.
   */
  registrationHelp?: ReactNode;
  /**
   * What connecting actually permits, said before the button rather than
   * after.
   *
   * Every provider Chief reads holds more than it needs — GitHub's `repo`
   * scope is read *and* write across private repositories — and burying that
   * is the omission this prop exists to prevent.
   */
  grants?: ReactNode;
  /**
   * A second way to connect, under the sign-in button.
   *
   * Only Atlassian has one, and only because its sign-in can be taken away by
   * somebody who is not the user: the MCP server is part of Rovo, so an
   * administrator switching that off ends the browser route entirely. Taking
   * the card's own `reload` means a connection made down there refreshes the
   * account list up here — `useIntegrations` is per-component state, so a
   * second copy of the hook would refresh a list nobody is looking at.
   */
  fallback?: (onChanged: () => void) => ReactNode;
}

/**
 * A connected service.
 *
 * Written once for every provider rather than per service: the hook already
 * takes the service by name, and the only thing that differed was the words.
 */
function Integration({
  service,
  title,
  description,
  connectLabel,
  addLabel,
  icon,
  registrationHelp,
  grants,
  fallback,
}: IntegrationProps) {
  const {
    accountsFor,
    login,
    connecting,
    status,
    disconnecting,
    error,
    notice,
    connect,
    cancel,
    disconnect,
    rename,
    reload,
  } = useIntegrations();

  const { states } = useSyncState();

  const accounts = accountsFor(service);
  // Only the sign-in flow blocks, and only the sign-in button: a browser tab
  // the user has not come back from is no reason another account cannot be
  // renamed or removed.
  const signingIn = status === 'working' || status === 'awaiting-user';
  const prompt = login !== null && connecting === service ? login : null;

  // The user is the one being waited on here, and a device code does not last
  // forever, so the wait is stated as what is left of it rather than as an
  // indicator that could run all afternoon. A browser sign-in has no code and
  // no countdown of its own, so it is simply waited on.
  const waited = useElapsed(prompt !== null);
  const remaining = prompt?.kind === 'device' ? Math.max(0, prompt.expiresIn - waited) : 0;

  // Named once: the sign-in button says it, and the client id field points at
  // it by name, so the two cannot drift apart on screen.
  const signInLabel = accounts.length > 0 ? addLabel : connectLabel;

  return (
    <SettingsSection
      title={title}
      description={description}
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
              sync={states.get(account.id)}
              onReconnect={() => connect(service, 'again')}
            />
          ))}
        </div>
      )}

      {prompt?.kind === 'device' && (
        <div className="mt-4 rounded-md border border-border p-4" role="status">
          {/* Chief opens the page with the code already in it, so this reads
              as confirmation rather than as an instruction. The code and the
              plain address stay on screen because the prefill is built from
              something GitHub does not document: if it stops working, this
              panel is still everything the user needs. */}
          <p className="text-sm">
            Your browser is open at{' '}
            <span className="font-mono text-[13px]" data-selectable>
              {prompt.verificationUri}
            </span>{' '}
            with this code filled in. Enter it yourself if it is not.
          </p>
          <p className="mt-2 font-mono text-xl tracking-[0.2em]" data-selectable>
            {prompt.userCode}
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

      {prompt?.kind === 'browser' && (
        <div className="mt-4 rounded-md border border-border p-4" role="status">
          <p className="text-sm">
            Finish signing in on the page that just opened. Chief is listening on this machine for
            the browser to come back.
          </p>
          {/* Shown as well as opened: if the browser did not open, this is the
              only way through, and it is a machine fact either way. */}
          <p className="mt-2 font-mono text-[12px] break-all text-muted-foreground" data-selectable>
            {prompt.url}
          </p>
          <p className="mt-2.5 flex items-center gap-2 micro text-muted-foreground">
            <Dots />
            Waiting for you to finish in the browser
          </p>
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

      {/* Amber, not red: nothing failed. The user is the one who has to do
          something about it, which is exactly what amber means here. */}
      {notice !== null && (
        <p
          className="mt-4 rounded-md border border-attention bg-attention-surface px-3.5 py-3 text-[13px] leading-snug text-attention-text"
          role="status"
        >
          {notice}
        </p>
      )}

      {grants !== undefined && (
        <p className="mt-4 text-[13px] leading-snug text-muted-foreground">{grants}</p>
      )}

      <div className="mt-4">
        <Button
          size="sm"
          variant={accounts.length > 0 ? 'outline' : 'default'}
          onClick={() => connect(service, accounts.length > 0 ? 'another' : 'first')}
          disabled={signingIn || status === 'loading'}
        >
          {icon}
          {signInLabel}
        </Button>
      </div>

      {registrationHelp !== undefined && (
        <SignInRegistration
          service={service}
          name={title}
          signInLabel={signInLabel}
          where={registrationHelp}
        />
      )}

      {fallback?.(reload)}
    </SettingsSection>
  );
}

/** Configuration surface for the model, integrations and local data. */
/**
 * The folder of markdown Chief reads and writes.
 *
 * Deliberately a folder the user can open, not a hidden one inside the app's
 * data: these files are theirs, and every source document describes this layer
 * as human-editable. So the most useful control here is the one that opens it.
 */
function Corpus() {
  const [location, setLocation] = useState<CorpusLocation | null>(null);
  const [problem, setProblem] = useState<string | null>(null);

  const load = useCallback(async () => {
    try {
      setLocation(await corpusLocation());
      setProblem(null);
    } catch (error) {
      setProblem(error instanceof Error ? error.message : String(error));
    }
  }, []);

  useEffect(() => {
    void load();
  }, [load]);

  return (
    <SettingsSection
      title="Your corpus"
      description="Notes, briefs and drafts, as plain markdown you can open in any editor. Chief reads from here and writes back to it; nothing in it is synchronised anywhere."
      state={
        location ? (
          <Chip tone="verified" dot>
            {location.files === 1 ? '1 file' : `${location.files} files`}
          </Chip>
        ) : (
          <Chip tone="quiet">Checking</Chip>
        )
      }
    >
      {location && (
        <>
          <p className="mt-3 font-mono text-[13px] break-all text-muted-foreground" data-selectable>
            {location.root}
          </p>
          {!location.exists && (
            <p className="mt-2 text-sm text-attention-text">
              This folder is not there. Chief will create it the next time it starts.
            </p>
          )}
        </>
      )}

      {problem && <p className="mt-3 text-sm text-attention-text">{problem}</p>}

      <div className="mt-4 flex flex-wrap gap-2">
        <Button
          size="sm"
          variant="outline"
          onClick={() => {
            // revealItemInDir rather than openPath: the renderer is granted
            // `opener:default`, which covers revealing an item and not opening
            // an arbitrary path, and widening a capability to save a click is
            // not a trade worth making.
            if (location) void revealItemInDir(location.root).catch(() => undefined);
          }}
          disabled={!location?.exists}
        >
          <FolderOpen aria-hidden />
          Show folder
        </Button>
        <Button size="sm" variant="outline" onClick={() => void load()}>
          <RefreshCw aria-hidden />
          Rescan
        </Button>
      </div>
    </SettingsSection>
  );
}

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
          {/* Stated rather than capped. llama.cpp has no allocation cap, so
              what the engine holds is chosen by the tier, this window and KV
              precision — and showing the arithmetic is what makes it a choice
              rather than a surprise on a machine with 8 GB. */}
          <dt className="pt-1 micro text-muted-foreground">Holds</dt>
          <dd className="font-mono text-[13px]">
            {gigabytes(report.residentMb)} · {report.kvCacheMb} MiB cache
          </dd>
          {measurement && (
            <>
              <dt className="pt-1 micro text-muted-foreground">First reply</dt>
              <dd className="font-mono text-[13px]">{seconds(measurement.firstTokenMs)}</dd>
              {measurement.warmFirstTokenMs !== null && (
                <>
                  {/* The number the specification's 1.5s target could sensibly
                      have been about: only the part of the prompt that changed
                      is read. Cold, 2,000 tokens is a minute or more. */}
                  <dt className="pt-1 micro text-muted-foreground">Warm reply</dt>
                  <dd className="font-mono text-[13px]">{seconds(measurement.warmFirstTokenMs)}</dd>
                </>
              )}
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

/**
 * Calendars subscribed to by address.
 *
 * Its own card rather than a third `Integration`: there is no sign-in, no
 * account to name at the provider and nothing to disconnect from — which is
 * the entire reason it works where Outlook does not.
 */
function Calendars() {
  const { accountsFor, reload } = useIntegrations();
  const subscriptions = accountsFor(CALENDAR);

  return (
    <SettingsSection
      title="Calendar subscriptions"
      description="Any calendar that publishes an address — Google, Outlook, iCloud, Fastmail. Nothing to register and nobody to ask, and Chief reads it straight from the provider."
      state={
        subscriptions.length > 0 ? (
          <Chip tone="verified" dot>
            {subscriptions.length === 1 ? '1 calendar' : `${subscriptions.length} calendars`}
          </Chip>
        ) : (
          <Chip tone="quiet">None</Chip>
        )
      }
    >
      <CalendarSubscriptions accounts={subscriptions} onChanged={reload} />
    </SettingsSection>
  );
}

/**
 * Linear, connected with a pasted key.
 *
 * Its own card for the same reason the calendar has one: there is no sign-in
 * flow, so an `Integration` would be a browser round trip that never happens.
 */
function Linear() {
  const { accountsFor, reload } = useIntegrations();
  const workspaces = accountsFor(LINEAR);

  return (
    <SettingsSection
      title="Linear"
      description="What is assigned to you and not finished — the one thing pull requests and mail cannot tell Chief, because they are the outputs of work rather than the record of it."
      state={
        workspaces.length > 0 ? (
          <Chip tone="verified" dot>
            {workspaces.length === 1 ? '1 workspace' : `${workspaces.length} workspaces`}
          </Chip>
        ) : (
          <Chip tone="quiet">Not connected</Chip>
        )
      }
    >
      <LinearKey accounts={workspaces} onChanged={reload} />
    </SettingsSection>
  );
}

export function SettingsView() {
  return (
    <div className="h-full overflow-y-auto">
      <div className="px-7 py-6">
        <div className="flex max-w-[680px] flex-col gap-3">
          <LocalModel />
          <Corpus />
          <SettingsSection
            title="Your profile"
            description="Drafts sound like a model until Chief has read some of your own writing. It can seed your writing style and your team from work you have already done — on this machine, and only when you ask."
            state={<Chip tone="quiet">On request</Chip>}
          >
            <ProfileBootstrap />
          </SettingsSection>
          <ThisMachine />
          <Calendars />
          <Linear />
          <Integration
            service={GITHUB}
            title="GitHub"
            description="Lets Chief read your pull requests. Sign-in happens in your browser and the token is stored only on this machine."
            connectLabel="Connect GitHub"
            addLabel="Add another GitHub account"
            icon={<Github aria-hidden />}
            grants={
              <>
                Chief asks for the <span className="font-mono text-[12px]">repo</span> scope, which
                is read <strong>and write</strong> across your public and private repositories. That
                is broader than it needs: GitHub offers OAuth apps no read-only scope for private
                repositories. Chief only reads.
              </>
            }
            registrationHelp={
              <>
                Make an <strong>OAuth app</strong> on GitHub under Settings → Developer settings,
                tick <strong>Enable Device Flow</strong>, and paste its client id. It is not a
                secret — Chief never needs the client secret, because the device flow has none.
              </>
            }
          />
          <Integration
            service={MICROSOFT}
            title="Outlook"
            description="Lets Chief read your mail and calendar. Sign-in opens your browser and comes back to a port on this machine; the token is stored only here."
            connectLabel="Connect Outlook"
            addLabel="Add another Outlook account"
            icon={<Mail aria-hidden />}
            registrationHelp={
              <>
                Register an <strong>application</strong> in Entra, add a{' '}
                <strong>Mobile and desktop</strong> redirect of{' '}
                <span className="font-mono text-[12px]">http://localhost</span>, and paste its
                application (client) id. It is not a secret — Chief signs in as a public client,
                which has none.
              </>
            }
          />
          <Integration
            service={ATLASSIAN}
            title="Jira"
            description="Lets Chief read the Jira issues assigned to you and not finished, and — connected with a token — the Confluence pages you have been writing. Sign-in opens your browser and comes back to a port on this machine; the token is stored only here."
            connectLabel="Connect Atlassian"
            addLabel="Add another Atlassian account"
            icon={<SquareKanban aria-hidden />}
            grants={
              <>
                Chief asks for <strong>read-only</strong> scopes, and Atlassian enforces that at
                sign-in rather than taking Chief's word for it — this connection cannot create, edit
                or transition anything, and Chief would have to ask you to sign in again to gain
                that. It also registers itself with Atlassian when you connect, so there is no
                client id to paste and nothing about this copy of Chief is shared with anyone
                else's.
              </>
            }
            fallback={(onChanged) => <AtlassianToken onChanged={onChanged} />}
          />
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
