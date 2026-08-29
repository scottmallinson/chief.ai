import { useCallback, useEffect, useState } from 'react';
import { PenLine } from 'lucide-react';

import { Button } from '@/components/ui/button';
import { Dots } from '@/components/ui/activity';
import { bootstrapProfile, profilePlan, type ProfilePlan, type ProfileReport } from '@/lib/profile';

type Status = 'loading' | 'ready' | 'working' | 'error';

function count(files: number): string {
  return files === 1 ? '1 file' : `${files} files`;
}

/**
 * Seeding the profile from the user's own work, with their consent.
 *
 * This is the first feature that reads the user's own writing in bulk in order
 * to build a profile of it. It stays on the machine like everything else, but
 * that is not on its own enough: the screen says **what will be read and what
 * will be written, before it runs**, and nothing happens until the button is
 * pressed. A profile assembled quietly would be the wrong way to do this even
 * though nothing leaves the laptop.
 *
 * The output is a draft. Files the user has since edited are listed as left
 * alone rather than silently skipped, so "nothing happened to that one" always
 * has a visible reason.
 */
export function ProfileBootstrap() {
  const [plan, setPlan] = useState<ProfilePlan | null>(null);
  const [report, setReport] = useState<ProfileReport | null>(null);
  const [status, setStatus] = useState<Status>('loading');
  const [error, setError] = useState<string | null>(null);

  const load = useCallback(() => {
    profilePlan()
      .then((found) => {
        setPlan(found);
        setStatus('ready');
      })
      .catch((cause: unknown) => {
        setError(cause instanceof Error ? cause.message : String(cause));
        setStatus('error');
      });
  }, []);

  useEffect(() => load(), [load]);

  function draft() {
    setStatus('working');
    setError(null);

    bootstrapProfile()
      .then((written) => {
        setReport(written);
        // The plan is now stale: what was written is no longer a starter.
        load();
      })
      .catch((cause: unknown) => {
        setError(cause instanceof Error ? cause.message : String(cause));
        setStatus('error');
      });
  }

  const isWorking = status === 'working';

  return (
    <>
      {plan !== null && (
        <>
          <div className="mt-3.5">
            <h3 className="micro text-muted-foreground">What Chief would read</h3>
            <ul className="mt-2 flex flex-col gap-1.5">
              {plan.reads.map((line) => (
                <li key={line} className="flex gap-2.5 text-[13px] leading-snug">
                  <span
                    className="mt-[7px] size-[3px] shrink-0 rounded-full bg-thinking"
                    aria-hidden
                  />
                  {line}
                </li>
              ))}
            </ul>
          </div>

          {plan.writes.length > 0 && (
            <div className="mt-3.5">
              <h3 className="micro text-muted-foreground">What it would write</h3>
              <ul className="mt-2 flex flex-col gap-1">
                {plan.writes.map((path) => (
                  <li
                    key={path}
                    className="font-mono text-[11px] break-all text-muted-foreground"
                    data-selectable
                  >
                    {path}
                  </li>
                ))}
              </ul>
            </div>
          )}

          {plan.keeps.length > 0 && (
            <div className="mt-3.5">
              <h3 className="micro text-muted-foreground">
                You have edited these, so Chief will not touch them
              </h3>
              <ul className="mt-2 flex flex-col gap-1">
                {plan.keeps.map((path) => (
                  <li
                    key={path}
                    className="font-mono text-[11px] break-all text-muted-foreground"
                    data-selectable
                  >
                    {path}
                  </li>
                ))}
              </ul>
            </div>
          )}
        </>
      )}

      {/*
        Not conditioned on `status`: the plan is reloaded the moment seeding
        finishes, which puts the status back to 'ready' and used to take this
        sentence with it — the user saw the button return and no word of what
        had just happened.
      */}
      {report !== null && error === null && (
        <p className="mt-3.5 text-sm leading-relaxed text-verified-text" role="status">
          {`Drafted ${count(report.written.length)}. Open the corpus and make it yours — Chief will not write over what you change.`}
        </p>
      )}

      {error !== null && (
        <p className="mt-3.5 text-sm leading-relaxed text-attention-text" role="alert">
          {error}
        </p>
      )}

      {isWorking && (
        <p className="mt-3.5 flex items-center gap-2 micro text-verified-text" role="status">
          <Dots />
          local · Reading your work
        </p>
      )}

      {plan !== null && plan.writes.length > 0 && (
        <div className="mt-4 flex flex-wrap gap-2">
          <Button size="sm" onClick={draft} disabled={isWorking}>
            <PenLine aria-hidden />
            Draft these from my work
          </Button>
        </div>
      )}
    </>
  );
}
