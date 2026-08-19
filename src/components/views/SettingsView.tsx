interface SettingsSectionProps {
  title: string;
  description: string;
  status: string;
}

function SettingsSection({ title, description, status }: SettingsSectionProps) {
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
    </section>
  );
}

/** Configuration surface for the model, integrations and local data. */
export function SettingsView() {
  return (
    <div className="mx-auto max-w-3xl space-y-4 p-6">
      <SettingsSection
        title="Local model"
        description="Chief talks to an Ollama instance on http://localhost:11434. Nothing is sent anywhere else."
        status="Not configured"
      />
      <SettingsSection
        title="Integrations"
        description="Connect GitHub and your calendar. Tokens are stored in the local SQLite database on this machine."
        status="Not connected"
      />
      <SettingsSection
        title="Local data"
        description="Your work log and embeddings live in a SQLite file inside this app's data directory."
        status="Not initialised"
      />
    </div>
  );
}
