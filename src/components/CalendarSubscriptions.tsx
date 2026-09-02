import { useState, type FormEvent } from 'react';
import { CalendarPlus } from 'lucide-react';

import { DisconnectAccount } from '@/components/DisconnectAccount';
import { Freshness } from '@/components/Freshness';
import { useSyncState } from '@/hooks/use-sync-state';
import { Button } from '@/components/ui/button';
import { Dots } from '@/components/ui/activity';
import { addCalendar, disconnect, type Account } from '@/lib/integrations';

/** How long a name may be, matching the one on a connected account. */
const NAME_LIMIT = 40;

interface CalendarSubscriptionsProps {
  /** The calendar subscriptions already saved. */
  accounts: Account[];
  /** Re-read the connections once one has been added or removed. */
  onChanged: () => void;
}

/**
 * Calendars subscribed to by address rather than signed in to.
 *
 * The whole point of this over OAuth is that there is nothing to register and
 * nobody to ask, so the interface is a field and a button rather than a flow.
 *
 * **The address is never shown once it is saved.** A subscription link grants
 * read access to somebody's entire calendar to anyone holding it — it is a
 * bearer credential that happens to look like a URL, and a settings screen that
 * prints it hands the calendar to whoever is looking at the laptop.
 */
export function CalendarSubscriptions({ accounts, onChanged }: CalendarSubscriptionsProps) {
  const { states } = useSyncState();
  const [url, setUrl] = useState('');
  const [label, setLabel] = useState('');
  const [busy, setBusy] = useState<number | 'adding' | null>(null);
  const [error, setError] = useState<string | null>(null);
  const [added, setAdded] = useState<string | null>(null);

  function subscribe(event: FormEvent) {
    event.preventDefault();
    if (url.trim() === '' || busy !== null) return;

    setBusy('adding');
    setError(null);
    setAdded(null);

    addCalendar(url.trim(), label.trim() === '' ? null : label.trim())
      .then((account) => {
        setAdded(account.identity ?? 'that calendar');
        setUrl('');
        setLabel('');
        onChanged();
      })
      .catch((cause: unknown) => {
        setError(cause instanceof Error ? cause.message : String(cause));
      })
      .finally(() => setBusy(null));
  }

  function remove(account: Account) {
    setBusy(account.id);
    setError(null);

    disconnect(account.id)
      .then(() => onChanged())
      .catch((cause: unknown) => {
        setError(cause instanceof Error ? cause.message : String(cause));
      })
      .finally(() => setBusy(null));
  }

  return (
    <>
      {accounts.length > 0 && (
        <ul className="mt-3.5 flex flex-col gap-2">
          {accounts.map((account) => {
            const name = account.label ?? account.identity ?? 'Calendar';

            return (
              <li
                key={account.id}
                className="flex items-center justify-between gap-3 rounded-md border border-border px-3 py-2"
              >
                <span className="flex min-w-0 flex-1 flex-col gap-1">
                  <span className="truncate text-sm">{name}</span>
                  {/* The card's own form below is the way back in, so no
                      reconnect button here: the key or the address is
                      re-entered where it was entered. */}
                  <Freshness state={states.get(account.id)} />
                </span>
                <DisconnectAccount
                  accountId={account.id}
                  name={name}
                  verb="Remove"
                  leaving={busy !== null}
                  onConfirm={() => remove(account)}
                />
              </li>
            );
          })}
        </ul>
      )}

      <form className="mt-3.5 flex flex-col gap-2" onSubmit={subscribe}>
        <label className="micro text-muted-foreground" htmlFor="calendar-url">
          Calendar address
        </label>
        <input
          id="calendar-url"
          type="text"
          value={url}
          onChange={(event) => setUrl(event.target.value)}
          placeholder="https://…/basic.ics"
          spellCheck={false}
          autoComplete="off"
          className="w-full rounded-md border border-input bg-background px-3 py-2 font-mono text-[13px]"
        />

        <label className="mt-1 micro text-muted-foreground" htmlFor="calendar-label">
          Name for this calendar
        </label>
        <input
          id="calendar-label"
          type="text"
          value={label}
          maxLength={NAME_LIMIT}
          onChange={(event) => setLabel(event.target.value)}
          placeholder="Work"
          className="w-full rounded-md border border-input bg-background px-3 py-2 text-sm"
        />

        <p className="mt-1 text-[13px] leading-snug text-muted-foreground">
          In Google Calendar this is the <strong>secret address in iCal format</strong>; in Outlook
          it is a published calendar link. Treat it like a password — anyone with it can read this
          calendar, so Chief stores it the way it stores a token and never shows it again.
        </p>

        {error !== null && (
          <p className="text-[13px] leading-snug text-attention-text" role="alert">
            {error}
          </p>
        )}

        {busy === 'adding' && (
          <p className="flex items-center gap-2 micro text-verified-text" role="status">
            <Dots />
            Checking that calendar
          </p>
        )}

        {added !== null && (
          <p className="micro text-verified-text" role="status">
            {`Subscribed to ${added}`}
          </p>
        )}

        <div className="mt-1 flex">
          <Button type="submit" size="sm" disabled={url.trim() === '' || busy !== null}>
            <CalendarPlus aria-hidden />
            Subscribe
          </Button>
        </div>
      </form>
    </>
  );
}
