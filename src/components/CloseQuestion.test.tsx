import { act, render, screen } from '@testing-library/react';
import userEvent from '@testing-library/user-event';
import { beforeEach, describe, expect, it, vi } from 'vitest';

import { CloseQuestion } from '@/components/CloseQuestion';

const invoke = vi.hoisted(() => vi.fn());
const listen = vi.hoisted(() => vi.fn());

vi.mock('@tauri-apps/api/core', () => ({ invoke }));
vi.mock('@tauri-apps/api/event', () => ({ listen }));

/** Render, and hand back what the backend emits when the window is first closed. */
async function rendered(): Promise<() => void> {
  let ask: (() => void) | undefined;

  listen.mockImplementation((event: string, handler: () => void) => {
    if (event === 'close-to-tray-question') ask = handler;
    return Promise.resolve(() => undefined);
  });

  render(<CloseQuestion />);
  await vi.waitFor(() => expect(ask).toBeDefined());

  return () => act(() => ask?.());
}

describe('CloseQuestion', () => {
  beforeEach(() => {
    invoke.mockReset();
    listen.mockReset();
    invoke.mockImplementation((command: string) => {
      if (command === 'window_behaviour') {
        return Promise.resolve({ tray: true, keepRunning: null, trayName: 'menu bar' });
      }
      return Promise.resolve(null);
    });
  });

  it('says nothing until the window is closed', async () => {
    await rendered();

    expect(screen.queryByRole('dialog')).not.toBeInTheDocument();
  });

  it('asks, naming this platform’s tray, when the window is closed', async () => {
    const close = await rendered();

    close();

    expect(await screen.findByRole('dialog')).toHaveAccessibleName(
      'Keep Chief running when the window closes?',
    );
    expect(screen.getByRole('button', { name: 'Keep running in the menu bar' })).toBeVisible();
  });

  it('stores the answer before finishing the close', async () => {
    const close = await rendered();
    close();

    await userEvent.click(await screen.findByRole('button', { name: 'Quit when closed' }));

    const commands = invoke.mock.calls.map(([command]) => command as string);
    expect(invoke).toHaveBeenCalledWith('set_keep_running', { keepRunning: false });
    expect(commands.indexOf('set_keep_running')).toBeLessThan(commands.indexOf('close_window'));
    expect(screen.queryByRole('dialog')).not.toBeInTheDocument();
  });

  it('still closes when the answer could not be stored', async () => {
    invoke.mockImplementation((command: string) => {
      if (command === 'window_behaviour') {
        return Promise.resolve({ tray: true, keepRunning: null, trayName: 'system tray' });
      }
      if (command === 'set_keep_running') return Promise.reject(new Error('database is locked'));
      return Promise.resolve(null);
    });
    const close = await rendered();
    close();

    await userEvent.click(
      await screen.findByRole('button', { name: 'Keep running in the system tray' }),
    );

    expect(invoke).toHaveBeenCalledWith('close_window');
  });
});
