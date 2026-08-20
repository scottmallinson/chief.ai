import {
  useEffect,
  useRef,
  useState,
  type FormEvent,
  type KeyboardEvent,
  type UIEvent,
} from 'react';
import { Send } from 'lucide-react';

import { Button } from '@/components/ui/button';
import { useChat } from '@/hooks/use-chat';
import { cn } from '@/lib/utils';
import type { ChatMessage } from '@/lib/agent';

/** How close to the bottom still counts as reading the live end, in pixels. */
const FOLLOW_THRESHOLD = 32;

function Bubble({ message }: { message: ChatMessage }) {
  const isUser = message.role === 'user';

  return (
    <li className={cn('flex', isUser ? 'justify-end' : 'justify-start')}>
      <div
        className={cn(
          'max-w-[80%] rounded-lg px-3 py-2 text-sm whitespace-pre-wrap',
          isUser ? 'bg-primary text-primary-foreground' : 'bg-muted text-foreground',
        )}
        data-selectable
      >
        <span className="sr-only">{isUser ? 'You said: ' : 'Chief said: '}</span>
        {message.content}
      </div>
    </li>
  );
}

/** The answer as it is being written, with a cursor to show it is still going. */
function Writing({ text }: { text: string }) {
  return (
    <li className="flex justify-start">
      <div
        className="max-w-[80%] rounded-lg bg-muted px-3 py-2 text-sm whitespace-pre-wrap text-foreground"
        aria-live="polite"
        data-selectable
      >
        <span className="sr-only">Chief is saying: </span>
        {text}
        <span className="ml-0.5 inline-block h-4 w-px animate-pulse bg-foreground align-middle" />
      </div>
    </li>
  );
}

function EmptyState() {
  return (
    <div className="flex h-full items-center justify-center p-6">
      <div className="max-w-md text-center">
        <h2 className="text-lg font-semibold">Ask about your work</h2>
        <p className="mt-2 text-sm text-muted-foreground">
          &ldquo;What did I ship this week?&rdquo; &middot; &ldquo;Which pull requests are still
          waiting on me?&rdquo;
        </p>
        <p className="mt-6 text-xs text-muted-foreground">
          Answers come from a model running on this machine. Nothing you type is sent anywhere else.
        </p>
      </div>
    </div>
  );
}

/** Chat surface for the agent, backed by the local model. */
export function ChatView() {
  const { messages, status, partial, activity, error, send } = useChat();
  const [draft, setDraft] = useState('');
  const transcript = useRef<HTMLDivElement>(null);
  // Whether the reader is at the live end of the answer. Someone who has
  // scrolled up to re-read something is not dragged back down by the next token.
  const following = useRef(true);

  const isThinking = status === 'thinking';

  // Follow the answer as it is written. This sets `scrollTop` on the transcript
  // itself rather than calling `scrollIntoView`, which walks *every* scrollable
  // ancestor — including the document — and so can scroll the window rather
  // than the conversation.
  useEffect(() => {
    const container = transcript.current;
    if (container === null || !following.current) return;

    container.scrollTop = container.scrollHeight;
  }, [messages, partial, activity]);

  function trackPosition(event: UIEvent<HTMLDivElement>) {
    const { scrollTop, scrollHeight, clientHeight } = event.currentTarget;

    following.current = scrollHeight - scrollTop - clientHeight <= FOLLOW_THRESHOLD;
  }

  function submit(event: FormEvent) {
    event.preventDefault();
    if (draft.trim() === '' || isThinking) return;

    send(draft);
    setDraft('');
  }

  function handleKeyDown(event: KeyboardEvent<HTMLTextAreaElement>) {
    // Enter sends, Shift+Enter starts a new line.
    if (event.key === 'Enter' && !event.shiftKey) {
      event.preventDefault();
      event.currentTarget.form?.requestSubmit();
    }
  }

  return (
    <div className="flex h-full flex-col">
      <div className="min-h-0 flex-1 overflow-y-auto" ref={transcript} onScroll={trackPosition}>
        {messages.length === 0 ? (
          <EmptyState />
        ) : (
          <ul className="mx-auto max-w-3xl space-y-3 p-6">
            {messages.map((message) => (
              <Bubble key={message.id} message={message} />
            ))}
            {isThinking && partial !== '' && <Writing text={partial} />}
            {isThinking && partial === '' && (
              <li className="text-sm text-muted-foreground" role="status">
                {activity === null ? 'Thinking…' : `${activity}…`}
              </li>
            )}
          </ul>
        )}
      </div>

      {error !== null && (
        <div className="mx-auto w-full max-w-3xl px-4">
          <p
            className="rounded-md border border-destructive/50 p-3 text-sm text-muted-foreground"
            role="alert"
          >
            {error}
          </p>
        </div>
      )}

      <form className="border-t border-border p-4" onSubmit={submit}>
        <div className="mx-auto flex max-w-3xl items-end gap-2">
          <textarea
            rows={1}
            value={draft}
            onChange={(event) => setDraft(event.target.value)}
            onKeyDown={handleKeyDown}
            placeholder="Message your chief of staff…"
            aria-label="Message your chief of staff"
            className="max-h-40 min-h-9 flex-1 resize-none rounded-md border border-input bg-background px-3 py-2 text-sm focus-visible:ring-2 focus-visible:ring-ring focus-visible:outline-none disabled:opacity-60"
          />
          <Button type="submit" size="icon" disabled={isThinking || draft.trim() === ''}>
            <Send aria-hidden />
            <span className="sr-only">Send message</span>
          </Button>
        </div>
      </form>
    </div>
  );
}
