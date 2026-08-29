import { render, screen } from '@testing-library/react';
import userEvent from '@testing-library/user-event';
import { beforeEach, describe, expect, it, vi } from 'vitest';

import { ProposalCard } from '@/components/ProposalCard';
import type { Proposal } from '@/lib/proposals';

const invoke = vi.hoisted(() => vi.fn());

vi.mock('@tauri-apps/api/core', () => ({ invoke }));

const proposal: Proposal = {
  id: 1,
  source: 'github',
  title: 'Ask for a review on scottmallinson/chief.ai #44',
  context: 'scottmallinson/chief.ai #44: Stop a long answer being thrown away\nOpen since…',
  path: 'proposed/2026-08-29-scottmallinson-chief-ai-44.md',
  status: 'drafted',
  createdAt: '2026-08-29T09:00:00Z',
  body: '# Ask for a review\n\nCould you take a look at #44 when you get a moment?',
};

function show(overrides: Partial<Parameters<typeof ProposalCard>[0]> = {}) {
  return render(
    <ProposalCard proposal={proposal} onEdit={vi.fn()} onDismissed={vi.fn()} {...overrides} />,
  );
}

describe('ProposalCard', () => {
  beforeEach(() => {
    invoke.mockReset();
    invoke.mockResolvedValue(null);
  });

  it('says what it drafted and what it is about', () => {
    show();

    expect(screen.getByText('Ask for a review on scottmallinson/chief.ai #44')).toBeInTheDocument();
    expect(screen.getByText(/Could you take a look at #44/)).toBeInTheDocument();
  });

  it('says plainly that nothing has been sent', () => {
    show();

    expect(screen.getByText('Not sent')).toBeInTheDocument();
  });

  it('names the file, so the draft can be opened outside Chief', () => {
    show();

    expect(
      screen.getByText('proposed/2026-08-29-scottmallinson-chief-ai-44.md'),
    ).toBeInTheDocument();
  });

  it('hands the draft to the drawer to edit', async () => {
    const onEdit = vi.fn();
    show({ onEdit });

    await userEvent.click(screen.getByRole('button', { name: 'Edit' }));

    expect(onEdit).toHaveBeenCalledWith(proposal);
  });

  it('dismisses on request, and tells its owner so', async () => {
    const onDismissed = vi.fn();
    show({ onDismissed });

    await userEvent.click(screen.getByRole('button', { name: 'Dismiss' }));

    expect(invoke).toHaveBeenCalledWith('dismiss_proposal', { id: 1 });
    expect(onDismissed).toHaveBeenCalledWith(1);
  });

  it('keeps the card when dismissing fails, rather than losing it quietly', async () => {
    const onDismissed = vi.fn();
    invoke.mockRejectedValue(new Error('the local database is not available'));
    show({ onDismissed });

    await userEvent.click(screen.getByRole('button', { name: 'Dismiss' }));

    expect(await screen.findByRole('alert')).toHaveTextContent(
      'the local database is not available',
    );
    expect(onDismissed).not.toHaveBeenCalled();
  });

  it('strips the heading Chief wrote, since the card already says it', () => {
    show();

    expect(screen.queryByText('# Ask for a review')).not.toBeInTheDocument();
  });
});
