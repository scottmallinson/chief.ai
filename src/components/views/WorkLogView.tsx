import { RefreshCw } from 'lucide-react';

import { Button } from '@/components/ui/button';
import { useWorkLog } from '@/hooks/use-work-log';
import type { WorkLogEntry } from '@/lib/work-log';

const timeFormat = new Intl.DateTimeFormat(undefined, {
  dateStyle: 'medium',
  timeStyle: 'short',
});

function formatTimestamp(timestamp: string): string {
  const parsed = new Date(timestamp);
  return Number.isNaN(parsed.getTime()) ? timestamp : timeFormat.format(parsed);
}

function Entry({ entry }: { entry: WorkLogEntry }) {
  return (
    <li className="rounded-lg border border-border p-4">
      <div className="flex items-center gap-2 text-xs text-muted-foreground">
        <span className="rounded-full border border-border px-2 py-0.5">{entry.source}</span>
        <time dateTime={entry.timestamp}>{formatTimestamp(entry.timestamp)}</time>
      </div>
      <p className="mt-2 text-sm" data-selectable>
        {entry.summary ?? entry.content}
      </p>
      {entry.summary !== null && (
        <p className="mt-1 text-xs text-muted-foreground" data-selectable>
          {entry.content}
        </p>
      )}
    </li>
  );
}

/**
 * Chronological record of the user's work, read from the local SQLite
 * database. Entries are written by the background summariser in a later step.
 */
export function WorkLogView() {
  const { entries, status, error, reload } = useWorkLog();

  return (
    <div className="mx-auto max-w-3xl p-6">
      {status === 'loading' && (
        <p className="text-sm text-muted-foreground" role="status">
          Reading your local work log…
        </p>
      )}

      {status === 'error' && (
        <div className="rounded-lg border border-destructive/50 p-6" role="alert">
          <h2 className="text-sm font-semibold">Could not read the work log</h2>
          <p className="mt-1 text-sm text-muted-foreground">{error}</p>
          <Button variant="outline" size="sm" className="mt-4" onClick={reload}>
            <RefreshCw aria-hidden />
            Try again
          </Button>
        </div>
      )}

      {status === 'ready' &&
        (entries.length === 0 ? (
          <div className="rounded-lg border border-dashed border-border p-10 text-center">
            <h2 className="text-sm font-semibold">No entries yet</h2>
            <p className="mx-auto mt-2 max-w-sm text-sm text-muted-foreground">
              Your daily log will fill in from your connected tools once the background summariser
              is in place.
            </p>
          </div>
        ) : (
          <ul className="space-y-3">
            {entries.map((entry) => (
              <Entry key={entry.id} entry={entry} />
            ))}
          </ul>
        ))}
    </div>
  );
}
