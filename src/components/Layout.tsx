import type { ReactNode } from 'react';

import { Sidebar } from '@/components/Sidebar';
import { NAV_ITEMS, type View } from '@/lib/navigation';

interface LayoutProps {
  activeView: View;
  onNavigate: (view: View) => void;
  /** The model answering on this machine, named for a person to read. */
  model?: string | undefined;
  children: ReactNode;
}

/**
 * The application shell: a 56px icon rail, then a 48px header over the view.
 *
 * The shell is pinned to the window and does not scroll. `main` clips rather
 * than scrolls, because a view that stacks two scroll regions gets two
 * scrollbars and no clear owner of the wheel — each view brings its own.
 *
 * The header says where the data is on every screen, which is the first thing
 * this product has to be able to state rather than imply.
 */
export function Layout({ activeView, onNavigate, model, children }: LayoutProps) {
  const current = NAV_ITEMS.find((item) => item.id === activeView);

  return (
    <div className="flex h-screen w-full overflow-hidden bg-background">
      <Sidebar activeView={activeView} onNavigate={onNavigate} />

      <div className="flex min-w-0 flex-1 flex-col">
        <header className="flex h-12 shrink-0 items-center justify-between gap-4 border-b border-border px-5">
          <h1 className="truncate text-base font-semibold tracking-[-0.015em]">{current?.label}</h1>
          <p className="truncate font-mono text-[11px] text-muted-foreground">
            on-device{model === undefined ? '' : ` · ${model}`}
          </p>
        </header>

        <main className="min-h-0 flex-1 overflow-hidden">{children}</main>
      </div>
    </div>
  );
}
