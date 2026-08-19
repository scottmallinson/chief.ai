import { render, screen } from '@testing-library/react';
import userEvent from '@testing-library/user-event';
import { beforeEach, describe, expect, it, vi } from 'vitest';

import { ChatView } from '@/components/views/ChatView';

const invoke = vi.hoisted(() => vi.fn());

vi.mock('@tauri-apps/api/core', () => ({ invoke }));

describe('ChatView', () => {
  beforeEach(() => {
    invoke.mockReset();
  });

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
    });
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
      new Error('could not reach Ollama at http://localhost:11434/. Is it running?'),
    );

    render(<ChatView />);
    await userEvent.type(
      screen.getByRole('textbox', { name: 'Message your chief of staff' }),
      'Anything there?{Enter}',
    );

    expect(await screen.findByRole('alert')).toHaveTextContent('could not reach Ollama');
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
