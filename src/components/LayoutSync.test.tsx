import { act, render, screen } from '@testing-library/react';
import { beforeEach, describe, expect, it, vi } from 'vitest';

import { Layout } from '@/components/Layout';
import type { SyncState } from '@/lib/sync-state';

const { invoke, listen } = vi.hoisted(() => ({ invoke: vi.fn(), listen: vi.fn() }));

vi.mock('@tauri-apps/api/core', () => ({ invoke }));
vi.mock('@tauri-apps/api/event', () => ({ listen }));

function state(minutesAgo: number): SyncState {
  return {
    accountId: 1,
    source: 'github',
    status: 'ok',
    lastSyncedAt: new Date(Date.now() - minutesAgo * 60_000).toISOString(),
    errorMessage: null,
  };
}

describe('the header sync time', () => {
  let announce: () => void = () => undefined;

  beforeEach(() => {
    invoke.mockReset();
    listen.mockReset();
    listen.mockImplementation((_name: string, handler: () => void) => {
      announce = handler;
      return Promise.resolve(() => undefined);
    });
  });

  it('moves when a sync finishes, without the window being reloaded', async () => {
    invoke.mockResolvedValue([state(125)]);

    render(
      <Layout activeView="today" onNavigate={vi.fn()} onOpenChat={vi.fn()} accounts={1}>
        <p>brief</p>
      </Layout>,
    );

    expect(await screen.findByText(/synced 2h ago/)).toBeInTheDocument();

    invoke.mockResolvedValue([state(0)]);
    act(() => {
      announce();
    });

    expect(await screen.findByText(/synced just now/)).toBeInTheDocument();
    expect(listen).toHaveBeenCalledWith('sync-state-changed', expect.any(Function));
  });
});
