import { ShieldCheck } from 'lucide-react';

import { cn } from '@/lib/utils';
import { NAV_ITEMS, type View } from '@/lib/navigation';

interface SidebarProps {
  activeView: View;
  onNavigate: (view: View) => void;
}

/**
 * Primary navigation rail. Purely presentational — the active view is owned by
 * `App` so that later steps can persist or deep-link it.
 */
export function Sidebar({ activeView, onNavigate }: SidebarProps) {
  return (
    <aside className="flex w-60 shrink-0 flex-col border-r border-sidebar-border bg-sidebar text-sidebar-foreground">
      <div className="flex h-14 items-center gap-2 px-4">
        <div className="flex size-7 items-center justify-center rounded-md bg-primary text-xs font-bold text-primary-foreground">
          C
        </div>
        <div className="leading-tight">
          <p className="text-sm font-semibold">Chief</p>
          <p className="text-[11px] text-muted-foreground">Your AI chief of staff</p>
        </div>
      </div>

      <nav aria-label="Main" className="flex-1 space-y-1 px-2 py-2">
        {NAV_ITEMS.map((item) => {
          const Icon = item.icon;
          const isActive = item.id === activeView;

          return (
            <button
              key={item.id}
              type="button"
              onClick={() => onNavigate(item.id)}
              aria-current={isActive ? 'page' : undefined}
              title={item.description}
              className={cn(
                'flex w-full items-center gap-3 rounded-md px-3 py-2 text-sm transition-colors',
                'focus-visible:ring-2 focus-visible:ring-ring focus-visible:outline-none',
                isActive
                  ? 'bg-sidebar-accent font-medium text-sidebar-accent-foreground'
                  : 'text-muted-foreground hover:bg-sidebar-accent/60 hover:text-sidebar-accent-foreground',
              )}
            >
              <Icon className="size-4" aria-hidden />
              {item.label}
            </button>
          );
        })}
      </nav>

      <div className="border-t border-sidebar-border px-4 py-3 text-[11px] text-muted-foreground">
        <p className="flex items-center gap-1.5">
          <ShieldCheck className="size-3.5" aria-hidden />
          On-device only
        </p>
        <p className="mt-1">Your data never leaves this machine.</p>
      </div>
    </aside>
  );
}
