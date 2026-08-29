import { cn } from '@/lib/utils';
import type { BriefDay } from '@/lib/brief';

interface BriefListProps {
  days: BriefDay[];
  /** The day showing in the detail, or null before one has been read. */
  selected: string | null;
  onSelect: (date: string) => void;
}

const dayFormat = new Intl.DateTimeFormat(undefined, { weekday: 'long' });
const dateFormat = new Intl.DateTimeFormat(undefined, { day: 'numeric', month: 'short' });

/** Parsed as local midnight: `new Date('2026-08-29')` is a day early out west. */
function parse(date: string): Date {
  const [year, month, day] = date.split('-').map(Number);

  return new Date(year ?? 0, (month ?? 1) - 1, day ?? 1);
}

/**
 * The days that have a brief, newest first — the list pane's whole content.
 *
 * Dates are mono because they are machine facts, and the weekday is not: a
 * person reads "Saturday" and scans "29 Aug".
 */
export function BriefList({ days, selected, onSelect }: BriefListProps) {
  return (
    <nav aria-label="Briefs" className="p-2">
      <h2 className="px-2 py-2 micro text-muted-foreground">Briefs</h2>

      {days.length === 0 ? (
        <p className="px-2 text-[13px] leading-snug text-muted-foreground">
          None written yet. Today’s will be the first.
        </p>
      ) : (
        <ul className="flex flex-col gap-0.5">
          {days.map((day) => {
            const when = parse(day.date);
            const isSelected = day.date === selected;

            return (
              <li key={day.date}>
                <button
                  type="button"
                  onClick={() => onSelect(day.date)}
                  aria-current={isSelected ? 'true' : undefined}
                  className={cn(
                    'flex w-full flex-col items-start gap-0.5 rounded-md px-2 py-2 text-left transition-colors duration-[120ms] ease-instrument',
                    isSelected
                      ? 'bg-sidebar-accent text-sidebar-accent-foreground'
                      : 'text-muted-foreground hover:bg-accent hover:text-accent-foreground',
                  )}
                >
                  <span className="text-[13px] font-semibold tracking-[-0.015em]">
                    {Number.isNaN(when.getTime()) ? day.date : dayFormat.format(when)}
                  </span>
                  <time className="font-mono text-[11px]" dateTime={day.date}>
                    {Number.isNaN(when.getTime()) ? '' : dateFormat.format(when)}
                  </time>
                </button>
              </li>
            );
          })}
        </ul>
      )}
    </nav>
  );
}
