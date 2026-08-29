import { useCallback, useEffect, useState } from 'react';

import { briefDays, generateBrief, todaysBrief, type Brief, type BriefDay } from '@/lib/brief';
import { listCorpus, readCorpusFile } from '@/lib/corpus';

type Status = 'loading' | 'ready' | 'writing' | 'error';

interface UseBrief {
  /** The brief on screen: today's to begin with, or whichever day was picked. */
  brief: Brief | null;
  /** Every day that has a brief, newest first. */
  days: BriefDay[];
  status: Status;
  error: string | null;
  /** Show an earlier day, read from the corpus. */
  select: (date: string) => void;
  /** Write today's brief now. Costs a model call, so it is never automatic. */
  write: () => void;
}

/**
 * The briefs: today's, the days before it, and the one being read.
 *
 * Reading and writing are deliberately separate. Opening the screen reads the
 * file; only the button writes one, because writing is a model call and a
 * screen that silently spends one every time it is opened is a screen nobody
 * can afford to glance at.
 *
 * The day list comes from the corpus listing rather than from a command of its
 * own, so a brief the user moved or deleted in the folder simply is not there —
 * Chief does not keep a second opinion about which files exist.
 */
export function useBrief(): UseBrief {
  const [brief, setBrief] = useState<Brief | null>(null);
  const [days, setDays] = useState<BriefDay[]>([]);
  const [status, setStatus] = useState<Status>('loading');
  const [error, setError] = useState<string | null>(null);

  // The listing is a nicety and the brief is the point, so a corpus that will
  // not list leaves the day list empty rather than emptying the screen.
  const refreshDays = useCallback(
    () =>
      listCorpus()
        .then((entries) => setDays(briefDays(entries)))
        .catch(() => setDays([])),
    [],
  );

  useEffect(() => {
    let cancelled = false;

    Promise.all([todaysBrief(), refreshDays()])
      .then(([found]) => {
        if (cancelled) return;
        setBrief(found);
        setStatus('ready');
      })
      .catch((cause: unknown) => {
        if (cancelled) return;
        setError(message(cause));
        setStatus('error');
      });

    return () => {
      cancelled = true;
    };
  }, [refreshDays]);

  const select = useCallback(
    (date: string) => {
      const day = days.find((candidate) => candidate.date === date);
      if (day === undefined) return;

      setError(null);

      readCorpusFile(day.path)
        .then((markdown) => {
          setBrief({ date: day.date, path: day.path, markdown, sources: [] });
          setStatus('ready');
        })
        .catch((cause: unknown) => {
          // The brief already on screen stays there: losing what you were
          // reading is a worse answer than a message saying the other day
          // could not be read.
          setError(message(cause));
          setStatus('error');
        });
    },
    [days],
  );

  const write = useCallback(() => {
    setStatus('writing');
    setError(null);

    generateBrief()
      .then(async (written) => {
        setBrief(written);
        setStatus('ready');
        await refreshDays();
      })
      .catch((cause: unknown) => {
        setError(message(cause));
        setStatus('error');
      });
  }, [refreshDays]);

  return { brief, days, status, error, select, write };
}

/** Tauri rejects with a string; anything else may be a real Error. */
function message(cause: unknown): string {
  return cause instanceof Error ? cause.message : String(cause);
}
