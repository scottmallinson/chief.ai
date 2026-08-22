import type { ReactNode } from 'react';
import { Github } from 'lucide-react';

import { Button } from '@/components/ui/button';
import { Chip, type ChipProps } from '@/components/ui/chip';
import { Dots } from '@/components/ui/activity';
import { useGithub } from '@/hooks/use-github';
import { type Account } from '@/lib/integrations';

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

/** The connection as a chip: verified when it is one, quiet when it is not. */
function connectionState(account: Account | null): {
  tone: ChipProps['tone'];
  label: string;
  dot: boolean;
} {
  if (account === null) {
    return { tone: 'quiet', label: 'Not connected', dot: false };
  }

  const parsed = new Date(account.connectedAt);

  return {
    tone: 'verified',
    label: Number.isNaN(parsed.getTime())
      ? 'Connected'
      : `Connected ${connectedFormat.format(parsed)}`,
    dot: true,
  };
}

function GithubIntegration() {
  const { account, login, status, error, connect, disconnect } = useGithub();

  const isBusy = status === 'working' || status === 'awaiting-user';
  const state = connectionState(account);

  return (
    <SettingsSection
      title="GitHub"
      description="Lets Chief read your pull requests. Sign-in happens in your browser and the token is stored only on this machine."
      state={
        status === 'loading' ? (
          <Chip tone="quiet">Checking</Chip>
        ) : (
          <Chip tone={state.tone} dot={state.dot}>
            {state.label}
          </Chip>
        )
      }
    >
      {login !== null && (
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
          <p className="mt-2.5 flex items-center gap-2 micro text-muted-foreground">
            <Dots />
            Waiting for you to finish in the browser
          </p>
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
        {account !== null ? (
          <Button variant="outline" size="sm" onClick={disconnect} disabled={isBusy}>
            Disconnect
          </Button>
        ) : (
          <Button size="sm" onClick={connect} disabled={isBusy || status === 'loading'}>
            <Github aria-hidden />
            Connect GitHub
          </Button>
        )}
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
