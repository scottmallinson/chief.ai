import { act, renderHook, waitFor } from '@testing-library/react';
import { beforeEach, describe, expect, it, vi } from 'vitest';

import { useIntegrations } from '@/hooks/use-integrations';

const invoke = vi.hoisted(() => vi.fn());
const openUrl = vi.hoisted(() => vi.fn());

vi.mock('@tauri-apps/api/core', () => ({ invoke }));
vi.mock('@tauri-apps/plugin-opener', () => ({ openUrl }));

const octocat = {
  id: 1,
  service: 'github',
  accountKey: 'octocat',
  label: null,
  identity: 'octocat',
  connectedAt: '2026-08-19T14:00:00.000Z',
};

describe('useIntegrations', () => {
  beforeEach(() => {
    invoke.mockReset();
    openUrl.mockReset();
    openUrl.mockResolvedValue(undefined);
  });

  it('reports the accounts already connected', async () => {
    invoke.mockResolvedValue([octocat]);

    const { result } = renderHook(() => useIntegrations());

    await waitFor(() => expect(result.current.status).toBe('idle'));
    expect(result.current.accountsFor('github')).toEqual([octocat]);
  });

  it('shows the code while it waits for the browser', async () => {
    const login = { userCode: 'ABCD-1234', verificationUri: 'https://github.com/login/device' };

    invoke.mockImplementation((command: string) => {
      if (command === 'connections') return Promise.resolve([]);
      if (command === 'start_login') return Promise.resolve(login);
      // Never settles, so the waiting state is observable.
      return new Promise(() => {});
    });

    const { result } = renderHook(() => useIntegrations());
    await waitFor(() => expect(result.current.status).toBe('idle'));

    act(() => result.current.connect('github'));

    await waitFor(() => expect(result.current.status).toBe('awaiting-user'));
    expect(result.current.login).toEqual(login);
    expect(openUrl).toHaveBeenCalledWith('https://github.com/login/device');
  });

  it('surfaces a refusal and stops waiting', async () => {
    invoke.mockImplementation((command: string) => {
      if (command === 'connections') return Promise.resolve([]);
      return Promise.reject(new Error('sign-in was declined on GitHub'));
    });

    const { result } = renderHook(() => useIntegrations());
    await waitFor(() => expect(result.current.status).toBe('idle'));

    act(() => result.current.connect('github'));

    await waitFor(() => expect(result.current.error).toBe('sign-in was declined on GitHub'));
    expect(result.current.login).toBeNull();
    expect(result.current.status).toBe('idle');
  });

  it('forgets one account without touching the others', async () => {
    invoke.mockImplementation((command: string) => {
      if (command === 'connections') return Promise.resolve([octocat]);
      if (command === 'disconnect') return Promise.resolve([]);
      return Promise.resolve([]);
    });

    const { result } = renderHook(() => useIntegrations());
    await waitFor(() => expect(result.current.accountsFor('github')).toHaveLength(1));

    act(() => result.current.disconnect(1));

    await waitFor(() => expect(result.current.accountsFor('github')).toHaveLength(0));
    expect(invoke).toHaveBeenCalledWith('disconnect', { accountId: 1 });
  });
});
