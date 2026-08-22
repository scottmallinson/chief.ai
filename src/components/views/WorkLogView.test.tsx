import { render, screen } from '@testing-library/react';
import userEvent from '@testing-library/user-event';
import { beforeEach, describe, expect, it, vi } from 'vitest';

import { WorkLogView } from '@/components/views/WorkLogView';
import type { WorkLogEntry } from '@/lib/work-log';

const invoke = vi.hoisted(() => vi.fn());

vi.mock('@tauri-apps/api/core', () => ({ invoke }));

const entry: WorkLogEntry = {
  id: 1,
  timestamp: '2026-08-19T09:00:00.000Z',
  source: 'github',
  content: 'Merged pull request #4',
  summary: 'Shipped the app shell',
  externalId: 'octocat/chief#4',
  accountId: 1,
};

describe('WorkLogView', () => {
  beforeEach(() => {
    invoke.mockReset();
  });

  it('renders entries from the local database', async () => {
    invoke.mockResolvedValue([entry]);

    render(<WorkLogView />);

    expect(await screen.findByText('Shipped the app shell')).toBeInTheDocument();
    expect(screen.getByText('Merged pull request #4')).toBeInTheDocument();
    expect(screen.getByText('github')).toBeInTheDocument();
    expect(invoke).toHaveBeenCalledWith('list_work_logs', { limit: undefined });
  });

  it('falls back to the raw content when there is no summary', async () => {
    invoke.mockResolvedValue([{ ...entry, summary: null }]);

    render(<WorkLogView />);

    expect(await screen.findByText('Merged pull request #4')).toBeInTheDocument();
  });

  it('reloads on request, so new entries appear without a restart', async () => {
    invoke.mockResolvedValueOnce([]);

    render(<WorkLogView />);
    expect(await screen.findByRole('heading', { name: 'No entries yet' })).toBeInTheDocument();

    invoke.mockResolvedValueOnce([entry]);
    await userEvent.click(screen.getByRole('button', { name: 'Refresh work log' }));

    expect(await screen.findByText('Shipped the app shell')).toBeInTheDocument();
  });

  it('shows the empty state when nothing is logged', async () => {
    invoke.mockResolvedValue([]);

    render(<WorkLogView />);

    expect(await screen.findByRole('heading', { name: 'No entries yet' })).toBeInTheDocument();
  });

  it('surfaces a database failure and retries on request', async () => {
    invoke.mockRejectedValueOnce(new Error('the local database is not available'));
    render(<WorkLogView />);

    expect(await screen.findByRole('alert')).toHaveTextContent(
      'the local database is not available',
    );

    invoke.mockResolvedValueOnce([entry]);
    await userEvent.click(screen.getByRole('button', { name: 'Try again' }));

    expect(await screen.findByText('Shipped the app shell')).toBeInTheDocument();
  });
});
