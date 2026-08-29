import { MessageSquare, ShieldCheck } from 'lucide-react';

import { ChiefMark } from '@/components/ChiefMark';
import { cn } from '@/lib/utils';
import { NAV_ITEMS, type View } from '@/lib/navigation';

interface SidebarProps {
  activeView: View;
  onNavigate: (view: View) => void;
  /** Open the chat drawer. Not a destination — see `navigation.ts`. */
  onOpenChat: () => void;
}

/** One rail tile: a 32px square carrying a 15px icon and no label. */
const TILE =
  'flex size-8 items-center justify-center rounded-md transition-colors duration-[120ms] ease-instrument';

/** The name, arriving as a tooltip half a second after the pointer does. */
const TOOLTIP =
  'pointer-events-none absolute top-1/2 left-full z-20 ml-2 -translate-y-1/2 rounded-sm border border-border bg-popover px-2 py-1 text-xs whitespace-nowrap text-popover-foreground opacity-0 shadow-[0_8px_24px_rgb(20_23_26_/_0.10)] transition-opacity duration-[120ms] ease-instrument group-hover:opacity-100 group-hover:delay-500';

/**
 * The 56px icon rail.
 *
 * Labels never appear in the rail itself — each item is a 32px tile carrying a
 * 15px icon, and its name arrives as a tooltip after 500ms. The delay is a CSS
 * transition rather than the platform's own `title` timing, so it is the same
 * half-second on all three desktops.
 *
 * Chat sits below the destinations, separated by a rule: it is an action, not
 * a place, and pressing it opens the drawer over whatever you were reading
 * rather than taking you away from it.
 *
 * Purely presentational: the active view is owned by `App` so that later steps
 * can persist or deep-link it.
 */
export function Sidebar({ activeView, onNavigate, onOpenChat }: SidebarProps) {
  return (
    <aside className="flex w-14 shrink-0 flex-col items-center gap-2 border-r border-sidebar-border bg-sidebar py-3 text-sidebar-foreground">
      <ChiefMark size={26} className="mb-2.5" />

      <nav aria-label="Main" className="flex flex-col items-center gap-1.5">
        {NAV_ITEMS.map((item) => {
          const Icon = item.icon;
          const isActive = item.id === activeView;

          return (
            <div key={item.id} className="group relative flex">
              <button
                type="button"
                onClick={() => onNavigate(item.id)}
                aria-current={isActive ? 'page' : undefined}
                aria-label={item.label}
                className={cn(
                  TILE,
                  isActive
                    ? 'bg-sidebar-accent text-sidebar-accent-foreground'
                    : 'text-muted-foreground hover:bg-accent hover:text-accent-foreground',
                )}
              >
                <Icon className="size-[15px]" aria-hidden />
              </button>

              <span aria-hidden className={TOOLTIP}>
                {item.description}
              </span>
            </div>
          );
        })}
      </nav>

      <div className="group relative mt-1.5 flex border-t border-sidebar-border pt-3">
        <button
          type="button"
          onClick={onOpenChat}
          aria-label="Ask Chief"
          className={cn(TILE, 'text-muted-foreground hover:bg-accent hover:text-accent-foreground')}
        >
          <MessageSquare className="size-[15px]" aria-hidden />
        </button>

        <span aria-hidden className={cn(TOOLTIP, 'top-auto bottom-0 translate-y-0')}>
          Ask your chief of staff about your work
        </span>
      </div>

      <div className="group relative mt-auto flex items-center">
        <ShieldCheck className="size-[15px] text-verified" aria-hidden />
        <span className="sr-only">On-device only</span>
        <span
          aria-hidden
          className="pointer-events-none absolute bottom-0 left-full z-20 ml-2 rounded-sm border border-border bg-popover px-2 py-1 text-xs whitespace-nowrap text-popover-foreground opacity-0 shadow-[0_8px_24px_rgb(20_23_26_/_0.10)] transition-opacity duration-[120ms] ease-instrument group-hover:opacity-100 group-hover:delay-500"
        >
          Nothing you type leaves this machine.
        </span>
      </div>
    </aside>
  );
}
