import { fireEvent, render, screen } from '@testing-library/react';
import userEvent from '@testing-library/user-event';
import { act } from 'react';
import { beforeEach, describe, expect, it, vi } from 'vitest';

import { ChatView } from '@/components/views/ChatView';

const invoke = vi.hoisted(() => vi.fn());
const listen = vi.hoisted(() => vi.fn());

vi.mock('@tauri-apps/api/core', () => ({ invoke }));
vi.mock('@tauri-apps/api/event', () => ({ listen }));

/** The handler the view registered for streamed updates. */
type Handler = (event: { payload: Record<string, unknown> }) => void;

let handlers: Handler[] = [];

/** Push an update to whatever the view is currently listening with. */
function stream(payload: Record<string, unknown>) {
  act(() => {
    for (const handler of handlers) handler({ payload });
  });
}

/** The request id the view generated for the question in flight. */
function requestId(): string {
  const call = invoke.mock.lastCall as [string, { requestId: string }] | undefined;

  return call?.[1].requestId ?? '';
}

/** Answer the question that is in flight, as the backend eventually would. */
function answer(reply: string) {
  resolve?.(reply);
}

let resolve: ((reply: string) => void) | undefined;

describe('ChatView', () => {
  beforeEach(() => {
    invoke.mockReset();
    listen.mockReset();
    handlers = [];
    resolve = undefined;

    listen.mockImplementation((_event: string, handler: Handler) => {
      handlers.push(handler);

      return Promise.resolve(() => {
        handlers = handlers.filter((registered) => registered !== handler);
      });
    });
  });

  /** Hold the answer open so the streaming states can be observed. */
  function holdTheAnswer() {
    invoke.mockImplementation(
      () =>
        new Promise<string>((settle) => {
          resolve = settle;
        }),
    );
  }

  async function ask(question: string) {
    await userEvent.type(
      screen.getByRole('textbox', { name: 'Message your chief of staff' }),
      `${question}{Enter}`,
    );
  }

  it('invites a question before anything is asked', () => {
    render(<ChatView />);

    expect(screen.getByRole('heading', { name: 'Ask about your work' })).toBeInTheDocument();
  });

  it('sends the question to the local model and shows the reply', async () => {
    invoke.mockResolvedValue('You merged two pull requests.');

    render(<ChatView />);
    await userEvent.type(
      screen.getByRole('textbox', { name: 'Message your chief of staff' }),
      'What did I ship?',
    );
    await userEvent.click(screen.getByRole('button', { name: 'Send message' }));

    expect(await screen.findByText('You merged two pull requests.')).toBeInTheDocument();
    expect(screen.getByText('What did I ship?')).toBeInTheDocument();
    expect(invoke).toHaveBeenCalledWith('ask_agent', {
      messages: [{ role: 'user', content: 'What did I ship?' }],
      requestId: expect.any(String) as string,
    });
  });

  it('shows the answer as it is written rather than waiting for all of it', async () => {
    holdTheAnswer();

    render(<ChatView />);
    await ask('What did I ship?');

    stream({ requestId: requestId(), kind: 'delta', text: 'You merged ' });
    expect(await screen.findByText(/You merged/)).toBeInTheDocument();

    stream({ requestId: requestId(), kind: 'delta', text: 'two pull requests.' });
    expect(await screen.findByText(/You merged two pull requests\./)).toBeInTheDocument();
  });

  it('ignores updates belonging to another question', async () => {
    holdTheAnswer();

    render(<ChatView />);
    await ask('What did I ship?');

    stream({ requestId: 'a-different-question', kind: 'delta', text: 'Not for you.' });

    expect(screen.queryByText('Not for you.')).not.toBeInTheDocument();
    expect(await screen.findByRole('status')).toHaveTextContent('local · Sent to model');
  });

  it('says which tool it is waiting on', async () => {
    holdTheAnswer();

    render(<ChatView />);
    await ask('What is waiting on me?');

    stream({ requestId: requestId(), kind: 'tool', name: 'fetch_github_prs' });

    expect(await screen.findByRole('status')).toHaveTextContent(
      'local · Reading your pull requests on GitHub',
    );
  });

  it('takes back thinking out loud that turned into a tool call', async () => {
    holdTheAnswer();

    render(<ChatView />);
    await ask('What is waiting on me?');

    stream({ requestId: requestId(), kind: 'delta', text: 'Let me look.' });
    expect(await screen.findByText(/Let me look\./)).toBeInTheDocument();

    stream({ requestId: requestId(), kind: 'restart' });
    expect(screen.queryByText(/Let me look\./)).not.toBeInTheDocument();
  });

  it('shows the finished answer once, not alongside the streamed one', async () => {
    holdTheAnswer();

    render(<ChatView />);
    await ask('What did I ship?');

    stream({ requestId: requestId(), kind: 'delta', text: 'You merged two pull requests.' });
    answer('You merged two pull requests.');

    // The finished message is labelled in the past tense; the streamed one is
    // not, so this is what tells them apart.
    expect(await screen.findByText('Chief said:')).toBeInTheDocument();
    expect(screen.getAllByText('You merged two pull requests.')).toHaveLength(1);
    expect(screen.queryByRole('status')).not.toBeInTheDocument();
  });

  it('sends the whole transcript so the model keeps context', async () => {
    invoke.mockResolvedValueOnce('Two.').mockResolvedValueOnce('Both were merged.');

    render(<ChatView />);
    const composer = screen.getByRole('textbox', { name: 'Message your chief of staff' });

    await userEvent.type(composer, 'How many?{Enter}');
    expect(await screen.findByText('Two.')).toBeInTheDocument();

    await userEvent.type(composer, 'And their state?{Enter}');
    expect(await screen.findByText('Both were merged.')).toBeInTheDocument();

    expect(invoke).toHaveBeenLastCalledWith('ask_agent', {
      messages: [
        { role: 'user', content: 'How many?' },
        { role: 'assistant', content: 'Two.' },
        { role: 'user', content: 'And their state?' },
      ],
      requestId: expect.any(String) as string,
    });
  });

  it('clears the composer once a question is sent', async () => {
    invoke.mockResolvedValue('Answered.');

    render(<ChatView />);
    const composer = screen.getByRole('textbox', { name: 'Message your chief of staff' });

    await userEvent.type(composer, 'What is next?{Enter}');

    expect(await screen.findByText('Answered.')).toBeInTheDocument();
    expect(composer).toHaveValue('');
  });

  it('tells the user when the local model cannot be reached', async () => {
    invoke.mockRejectedValue(
      new Error("Chief's local model engine is not running at http://127.0.0.1:11435/."),
    );

    render(<ChatView />);
    await userEvent.type(
      screen.getByRole('textbox', { name: 'Message your chief of staff' }),
      'Anything there?{Enter}',
    );

    expect(await screen.findByRole('alert')).toHaveTextContent('local model engine is not running');
  });

  it('stops listening once a question is answered', async () => {
    invoke.mockResolvedValue('Answered.');

    render(<ChatView />);
    await userEvent.type(
      screen.getByRole('textbox', { name: 'Message your chief of staff' }),
      'What is next?{Enter}',
    );

    expect(await screen.findByText('Answered.')).toBeInTheDocument();
    expect(handlers).toHaveLength(0);
  });

  it('says how many sources an answer drew on, so it is not taken on trust', async () => {
    holdTheAnswer();

    render(<ChatView />);
    await ask('What is waiting on me?');

    stream({ requestId: requestId(), kind: 'tool', name: 'fetch_github_prs' });
    // The same tool twice is still one source.
    stream({ requestId: requestId(), kind: 'tool', name: 'fetch_github_prs' });
    answer('Two are waiting on you.');

    expect(await screen.findByText('chief · local · 1 source')).toBeInTheDocument();
  });

  it('claims no sources for an answer the model wrote unaided', async () => {
    invoke.mockResolvedValue('Nothing is waiting on you.');

    render(<ChatView />);
    await ask('What is waiting on me?');

    expect(await screen.findByText('chief · local')).toBeInTheDocument();
  });

  /**
   * Ask without `userEvent`, which waits on real timers of its own and so
   * cannot be driven while the clock is mocked. The wait these two are about
   * starts the moment the question is sent, so the clock has to be mocked
   * before that happens.
   */
  function askOnAMockedClock(question: string) {
    const composer = screen.getByRole('textbox', { name: 'Message your chief of staff' });

    fireEvent.change(composer, { target: { value: question } });
    fireEvent.submit(composer.closest('form') as HTMLFormElement);
  }

  it('says how long a wait has been going once it passes three seconds', async () => {
    vi.useFakeTimers();

    try {
      holdTheAnswer();
      render(<ChatView />);
      askOnAMockedClock('What did I ship?');

      expect(screen.getByRole('status')).toHaveTextContent('local · Sent to model');

      await act(async () => {
        await vi.advanceTimersByTimeAsync(4000);
      });

      expect(screen.getByRole('status')).toHaveTextContent('local · Sent to model · 4s');
    } finally {
      vi.useRealTimers();
    }
  });

  it('explains a long wait rather than leaving the reader guessing', async () => {
    vi.useFakeTimers();

    try {
      holdTheAnswer();
      render(<ChatView />);
      askOnAMockedClock('What did I ship?');

      await act(async () => {
        await vi.advanceTimersByTimeAsync(4000);
      });
      expect(screen.queryByText(/Nothing has stalled/)).not.toBeInTheDocument();

      await act(async () => {
        await vi.advanceTimersByTimeAsync(12_000);
      });
      expect(screen.getByText(/Nothing has stalled/)).toBeInTheDocument();
    } finally {
      vi.useRealTimers();
    }
  });

  it('offers an opening question and puts it in the composer', async () => {
    render(<ChatView />);

    await userEvent.click(screen.getByRole('button', { name: 'Draft my standup' }));

    expect(screen.getByRole('textbox', { name: 'Message your chief of staff' })).toHaveValue(
      'Draft my standup',
    );
  });

  it('will not send an empty question', async () => {
    render(<ChatView />);

    await userEvent.type(
      screen.getByRole('textbox', { name: 'Message your chief of staff' }),
      '   {Enter}',
    );

    expect(invoke).not.toHaveBeenCalled();
  });
});
