import { render, screen } from '@testing-library/react';
import userEvent from '@testing-library/user-event';
import { beforeEach, describe, expect, it, vi } from 'vitest';

import { DraftEditor } from '@/components/DraftEditor';
import type { Proposal } from '@/lib/proposals';

const invoke = vi.hoisted(() => vi.fn());

vi.mock('@tauri-apps/api/core', () => ({ invoke }));

const proposal: Proposal = {
  id: 1,
  source: 'github',
  title: 'Ask for a review on scottmallinson/chief.ai #44',
  context: 'scottmallinson/chief.ai #44',
  path: 'proposed/2026-08-29-scottmallinson-chief-ai-44.md',
  status: 'drafted',
  createdAt: '2026-08-29T09:00:00Z',
  body: 'I would be most grateful if you could review #44.',
};

const editor = { name: 'Draft' } as const;

describe('DraftEditor', () => {
  beforeEach(() => {
    invoke.mockReset();
    invoke.mockResolvedValue(null);
  });

  it('opens with the draft already loaded', () => {
    render(<DraftEditor proposal={proposal} onSaved={vi.fn()} />);

    expect(screen.getByRole('textbox', editor)).toHaveValue(
      'I would be most grateful if you could review #44.',
    );
  });

  it('writes an edit back to the file it came from', async () => {
    const onSaved = vi.fn();
    render(<DraftEditor proposal={proposal} onSaved={onSaved} />);

    const text = screen.getByRole('textbox', editor);
    await userEvent.clear(text);
    await userEvent.type(text, 'Mind reviewing #44?');
    await userEvent.click(screen.getByRole('button', { name: 'Save' }));

    expect(invoke).toHaveBeenCalledWith('save_proposal', {
      path: proposal.path,
      body: 'Mind reviewing #44?',
    });
    expect(onSaved).toHaveBeenCalled();
  });

  it('rewrites the draft in place when asked', async () => {
    invoke.mockImplementation((command: string) =>
      Promise.resolve(command === 'refine_draft' ? 'Mind taking a look at #44?' : null),
    );

    render(<DraftEditor proposal={proposal} onSaved={vi.fn()} />);
    await userEvent.click(screen.getByRole('button', { name: 'Make it less formal' }));

    expect(await screen.findByRole('textbox', editor)).toHaveValue('Mind taking a look at #44?');
  });

  it('sends whatever is in the box, not what Chief first wrote', async () => {
    invoke.mockImplementation((command: string) =>
      Promise.resolve(command === 'refine_draft' ? 'Shorter.' : null),
    );

    render(<DraftEditor proposal={proposal} onSaved={vi.fn()} />);
    const text = screen.getByRole('textbox', editor);
    await userEvent.clear(text);
    await userEvent.type(text, 'My own words.');
    await userEvent.click(screen.getByRole('button', { name: 'Make it shorter' }));

    expect(invoke).toHaveBeenCalledWith('refine_draft', {
      body: 'My own words.',
      instruction: 'Make it shorter',
    });
  });

  it('keeps the draft when a rewrite fails, rather than losing it', async () => {
    invoke.mockImplementation((command: string) =>
      command === 'refine_draft'
        ? Promise.reject(new Error('the engine is not running'))
        : Promise.resolve(null),
    );

    render(<DraftEditor proposal={proposal} onSaved={vi.fn()} />);
    await userEvent.click(screen.getByRole('button', { name: 'Make it less formal' }));

    expect(await screen.findByRole('alert')).toHaveTextContent('the engine is not running');
    expect(screen.getByRole('textbox', editor)).toHaveValue(
      'I would be most grateful if you could review #44.',
    );
  });

  it('says it is working, and will not be asked twice', async () => {
    invoke.mockImplementation((command: string) =>
      command === 'refine_draft' ? new Promise(() => undefined) : Promise.resolve(null),
    );

    render(<DraftEditor proposal={proposal} onSaved={vi.fn()} />);
    const less = screen.getByRole('button', { name: 'Make it less formal' });
    await userEvent.click(less);

    expect(less).toBeDisabled();
    expect(screen.getByRole('status')).toHaveTextContent('Rewriting');
  });

  it('says nothing has been sent, on the screen where it would be', () => {
    render(<DraftEditor proposal={proposal} onSaved={vi.fn()} />);

    expect(screen.getByText(/Chief has sent nothing/)).toBeInTheDocument();
  });
});
