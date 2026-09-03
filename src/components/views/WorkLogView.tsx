import { RefreshCw } from 'lucide-react';

import { Button } from '@/components/ui/button';
import { OpenSource } from '@/components/OpenSource';
import { Chip } from '@/components/ui/chip';
import { Dots } from '@/components/ui/activity';
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
    <li className="rounded-lg border border-border bg-card p-4">
      <div className="flex items-center gap-2.5">
        <Chip tone="machine">{entry.source}</Chip>
        <time className="font-mono text-xs text-muted-foreground" dateTime={entry.timestamp}>
          {formatTimestamp(entry.timestamp)}
        </time>
        <OpenSource url={entry.url} label={entry.summary ?? entry.content} />
      </div>
      <p className="mt-2.5 text-sm leading-relaxed" data-selectable>
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
 * database. Entries are written by the background daemon.
 */
export function WorkLogView() {
  const { entries, status, error, reload } = useWorkLog();
  const isLoading = status === 'loading';

  return (
    <div className="h-full overflow-y-auto">
      <div className="px-7 py-6">
        <div className="max-w-[680px]">
          <div className="mb-4 flex items-center justify-between gap-4">
            <p className="text-xs text-muted-foreground">
              Filled in automatically from your connected tools, summarised on this machine.
            </p>
            <Button
              variant="ghost"
              size="sm"
              onClick={reload}
              disabled={isLoading}
              aria-label="Refresh work log"
            >
              {isLoading ? <Dots /> : <RefreshCw aria-hidden />}
              Refresh
            </Button>
          </div>

          {isLoading && (
            <p className="micro text-muted-foreground" role="status">
              Reading your local work log
            </p>
          )}

          {status === 'error' && (
            <div
              className="rounded-md border border-destructive bg-destructive-surface px-3.5 py-3 text-destructive-text"
              role="alert"
            >
              <h2 className="text-[13px] font-semibold">Could not read the work log</h2>
              <p className="mt-1 text-[13px] leading-snug">{error}</p>
              <Button variant="outline" size="sm" className="mt-3.5" onClick={reload}>
                <RefreshCw aria-hidden />
                Try again
              </Button>
            </div>
          )}

          {status === 'ready' &&
            (entries.length === 0 ? (
              <div className="rounded-lg border border-dashed border-input p-6 text-center">
                <h2 className="text-[15px] font-semibold tracking-[-0.015em]">No entries yet</h2>
                <p className="mx-auto mt-1.5 max-w-[280px] text-[13px] leading-snug text-muted-foreground">
                  Chief writes your log while you work, from the tools you have connected.
                </p>
              </div>
            ) : (
              <ul className="flex flex-col gap-3">
                {entries.map((entry) => (
                  <Entry key={entry.id} entry={entry} />
                ))}
              </ul>
            ))}
        </div>
      </div>
    </div>
  );
}
