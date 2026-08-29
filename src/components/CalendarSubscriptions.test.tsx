import { render, screen } from '@testing-library/react';
import userEvent from '@testing-library/user-event';
import { beforeEach, describe, expect, it, vi } from 'vitest';

import { CalendarSubscriptions } from '@/components/CalendarSubscriptions';
import type { Account } from '@/lib/integrations';

const invoke = vi.hoisted(() => vi.fn());

vi.mock('@tauri-apps/api/core', () => ({ invoke }));

const work: Account = {
  id: 3,
  service: 'calendar',
  accountKey: 'https://calendar.example.com/private-abcdef/basic.ics',
  label: null,
  identity: 'Work',
  connectedAt: '2026-08-29T09:00:00Z',
};

function show(accounts: Account[] = []) {
  return render(<CalendarSubscriptions accounts={accounts} onChanged={vi.fn()} />);
}

describe('CalendarSubscriptions', () => {
  beforeEach(() => {
    invoke.mockReset();
    invoke.mockResolvedValue(work);
  });

  it('says what a subscription is, in the words a provider uses', () => {
    show();

    expect(screen.getByText(/secret address/i)).toBeInTheDocument();
  });

  it('subscribes to a calendar the user pastes', async () => {
    const onChanged = vi.fn();
    render(<CalendarSubscriptions accounts={[]} onChanged={onChanged} />);

    await userEvent.type(
      screen.getByLabelText('Calendar address'),
      'https://calendar.example.com/private-abcdef/basic.ics',
    );
    await userEvent.type(screen.getByLabelText('Name for this calendar'), 'Work');
    await userEvent.click(screen.getByRole('button', { name: 'Subscribe' }));

    expect(invoke).toHaveBeenCalledWith('add_calendar', {
      url: 'https://calendar.example.com/private-abcdef/basic.ics',
      label: 'Work',
    });
    expect(onChanged).toHaveBeenCalled();
  });

  it('will not subscribe to nothing', () => {
    show();

    expect(screen.getByRole('button', { name: 'Subscribe' })).toBeDisabled();
  });

  it('clears the field once it has been accepted', async () => {
    show();

    const address = screen.getByLabelText('Calendar address');
    await userEvent.type(address, 'https://calendar.example.com/a/basic.ics');
    await userEvent.click(screen.getByRole('button', { name: 'Subscribe' }));

    expect(await screen.findByText(/Subscribed/)).toBeInTheDocument();
    expect(address).toHaveValue('');
  });

  it('says what went wrong without the address coming back', async () => {
    invoke.mockRejectedValue(new Error('a calendar subscription has to be an https:// address'));
    show();

    await userEvent.type(screen.getByLabelText('Calendar address'), 'http://example.com/a.ics');
    await userEvent.click(screen.getByRole('button', { name: 'Subscribe' }));

    expect(await screen.findByRole('alert')).toHaveTextContent('https:// address');
  });

  it('lists a calendar by its name', () => {
    show([work]);

    expect(screen.getByText('Work')).toBeInTheDocument();
  });

  /** The failure that hands somebody's calendar to whoever is looking. */
  it('never shows the address, because it is a credential', () => {
    show([work]);

    expect(screen.queryByText(/private-abcdef/)).not.toBeInTheDocument();
    expect(screen.queryByText(work.accountKey)).not.toBeInTheDocument();
  });

  it('removes a subscription on request', async () => {
    const onChanged = vi.fn();
    invoke.mockResolvedValue([]);
    render(<CalendarSubscriptions accounts={[work]} onChanged={onChanged} />);

    await userEvent.click(screen.getByRole('button', { name: 'Remove Work' }));

    expect(invoke).toHaveBeenCalledWith('disconnect', { accountId: 3 });
    expect(onChanged).toHaveBeenCalled();
  });

  it('says it is working, and will not be asked twice', async () => {
    invoke.mockImplementation(() => new Promise(() => undefined));
    show();

    await userEvent.type(screen.getByLabelText('Calendar address'), 'https://example.com/a.ics');
    const subscribe = screen.getByRole('button', { name: 'Subscribe' });
    await userEvent.click(subscribe);

    expect(subscribe).toBeDisabled();
    expect(screen.getByRole('status')).toHaveTextContent('Checking that calendar');
  });
});
