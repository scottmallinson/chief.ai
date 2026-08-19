import { useCallback, useRef, useState } from 'react';

import { askAgent, type ChatMessage } from '@/lib/agent';

type Status = 'idle' | 'thinking';

interface UseChat {
  messages: ChatMessage[];
  status: Status;
  error: string | null;
  send: (content: string) => void;
}

/** Hold a conversation with the local model. */
export function useChat(): UseChat {
  const [messages, setMessages] = useState<ChatMessage[]>([]);
  const [status, setStatus] = useState<Status>('idle');
  const [error, setError] = useState<string | null>(null);
  const nextId = useRef(0);

  const identify = useCallback(() => {
    nextId.current += 1;
    return `message-${nextId.current}`;
  }, []);

  const send = useCallback(
    (content: string) => {
      const question = content.trim();
      if (question === '' || status === 'thinking') return;

      const asked: ChatMessage = { id: identify(), role: 'user', content: question };
      const transcript = [...messages, asked];

      setMessages(transcript);
      setStatus('thinking');
      setError(null);

      askAgent(transcript.map(({ role, content: text }) => ({ role, content: text })))
        .then((reply) => {
          setMessages((current) => [
            ...current,
            { id: identify(), role: 'assistant', content: reply },
          ]);
        })
        .catch((cause: unknown) => {
          setError(cause instanceof Error ? cause.message : String(cause));
        })
        .finally(() => setStatus('idle'));
    },
    [identify, messages, status],
  );

  return { messages, status, error, send };
}
