import type { ReactNode } from 'react';

import { Sidebar } from '@/components/Sidebar';
import { useSyncState } from '@/hooks/use-sync-state';
import { howLongAgo, oldest } from '@/lib/sync-state';
import { NAV_ITEMS, type View } from '@/lib/navigation';

interface LayoutProps {
  activeView: View;
  onNavigate: (view: View) => void;
  /** Open the chat drawer over whatever is showing. */
  onOpenChat: () => void;
  /** The model answering on this machine, named for a person to read. */
  model?: string | undefined;
  /** How many accounts are connected, so the header can tell "none" from
   *  "some, and one of them has never been read". */
  accounts?: number | undefined;
  /**
   * The 240px column between the rail and the detail, when the destination has
   * something to put in it. Left out entirely otherwise: an empty pane is 240px
   * of the window spent saying nothing.
   */
  list?: ReactNode | undefined;
  children: ReactNode;
}

/**
 * The application shell: a 56px icon rail, an optional 240px list pane, then a
 * 48px header over the view.
 *
 * The shell is pinned to the window and does not scroll. `main` clips rather
 * than scrolls, because a view that stacks two scroll regions gets two
 * scrollbars and no clear owner of the wheel — each view brings its own. The
 * list pane scrolls on its own for the same reason: it is beside the detail,
 * not inside it.
 *
 * The header says where the data is on every screen, which is the first thing
 * this product has to be able to state rather than imply.
 */
export function Layout({
  activeView,
  onNavigate,
  onOpenChat,
  model,
  accounts = 0,
  list,
  children,
}: LayoutProps) {
  const current = NAV_ITEMS.find((item) => item.id === activeView);
  const { all } = useSyncState();

  // The **oldest** successful read, not the newest. A header showing the
  // newest would say everything was fresh while one account had been failing
  // for a week, which is the reading D9 cannot afford to give: an answer
  // assembled from local rows is only as good as its stalest source.
  const since = oldest(all, accounts);
  const freshness = since === null ? null : howLongAgo(since);

  return (
    <div className="flex h-screen w-full overflow-hidden bg-background">
      <Sidebar activeView={activeView} onNavigate={onNavigate} onOpenChat={onOpenChat} />

      {list !== undefined && (
        <div
          data-testid="list-pane"
          className="w-60 shrink-0 overflow-y-auto border-r border-border bg-sidebar"
        >
          {list}
        </div>
      )}

      <div className="flex min-w-0 flex-1 flex-col">
        <header className="flex h-12 shrink-0 items-center justify-between gap-4 border-b border-border px-5">
          <h1 className="truncate text-base font-semibold tracking-[-0.015em]">{current?.label}</h1>
          <p className="truncate font-mono text-[11px] text-muted-foreground">
            on-device{model === undefined ? '' : ` · ${model}`}
            {freshness === null ? '' : ` · synced ${freshness}`}
          </p>
        </header>

        <main className="min-h-0 flex-1 overflow-hidden">{children}</main>
      </div>
    </div>
  );
}
