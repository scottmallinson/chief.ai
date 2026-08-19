import { render, screen } from '@testing-library/react';
import userEvent from '@testing-library/user-event';
import { beforeEach, describe, expect, it, vi } from 'vitest';

import { SetupView } from '@/components/views/SetupView';

const invoke = vi.hoisted(() => vi.fn());
const listen = vi.hoisted(() => vi.fn());
const openUrl = vi.hoisted(() => vi.fn());

vi.mock('@tauri-apps/api/core', () => ({ invoke }));
vi.mock('@tauri-apps/api/event', () => ({ listen }));
vi.mock('@tauri-apps/plugin-opener', () => ({ openUrl }));

const noOllama = {
  ollamaRunning: false,
  ollamaVersion: null,
  model: 'llama3.2:3b',
  modelInstalled: false,
  problem: 'could not reach Ollama at http://localhost:11434/. Is it running?',
};

const noModel = {
  ollamaRunning: true,
  ollamaVersion: '0.5.1',
  model: 'llama3.2:3b',
  modelInstalled: false,
  problem: null,
};

const allSet = { ...noModel, modelInstalled: true };

describe('SetupView', () => {
  beforeEach(() => {
    invoke.mockReset();
    listen.mockReset();
    openUrl.mockReset();
    listen.mockResolvedValue(() => undefined);
    openUrl.mockResolvedValue(undefined);
  });

  it('offers to install Ollama when it is not running', async () => {
    invoke.mockResolvedValue(noOllama);

    render(<SetupView onSkip={vi.fn()} />);

    expect(await screen.findByRole('button', { name: 'Get Ollama' })).toBeInTheDocument();
    expect(screen.getByRole('alert')).toHaveTextContent('could not reach Ollama');
  });

  it('opens the download page in the browser', async () => {
    invoke.mockResolvedValue(noOllama);

    render(<SetupView onSkip={vi.fn()} />);
    await userEvent.click(await screen.findByRole('button', { name: 'Get Ollama' }));

    expect(openUrl).toHaveBeenCalledWith('https://ollama.com/download');
  });

  it('offers the model download once Ollama is running', async () => {
    invoke.mockResolvedValue(noModel);

    render(<SetupView onSkip={vi.fn()} />);

    expect(await screen.findByRole('button', { name: 'Download model' })).toBeInTheDocument();
    expect(screen.queryByRole('button', { name: 'Get Ollama' })).not.toBeInTheDocument();
  });

  it('downloads the model and rechecks when it finishes', async () => {
    invoke.mockImplementation((command: string) =>
      Promise.resolve(command === 'pull_model' ? undefined : noModel),
    );

    render(<SetupView onSkip={vi.fn()} />);
    await userEvent.click(await screen.findByRole('button', { name: 'Download model' }));

    expect(invoke).toHaveBeenCalledWith('pull_model');
  });

  it('surfaces a download failure', async () => {
    invoke.mockImplementation((command: string) =>
      command === 'pull_model'
        ? Promise.reject(new Error('no space left on device'))
        : Promise.resolve(noModel),
    );

    render(<SetupView onSkip={vi.fn()} />);
    await userEvent.click(await screen.findByRole('button', { name: 'Download model' }));

    expect(await screen.findByRole('alert')).toHaveTextContent('no space left on device');
  });

  it('invites the user in once everything is in place', async () => {
    const onSkip = vi.fn();
    invoke.mockResolvedValue(allSet);

    render(<SetupView onSkip={onSkip} />);
    await userEvent.click(await screen.findByRole('button', { name: 'Start using Chief' }));

    expect(onSkip).toHaveBeenCalled();
  });
});
