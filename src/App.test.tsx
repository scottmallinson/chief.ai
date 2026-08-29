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
  tier: 'standard',
  modelSizeMb: 2400,
  modelInstalled: true,
  engine: 'ready',
  problem: null,
};

const notReady = {
  model: 'Llama 3.2 3B Instruct (Q4_K_M)',
  tier: 'standard',
  modelSizeMb: 2400,
  modelInstalled: false,
  engine: 'down',
  problem: 'the model has not been downloaded yet.',
};

/**
 * What each command answers, in the shape the Rust side actually returns.
 *
 * `todays_brief` gives a brief or null and never a list. A stub that answered
 * `[]` to everything handed `TodayView` an object with no `markdown` and took
 * the whole screen down — which is the shape of bug a realistic stub prevents
 * and a lazy one creates.
 */
function answer(command: string, readiness: unknown): unknown {
  switch (command) {
    case 'check_readiness':
      return readiness;
    case 'todays_brief':
      return null;
    default:
      return [];
  }
}

describe('App', () => {
  beforeEach(() => {
    invoke.mockReset();
    listen.mockReset();
    listen.mockResolvedValue(() => undefined);
  });

  it('opens on today once the machine is ready', async () => {
    invoke.mockImplementation((command: string) => Promise.resolve(answer(command, ready)));

    render(<App />);

    expect(await screen.findByRole('heading', { level: 1, name: 'Today' })).toBeInTheDocument();
    expect(await screen.findByRole('heading', { name: 'No brief for today' })).toBeInTheDocument();
  });

  it('keeps chat out of the rail, and opens it as a drawer instead', async () => {
    invoke.mockImplementation((command: string) => Promise.resolve(answer(command, ready)));

    render(<App />);
    await userEvent.click(await screen.findByRole('button', { name: 'Ask Chief' }));

    expect(screen.getByRole('dialog', { name: 'Ask Chief' })).toBeInTheDocument();
    expect(screen.getByRole('heading', { name: 'Ask about your work' })).toBeInTheDocument();
  });

  it('leaves the destination where it was when the drawer closes', async () => {
    invoke.mockImplementation((command: string) => Promise.resolve(answer(command, ready)));

    render(<App />);
    await userEvent.click(await screen.findByRole('button', { name: 'Ask Chief' }));
    await userEvent.keyboard('{Escape}');

    expect(screen.queryByRole('dialog')).not.toBeInTheDocument();
    expect(screen.getByRole('heading', { level: 1, name: 'Today' })).toBeInTheDocument();
  });

  it('switches views from the sidebar', async () => {
    invoke.mockImplementation((command: string) => Promise.resolve(answer(command, ready)));

    render(<App />);
    await userEvent.click(await screen.findByRole('button', { name: 'Work Log' }));

    expect(screen.getByRole('heading', { level: 1, name: 'Work Log' })).toBeInTheDocument();
    expect(await screen.findByRole('heading', { name: 'No entries yet' })).toBeInTheDocument();
  });

  it('shows setup first when the machine is not ready', async () => {
    invoke.mockImplementation((command: string) => Promise.resolve(answer(command, notReady)));

    render(<App />);

    expect(await screen.findByRole('heading', { name: 'Set up Chief' })).toBeInTheDocument();
    expect(screen.queryByRole('button', { name: 'Work Log' })).not.toBeInTheDocument();
  });

  it('lets the user in anyway', async () => {
    invoke.mockImplementation((command: string) => Promise.resolve(answer(command, notReady)));

    render(<App />);
    await userEvent.click(await screen.findByRole('button', { name: 'Skip for now' }));

    expect(screen.getByRole('heading', { level: 1, name: 'Today' })).toBeInTheDocument();
  });
});
