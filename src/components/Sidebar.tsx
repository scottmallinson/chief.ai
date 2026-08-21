import { ShieldCheck } from 'lucide-react';

import { ChiefMark } from '@/components/ChiefMark';
import { cn } from '@/lib/utils';
import { NAV_ITEMS, type View } from '@/lib/navigation';

interface SidebarProps {
  activeView: View;
  onNavigate: (view: View) => void;
}

/**
 * The 56px icon rail.
 *
 * Labels never appear in the rail itself — each item is a 32px tile carrying a
 * 15px icon, and its name arrives as a tooltip after 500ms. The delay is a CSS
 * transition rather than the platform's own `title` timing, so it is the same
 * half-second on all three desktops.
 *
 * Purely presentational: the active view is owned by `App` so that later steps
 * can persist or deep-link it.
 */
export function Sidebar({ activeView, onNavigate }: SidebarProps) {
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
                  'flex size-8 items-center justify-center rounded-md transition-colors duration-[120ms] ease-instrument',
                  isActive
                    ? 'bg-sidebar-accent text-sidebar-accent-foreground'
                    : 'text-muted-foreground hover:bg-accent hover:text-accent-foreground',
                )}
              >
                <Icon className="size-[15px]" aria-hidden />
              </button>

              <span
                aria-hidden
                className="pointer-events-none absolute top-1/2 left-full z-20 ml-2 -translate-y-1/2 rounded-sm border border-border bg-popover px-2 py-1 text-xs whitespace-nowrap text-popover-foreground opacity-0 shadow-[0_8px_24px_rgb(20_23_26_/_0.10)] transition-opacity duration-[120ms] ease-instrument group-hover:opacity-100 group-hover:delay-500"
              >
                {item.description}
              </span>
            </div>
          );
        })}
      </nav>

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
