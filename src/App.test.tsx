import { render, screen } from '@testing-library/react';
import userEvent from '@testing-library/user-event';
import { beforeEach, describe, expect, it, vi } from 'vitest';

import App from '@/App';

const invoke = vi.hoisted(() => vi.fn());
const listen = vi.hoisted(() => vi.fn());

vi.mock('@tauri-apps/api/core', () => ({ invoke }));
vi.mock('@tauri-apps/api/event', () => ({ listen }));
vi.mock('@tauri-apps/plugin-opener', () => ({ openUrl: vi.fn() }));

const ready = {
  model: 'Llama 3.2 3B Instruct (Q4_K_M)',
  modelInstalled: true,
  engine: 'ready',
  problem: null,
};

const notReady = {
  model: 'Llama 3.2 3B Instruct (Q4_K_M)',
  modelInstalled: false,
  engine: 'down',
  problem: 'the model has not been downloaded yet.',
};

describe('App', () => {
  beforeEach(() => {
    invoke.mockReset();
    listen.mockReset();
    listen.mockResolvedValue(() => undefined);
  });

  it('opens on the chat view once the machine is ready', async () => {
    invoke.mockImplementation((command: string) =>
      Promise.resolve(command === 'check_readiness' ? ready : []),
    );

    render(<App />);

    expect(await screen.findByRole('heading', { name: 'Ask about your work' })).toBeInTheDocument();
  });

  it('switches views from the sidebar', async () => {
    invoke.mockImplementation((command: string) =>
      Promise.resolve(command === 'check_readiness' ? ready : []),
    );

    render(<App />);
    await userEvent.click(await screen.findByRole('button', { name: 'Work Log' }));

    expect(screen.getByRole('heading', { level: 1, name: 'Work Log' })).toBeInTheDocument();
    expect(await screen.findByRole('heading', { name: 'No entries yet' })).toBeInTheDocument();
  });

  it('shows setup first when the machine is not ready', async () => {
    invoke.mockResolvedValue(notReady);

    render(<App />);

    expect(await screen.findByRole('heading', { name: 'Set up Chief' })).toBeInTheDocument();
    expect(screen.queryByRole('button', { name: 'Work Log' })).not.toBeInTheDocument();
  });

  it('lets the user in anyway', async () => {
    invoke.mockResolvedValue(notReady);

    render(<App />);
    await userEvent.click(await screen.findByRole('button', { name: 'Skip for now' }));

    expect(screen.getByRole('heading', { name: 'Ask about your work' })).toBeInTheDocument();
  });
});
