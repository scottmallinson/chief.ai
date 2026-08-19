import type { ReactNode } from 'react';

import { Sidebar } from '@/components/Sidebar';
import { NAV_ITEMS, type View } from '@/lib/navigation';

interface LayoutProps {
  activeView: View;
  onNavigate: (view: View) => void;
  children: ReactNode;
}

/**
 * The application shell: a fixed navigation rail beside a scrollable content
 * area. Every view renders inside `children`.
 */
export function Layout({ activeView, onNavigate, children }: LayoutProps) {
  const current = NAV_ITEMS.find((item) => item.id === activeView);

  return (
    <div className="flex h-full w-full overflow-hidden bg-background">
      <Sidebar activeView={activeView} onNavigate={onNavigate} />

      <div className="flex min-w-0 flex-1 flex-col">
        <header className="flex h-14 shrink-0 items-center justify-between border-b border-border px-6">
          <div className="min-w-0">
            <h1 className="truncate text-sm font-semibold">{current?.label}</h1>
            <p className="truncate text-xs text-muted-foreground">{current?.description}</p>
          </div>
        </header>

        <main className="min-h-0 flex-1 overflow-y-auto">{children}</main>
      </div>
    </div>
  );
}
