import { render, screen } from '@testing-library/react';
import userEvent from '@testing-library/user-event';
import { beforeEach, describe, expect, it, vi } from 'vitest';

import { FactsEditor } from '@/components/FactsEditor';
import type { Fact } from '@/lib/facts';

const invoke = vi.hoisted(() => vi.fn());

vi.mock('@tauri-apps/api/core', () => ({ invoke }));

const style: Fact = {
  id: 1,
  about: 'you',
  what: 'greeting',
  scope: '',
  value: 'Hi',
  count: 17,
  total: 20,
  source: 'your last 20 pull request descriptions',
  seenOn: '2026-10-10',
  origin: 'counted',
};

const reviewer: Fact = {
  id: 2,
  about: 'priya-n',
  what: 'reviews',
  scope: 'acme/api',
  value: '',
  count: 14,
  total: 20,
  source: 'GitHub reviews on your pull requests',
  seenOn: '2026-10-12',
  origin: 'counted',
};

function backend(facts: Fact[]) {
  invoke.mockImplementation((command: string) => {
    if (command === 'list_facts') return Promise.resolve(facts);
    return Promise.resolve(null);
  });
}

describe('FactsEditor', () => {
  beforeEach(() => {
    invoke.mockReset();
  });

  it('says each fact as a count, with where it came from and when', async () => {
    backend([style, reviewer]);

    render(<FactsEditor />);

    expect(await screen.findByText(/Opens with “Hi” \(17 of 20\)/)).toBeInTheDocument();
    expect(
      screen.getByText(/@priya-n reviewed 14 of your last 20 pull requests in acme\/api/),
    ).toBeInTheDocument();
    expect(screen.getByText(/GitHub reviews on your pull requests · 12 Oct/)).toBeInTheDocument();
  });

  it('says so when nothing has been counted', async () => {
    backend([]);

    render(<FactsEditor />);

    expect(await screen.findByText(/Nothing yet/)).toBeInTheDocument();
  });

  it('marks a fact the user wrote as theirs', async () => {
    backend([{ ...reviewer, origin: 'yours', count: 0, total: 0 }]);

    render(<FactsEditor />);

    expect(await screen.findByText('yours')).toBeInTheDocument();
    expect(screen.getByText(/@priya-n reviews pull requests in acme\/api/)).toBeInTheDocument();
  });

  it('removes a fact when asked, and reads the list again', async () => {
    backend([reviewer]);

    render(<FactsEditor />);
    await userEvent.click(await screen.findByRole('button', { name: /^Remove: @priya-n/ }));

    expect(invoke).toHaveBeenCalledWith('delete_fact', { id: 2 });
    expect(invoke.mock.calls.filter(([command]) => command === 'list_facts')).toHaveLength(2);
  });

  it('adds a reviewer from a handle and a repository', async () => {
    backend([]);

    render(<FactsEditor />);
    await userEvent.click(await screen.findByRole('button', { name: 'Add a fact' }));
    await userEvent.type(screen.getByLabelText('GitHub handle'), '@Priya-N');
    await userEvent.type(screen.getByLabelText('Repository'), 'acme/api');
    await userEvent.click(screen.getByRole('button', { name: 'Save' }));

    expect(invoke).toHaveBeenCalledWith('save_fact', {
      id: null,
      fact: { about: 'Priya-N', what: 'reviews', scope: 'acme/api', value: '' },
    });
  });

  it('will not save a reviewer with no repository', async () => {
    backend([]);

    render(<FactsEditor />);
    await userEvent.click(await screen.findByRole('button', { name: 'Add a fact' }));
    await userEvent.type(screen.getByLabelText('GitHub handle'), 'priya-n');

    expect(screen.getByRole('button', { name: 'Save' })).toBeDisabled();
  });

  it('corrects a fact in place, keeping its id', async () => {
    backend([style]);

    render(<FactsEditor />);
    await userEvent.click(await screen.findByRole('button', { name: /^Edit: Opens with/ }));
    const value = screen.getByLabelText('Value');
    await userEvent.clear(value);
    await userEvent.type(value, 'Hey');
    await userEvent.click(screen.getByRole('button', { name: 'Save' }));

    expect(invoke).toHaveBeenCalledWith('save_fact', {
      id: 1,
      fact: { about: 'you', what: 'greeting', scope: '', value: 'Hey' },
    });
  });

  it('shows a failure to save rather than swallowing it', async () => {
    invoke.mockImplementation((command: string) => {
      if (command === 'list_facts') return Promise.resolve([]);
      return Promise.reject(new Error('the local database is not available'));
    });

    render(<FactsEditor />);
    await userEvent.click(await screen.findByRole('button', { name: 'Add a fact' }));
    await userEvent.type(screen.getByLabelText('GitHub handle'), 'priya-n');
    await userEvent.type(screen.getByLabelText('Repository'), 'acme/api');
    await userEvent.click(screen.getByRole('button', { name: 'Save' }));

    expect(await screen.findByRole('alert')).toHaveTextContent(/not available/);
  });
});
