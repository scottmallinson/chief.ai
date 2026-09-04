import { render, screen } from '@testing-library/react';
import userEvent from '@testing-library/user-event';
import { beforeEach, describe, expect, it, vi } from 'vitest';

import { WorkLogView } from '@/components/views/WorkLogView';
import type { SyncState } from '@/lib/sync-state';
import type { Pass, WorkLogEntry } from '@/lib/work-log';

const invoke = vi.hoisted(() => vi.fn());

const openUrl = vi.hoisted(() => vi.fn());

vi.mock('@tauri-apps/api/core', () => ({ invoke }));
vi.mock('@tauri-apps/plugin-opener', () => ({ openUrl }));

const entry: WorkLogEntry = {
  id: 1,
  timestamp: '2026-08-19T09:00:00.000Z',
  source: 'github',
  title: 'scottmallinson/chief.ai #4: Ship the app shell',
  content: 'Merged pull request #4',
  // The state word deterministic ingestion writes, not a sentence.
  summary: 'merged',
  url: 'https://github.com/scottmallinson/chief.ai/pull/44',
  externalId: 'octocat/chief#4',
  accountId: 1,
};

const fresh: SyncState = {
  accountId: 1,
  source: 'github',
  status: 'ok',
  lastSyncedAt: new Date().toISOString(),
  errorMessage: null,
};

/**
 * What each command answers, in the shape it actually returns.
 *
 * **Dispatching on the name rather than answering `[]` to everything.** The
 * single-answer stub is the most expensive shortcut in this repository — it has
 * taken the whole screen down three times — and this screen now makes three
 * different commands whose replies have three different shapes. `sync_now`
 * returns an object; a stub that handed it a list would have `pass.accounts`
 * come back undefined and the render throw, and the test would look green right
 * up until it did not.
 *
 * An unstubbed command rejects rather than resolving, so a call this screen was
 * not supposed to make shows up here rather than passing quietly.
 */
function answering(overrides: Record<string, unknown> = {}) {
  invoke.mockImplementation((command: string) => {
    if (Object.hasOwnProperty.call(overrides, command)) {
      const answer = overrides[command];

      return answer instanceof Error ? Promise.reject(answer) : Promise.resolve(answer);
    }

    switch (command) {
      case 'list_work_logs':
        return Promise.resolve([]);
      case 'sync_status':
        return Promise.resolve([]);
      case 'sync_now':
        return Promise.resolve({ written: 0, accounts: [] } satisfies Pass);
      default:
        return Promise.reject(new Error(`nothing stubbed ${command}`));
    }
  });
}

/** Every command name this render asked for, in order. */
function commands(): string[] {
  return invoke.mock.calls.map((call: unknown[]) => String(call[0]));
}

describe('WorkLogView', () => {
  beforeEach(() => {
    invoke.mockReset();
    openUrl.mockReset();
    openUrl.mockResolvedValue(undefined);
  });

  /**
   * A row leads with what it is about, not with what state it is in.
   *
   * **These tests used to assert the opposite** — that the headline was
   * `summary` — which was right while `summary` was a model-written sentence
   * and wrong from the moment deterministic ingestion made it the single word
   * "merged". Measured in the running app, this screen's sibling feed read
   * `merged`, `merged`, `open` five rows deep with nothing saying what had
   * been merged.
   *
   * Proved by leading with `summary` again:
   *
   * ```text
   * Unable to find an element with the text:
   * scottmallinson/chief.ai #4: Ship the app shell
   * ```
   */
  it('leads with what the entry is about, and keeps the state beside it', async () => {
    answering({ list_work_logs: [entry] });

    render(<WorkLogView />);

    expect(
      await screen.findByText('scottmallinson/chief.ai #4: Ship the app shell'),
    ).toBeInTheDocument();
    expect(screen.getByText('merged')).toBeInTheDocument();
    expect(invoke).toHaveBeenCalledWith('list_work_logs', { limit: undefined });
  });

  /**
   * A row missing its title renders rather than taking the screen down.
   *
   * The column is `NOT NULL DEFAULT ''`, so Rust always sends a string — but
   * this renderer runs over every row of two feeds, and CLAUDE.md records
   * three separate occasions where a view received a shape it did not expect
   * and a render threw. Proved by reading `entry.title.trim()` unguarded:
   *
   * ```text
   * TypeError: Cannot read properties of undefined (reading 'trim')
   * ```
   */
  it('renders a row that arrived without a title, rather than throwing', async () => {
    const untitled = { ...entry } as Partial<WorkLogEntry>;
    delete untitled.title;

    answering({ list_work_logs: [untitled] });

    render(<WorkLogView />);

    expect(await screen.findByText('Merged pull request #4')).toBeInTheDocument();
  });

  it('falls back to the raw content for an entry the user typed themselves', async () => {
    // No title: migration 8 gave the column a default rather than inventing
    // one, so a hand-written entry has nothing but what was typed.
    answering({ list_work_logs: [{ ...entry, title: '', summary: null }] });

    render(<WorkLogView />);

    expect(await screen.findByText('Merged pull request #4')).toBeInTheDocument();
  });

  /**
   * The defect REC-63 is about.
   *
   * Deterministic ingestion writes a state — "merged", "open", a meeting's
   * time — and the row puts it in a chip. The daemon before it asked the model
   * for a sentence, and migration 8 gave those rows a title as well, so the
   * same column now holds both shapes. The sentence was going into the chip,
   * which is how a work log ends up labelled "merged" on one row and with a
   * paragraph on the next.
   *
   * Proved by chipping whatever the summary said:
   *
   * ```text
   * expected null not to be null   // the chip was found, and should not be
   * ```
   */
  it('puts a sentence from the old daemon under the row, never in its label', async () => {
    const sentence =
      "The developer merged a pull request to resolve a bug related to fetching the llama.cpp engine before Rust's checks.";

    answering({ list_work_logs: [{ ...entry, summary: sentence }] });

    render(<WorkLogView />);

    const prose = await screen.findByText(sentence);

    expect(prose).toBeInTheDocument();
    // A chip is a `<span>` carrying the machine tone; prose is a paragraph.
    expect(prose.tagName, 'a sentence is prose, not a label').toBe('P');
    expect(screen.getByText('scottmallinson/chief.ai #4: Ship the app shell')).toBeInTheDocument();
  });

  it('keeps a state word as the label it is', async () => {
    answering({ list_work_logs: [entry] });

    render(<WorkLogView />);

    const label = await screen.findByText('merged');

    expect(label.tagName, 'a state word is a label').toBe('SPAN');
  });

  /** A summary that only repeats the headline is not worth a second line. */
  it('does not repeat the headline back underneath itself', async () => {
    answering({ list_work_logs: [{ ...entry, summary: entry.title }] });

    render(<WorkLogView />);

    expect(
      await screen.findByText('scottmallinson/chief.ai #4: Ship the app shell'),
    ).toBeInTheDocument();
    expect(screen.getAllByText('scottmallinson/chief.ai #4: Ship the app shell')).toHaveLength(1);
  });

  it('shows the empty state when nothing is logged', async () => {
    answering();

    render(<WorkLogView />);

    expect(await screen.findByRole('heading', { name: 'No entries yet' })).toBeInTheDocument();
  });

  it('surfaces a database failure and retries on request', async () => {
    answering({ list_work_logs: new Error('the local database is not available') });
    render(<WorkLogView />);

    expect(await screen.findByRole('alert')).toHaveTextContent(
      'the local database is not available',
    );

    answering({ list_work_logs: [entry] });
    await userEvent.click(screen.getByRole('button', { name: 'Try again' }));

    expect(
      await screen.findByText('scottmallinson/chief.ai #4: Ship the app shell'),
    ).toBeInTheDocument();
  });

  describe('refreshing', () => {
    /**
     * The defect REC-56 is about.
     *
     * "Clicking Refresh in the work log doesn't appear to do anything. None of
     * my recent activity in GitHub has been pulled into Chief." Which was
     * correct: the button called `list_work_logs`, a `SELECT`, and asked no
     * connected service for anything. A user whose log had stopped three days
     * earlier could click it all day and learn nothing.
     *
     * Proved by pointing the button back at `reload`:
     *
     * ```text
     * → Refresh has to ask the services, not just re-read the rows
     *   - Expected: true
     *   + Received: false
     * ```
     */
    it('asks the connected services, and then re-reads what the pass wrote', async () => {
      answering({
        sync_now: { written: 1, accounts: [fresh] } satisfies Pass,
      });

      render(<WorkLogView />);
      expect(await screen.findByRole('heading', { name: 'No entries yet' })).toBeInTheDocument();

      answering({
        list_work_logs: [entry],
        sync_now: { written: 1, accounts: [fresh] } satisfies Pass,
      });
      await userEvent.click(screen.getByRole('button', { name: 'Refresh work log' }));

      expect(
        await screen.findByText('scottmallinson/chief.ai #4: Ship the app shell'),
      ).toBeInTheDocument();
      expect(
        commands().includes('sync_now'),
        'Refresh has to ask the services, not just re-read the rows',
      ).toBe(true);
    });

    it('says how fresh each connected account is', async () => {
      answering({ sync_status: [fresh] });

      render(<WorkLogView />);

      expect(await screen.findByTestId('freshness')).toHaveTextContent('Synced just now');
    });

    it('says so when nothing is connected, rather than looking merely empty', async () => {
      answering();

      render(<WorkLogView />);

      expect(await screen.findByText(/Nothing is connected yet/)).toBeInTheDocument();
    });

    /**
     * A pass returns `Ok` even when every account was refused: a failure
     * belongs to the account it happened to and is stepped over, so one bad
     * credential cannot freeze another account's log. The count is therefore
     * not the report — the states are.
     */
    it('shows an account that was refused, on a pass that itself succeeded', async () => {
      const refused: SyncState = {
        accountId: 1,
        source: 'github',
        status: 'authRequired',
        lastSyncedAt: '2026-08-30T09:00:00.000Z',
        errorMessage: 'GitHub refused the stored credential.',
      };

      answering({ sync_now: { written: 0, accounts: [refused] } satisfies Pass });

      render(<WorkLogView />);
      await userEvent.click(screen.getByRole('button', { name: 'Refresh work log' }));

      expect(await screen.findByTestId('freshness')).toHaveTextContent('Sign in again');
      expect(screen.getByText('GitHub refused the stored credential.')).toBeInTheDocument();
    });

    /** A pass that could not be started at all, which is not an account's fault. */
    it('says why a pass could not be run', async () => {
      answering({ sync_now: new Error('the local database is not available') });

      render(<WorkLogView />);
      await userEvent.click(screen.getByRole('button', { name: 'Refresh work log' }));

      expect(await screen.findByRole('alert')).toHaveTextContent(
        'the local database is not available',
      );
    });
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
      answering({ list_work_logs: [entry] });

      render(<WorkLogView />);

      await userEvent.click(await screen.findByRole('button', { name: /^Open / }));

      expect(openUrl).toHaveBeenCalledWith('https://github.com/scottmallinson/chief.ai/pull/44');
      // Opening a row reads nothing and asks nobody: the two commands are the
      // ones the screen makes on mount, and neither reaches a model or a
      // network.
      expect(commands()).toEqual(['list_work_logs', 'sync_status']);
    });

    /**
     * The common case, not the exception: entries the user typed have no URL,
     * and neither does anything logged before migration 8.
     */
    it('offers nothing, and does not crash, when there is nowhere to go', async () => {
      answering({ list_work_logs: [{ ...entry, url: null }] });

      render(<WorkLogView />);

      expect(
        await screen.findByText('scottmallinson/chief.ai #4: Ship the app shell'),
      ).toBeInTheDocument();
      expect(screen.queryByRole('button', { name: /^Open / })).not.toBeInTheDocument();
    });

    it('treats a blank url as no url', async () => {
      answering({ list_work_logs: [{ ...entry, url: '   ' }] });

      render(<WorkLogView />);

      expect(
        await screen.findByText('scottmallinson/chief.ai #4: Ship the app shell'),
      ).toBeInTheDocument();
      expect(screen.queryByRole('button', { name: /^Open / })).not.toBeInTheDocument();
    });

    /** An opener the platform refused leaves the row as it was. */
    it('survives an opener that refuses', async () => {
      answering({ list_work_logs: [entry] });
      openUrl.mockRejectedValue(new Error('no handler for https'));

      render(<WorkLogView />);

      await userEvent.click(await screen.findByRole('button', { name: /^Open / }));

      expect(
        screen.getByText('scottmallinson/chief.ai #4: Ship the app shell'),
      ).toBeInTheDocument();
    });
  });
});
