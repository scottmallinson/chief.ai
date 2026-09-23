import { useEffect, useState } from 'react';

import { Button } from '@/components/ui/button';
import { closeWindow, onCloseQuestion, setKeepRunning, windowBehaviour } from '@/lib/background';

/**
 * Asked the first time the window is closed: keep Chief running, or quit?
 *
 * Asked rather than decided, because a process that keeps running after its
 * window has gone is something a person should know about rather than find
 * out about. Whatever they answer can be changed in Settings. There is no way
 * to dismiss it without answering: the window is already on its way out, and
 * both answers finish that.
 */
export function CloseQuestion() {
  const [trayName, setTrayName] = useState<string | null>(null);
  const [saving, setSaving] = useState(false);

  useEffect(() => {
    let stop: (() => void) | undefined;
    let cancelled = false;

    void onCloseQuestion(() => {
      // Named for this platform. If the name cannot be read the question is
      // still worth asking, so it falls back to the general word.
      windowBehaviour()
        .then((behaviour) => setTrayName(behaviour.trayName))
        .catch(() => setTrayName('system tray'));
    })
      .then((unlisten) => {
        if (cancelled) unlisten();
        else stop = unlisten;
      })
      .catch(() => undefined);

    return () => {
      cancelled = true;
      stop?.();
    };
  }, []);

  if (trayName === null) return null;

  const answer = async (keepRunning: boolean) => {
    setSaving(true);

    try {
      await setKeepRunning(keepRunning);
    } catch {
      // Not stored, so the question comes back next launch. The close still
      // goes ahead: the backend applies the default to an unanswered one.
    }

    setTrayName(null);
    setSaving(false);
    await closeWindow().catch(() => undefined);
  };

  return (
    <div className="fixed inset-0 z-50 flex items-center justify-center p-4">
      <div aria-hidden className="bg-graphite/40 absolute inset-0" />
      <div
        role="dialog"
        aria-modal="true"
        aria-labelledby="close-question-title"
        className="relative w-full max-w-[440px] rounded-xl border border-border bg-background p-5 shadow-[0_8px_24px_rgb(20_23_26_/_0.10)]"
      >
        <h2 id="close-question-title" className="text-base font-semibold tracking-[-0.015em]">
          Keep Chief running when the window closes?
        </h2>
        <p className="mt-2 text-sm leading-relaxed text-muted-foreground">
          Chief can stay in the {trayName} and keep your work log, today&apos;s brief and drafts up
          to date while the window is closed, so they are ready when you open it. It reads the same
          accounts it reads now and sends nothing anywhere new. The model gives its memory back
          after ten minutes of not being used.
        </p>
        <p className="mt-2 text-sm leading-relaxed text-muted-foreground">
          You can change this in Settings.
        </p>
        <div className="mt-4 flex flex-wrap justify-end gap-2">
          <Button variant="outline" size="sm" disabled={saving} onClick={() => void answer(false)}>
            Quit when closed
          </Button>
          <Button size="sm" disabled={saving} onClick={() => void answer(true)}>
            Keep running in the {trayName}
          </Button>
        </div>
      </div>
    </div>
  );
}
