import { cn } from '@/lib/utils';
import type { BriefDay } from '@/lib/brief';

interface BriefListProps {
  days: BriefDay[];
  /** The day showing in the detail. Always a day, never nothing. */
  selected: string;
  /** Today, which is named rather than dated and may have no brief yet. */
  today: string;
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
 * The days a brief can be read for, newest first — the list pane's whole
 * content.
 *
 * Dates are mono because they are machine facts, and the weekday is not: a
 * person reads "Saturday" and scans "29 Aug".
 *
 * Today is always here, named rather than given its weekday, and says "not
 * written" until it has been. It is the row that has to exist: a list of the
 * files that exist has nothing to click on the day whose file does not.
 */
export function BriefList({ days, selected, today, onSelect }: BriefListProps) {
  return (
    <nav aria-label="Briefs" className="p-2">
      <h2 className="px-2 py-2 micro text-muted-foreground">Briefs</h2>

      <ul className="flex flex-col gap-0.5">
        {days.map((day) => {
          const when = parse(day.date);
          const isSelected = day.date === selected;
          const isToday = day.date === today;

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
                  {isToday
                    ? 'Today'
                    : Number.isNaN(when.getTime())
                      ? day.date
                      : dayFormat.format(when)}
                </span>
                <time className="font-mono text-[11px]" dateTime={day.date}>
                  {Number.isNaN(when.getTime()) ? '' : dateFormat.format(when)}
                  {day.written ? '' : ' · not written'}
                </time>
              </button>
            </li>
          );
        })}
      </ul>
    </nav>
  );
}
