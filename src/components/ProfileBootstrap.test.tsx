import { render, screen } from '@testing-library/react';
import userEvent from '@testing-library/user-event';
import { beforeEach, describe, expect, it, vi } from 'vitest';

import { ProfileBootstrap } from '@/components/ProfileBootstrap';

const invoke = vi.hoisted(() => vi.fn());

vi.mock('@tauri-apps/api/core', () => ({ invoke }));

const plan = {
  reads: [
    'The descriptions you wrote on your last 20 pull requests',
    'Who reviewed your last 30 pull requests (one request to GitHub for each)',
    'Who you met with over the last 28 days, and how often',
  ],
  writes: [
    'How you write: greeting, sign-off, length, emoji, spelling',
    'Who reviews your pull requests, repository by repository',
    'Who you meet with most',
  ],
  keeps: [],
};

function backend(overrides: Record<string, unknown> = {}) {
  invoke.mockImplementation((command: string) => {
    if (command in overrides) {
      const answer = overrides[command];
      return answer instanceof Error ? Promise.reject(answer) : Promise.resolve(answer);
    }
    if (command === 'profile_plan') return Promise.resolve(plan);
    return Promise.resolve(null);
  });
}

describe('ProfileBootstrap', () => {
  beforeEach(() => {
    invoke.mockReset();
  });

  it('says what will be read before anything is read', async () => {
    backend();

    render(<ProfileBootstrap />);

    expect(
      await screen.findByText('The descriptions you wrote on your last 20 pull requests'),
    ).toBeInTheDocument();
    expect(
      screen.getByText('Who you met with over the last 28 days, and how often'),
    ).toBeInTheDocument();
  });

  it('says the cost of reading reviews before it is paid', async () => {
    backend();

    render(<ProfileBootstrap />);

    expect(await screen.findByText(/one request to GitHub for each/)).toBeInTheDocument();
  });

  it('names the facts it would count', async () => {
    backend();

    render(<ProfileBootstrap />);

    expect(
      await screen.findByText('Who reviews your pull requests, repository by repository'),
    ).toBeInTheDocument();
  });

  it('reads nothing until the user asks it to', async () => {
    backend();

    render(<ProfileBootstrap />);
    await screen.findByRole('button', { name: 'Count these from my work' });

    expect(
      invoke.mock.calls.every(([command]) => command === 'profile_plan'),
      'planning must not run the thing it describes',
    ).toBe(true);
  });

  it('seeds the profile when the user asks', async () => {
    backend({
      bootstrap_profile: { written: ['5 facts about how you write'], kept: [] },
    });

    render(<ProfileBootstrap />);
    await userEvent.click(await screen.findByRole('button', { name: 'Count these from my work' }));

    expect(await screen.findByText(/Counted 5 facts about how you write/)).toBeInTheDocument();
  });

  it('says what was left alone, and why', async () => {
    backend({
      profile_plan: {
        ...plan,
        keeps: ['2 facts you changed or removed stay as you left them'],
      },
    });

    render(<ProfileBootstrap />);

    expect(await screen.findByText('Left as it is')).toBeInTheDocument();
    expect(await screen.findByText(/2 facts you changed or removed/)).toBeInTheDocument();
  });

  it('offers nothing to press when there is nothing to count', async () => {
    backend({
      profile_plan: {
        ...plan,
        writes: [],
        keeps: ['2 facts you changed or removed stay as you left them'],
      },
    });

    render(<ProfileBootstrap />);
    await screen.findByText('Left as it is');

    expect(
      screen.queryByRole('button', { name: 'Count these from my work' }),
    ).not.toBeInTheDocument();
  });

  it('says so in words when there is nothing to learn from', async () => {
    backend({
      bootstrap_profile: new Error('there is nothing to learn from yet'),
    });

    render(<ProfileBootstrap />);
    await userEvent.click(await screen.findByRole('button', { name: 'Count these from my work' }));

    expect(await screen.findByText(/nothing to learn from yet/)).toBeInTheDocument();
  });

  it('says it is working, and will not be asked twice', async () => {
    backend({ bootstrap_profile: new Promise(() => undefined) });

    render(<ProfileBootstrap />);
    const draft = await screen.findByRole('button', { name: 'Count these from my work' });
    await userEvent.click(draft);

    expect(draft).toBeDisabled();
  });
});
