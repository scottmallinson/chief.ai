import { cva, type VariantProps } from 'class-variance-authority';
import type * as React from 'react';

import { cn } from '@/lib/utils';

/**
 * Chips carry state, never actions.
 *
 * A tinted fill with dark text, rather than a solid brand colour behind small
 * type — 11px on #3F6B4F is not readable and would make the palette shout. One
 * amber chip per screen at most: if two things need you, the second is not
 * urgent.
 */
const chipVariants = cva(
  // `max-w-full` and `overflow-hidden` are the last line of defence, not the
  // design: a chip is a label and its text is short by construction. But it is
  // `whitespace-nowrap`, so text nobody expected spills out of whatever row it
  // is in rather than wrapping — which is how one work-log row put a scrollbar
  // on the whole window. Clipping is an ugly chip; the alternative was a
  // broken screen.
  'inline-flex max-w-full min-w-0 items-center gap-1.5 overflow-hidden rounded-sm px-[9px] py-[3px] text-[11px] font-semibold whitespace-nowrap',
  {
    variants: {
      tone: {
        verified: 'bg-verified-surface text-verified-text',
        local: 'bg-thinking-surface text-thinking-text',
        attention: 'bg-attention-surface text-attention-text',
        destructive: 'bg-destructive-surface text-destructive-text',
        /** A machine fact: a model name, a port, an identifier. */
        machine: 'bg-muted font-mono font-normal text-muted-foreground',
        /** Nothing is going on here. An outline, so it recedes. */
        quiet: 'border border-border font-normal text-muted-foreground',
      },
    },
    defaultVariants: { tone: 'quiet' },
  },
);

type ChipProps = React.ComponentProps<'span'> &
  VariantProps<typeof chipVariants> & {
    /** Lead with a solid dot in the tone's full-strength colour. */
    dot?: boolean;
  };

const DOTS = {
  verified: 'bg-verified',
  local: 'bg-thinking',
  attention: 'bg-attention',
  destructive: 'bg-destructive',
  machine: 'bg-muted-foreground',
  quiet: 'bg-muted-foreground',
} as const;

function Chip({ className, tone, dot = false, children, ...props }: ChipProps) {
  return (
    <span className={cn(chipVariants({ tone, className }))} {...props}>
      {dot && <span className={cn('size-[5px] rounded-full', DOTS[tone ?? 'quiet'])} aria-hidden />}
      {children}
    </span>
  );
}

export { Chip, chipVariants, type ChipProps };
