import { render, screen } from '@testing-library/react';
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
    await userEvent.click(await screen.findByRole('button', { name: 'Start using Chief' }));

    expect(onSkip).toHaveBeenCalled();
  });
});
