import { render, screen } from '@testing-library/react';
import userEvent from '@testing-library/user-event';
import { beforeEach, describe, expect, it, vi } from 'vitest';

import { WorkLogView } from '@/components/views/WorkLogView';
import type { WorkLogEntry } from '@/lib/work-log';

const invoke = vi.hoisted(() => vi.fn());

const openUrl = vi.hoisted(() => vi.fn());

vi.mock('@tauri-apps/api/core', () => ({ invoke }));
vi.mock('@tauri-apps/plugin-opener', () => ({ openUrl }));

const entry: WorkLogEntry = {
  id: 1,
  timestamp: '2026-08-19T09:00:00.000Z',
  source: 'github',
  content: 'Merged pull request #4',
  summary: 'Shipped the app shell',
  url: 'https://github.com/scottmallinson/chief.ai/pull/44',
  externalId: 'octocat/chief#4',
  accountId: 1,
};

describe('WorkLogView', () => {
  beforeEach(() => {
    invoke.mockReset();
    openUrl.mockReset();
    openUrl.mockResolvedValue(undefined);
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
  describe('opening the thing an entry is about', () => {
    /**
     * The other half of the trade DLE-1 made. The retrieval context omits
     * links because a GitHub URL is 13–15 tokens and more than half a row —
     * which only works if the interface puts one back from the column, and a
     * model that has never seen a link cannot invent one.
     *
     * Proved by re-deriving the target from `externalId` instead:
     *
     *   expected "spy" to be called with arguments:
     *     [ 'https://github.com/scottmallinson/chief.ai/pull/44' ]
     */
    it('opens the stored url verbatim, and asks nothing to produce it', async () => {
      invoke.mockResolvedValue([entry]);

      render(<WorkLogView />);

      await userEvent.click(await screen.findByRole('button', { name: /^Open / }));

      expect(openUrl).toHaveBeenCalledWith('https://github.com/scottmallinson/chief.ai/pull/44');
      // Reading the log is the only command this render makes: no model call,
      // and nothing that reaches a network.
      expect(invoke).toHaveBeenCalledTimes(1);
      expect(invoke).toHaveBeenCalledWith('list_work_logs', { limit: undefined });
    });

    /**
     * The common case, not the exception: entries the user typed have no URL,
     * and neither does anything logged before migration 8.
     */
    it('offers nothing, and does not crash, when there is nowhere to go', async () => {
      invoke.mockResolvedValue([{ ...entry, url: null }]);

      render(<WorkLogView />);

      expect(await screen.findByText('Shipped the app shell')).toBeInTheDocument();
      expect(screen.queryByRole('button', { name: /^Open / })).not.toBeInTheDocument();
    });

    it('treats a blank url as no url', async () => {
      invoke.mockResolvedValue([{ ...entry, url: '   ' }]);

      render(<WorkLogView />);

      expect(await screen.findByText('Shipped the app shell')).toBeInTheDocument();
      expect(screen.queryByRole('button', { name: /^Open / })).not.toBeInTheDocument();
    });

    /** An opener the platform refused leaves the row as it was. */
    it('survives an opener that refuses', async () => {
      invoke.mockResolvedValue([entry]);
      openUrl.mockRejectedValue(new Error('no handler for https'));

      render(<WorkLogView />);

      await userEvent.click(await screen.findByRole('button', { name: /^Open / }));

      expect(screen.getByText('Shipped the app shell')).toBeInTheDocument();
    });
  });
});
