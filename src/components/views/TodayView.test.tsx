import { render, screen } from '@testing-library/react';
import userEvent from '@testing-library/user-event';
import { beforeEach, describe, expect, it, vi } from 'vitest';

import { TodayView } from '@/components/views/TodayView';
import type { Brief } from '@/lib/brief';
import type { WorkLogEntry } from '@/lib/work-log';

const invoke = vi.hoisted(() => vi.fn());

vi.mock('@tauri-apps/api/core', () => ({ invoke }));

const brief: Brief = {
  date: '2026-08-29',
  path: 'briefs/2026-08-29.md',
  markdown: '- Standup at 09:30 with Ana\n- Review chief.ai #44 before the retro',
  sources: ['calendar', 'github'],
};

const shipped: WorkLogEntry = {
  id: 1,
  timestamp: '2026-08-28T16:00:00Z',
  source: 'github',
  content: 'Merged pull request #44',
  summary: 'Stopped a long answer being thrown away at five minutes.',
  externalId: 'scottmallinson/chief.ai#44',
  accountId: 1,
};

/** The view as `App` renders it, with the parts a test does not care about set. */
function show(props: Partial<Parameters<typeof TodayView>[0]> = {}) {
  return render(
    <TodayView
      brief={brief}
      status="ready"
      error={null}
      onWrite={vi.fn()}
      proposals={[]}
      onEditProposal={vi.fn()}
      onProposalDismissed={vi.fn()}
      {...props}
    />,
  );
}

describe('TodayView', () => {
  beforeEach(() => {
    invoke.mockReset();
    invoke.mockResolvedValue([]);
  });

  it('reads the brief without anybody opening a file manager', () => {
    show();

    expect(screen.getByText('Standup at 09:30 with Ana')).toBeInTheDocument();
    expect(screen.getByText('Review chief.ai #44 before the retro')).toBeInTheDocument();
  });

  it('says which day it is, and where the brief came from', () => {
    // Against the same formatter rather than a literal: how a date reads is
    // Intl's business and the reader's locale, and CI does not keep ours.
    const day = new Intl.DateTimeFormat(undefined, { dateStyle: 'full' }).format(
      new Date(2026, 7, 29),
    );

    show();

    expect(screen.getByRole('heading', { name: day })).toBeInTheDocument();
    expect(screen.getByText('briefs/2026-08-29.md')).toBeInTheDocument();
  });

  it('names the sources that had something to say', () => {
    show();

    expect(screen.getByText('calendar')).toBeInTheDocument();
    expect(screen.getByText('github')).toBeInTheDocument();
  });

  it('shows what was recently shipped beside it', async () => {
    invoke.mockResolvedValue([shipped]);

    show();

    expect(
      await screen.findByText('Stopped a long answer being thrown away at five minutes.'),
    ).toBeInTheDocument();
  });

  it('falls back to the raw entry when nothing has summarised it yet', async () => {
    invoke.mockResolvedValue([{ ...shipped, summary: null }]);

    show();

    expect(await screen.findByText('Merged pull request #44')).toBeInTheDocument();
  });

  it('shows the drafts Chief prepared, and says none were sent', () => {
    show({
      proposals: [
        {
          id: 1,
          source: 'github',
          title: 'Ask for a review on scottmallinson/chief.ai #44',
          context: 'scottmallinson/chief.ai #44',
          path: 'proposed/2026-08-29-scottmallinson-chief-ai-44.md',
          status: 'drafted',
          createdAt: '2026-08-29T09:00:00Z',
          body: 'Could you take a look at #44?',
        },
      ],
    });

    expect(screen.getByText('Drafted for you · nothing sent')).toBeInTheDocument();
    expect(screen.getByText('Could you take a look at #44?')).toBeInTheDocument();
  });

  it('offers to write one when today has no brief yet', () => {
    show({ brief: null });

    expect(screen.getByRole('button', { name: 'Write today’s brief' })).toBeInTheDocument();
  });

  it('asks for a brief rather than writing one itself', async () => {
    const onWrite = vi.fn();
    show({ brief: null, onWrite });

    await userEvent.click(screen.getByRole('button', { name: 'Write today’s brief' }));

    expect(onWrite).toHaveBeenCalled();
  });

  it('says it is working while the model writes', () => {
    show({ brief: null, status: 'writing' });

    expect(screen.getByRole('status')).toHaveTextContent('Writing your brief');
    expect(screen.getByRole('button', { name: 'Write today’s brief' })).toBeDisabled();
  });

  it('says so in words when the brief cannot be written', () => {
    show({ brief: null, status: 'error', error: 'nothing to write a brief from yet.' });

    expect(screen.getByRole('alert')).toHaveTextContent('nothing to write a brief from yet.');
  });

  it('keeps the brief on screen when reading another day failed', () => {
    show({ status: 'error', error: 'briefs/2026-08-28.md is not in the corpus.' });

    expect(screen.getByRole('alert')).toBeInTheDocument();
    expect(screen.getByText('Standup at 09:30 with Ana')).toBeInTheDocument();
  });
});
