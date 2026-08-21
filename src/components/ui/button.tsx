import { Slot } from '@radix-ui/react-slot';
import { cva, type VariantProps } from 'class-variance-authority';
import type * as React from 'react';

import { cn } from '@/lib/utils';

/**
 * Buttons are 6px rectangles at one of three heights: 30, 34, 44. No pills.
 * Weight 500 is reserved for exactly this — prose is 400 and anything that
 * leads is 600 — and icons are 15px, taking the text colour rather than
 * carrying one of their own.
 *
 * Focus is the 3px slate ring set once in `globals.css`, so it is the same
 * shape on every control and every platform.
 */
const buttonVariants = cva(
  "inline-flex items-center justify-center gap-2 rounded-md font-medium whitespace-nowrap transition-colors duration-[120ms] ease-instrument disabled:pointer-events-none disabled:opacity-40 [&_svg]:pointer-events-none [&_svg:not([class*='size-'])]:size-[15px] [&_svg]:shrink-0",
  {
    variants: {
      variant: {
        default: 'bg-primary text-primary-foreground hover:bg-primary/90',
        destructive: 'bg-destructive text-destructive-foreground hover:bg-destructive/90',
        outline: 'border border-input bg-background hover:bg-accent hover:text-accent-foreground',
        secondary: 'bg-secondary text-secondary-foreground hover:bg-secondary-hover',
        ghost: 'text-muted-foreground hover:bg-accent hover:text-accent-foreground',
        link: 'text-thinking underline underline-offset-[3px] hover:brightness-90',
      },
      size: {
        default: 'h-[34px] px-4 text-[13px]',
        sm: 'h-[30px] gap-[7px] px-3 text-xs',
        lg: 'h-11 gap-[9px] px-5 text-[15px]',
        icon: 'size-[34px]',
      },
    },
    defaultVariants: {
      variant: 'default',
      size: 'default',
    },
  },
);

type ButtonProps = React.ComponentProps<'button'> &
  VariantProps<typeof buttonVariants> & {
    /** Render the child element instead of a `<button>`, keeping the styles. */
    asChild?: boolean;
  };

function Button({ className, variant, size, asChild = false, ...props }: ButtonProps) {
  const Comp = asChild ? Slot : 'button';
  return (
    <Comp
      data-slot="button"
      className={cn(buttonVariants({ variant, size, className }))}
      {...props}
    />
  );
}

export { Button, buttonVariants, type ButtonProps };
