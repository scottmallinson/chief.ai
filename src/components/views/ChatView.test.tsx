import { fireEvent, render, screen } from '@testing-library/react';
import userEvent from '@testing-library/user-event';
import { act } from 'react';
import { beforeEach, describe, expect, it, vi } from 'vitest';

import { ChatView } from '@/components/views/ChatView';
import openers from '@/lib/openers.json';
import type { Answer } from '@/lib/agent';
import { useChat } from '@/hooks/use-chat';

/** The view as `App` composes it: the conversation is owned outside it. */
function Chat() {
  return <ChatView chat={useChat()} />;
}

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

/**
 * What `ask_agent` actually resolves with.
 *
 * The shape, not a bare string: a stub that answers one thing to everything is
 * what CLAUDE.md calls the most expensive shortcut in this repository, and the
 * provenance footer is exactly the kind of field a view reaches for and finds
 * `undefined`.
 */
function replied(content: string, provenance: string | null = null): Answer {
  return { content, provenance };
}

/** Answer the question that is in flight, as the backend eventually would. */
function answer(reply: string, provenance: string | null = null) {
  resolve?.(replied(reply, provenance));
}

let resolve: ((reply: Answer) => void) | undefined;

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
        new Promise<Answer>((settle) => {
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
    render(<Chat />);

    expect(screen.getByRole('heading', { name: 'Ask about your work' })).toBeInTheDocument();
  });

  /**
   * The composer offers exactly what the shared file says, and nothing of its
   * own.
   *
   * This screen used to hold its own array of opener strings, and all three of
   * them missed the router — so the one screen that suggests what to ask
   * suggested three things `intent` was built to catch and caught none of. The
   * Rust side asserts each of these routes as it claims to; this side asserts
   * the screen is rendering that list rather than a copy that has drifted from
   * it.
   *
   * Proved by putting a literal back in `ChatView`:
   *
   * ```text
   * → the composer must offer the shared list, not one of its own
   *   - Expected: [ 'What did I ship this week?', … ]
   *   + Received: [ 'What did I ship this week?', 'Draft my standup', … ]
   * ```
   */
  it('offers the questions the router is checked against', () => {
    render(<Chat />);

    const offered = screen
      .getAllByRole('button')
      .map((button) => button.textContent)
      .filter((label) => openers.some((opener) => opener.question === label));

    expect(offered, 'the composer must offer the shared list, not one of its own').toEqual(
      openers.map((opener) => opener.question),
    );
  });

  it('sends the question to the local model and shows the reply', async () => {
    invoke.mockResolvedValue(replied('You merged two pull requests.'));

    render(<Chat />);
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

    render(<Chat />);
    await ask('What did I ship?');

    stream({ requestId: requestId(), kind: 'delta', text: 'You merged ' });
    expect(await screen.findByText(/You merged/)).toBeInTheDocument();

    stream({ requestId: requestId(), kind: 'delta', text: 'two pull requests.' });
    expect(await screen.findByText(/You merged two pull requests\./)).toBeInTheDocument();
  });

  it('ignores updates belonging to another question', async () => {
    holdTheAnswer();

    render(<Chat />);
    await ask('What did I ship?');

    stream({ requestId: 'a-different-question', kind: 'delta', text: 'Not for you.' });

    expect(screen.queryByText('Not for you.')).not.toBeInTheDocument();
    expect(await screen.findByRole('status')).toHaveTextContent('local · Sent to model');
  });

  it('says which tool it is waiting on', async () => {
    holdTheAnswer();

    render(<Chat />);
    await ask('What is waiting on me?');

    stream({ requestId: requestId(), kind: 'tool', name: 'fetch_github_prs' });

    expect(await screen.findByRole('status')).toHaveTextContent(
      'local · Reading your pull requests on GitHub',
    );
  });

  it('takes back thinking out loud that turned into a tool call', async () => {
    holdTheAnswer();

    render(<Chat />);
    await ask('What is waiting on me?');

    stream({ requestId: requestId(), kind: 'delta', text: 'Let me look.' });
    expect(await screen.findByText(/Let me look\./)).toBeInTheDocument();

    stream({ requestId: requestId(), kind: 'restart' });
    expect(screen.queryByText(/Let me look\./)).not.toBeInTheDocument();
  });

  it('shows the finished answer once, not alongside the streamed one', async () => {
    holdTheAnswer();

    render(<Chat />);
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
    invoke
      .mockResolvedValueOnce(replied('Two.'))
      .mockResolvedValueOnce(replied('Both were merged.'));

    render(<Chat />);
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
    invoke.mockResolvedValue(replied('Answered.'));

    render(<Chat />);
    const composer = screen.getByRole('textbox', { name: 'Message your chief of staff' });

    await userEvent.type(composer, 'What is next?{Enter}');

    expect(await screen.findByText('Answered.')).toBeInTheDocument();
    expect(composer).toHaveValue('');
  });

  it('tells the user when the local model cannot be reached', async () => {
    invoke.mockRejectedValue(
      new Error("Chief's local model engine is not running at http://127.0.0.1:11435/."),
    );

    render(<Chat />);
    await userEvent.type(
      screen.getByRole('textbox', { name: 'Message your chief of staff' }),
      'Anything there?{Enter}',
    );

    expect(await screen.findByRole('alert')).toHaveTextContent('local model engine is not running');
  });

  it('stops listening once a question is answered', async () => {
    invoke.mockResolvedValue(replied('Answered.'));

    render(<Chat />);
    await userEvent.type(
      screen.getByRole('textbox', { name: 'Message your chief of staff' }),
      'What is next?{Enter}',
    );

    expect(await screen.findByText('Answered.')).toBeInTheDocument();
    expect(handlers).toHaveLength(0);
  });

  describe('where an answer came from', () => {
    /**
     * The footer replaced a count of the tool names seen on the stream. That
     * count stopped meaning anything once a read became a query: an answer
     * built entirely from the user's own work log would have shown zero.
     */
    it('says where an answer came from, under the answer', async () => {
      invoke.mockResolvedValue(
        replied('You merged two pull requests.', 'Sources: work log · GitHub (to 14:02)'),
      );

      render(<Chat />);
      await ask('What did I ship?');

      expect(await screen.findByText('Sources: work log · GitHub (to 14:02)')).toBeInTheDocument();
    });

    /**
     * **The one piece of text on the screen that has to be true.**
     *
     * The provenance line is the only thing left telling the reader how fresh
     * an answer is, now that a read no longer takes as long as somebody else's
     * API decides — so a model that writes its own `Sources:` line must not be
     * able to pass it off as this one. Rust composes the footer from the rows
     * and the renderer puts it in its own element.
     *
     * Proved by rendering `message.content` where the footer goes:
     *
     *   Unable to find an element with the text: Sources: work log · GitHub
     */
    it('cannot be made to say something the model wrote', async () => {
      invoke.mockResolvedValue(
        replied(
          'Everything is fine.\n\nSources: every system, fully verified',
          'Sources: work log · GitHub',
        ),
      );

      render(<Chat />);
      await ask('What did I ship?');

      // The real footer is on screen, in its own element…
      expect(await screen.findByText('Sources: work log · GitHub')).toBeInTheDocument();
      // …and the model's imitation is nowhere but inside the answer's body.
      expect(screen.queryByText('Sources: every system, fully verified')).not.toBeInTheDocument();
    });

    /**
     * An answer the tool loop produced read no stored rows, so there is
     * nothing to name and no "to" that would mean anything. An empty footer
     * claiming otherwise would be worse than none.
     */
    it('shows no footer at all when nothing local was read', async () => {
      invoke.mockResolvedValue(replied('Nothing is waiting on you.'));

      render(<Chat />);
      await ask('What is waiting on me?');

      expect(await screen.findByText('Nothing is waiting on you.')).toBeInTheDocument();
      expect(screen.queryByText(/^Sources:/)).not.toBeInTheDocument();
    });

    /** The label above the answer is unchanged: it says where it ran. */
    it('still says the answer was produced on this machine', async () => {
      invoke.mockResolvedValue(replied('Nothing is waiting on you.'));

      render(<Chat />);
      await ask('What is waiting on me?');

      expect(await screen.findByText('chief · local')).toBeInTheDocument();
    });
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
      render(<Chat />);
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
      render(<Chat />);
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
    render(<Chat />);

    await userEvent.click(screen.getByRole('button', { name: 'Draft my standup' }));

    expect(screen.getByRole('textbox', { name: 'Message your chief of staff' })).toHaveValue(
      'Draft my standup',
    );
  });

  it('will not send an empty question', async () => {
    render(<Chat />);

    await userEvent.type(
      screen.getByRole('textbox', { name: 'Message your chief of staff' }),
      '   {Enter}',
    );

    expect(invoke).not.toHaveBeenCalled();
  });
});
