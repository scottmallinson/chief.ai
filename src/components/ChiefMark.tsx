import { cn } from '@/lib/utils';

/** How the tile and the arc are coloured. */
export type MarkTone = 'default' | 'thinking';

interface ChiefMarkProps {
  /** Edge length in pixels. The system draws it at 64, 40, 32, 24 and 16. */
  size?: number;
  tone?: MarkTone;
  /**
   * Turn half a revolution every 2.4s: the request reached the daemon, and no
   * text exists yet to say so.
   */
  breathing?: boolean;
  className?: string;
}

/**
 * The open arc, on a tile of radius 8.
 *
 * The stroke thickens from 3 to 4 units as the tile shrinks, so the arc still
 * reads in the dock, the tray and a favicon. Never outline it, tilt it, or
 * close the arc into a C.
 */
export function ChiefMark({
  size = 32,
  tone = 'default',
  breathing = false,
  className,
}: ChiefMarkProps) {
  // 3 units at 64px, 4 at 16px, straight-line between.
  const stroke = Math.min(4, Math.max(3, 4 - ((size - 16) / 48) * 1));

  // Graphite on paper in the light theme; paper on graphite in the dark one,
  // which is the reversed mark the system asks for, with no second component.
  const tile = tone === 'thinking' ? 'fill-muted' : 'fill-foreground';
  const arc = tone === 'thinking' ? 'stroke-thinking' : 'stroke-background';

  return (
    <svg
      width={size}
      height={size}
      viewBox="0 0 32 32"
      aria-hidden
      className={cn('shrink-0', breathing && 'motion-loop animate-breathe', className)}
    >
      <rect width="32" height="32" rx="8" className={tile} />
      <path
        d="M21.5 11.3a7 7 0 1 0 0 9.4"
        fill="none"
        strokeWidth={stroke}
        strokeLinecap="round"
        className={arc}
      />
    </svg>
  );
}
