import { useEffect, useRef, type KeyboardEvent, type ReactNode } from 'react';
import { X } from 'lucide-react';

interface DrawerProps {
  open: boolean;
  /** Names the dialog. Read out on open, and shown at the top of the panel. */
  title: string;
  onClose: () => void;
  children: ReactNode;
}

/** Everything that can hold focus, in the order Tab visits it. */
const FOCUSABLE = [
  'a[href]',
  'button:not([disabled])',
  'textarea:not([disabled])',
  'input:not([disabled])',
  'select:not([disabled])',
  '[tabindex]:not([tabindex="-1"])',
].join(',');

/**
 * A panel over the shell, not beside it.
 *
 * It **overlays**: the detail column behind is exactly as wide with the drawer
 * open as with it closed, which is the whole reason chat moved here rather than
 * staying a destination. A panel that pushes reflows the reading measure every
 * time somebody asks a question, and the answer they are reading moves under
 * them. One shadow level over a scrim, per the design system's single elevation.
 *
 * Focus is trapped while it is open and handed back to whatever opened it when
 * it closes — a drawer that dumps focus on `body` leaves a keyboard user at the
 * top of the document with no way back to where they were.
 */
export function Drawer({ open, title, onClose, children }: DrawerProps) {
  const panel = useRef<HTMLDivElement>(null);
  // Whatever had focus when this opened, so it can be given back.
  const opener = useRef<HTMLElement | null>(null);

  useEffect(() => {
    if (!open) return;

    opener.current = document.activeElement as HTMLElement | null;
    // The close button rather than the composer: focus lands somewhere that
    // says what this is and how to leave it, and Tab reaches the rest at once.
    panel.current?.querySelector<HTMLElement>(FOCUSABLE)?.focus();

    return () => {
      opener.current?.focus();
      opener.current = null;
    };
  }, [open]);

  if (!open) return null;

  function handleKeyDown(event: KeyboardEvent<HTMLDivElement>) {
    if (event.key === 'Escape') {
      event.stopPropagation();
      onClose();
      return;
    }

    if (event.key !== 'Tab') return;

    const stops = [...(panel.current?.querySelectorAll<HTMLElement>(FOCUSABLE) ?? [])];
    if (stops.length === 0) return;

    const first = stops[0];
    const last = stops[stops.length - 1];
    const leaving = document.activeElement === (event.shiftKey ? first : last);

    // Only the two ends need handling; everything between them is the browser's
    // own tab order, which is the one a user's settings and extensions expect.
    if (!leaving) return;

    event.preventDefault();
    (event.shiftKey ? last : first)?.focus();
  }

  return (
    <div className="fixed inset-0 z-40 flex justify-end">
      <div
        data-testid="drawer-scrim"
        onClick={onClose}
        aria-hidden
        className="bg-graphite/40 absolute inset-0"
      />

      <div
        ref={panel}
        role="dialog"
        aria-modal="true"
        aria-label={title}
        onKeyDown={handleKeyDown}
        className="relative flex h-full w-full max-w-[520px] flex-col border-l border-border bg-background shadow-[0_8px_24px_rgb(20_23_26_/_0.10)] motion-safe:animate-drawer"
      >
        <header className="flex h-12 shrink-0 items-center justify-between gap-4 border-b border-border pr-3 pl-5">
          <h2 className="truncate text-base font-semibold tracking-[-0.015em]">{title}</h2>
          <button
            type="button"
            onClick={onClose}
            className="flex size-8 shrink-0 items-center justify-center rounded-md text-muted-foreground transition-colors duration-[120ms] ease-instrument hover:bg-accent hover:text-accent-foreground"
          >
            <X className="size-[15px]" aria-hidden />
            <span className="sr-only">Close</span>
          </button>
        </header>

        <div className="min-h-0 flex-1 overflow-hidden">{children}</div>
      </div>
    </div>
  );
}
