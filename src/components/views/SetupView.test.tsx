import { render, screen, waitFor } from '@testing-library/react';
import userEvent from '@testing-library/user-event';
import { beforeEach, describe, expect, it, vi } from 'vitest';

import { SetupView } from '@/components/views/SetupView';

const invoke = vi.hoisted(() => vi.fn());
const listen = vi.hoisted(() => vi.fn());

vi.mock('@tauri-apps/api/core', () => ({ invoke }));
vi.mock('@tauri-apps/api/event', () => ({ listen }));

const noModel = {
  model: 'Llama 3.2 3B Instruct (Q4_K_M)',
  tier: 'standard',
  modelSizeMb: 2400,
  modelInstalled: false,
  engine: 'down',
  problem: 'the model has not been downloaded yet.',
};

const stoppedEngine = {
  ...noModel,
  tier: 'standard',
  modelSizeMb: 2400,
  modelInstalled: true,
  problem: 'the inference engine started but never began answering.',
};

const loadingEngine = { ...stoppedEngine, engine: 'loading', problem: null };

const allSet = { ...stoppedEngine, engine: 'ready', problem: null };

describe('SetupView', () => {
  beforeEach(() => {
    invoke.mockReset();
    listen.mockReset();
    listen.mockResolvedValue(() => undefined);
  });

  it('asks for the model first, and says why nothing works without it', async () => {
    invoke.mockResolvedValue(noModel);

    render(<SetupView onSkip={vi.fn()} />);

    expect(await screen.findByRole('button', { name: 'Download model' })).toBeInTheDocument();
    expect(screen.getByRole('alert')).toHaveTextContent('not been downloaded');
  });

  it('does not offer to start an engine with no model to load', async () => {
    invoke.mockResolvedValue(noModel);

    render(<SetupView onSkip={vi.fn()} />);
    await screen.findByRole('button', { name: 'Download model' });

    expect(screen.queryByRole('button', { name: 'Start engine' })).not.toBeInTheDocument();
  });

  it('downloads the model and rechecks when it finishes', async () => {
    invoke.mockImplementation((command: string) =>
      Promise.resolve(command === 'download_model' ? undefined : noModel),
    );

    render(<SetupView onSkip={vi.fn()} />);
    await userEvent.click(await screen.findByRole('button', { name: 'Download model' }));

    expect(invoke).toHaveBeenCalledWith('download_model');
  });

  it('surfaces a download failure', async () => {
    invoke.mockImplementation((command: string) =>
      command === 'download_model'
        ? Promise.reject(new Error('no space left on device'))
        : Promise.resolve(noModel),
    );

    render(<SetupView onSkip={vi.fn()} />);
    await userEvent.click(await screen.findByRole('button', { name: 'Download model' }));

    expect(await screen.findByRole('alert')).toHaveTextContent('no space left on device');
  });

  it('offers to start the engine once the model is here', async () => {
    invoke.mockImplementation((command: string) =>
      Promise.resolve(command === 'start_engine' ? undefined : stoppedEngine),
    );

    render(<SetupView onSkip={vi.fn()} />);
    await userEvent.click(await screen.findByRole('button', { name: 'Start engine' }));

    expect(invoke).toHaveBeenCalledWith('start_engine');
    expect(screen.queryByRole('button', { name: 'Download model' })).not.toBeInTheDocument();
  });

  it('surfaces an engine that will not start', async () => {
    invoke.mockImplementation((command: string) =>
      command === 'start_engine'
        ? Promise.reject(new Error("Chief's inference engine is missing from this installation."))
        : Promise.resolve(stoppedEngine),
    );

    render(<SetupView onSkip={vi.fn()} />);
    await userEvent.click(await screen.findByRole('button', { name: 'Start engine' }));

    expect(await screen.findByRole('alert')).toHaveTextContent('engine is missing');
  });

  it('waits rather than complaining while the model loads', async () => {
    invoke.mockResolvedValue(loadingEngine);

    render(<SetupView onSkip={vi.fn()} />);

    expect(await screen.findByRole('button', { name: 'Starting…' })).toBeInTheDocument();
    expect(screen.queryByRole('alert')).not.toBeInTheDocument();
  });

  it('invites the user in once everything is in place', async () => {
    const onSkip = vi.fn();
    invoke.mockResolvedValue(allSet);

    render(<SetupView onSkip={onSkip} />);
    const start = await screen.findByRole('button', { name: 'Start using Chief' });

    expect(onSkip).not.toHaveBeenCalled();

    await userEvent.click(start);

    expect(onSkip).toHaveBeenCalled();
  });

  /**
   * EM-11. A window opened hidden at login meets an engine that is still
   * reading the weights, and the person who set Chief up weeks ago must not be
   * asked to click through a first-run screen when it finishes.
   */
  it('lets itself out when an engine that was loading at launch starts answering', async () => {
    const onSkip = vi.fn();
    invoke.mockResolvedValueOnce(loadingEngine).mockResolvedValue(allSet);

    render(<SetupView onSkip={onSkip} />);

    await waitFor(() => expect(onSkip).toHaveBeenCalled(), { timeout: 4000 });
  });

  it('waits to be told when the engine was not loading at launch', async () => {
    const onSkip = vi.fn();
    invoke.mockResolvedValueOnce(stoppedEngine).mockResolvedValue(allSet);

    render(<SetupView onSkip={onSkip} />);
    await screen.findByRole('button', { name: 'Start engine' });
    await userEvent.click(screen.getByRole('button', { name: 'Start engine' }));
    await screen.findByRole('button', { name: 'Start using Chief' });

    expect(onSkip).not.toHaveBeenCalled();
  });
});
