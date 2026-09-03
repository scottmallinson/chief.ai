import { RefreshCw } from 'lucide-react';

import { Button } from '@/components/ui/button';
import { OpenSource } from '@/components/OpenSource';
import { Chip } from '@/components/ui/chip';
import { Dots } from '@/components/ui/activity';
import { Freshness } from '@/components/Freshness';
import { useSync } from '@/hooks/use-sync';
import { useWorkLog } from '@/hooks/use-work-log';
import { readEntry, type WorkLogEntry } from '@/lib/work-log';

const timeFormat = new Intl.DateTimeFormat(undefined, {
  dateStyle: 'medium',
  timeStyle: 'short',
});

function formatTimestamp(timestamp: string): string {
  const parsed = new Date(timestamp);
  return Number.isNaN(parsed.getTime()) ? timestamp : timeFormat.format(parsed);
}

function Entry({ entry }: { entry: WorkLogEntry }) {
  const { headline, detail } = readEntry(entry);

  return (
    <li className="rounded-lg border border-border bg-card p-4">
      <div className="flex items-center gap-2.5">
        <Chip tone="machine">{entry.source}</Chip>
        {detail !== null && <Chip tone="machine">{detail}</Chip>}
        <time className="font-mono text-xs text-muted-foreground" dateTime={entry.timestamp}>
          {formatTimestamp(entry.timestamp)}
        </time>
        <OpenSource url={entry.url} label={headline} />
      </div>
      <p className="mt-2.5 text-sm leading-relaxed" data-selectable>
        {headline}
      </p>
    </li>
  );
}

/**
 * Chronological record of the user's work, read from the local SQLite
 * database. Entries are written by the background daemon — and, since this
 * screen grew a button that means it, by whoever is looking at them.
 *
 * **Two different acts, and they used to be one button.** Reading the rows is
 * a `SELECT`; bringing them up to date is a pass out to every connected
 * service. "Refresh" did the first while being read as the second, so a log
 * that had stopped three days earlier could be refreshed all day without
 * anything being asked of anybody, and without the screen ever saying so.
 *
 * Freshness sits here rather than only in Settings for the reason `Freshness`
 * itself gives: once a read question is answered from these rows, the answer
 * is only as good as the last pass, and staleness is invisible in a way a
 * spinner is not.
 */
export function WorkLogView() {
  const { entries, status, error, reload } = useWorkLog();
  const { accounts, syncing, error: syncError, sync } = useSync(reload);
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
              onClick={sync}
              disabled={syncing}
              aria-label="Refresh work log"
            >
              {syncing ? <Dots /> : <RefreshCw aria-hidden />}
              Refresh
            </Button>
          </div>

          <div className="mb-4 flex flex-col gap-1.5">
            {syncing && (
              <p className="micro text-thinking" role="status">
                local · Reading your connected accounts
              </p>
            )}

            {accounts.length === 0 && !syncing && (
              <p className="micro text-muted-foreground">
                Nothing is connected yet · connect an account in Settings
              </p>
            )}

            {accounts.map((account) => (
              <div key={account.accountId} className="flex flex-wrap items-center gap-2">
                <span className="micro text-muted-foreground">{account.source}</span>
                <Freshness state={account} />
                {account.errorMessage !== null && (
                  <span className="text-[11px] leading-snug text-muted-foreground">
                    {account.errorMessage}
                  </span>
                )}
              </div>
            ))}
          </div>

          {syncError !== null && (
            <div
              className="mb-4 rounded-md border border-destructive bg-destructive-surface px-3.5 py-3 text-destructive-text"
              role="alert"
            >
              <h2 className="text-[13px] font-semibold">Could not read your accounts</h2>
              <p className="mt-1 text-[13px] leading-snug">{syncError}</p>
            </div>
          )}

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
