import { useCallback, useEffect, useState } from 'react';

import {
  briefDays,
  generateBrief,
  todaysBrief,
  todayDate,
  type Brief,
  type BriefDay,
} from '@/lib/brief';
import { listCorpus, readCorpusFile } from '@/lib/corpus';

type Status = 'loading' | 'ready' | 'writing' | 'error';

interface UseBrief {
  /** The brief on screen. Null when the selected day has none written yet. */
  brief: Brief | null;
  /** Every day that can be shown, newest first. Today is always among them. */
  days: BriefDay[];
  /** Today, as a brief is filed under it. */
  today: string;
  /** The day being shown, which is a day whether or not it has a brief. */
  selected: string;
  status: Status;
  error: string | null;
  /** Show a day: today re-read from the corpus, or an earlier one. */
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
 * Chief does not keep a second opinion about which files exist. Today is the
 * one exception and is always in the list: see [`briefDays`].
 *
 * **The selected day is held separately from the brief.** They used to be the
 * same thing — the list pane highlighted `brief.date` — which meant a day with
 * no brief could not be the selected day, and today, before one is written, is
 * exactly that. Selecting today then left nothing selected and nothing on
 * screen, and there was no way back to the button that writes one.
 */
export function useBrief(): UseBrief {
  // Read once per mount rather than per render: two renders either side of
  // midnight would otherwise disagree about which day is selected.
  const [today] = useState(todayDate);
  const [selected, setSelected] = useState(today);
  const [brief, setBrief] = useState<Brief | null>(null);
  const [days, setDays] = useState<BriefDay[]>([]);
  const [status, setStatus] = useState<Status>('loading');
  const [error, setError] = useState<string | null>(null);

  // The listing is a nicety and the brief is the point, so a corpus that will
  // not list leaves the day list holding today alone rather than emptying the
  // screen.
  const refreshDays = useCallback(
    () =>
      listCorpus()
        .then((entries) => setDays(briefDays(entries, today)))
        .catch(() => setDays(briefDays([], today))),
    [today],
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
      setError(null);

      // Today is read through the command rather than as a file, so it comes
      // back with the sources it was written from — and so selecting it is the
      // way to pick up a brief the daemon wrote while the window was open.
      if (date === today) {
        setSelected(date);

        todaysBrief()
          .then((found) => {
            setBrief(found);
            setStatus('ready');
          })
          .catch((cause: unknown) => {
            setError(message(cause));
            setStatus('error');
          });

        void refreshDays();
        return;
      }

      const day = days.find((candidate) => candidate.date === date);
      if (day === undefined) return;

      readCorpusFile(day.path)
        .then((markdown) => {
          setSelected(date);
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
    [days, refreshDays, today],
  );

  const write = useCallback(() => {
    setStatus('writing');
    setError(null);

    generateBrief()
      .then(async (written) => {
        setSelected(written.date);
        setBrief(written);
        setStatus('ready');
        await refreshDays();
      })
      .catch((cause: unknown) => {
        setError(message(cause));
        setStatus('error');
      });
  }, [refreshDays]);

  return { brief, days, today, selected, status, error, select, write };
}

/** Tauri rejects with a string; anything else may be a real Error. */
function message(cause: unknown): string {
  return cause instanceof Error ? cause.message : String(cause);
}
