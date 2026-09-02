import { render, screen } from '@testing-library/react';
import userEvent from '@testing-library/user-event';
import { beforeEach, describe, expect, it, vi } from 'vitest';

import { LinearKey } from '@/components/LinearKey';
import type { Account } from '@/lib/integrations';

const invoke = vi.hoisted(() => vi.fn());

vi.mock('@tauri-apps/api/core', () => ({ invoke }));

const workspace: Account = {
  id: 4,
  service: 'linear',
  accountKey: 'scott@example.com',
  label: null,
  identity: 'Scott Mallinson',
  connectedAt: '2026-08-29T09:00:00Z',
};

const KEY = 'lin_api_0123456789abcdef';

function show(accounts: Account[] = []) {
  return render(<LinearKey accounts={accounts} onChanged={vi.fn()} />);
}

describe('LinearKey', () => {
  beforeEach(() => {
    invoke.mockReset();
    // Dispatched on the command name. `sync_status` answers `SyncState[]` and
    // nothing else does, and a stub that answers one shape to everything is
    // what CLAUDE.md calls the most expensive shortcut in this repository —
    // it took this whole card down when the freshness line was added.
    invoke.mockImplementation((command: string) =>
      Promise.resolve(command === 'sync_status' ? [] : workspace),
    );
  });

  it('says where the key comes from', () => {
    show();

    expect(screen.getByText(/personal API key/i)).toBeInTheDocument();
  });

  it('is honest that the key carries full access', () => {
    show();

    // Linear has no read-only key. Leaving that implied would be the same
    // omission as not saying what the GitHub `repo` scope grants.
    expect(screen.getByText(/read and write/i)).toBeInTheDocument();
  });

  it('connects with the key the user pastes', async () => {
    const onChanged = vi.fn();
    render(<LinearKey accounts={[]} onChanged={onChanged} />);

    await userEvent.type(screen.getByLabelText('Linear API key'), KEY);
    await userEvent.click(screen.getByRole('button', { name: 'Connect' }));

    expect(invoke).toHaveBeenCalledWith('add_linear_key', { key: KEY });
    expect(onChanged).toHaveBeenCalled();
  });

  it('will not connect with nothing', () => {
    show();

    expect(screen.getByRole('button', { name: 'Connect' })).toBeDisabled();
  });

  it('clears the field once the key has been accepted', async () => {
    show();

    const field = screen.getByLabelText('Linear API key');
    await userEvent.type(field, KEY);
    await userEvent.click(screen.getByRole('button', { name: 'Connect' }));

    expect(await screen.findByText(/Connected/)).toBeInTheDocument();
    expect(field).toHaveValue('');
  });

  it('says a rejected key was rejected, there and then', async () => {
    invoke.mockRejectedValue(new Error('that Linear key was not accepted'));
    show();

    await userEvent.type(screen.getByLabelText('Linear API key'), 'wrong');
    await userEvent.click(screen.getByRole('button', { name: 'Connect' }));

    expect(await screen.findByRole('alert')).toHaveTextContent('not accepted');
  });

  it('lists a connected workspace by whose it is', () => {
    show([workspace]);

    expect(screen.getByText('Scott Mallinson')).toBeInTheDocument();
  });

  /** The failure that hands somebody's whole Linear account to a passer-by. */
  it('never shows the key, because it is a credential', () => {
    show([workspace]);

    expect(screen.queryByText(/lin_api/)).not.toBeInTheDocument();
  });

  it('masks the key while it is being typed', () => {
    show();

    expect(screen.getByLabelText('Linear API key')).toHaveAttribute('type', 'password');
  });

  it('disconnects a workspace on request', async () => {
    const onChanged = vi.fn();
    invoke.mockResolvedValue([]);
    render(<LinearKey accounts={[workspace]} onChanged={onChanged} />);

    await userEvent.click(screen.getByRole('button', { name: 'Disconnect Scott Mallinson' }));

    expect(invoke).toHaveBeenCalledWith('disconnect', { accountId: 4 });
    expect(onChanged).toHaveBeenCalled();
  });

  it('says it is checking, and will not be asked twice', async () => {
    invoke.mockImplementation(() => new Promise(() => undefined));
    show();

    await userEvent.type(screen.getByLabelText('Linear API key'), KEY);
    const connect = screen.getByRole('button', { name: 'Connect' });
    await userEvent.click(connect);

    expect(connect).toBeDisabled();
    expect(screen.getByRole('status')).toHaveTextContent('Checking that key');
  });
});
