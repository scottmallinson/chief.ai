import { render, screen } from '@testing-library/react';
import userEvent from '@testing-library/user-event';
import { beforeEach, describe, expect, it, vi } from 'vitest';

import { ProfileBootstrap } from '@/components/ProfileBootstrap';

const invoke = vi.hoisted(() => vi.fn());

vi.mock('@tauri-apps/api/core', () => ({ invoke }));

const plan = {
  reads: [
    'The descriptions you wrote on your last 20 pull requests',
    'Who you met with over the last 28 days, and how often',
  ],
  writes: ['context/agents/profile/writing_style.md', 'context/agents/org/team_structure.md'],
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

  it('names the files it would write', async () => {
    backend();

    render(<ProfileBootstrap />);

    expect(await screen.findByText('context/agents/profile/writing_style.md')).toBeInTheDocument();
  });

  it('reads nothing until the user asks it to', async () => {
    backend();

    render(<ProfileBootstrap />);
    await screen.findByRole('button', { name: 'Draft these from my work' });

    expect(
      invoke.mock.calls.every(([command]) => command === 'profile_plan'),
      'planning must not run the thing it describes',
    ).toBe(true);
  });

  it('seeds the profile when the user asks', async () => {
    backend({
      bootstrap_profile: { written: ['context/agents/profile/writing_style.md'], kept: [] },
    });

    render(<ProfileBootstrap />);
    await userEvent.click(await screen.findByRole('button', { name: 'Draft these from my work' }));

    expect(await screen.findByText(/Drafted 1 file/)).toBeInTheDocument();
  });

  it('says which files were left alone, and why', async () => {
    backend({
      profile_plan: { ...plan, writes: [], keeps: ['context/agents/profile/writing_style.md'] },
    });

    render(<ProfileBootstrap />);

    expect(
      await screen.findByText(/You have edited these, so Chief will not touch them/),
    ).toBeInTheDocument();
  });

  it('offers nothing to press when every file is the user’s', async () => {
    backend({
      profile_plan: { ...plan, writes: [], keeps: ['context/agents/profile/writing_style.md'] },
    });

    render(<ProfileBootstrap />);
    await screen.findByText(/You have edited these/);

    expect(
      screen.queryByRole('button', { name: 'Draft these from my work' }),
    ).not.toBeInTheDocument();
  });

  it('says so in words when there is nothing to learn from', async () => {
    backend({
      bootstrap_profile: new Error('there is nothing to learn a writing style from yet'),
    });

    render(<ProfileBootstrap />);
    await userEvent.click(await screen.findByRole('button', { name: 'Draft these from my work' }));

    expect(
      await screen.findByText(/nothing to learn a writing style from yet/),
    ).toBeInTheDocument();
  });

  it('says it is working, and will not be asked twice', async () => {
    backend({ bootstrap_profile: new Promise(() => undefined) });

    render(<ProfileBootstrap />);
    const draft = await screen.findByRole('button', { name: 'Draft these from my work' });
    await userEvent.click(draft);

    expect(draft).toBeDisabled();
  });
});
