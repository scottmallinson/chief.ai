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
  title: 'scottmallinson/chief.ai #44: Stop a long answer being thrown away',
  content: 'Merged pull request #44',
  summary: 'Stopped a long answer being thrown away at five minutes.',
  url: 'https://github.com/scottmallinson/chief.ai/pull/44',
  externalId: 'scottmallinson/chief.ai#44',
  accountId: 1,
};

/** The view as `App` renders it, with the parts a test does not care about set. */
function show(props: Partial<Parameters<typeof TodayView>[0]> = {}) {
  return render(
    <TodayView
      brief={brief}
      day="2026-08-29"
      today="2026-08-29"
      status="ready"
      error={null}
      onWrite={vi.fn()}
      onShowToday={vi.fn()}
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

  /**
   * The feed leads with what happened, not with the word for its state.
   *
   * Proved by leading with `summary` again:
   *
   * ```text
   * Unable to find an element with the text:
   * scottmallinson/chief.ai #44: Stop a long answer being thrown away
   * ```
   */
  it('shows what was recently shipped beside it, by name', async () => {
    invoke.mockResolvedValue([shipped]);

    show();

    expect(
      await screen.findByText('scottmallinson/chief.ai #44: Stop a long answer being thrown away'),
    ).toBeInTheDocument();
  });

  it('offers a way to open what a feed row is about', async () => {
    invoke.mockResolvedValue([shipped]);

    show();

    // Named for the entry rather than "Open", so a row read aloud says where
    // it goes.
    expect(
      await screen.findByRole('button', {
        name: 'Open scottmallinson/chief.ai #44: Stop a long answer being thrown away',
      }),
    ).toBeInTheDocument();
  });

  it('offers nothing on a feed row the user typed themselves', async () => {
    invoke.mockResolvedValue([{ ...shipped, title: '', url: null }]);

    show();

    expect(await screen.findByText('Merged pull request #44')).toBeInTheDocument();
    expect(screen.queryByRole('button', { name: /^Open / })).not.toBeInTheDocument();
  });

  it('falls back to the raw entry when there is no title to lead with', async () => {
    invoke.mockResolvedValue([{ ...shipped, title: '', summary: null }]);

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

  /**
   * The guard on REC-55, at the screen.
   *
   * Proved by putting the write control back inside the `brief === null` branch
   * it used to live in:
   *
   * ```text
   * TestingLibraryElementError: Unable to find an accessible element with the
   * role "button" and name "Write it again"
   * ```
   *
   * Which is the reported defect exactly: a brief on screen and no way to ask
   * for today's.
   */
  it('can still be asked for today’s brief while one is on screen', async () => {
    const onWrite = vi.fn();
    show({ onWrite });

    await userEvent.click(screen.getByRole('button', { name: 'Write it again' }));

    expect(onWrite).toHaveBeenCalled();
  });

  it('offers the way back to today while an earlier day is on screen', async () => {
    const onShowToday = vi.fn();
    show({
      brief: { ...brief, date: '2026-08-28', path: 'briefs/2026-08-28.md', sources: [] },
      day: '2026-08-28',
      onShowToday,
    });

    // And not the rewrite: writing a brief writes *today's*, whatever day is
    // being read, so offering it here would be a button that changes a
    // different day from the one on screen.
    expect(screen.queryByRole('button', { name: 'Write it again' })).not.toBeInTheDocument();

    await userEvent.click(screen.getByRole('button', { name: 'Back to today' }));

    expect(onShowToday).toHaveBeenCalled();
  });

  it('does not offer to write a brief for a day that is not today', () => {
    show({ brief: null, day: '2026-08-28' });

    expect(screen.getByRole('heading', { name: 'That day has no brief' })).toBeInTheDocument();
    expect(screen.queryByRole('button', { name: 'Write today’s brief' })).not.toBeInTheDocument();
    expect(screen.getByRole('button', { name: 'Back to today' })).toBeInTheDocument();
  });
});
