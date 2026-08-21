import { useEffect, useState } from 'react';

/**
 * Whole seconds since `running` last became true, or 0 while it is false.
 *
 * A slow machine stated plainly is fine; silence is not. Past three seconds a
 * caption gains the elapsed time, so a wait that is taking a while says so
 * instead of looking like a hang.
 */
export function useElapsed(running: boolean): number {
  const [seconds, setSeconds] = useState(0);

  useEffect(() => {
    if (!running) {
      setSeconds(0);
      return;
    }

    const started = Date.now();
    const timer = setInterval(() => {
      setSeconds(Math.floor((Date.now() - started) / 1000));
    }, 1000);

    return () => clearInterval(timer);
  }, [running]);

  return seconds;
}
