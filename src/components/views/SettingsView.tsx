import { Github } from 'lucide-react';

import { Button } from '@/components/ui/button';
import { useGithub } from '@/hooks/use-github';

interface SettingsSectionProps {
  title: string;
  description: string;
  status: string;
  children?: React.ReactNode;
}

function SettingsSection({ title, description, status, children }: SettingsSectionProps) {
  return (
    <section className="rounded-lg border border-border p-4">
      <div className="flex items-start justify-between gap-4">
        <div className="min-w-0">
          <h2 className="text-sm font-medium">{title}</h2>
          <p className="mt-1 text-sm text-muted-foreground">{description}</p>
        </div>
        <span className="shrink-0 rounded-full border border-border px-2 py-0.5 text-[11px] text-muted-foreground">
          {status}
        </span>
      </div>
      {children}
    </section>
  );
}

const connectedFormat = new Intl.DateTimeFormat(undefined, { dateStyle: 'medium' });

function connectedSince(timestamp: string | null): string {
  if (timestamp === null) return 'Not connected';

  const parsed = new Date(timestamp);
  return Number.isNaN(parsed.getTime())
    ? 'Connected'
    : `Connected ${connectedFormat.format(parsed)}`;
}

function GithubIntegration() {
  const { connection, login, status, error, connect, disconnect } = useGithub();

  const isBusy = status === 'working' || status === 'awaiting-user';

  return (
    <SettingsSection
      title="GitHub"
      description="Lets Chief read your pull requests. Sign-in happens in your browser and the token is stored only on this machine."
      status={status === 'loading' ? 'Checking…' : connectedSince(connection?.connectedAt ?? null)}
    >
      {login !== null && (
        <div className="mt-4 rounded-md border border-border p-4" role="status">
          <p className="text-sm">
            Enter this code at{' '}
            <span className="font-medium" data-selectable>
              {login.verificationUri}
            </span>
          </p>
          <p className="mt-2 font-mono text-2xl tracking-[0.2em]" data-selectable>
            {login.userCode}
          </p>
          <p className="mt-2 text-xs text-muted-foreground">
            Waiting for you to finish in the browser…
          </p>
        </div>
      )}

      {error !== null && (
        <p
          className="mt-4 rounded-md border border-destructive/50 p-3 text-sm text-muted-foreground"
          role="alert"
        >
          {error}
        </p>
      )}

      <div className="mt-4">
        {connection?.connected === true ? (
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
    <div className="mx-auto max-w-3xl space-y-4 p-6">
      <SettingsSection
        title="Local model"
        description="Chief talks to an Ollama instance on http://localhost:11434. The client refuses any address that is not on this machine."
        status="llama3.2:3b"
      />
      <GithubIntegration />
      <SettingsSection
        title="Local data"
        description="Your work log and integration tokens live in a SQLite file inside this app's config directory. Nothing is synchronised anywhere."
        status="Ready"
      />
    </div>
  );
}
