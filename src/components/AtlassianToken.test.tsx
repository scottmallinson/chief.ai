import { render, screen } from '@testing-library/react';
import userEvent from '@testing-library/user-event';
import { beforeEach, describe, expect, it, vi } from 'vitest';

import { AtlassianToken } from '@/components/AtlassianToken';

const invoke = vi.hoisted(() => vi.fn());

vi.mock('@tauri-apps/api/core', () => ({ invoke }));

const TOKEN = 'ATATT3xFfGF0aBcDeFgHiJkLmNoPqRsTuVwXyZ';

const account = {
  id: 7,
  service: 'atlassian',
  accountKey: 'acme.atlassian.net',
  label: null,
  identity: 'scott@example.com',
  connectedAt: '2026-09-14T09:00:00Z',
};

/** Open the fold. It is shut by default, which is most of what it is for. */
async function open(onChanged = vi.fn()) {
  render(<AtlassianToken onChanged={onChanged} />);
  await userEvent.click(screen.getByRole('button', { name: 'Use a token' }));

  return onChanged;
}

async function fill() {
  await userEvent.type(screen.getByLabelText('Your Atlassian site'), 'acme');
  await userEvent.type(
    screen.getByLabelText('The email the token belongs to'),
    'scott@example.com',
  );
  await userEvent.type(screen.getByLabelText('API token'), TOKEN);
}

describe('AtlassianToken', () => {
  beforeEach(() => {
    invoke.mockReset();
    invoke.mockResolvedValue(account);
  });

  it('is folded away until it is wanted', () => {
    render(<AtlassianToken onChanged={vi.fn()} />);

    // The browser sign-in above is the route almost everybody takes. This is
    // what is left when an administrator has taken it away.
    expect(screen.queryByLabelText('API token')).not.toBeInTheDocument();
    expect(screen.getByRole('button', { name: 'Use a token' })).toHaveAttribute(
      'aria-expanded',
      'false',
    );
  });

  it('says why the other way in exists', async () => {
    await open();

    expect(screen.getByText(/administrator can\s+switch off/i)).toBeInTheDocument();
  });

  it('is honest that the token carries full access', async () => {
    await open();

    // Atlassian issues no read-only API token, exactly as Linear does not.
    // Leaving that implied would be the omission the `repo` scope is
    // criticised for a card away.
    expect(screen.getByText(/no read-only kind/i)).toBeInTheDocument();
  });

  it('connects with the site, the email and the token', async () => {
    const onChanged = await open();
    await fill();

    await userEvent.click(screen.getByRole('button', { name: 'Connect with a token' }));

    expect(invoke).toHaveBeenCalledWith('add_atlassian_token', {
      site: 'acme',
      email: 'scott@example.com',
      token: TOKEN,
    });
    expect(onChanged).toHaveBeenCalled();
  });

  it('will not connect with two of the three', async () => {
    await open();

    await userEvent.type(screen.getByLabelText('Your Atlassian site'), 'acme');
    await userEvent.type(screen.getByLabelText('API token'), TOKEN);

    // A token with no email cannot be sent: Atlassian's Basic auth is
    // `email:token`, so two thirds of it is not a credential.
    expect(screen.getByRole('button', { name: 'Connect with a token' })).toBeDisabled();
  });

  it('names the site it connected to', async () => {
    await open();
    await fill();

    await userEvent.click(screen.getByRole('button', { name: 'Connect with a token' }));

    expect(await screen.findByText('Connected to acme.atlassian.net')).toBeInTheDocument();
  });

  it('clears the token once it has been accepted, and keeps the site', async () => {
    await open();
    await fill();

    await userEvent.click(screen.getByRole('button', { name: 'Connect with a token' }));

    expect(await screen.findByText(/Connected to/)).toBeInTheDocument();
    expect(screen.getByLabelText('API token')).toHaveValue('');
    expect(screen.getByLabelText('Your Atlassian site')).toHaveValue('acme');
  });

  it('says a rejected token was rejected, there and then', async () => {
    invoke.mockRejectedValue(new Error('Atlassian did not accept that token'));
    await open();
    await fill();

    await userEvent.click(screen.getByRole('button', { name: 'Connect with a token' }));

    expect(await screen.findByRole('alert')).toHaveTextContent('did not accept');
  });

  /** The failure that hands somebody's whole Jira to whoever walks past. */
  it('masks the token while it is being typed', async () => {
    await open();

    expect(screen.getByLabelText('API token')).toHaveAttribute('type', 'password');
  });

  it('never puts the token in the DOM as text', async () => {
    await open();
    await fill();

    await userEvent.click(screen.getByRole('button', { name: 'Connect with a token' }));
    expect(await screen.findByText(/Connected to/)).toBeInTheDocument();

    expect(document.body).not.toHaveTextContent(TOKEN);
  });

  it('says it is checking, and will not be asked twice', async () => {
    invoke.mockImplementation(() => new Promise(() => undefined));
    await open();
    await fill();

    const connect = screen.getByRole('button', { name: 'Connect with a token' });
    await userEvent.click(connect);

    expect(connect).toBeDisabled();
    expect(screen.getByRole('status')).toHaveTextContent('Checking that token');
  });
});
