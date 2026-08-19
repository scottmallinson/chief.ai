import { render, screen } from '@testing-library/react';
import userEvent from '@testing-library/user-event';
import { beforeEach, describe, expect, it, vi } from 'vitest';

import { SettingsView } from '@/components/views/SettingsView';

const invoke = vi.hoisted(() => vi.fn());
const openUrl = vi.hoisted(() => vi.fn());

vi.mock('@tauri-apps/api/core', () => ({ invoke }));
vi.mock('@tauri-apps/plugin-opener', () => ({ openUrl }));

const disconnected = { service: 'github', connected: false, connectedAt: null };
const connected = {
  service: 'github',
  connected: true,
  connectedAt: '2026-08-19T14:00:00.000Z',
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

  it('shows the device code and opens the browser', async () => {
    invoke.mockImplementation((command: string) => {
      if (command === 'github_connection') return Promise.resolve(disconnected);
      if (command === 'start_github_login') {
        return Promise.resolve({
          userCode: 'WDJB-MJHT',
          verificationUri: 'https://github.com/login/device',
          expiresIn: 900,
        });
      }
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
      if (command === 'github_connection') return Promise.resolve(disconnected);
      if (command === 'start_github_login') {
        return Promise.resolve({
          userCode: 'WDJB-MJHT',
          verificationUri: 'https://github.com/login/device',
          expiresIn: 900,
        });
      }
      return Promise.resolve(connected);
    });

    render(<SettingsView />);
    await userEvent.click(await screen.findByRole('button', { name: 'Connect GitHub' }));

    expect(await screen.findByRole('button', { name: 'Disconnect' })).toBeInTheDocument();
    expect(screen.queryByText('WDJB-MJHT')).not.toBeInTheDocument();
  });

  it('surfaces a missing client id rather than failing silently', async () => {
    invoke.mockImplementation((command: string) => {
      if (command === 'github_connection') return Promise.resolve(disconnected);
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
      Promise.resolve(command === 'disconnect_github' ? disconnected : connected),
    );

    render(<SettingsView />);
    await userEvent.click(await screen.findByRole('button', { name: 'Disconnect' }));

    expect(await screen.findByRole('button', { name: 'Connect GitHub' })).toBeInTheDocument();
  });
});
