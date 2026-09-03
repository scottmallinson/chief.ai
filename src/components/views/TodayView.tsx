import { CalendarDays, PenLine, RefreshCw } from 'lucide-react';

import { Button } from '@/components/ui/button';
import { Chip } from '@/components/ui/chip';
import { Dots } from '@/components/ui/activity';
import { ProposalCard } from '@/components/ProposalCard';
import { useWorkLog } from '@/hooks/use-work-log';
import type { Proposal } from '@/lib/proposals';
import { readBrief, type Brief, type BriefBlock } from '@/lib/brief';
import { OpenSource } from '@/components/OpenSource';
import type { WorkLogEntry } from '@/lib/work-log';

interface TodayViewProps {
  /** The brief on screen: today's, or whichever day the list pane selected. */
  brief: Brief | null;
  /** Which day is selected. A day is always selected, brief or no brief. */
  day: string;
  /** Today, so this view can tell "no brief yet" from "an earlier day". */
  today: string;
  status: 'loading' | 'ready' | 'writing' | 'error';
  error: string | null;
  /** Write today's brief. Costs a model call, so it is always a deliberate act. */
  onWrite: () => void;
  /** Come back to today from an earlier day. */
  onShowToday: () => void;
  /** The drafts Chief has prepared. Nothing here has been sent. */
  proposals: Proposal[];
  /** Open one in the drawer to change it. */
  onEditProposal: (proposal: Proposal) => void;
  onProposalDismissed: (id: number) => void;
}

/** How much of the log counts as "recently", beside a brief about today. */
const RECENT = 5;

const dayFormat = new Intl.DateTimeFormat(undefined, { dateStyle: 'full' });
const timeFormat = new Intl.DateTimeFormat(undefined, { dateStyle: 'medium', timeStyle: 'short' });

function formatDay(date: string): string {
  // Parsed as a local midnight rather than as UTC: `new Date('2026-08-29')` is
  // midnight UTC, which is the day before for anybody west of Greenwich.
  const [year, month, day] = date.split('-').map(Number);
  const parsed = new Date(year ?? 0, (month ?? 1) - 1, day ?? 1);

  return Number.isNaN(parsed.getTime()) ? date : dayFormat.format(parsed);
}

function formatTimestamp(timestamp: string): string {
  const parsed = new Date(timestamp);
  return Number.isNaN(parsed.getTime()) ? timestamp : timeFormat.format(parsed);
}

/** One piece of the brief, at the sizes the design system allows. */
function Block({ block }: { block: BriefBlock }) {
  if (block.kind === 'heading') {
    return <h3 className="mt-5 text-[15px] font-semibold tracking-[-0.015em]">{block.text}</h3>;
  }

  if (block.kind === 'paragraph') {
    return (
      <p className="mt-3 text-sm leading-relaxed" data-selectable>
        {block.text}
      </p>
    );
  }

  return (
    <ul className="mt-3 flex flex-col gap-2.5">
      {block.items.map((item) => (
        <li key={item} className="flex gap-2.5 text-sm leading-relaxed" data-selectable>
          <span className="mt-[9px] size-[3px] shrink-0 rounded-full bg-thinking" aria-hidden />
          {item}
        </li>
      ))}
    </ul>
  );
}

function Shipped({ entry }: { entry: WorkLogEntry }) {
  return (
    <li className="border-l-2 border-border pl-3.5">
      <p className="flex items-center gap-2 micro text-muted-foreground">
        <span>
          {entry.source} · {formatTimestamp(entry.timestamp)}
        </span>
        <OpenSource url={entry.url} label={entry.summary ?? entry.content} />
      </p>
      <p className="mt-1.5 text-[13px] leading-snug" data-selectable>
        {entry.summary ?? entry.content}
      </p>
    </li>
  );
}

/**
 * Today: the brief, and what has happened since the last one.
 *
 * The brief is the product's flagship output and until now it was a file in a
 * folder — written every day, read by nobody who did not go looking for it.
 * This is the screen that makes it the first thing the app says.
 *
 * The brief itself is owned by `App`, because the list pane beside this view is
 * a sibling in the shell rather than a child of it, and both have to be looking
 * at the same day.
 */
export function TodayView({
  brief,
  day,
  today,
  status,
  error,
  onWrite,
  onShowToday,
  proposals,
  onEditProposal,
  onProposalDismissed,
}: TodayViewProps) {
  const { entries } = useWorkLog();

  const isWriting = status === 'writing';
  const isToday = day === today;
  const blocks = brief === null ? [] : readBrief(brief.markdown);

  return (
    <div className="h-full overflow-y-auto">
      <div className="px-7 py-6">
        <div className="max-w-[680px]">
          {status === 'loading' && (
            <p className="micro text-muted-foreground" role="status">
              Reading today’s brief
            </p>
          )}

          {error !== null && (
            <div
              className="rounded-md border border-destructive bg-destructive-surface px-3.5 py-3 text-destructive-text"
              role="alert"
            >
              <h2 className="text-[13px] font-semibold">Could not write the brief</h2>
              <p className="mt-1 text-[13px] leading-snug">{error}</p>
            </div>
          )}

          {brief !== null && (
            <section aria-labelledby="brief-day">
              <div className="flex flex-wrap items-center gap-2.5">
                <h2 id="brief-day" className="text-xl font-semibold tracking-[-0.015em]">
                  {formatDay(brief.date)}
                </h2>
                {brief.sources.map((source) => (
                  <Chip key={source} tone="machine">
                    {source}
                  </Chip>
                ))}
              </div>

              <p className="mt-1 font-mono text-[11px] text-muted-foreground" data-selectable>
                {brief.path}
              </p>

              <div className="mt-3.5 flex flex-wrap items-center gap-2">
                {isToday ? (
                  <Button variant="outline" size="sm" onClick={onWrite} disabled={isWriting}>
                    {isWriting ? <Dots /> : <RefreshCw aria-hidden />}
                    Write it again
                  </Button>
                ) : (
                  <Button variant="outline" size="sm" onClick={onShowToday}>
                    <CalendarDays aria-hidden />
                    Back to today
                  </Button>
                )}
              </div>

              {blocks.map((block, index) => (
                <Block key={index} block={block} />
              ))}
            </section>
          )}

          {brief === null && status !== 'loading' && (
            <div className="rounded-lg border border-dashed border-input p-6 text-center">
              <h2 className="text-[15px] font-semibold tracking-[-0.015em]">
                {isToday ? 'No brief for today' : 'That day has no brief'}
              </h2>
              <p className="mx-auto mt-1.5 max-w-[320px] text-[13px] leading-snug text-muted-foreground">
                Chief reads your calendar, your pull requests and your work log, and writes the
                brief here on this machine.
              </p>
              {isToday ? (
                <Button className="mt-4" onClick={onWrite} disabled={isWriting}>
                  {isWriting ? <Dots /> : <PenLine aria-hidden />}
                  Write today’s brief
                </Button>
              ) : (
                <Button className="mt-4" variant="outline" onClick={onShowToday}>
                  <CalendarDays aria-hidden />
                  Back to today
                </Button>
              )}
            </div>
          )}

          {isWriting && (
            <p className="mt-4 micro text-verified-text" role="status">
              local · Writing your brief
            </p>
          )}

          {proposals.length > 0 && (
            <section aria-labelledby="proposed" className="mt-8 border-t border-border pt-6">
              <h2 id="proposed" className="micro text-muted-foreground">
                Drafted for you · nothing sent
              </h2>
              <ul className="mt-3.5 flex flex-col gap-3">
                {proposals.map((proposal) => (
                  <ProposalCard
                    key={proposal.id}
                    proposal={proposal}
                    onEdit={onEditProposal}
                    onDismissed={onProposalDismissed}
                  />
                ))}
              </ul>
            </section>
          )}

          {entries.length > 0 && (
            <section aria-labelledby="recent" className="mt-8 border-t border-border pt-6">
              <h2 id="recent" className="micro text-muted-foreground">
                Recently logged
              </h2>
              <ul className="mt-3.5 flex flex-col gap-4">
                {entries.slice(0, RECENT).map((entry) => (
                  <Shipped key={entry.id} entry={entry} />
                ))}
              </ul>
            </section>
          )}
        </div>
      </div>
    </div>
  );
}
