import { cn } from '@/lib/utils';

/**
 * The four things that are allowed to move.
 *
 * Answer speed belongs to the hardware; showing effort belongs to us. From the
 * moment a request is dispatched to the last token written there is always one
 * quiet thing moving, because motion is the only honest progress bar available
 * when the total is unknowable. A loop must mean work is in flight — if nothing
 * is running, nothing moves — and indicators are slate or green, never amber:
 * amber means *you* are needed, and a machine working is not that.
 *
 * `prefers-reduced-motion` swaps each loop for a static dot and the same
 * caption. That is CSS, not React: see `.motion-loop` / `.motion-still`.
 */

/**
 * Three slate dots on a control the user is waiting on. Staggered rather than
 * spun: the system has no spinner, because a spinner implies a rate.
 */
export function Dots() {
  return (
    <span className="inline-flex items-center gap-1" aria-hidden>
      {[0, 150, 300].map((delay) => (
        <span
          key={delay}
          className="size-[5px] animate-dot rounded-full bg-thinking"
          style={{ animationDelay: `${delay}ms` }}
        />
      ))}
    </span>
  );
}

/**
 * A slate hairline sweeping every 1.4s: work is in flight and we do not know
 * how much is left. Never a percentage of a total we do not have.
 */
export function Sweep({ className }: { className?: string }) {
  return (
    <>
      <span
        className={cn(
          'motion-loop inline-flex h-[3px] w-11 overflow-hidden rounded-full bg-muted',
          className,
        )}
        aria-hidden
      >
        <span className="block h-full w-1/3 animate-sweep rounded-full bg-thinking" />
      </span>
      <span className="motion-still size-1.5 shrink-0 rounded-full bg-thinking" aria-hidden />
    </>
  );
}

/** A 1px caret at the live end of the text. It goes with the final token. */
export function Caret() {
  return (
    <span
      className="ml-0.5 inline-block h-[15px] w-px animate-caret bg-current align-middle"
      aria-hidden
    />
  );
}
