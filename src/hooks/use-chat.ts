import { useCallback, useRef, useState } from 'react';

import { askAgent, describeTool, type ChatMessage } from '@/lib/agent';

type Status = 'idle' | 'thinking';

interface UseChat {
  messages: ChatMessage[];
  status: Status;
  /** The answer being written right now, before the model has finished. */
  partial: string;
  /** What the model is busy doing, when it is not writing. */
  activity: string | null;
  error: string | null;
  send: (content: string) => void;
}

/** Hold a conversation with the local model. */
export function useChat(): UseChat {
  const [messages, setMessages] = useState<ChatMessage[]>([]);
  const [status, setStatus] = useState<Status>('idle');
  const [partial, setPartial] = useState('');
  const [activity, setActivity] = useState<string | null>(null);
  const [error, setError] = useState<string | null>(null);
  const nextId = useRef(0);

  const identify = useCallback((kind: string) => {
    nextId.current += 1;
    return `${kind}-${nextId.current}`;
  }, []);

  const settle = useCallback(() => {
    setPartial('');
    setActivity(null);
    setStatus('idle');
  }, []);

  const send = useCallback(
    (content: string) => {
      const question = content.trim();
      if (question === '' || status === 'thinking') return;

      const asked: ChatMessage = { id: identify('message'), role: 'user', content: question };
      const transcript = [...messages, asked];

      setMessages(transcript);
      setStatus('thinking');
      setPartial('');
      setActivity(null);
      setError(null);

      askAgent(
        transcript.map(({ role, content: text }) => ({ role, content: text })),
        identify('request'),
        (update) => {
          switch (update.kind) {
            case 'delta':
              setActivity(null);
              setPartial((written) => written + update.text);
              break;
            case 'restart':
              // What was written turned out to be preamble to a tool call.
              setPartial('');
              break;
            case 'tool':
              setPartial('');
              setActivity(describeTool(update.name));
              break;
          }
        },
      )
        .then((reply) => {
          // Cleared in the same update as the finished message, so the answer
          // is never on screen twice.
          settle();
          setMessages((current) => [
            ...current,
            { id: identify('message'), role: 'assistant', content: reply },
          ]);
        })
        .catch((cause: unknown) => {
          settle();
          setError(cause instanceof Error ? cause.message : String(cause));
        });
    },
    [identify, messages, settle, status],
  );

  return { messages, status, partial, activity, error, send };
}
