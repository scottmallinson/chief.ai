import { render, screen } from '@testing-library/react';
import userEvent from '@testing-library/user-event';
import { beforeEach, describe, expect, it, vi } from 'vitest';

import { SettingsView } from '@/components/views/SettingsView';

const invoke = vi.hoisted(() => vi.fn());
const openUrl = vi.hoisted(() => vi.fn());

vi.mock('@tauri-apps/api/core', () => ({ invoke }));
vi.mock('@tauri-apps/plugin-opener', () => ({ openUrl }));

// The backend answers every one of these commands with the accounts as they
// now stand, so a fixture is a list rather than a status.
const disconnected: unknown[] = [];
const connected = [
  {
    id: 7,
    service: 'github',
    accountKey: 'octocat',
    label: null,
    identity: 'octocat',
    connectedAt: '2026-08-19T14:00:00.000Z',
  },
];

const deviceLogin = {
  userCode: 'WDJB-MJHT',
  verificationUri: 'https://github.com/login/device',
  expiresIn: 900,
};

describe('SettingsView', () => {
  beforeEach(() => {
    invoke.mockReset();
    openUrl.mockReset();
    openUrl.mockResolvedValue(undefined);
  });

  it('offers to connect GitHub when it is not connected', async () => {
    invoke.mockResolvedValue(disconnected);

    render(<SettingsView />);

    expect(await screen.findByRole('button', { name: 'Connect GitHub' })).toBeInTheDocument();
  });

  it('carries the connection as state, not as an action', async () => {
    invoke.mockResolvedValue(disconnected);

    render(<SettingsView />);

    expect(await screen.findByText('Not connected')).toBeInTheDocument();
    expect(screen.getByText('On this machine')).toBeInTheDocument();
    expect(screen.getByText('llama-3.2-3b-instruct')).toBeInTheDocument();
  });

  it('says when a connected account was connected', async () => {
    invoke.mockResolvedValue(connected);

    render(<SettingsView />);

    expect(await screen.findByText(/^Connected /)).toBeInTheDocument();
  });

  it('shows the device code and opens the browser', async () => {
    invoke.mockImplementation((command: string) => {
      if (command === 'connections') return Promise.resolve(disconnected);
      if (command === 'start_login') return Promise.resolve(deviceLogin);
      // Never settles, so the waiting state stays on screen.
      return new Promise(() => {});
    });

    render(<SettingsView />);
    await userEvent.click(await screen.findByRole('button', { name: 'Connect GitHub' }));

    expect(await screen.findByText('WDJB-MJHT')).toBeInTheDocument();
    expect(openUrl).toHaveBeenCalledWith('https://github.com/login/device');
  });

  it('reports the connection once the user finishes', async () => {
    invoke.mockImplementation((command: string) => {
      if (command === 'connections') return Promise.resolve(disconnected);
      if (command === 'start_login') return Promise.resolve(deviceLogin);
      return Promise.resolve(connected);
    });

    render(<SettingsView />);
    await userEvent.click(await screen.findByRole('button', { name: 'Connect GitHub' }));

    expect(await screen.findByRole('button', { name: 'Disconnect' })).toBeInTheDocument();
    expect(screen.queryByText('WDJB-MJHT')).not.toBeInTheDocument();
  });

  it('surfaces a missing client id rather than failing silently', async () => {
    invoke.mockImplementation((command: string) => {
      if (command === 'connections') return Promise.resolve(disconnected);
      return Promise.reject(
        new Error('no GitHub client id is configured. Register an OAuth app...'),
      );
    });

    render(<SettingsView />);
    await userEvent.click(await screen.findByRole('button', { name: 'Connect GitHub' }));

    expect(await screen.findByRole('alert')).toHaveTextContent('no GitHub client id is configured');
  });

  it('lets a connected account be disconnected', async () => {
    invoke.mockImplementation((command: string) =>
      Promise.resolve(command === 'disconnect' ? disconnected : connected),
    );

    render(<SettingsView />);
    await userEvent.click(await screen.findByRole('button', { name: 'Disconnect' }));

    expect(await screen.findByRole('button', { name: 'Connect GitHub' })).toBeInTheDocument();
    // Keyed on the account, not the service: forgetting one of two GitHub
    // accounts has to leave the other connected.
    expect(invoke).toHaveBeenCalledWith('disconnect', { accountId: 7 });
  });

  it('names the service it is signing in to', async () => {
    invoke.mockImplementation((command: string) => {
      if (command === 'connections') return Promise.resolve(disconnected);
      if (command === 'start_login') return Promise.resolve(deviceLogin);
      return new Promise(() => {});
    });

    render(<SettingsView />);
    await userEvent.click(await screen.findByRole('button', { name: 'Connect GitHub' }));

    await screen.findByText('WDJB-MJHT');
    expect(invoke).toHaveBeenCalledWith('start_login', { service: 'github' });
  });
});
