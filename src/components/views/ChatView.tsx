import {
  useEffect,
  useRef,
  useState,
  type FormEvent,
  type KeyboardEvent,
  type ReactNode,
  type UIEvent,
} from 'react';
import { Send } from 'lucide-react';

import { ChiefMark } from '@/components/ChiefMark';
import { Button } from '@/components/ui/button';
import { Caret, Sweep } from '@/components/ui/activity';
import type { UseChat } from '@/hooks/use-chat';
import { useElapsed } from '@/hooks/use-elapsed';
import type { ChatMessage } from '@/lib/agent';

/** How close to the bottom still counts as reading the live end, in pixels. */
const FOLLOW_THRESHOLD = 32;

/** After this long the caption starts saying how long it has been. */
const SAY_HOW_LONG = 3;

/** After this long the wait is worth explaining rather than just counting. */
const EXPLAIN_THE_WAIT = 15;

/** Questions worth having on hand, in the words the system would use. */
const OPENERS = ['What did I ship this week?', 'Draft my standup', 'What is waiting on me?'];

const clockFormat = new Intl.DateTimeFormat(undefined, { hour: '2-digit', minute: '2-digit' });

function clock(timestamp: string): string {
  const parsed = new Date(timestamp);
  return Number.isNaN(parsed.getTime()) ? '' : clockFormat.format(parsed);
}

/** The question, as the reader asked it. */
function Question({ message }: { message: ChatMessage }) {
  return (
    <li>
      <p className="micro text-muted-foreground">you · {clock(message.at)}</p>
      <p className="mt-1.5 text-[15px] font-semibold tracking-[-0.015em]" data-selectable>
        <span className="sr-only">You said: </span>
        {message.content}
      </p>
    </li>
  );
}

/**
 * An answer, or an answer being worked on.
 *
 * A rule down the left rather than a bubble: this is a briefing, and a chief of
 * staff reports rather than chats. The label says where the answer came from
 * before the answer does.
 */
function Answer({ label, children }: { label: ReactNode; children: ReactNode }) {
  return (
    <li className="border-l-2 border-border pl-3.5">
      {label}
      <p className="mt-2 text-sm leading-relaxed whitespace-pre-wrap" data-selectable>
        {children}
      </p>
    </li>
  );
}

function EmptyState() {
  return (
    <div className="flex h-full items-center justify-center p-6">
      <div className="max-w-[420px] rounded-lg border border-dashed border-input p-6 text-center">
        <h2 className="text-[15px] font-semibold tracking-[-0.015em]">Ask about your work</h2>
        <p className="mx-auto mt-1.5 max-w-[280px] text-[13px] leading-snug text-muted-foreground">
          Answers come from a model running on this machine. Nothing you type leaves it.
        </p>
      </div>
    </div>
  );
}

/**
 * Chat surface for the agent, backed by the local model.
 *
 * The conversation is owned above this component, not in it. Chat lives in a
 * drawer now, and a drawer unmounts when it closes — holding the transcript
 * here meant closing the panel silently threw away everything the model had
 * said, which is the same mistake as discarding an interrupted answer.
 */
export function ChatView({ chat }: { chat: UseChat }) {
  const { messages, status, partial, activity, error, send } = chat;
  const [draft, setDraft] = useState('');
  const transcript = useRef<HTMLDivElement>(null);
  const composer = useRef<HTMLTextAreaElement>(null);
  // Whether the reader is at the live end of the answer. Someone who has
  // scrolled up to re-read something is not dragged back down by the next token.
  const following = useRef(true);

  const isThinking = status === 'thinking';
  const isWriting = isThinking && partial !== '';
  const elapsed = useElapsed(isThinking);

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

  // Only one thing moves at a time, and it always means work is in flight: the
  // mark turns until the model says something, the hairline sweeps while a tool
  // runs, and the caret sits at the live end of the text once it is writing.
  const step = activity ?? 'Sent to model';
  const caption = elapsed < SAY_HOW_LONG ? step : `${step} · ${elapsed}s`;

  const working = (
    <p className="flex items-center gap-2 micro text-verified-text" role="status">
      {activity === null ? <ChiefMark size={16} tone="thinking" breathing /> : <Sweep />}
      local · {caption}
    </p>
  );

  return (
    <div className="flex h-full flex-col">
      <div className="min-h-0 flex-1 overflow-y-auto" ref={transcript} onScroll={trackPosition}>
        {messages.length === 0 ? (
          <EmptyState />
        ) : (
          <div className="px-7 py-6">
            <ul className="flex max-w-[680px] flex-col gap-6">
              {messages.map((message) =>
                message.role === 'user' ? (
                  <Question key={message.id} message={message} />
                ) : (
                  <Answer
                    key={message.id}
                    label={<p className="micro text-verified-text">chief · local</p>}
                  >
                    <span className="sr-only">Chief said: </span>
                    {message.content}
                    {/* Under the answer, not in it. The text above is what the
                        model wrote; this line is what Rust knows, and keeping
                        them in separate elements is what stops a model that
                        writes its own "Sources:" line from being read as
                        this one. */}
                    {message.provenance !== null && (
                      <span className="mt-2.5 block micro text-muted-foreground">
                        {message.provenance}
                      </span>
                    )}
                  </Answer>
                ),
              )}

              {isWriting && (
                <Answer label={working}>
                  <span className="sr-only">Chief is saying: </span>
                  {partial}
                  <Caret />
                </Answer>
              )}

              {isThinking && partial === '' && (
                <li className="border-l-2 border-border pl-3.5">
                  {working}
                  {elapsed >= EXPLAIN_THE_WAIT && (
                    <p className="mt-2 text-xs text-muted-foreground">
                      A local model takes this long on a busy machine. Nothing has stalled.
                    </p>
                  )}
                </li>
              )}
            </ul>
          </div>
        )}
      </div>

      {error !== null && (
        <div className="px-7">
          <p
            className="max-w-[680px] rounded-md border border-destructive bg-destructive-surface px-3.5 py-3 text-[13px] leading-snug text-destructive-text"
            role="alert"
          >
            {error}
          </p>
        </div>
      )}

      <form className="border-t border-border px-7 py-3.5" onSubmit={submit}>
        <div className="flex max-w-[680px] flex-col gap-2.5">
          {/* Chip-shaped, but not `Chip`: chips carry state, never actions. */}
          <div className="flex flex-wrap gap-1.5">
            {OPENERS.map((opener) => (
              <button
                key={opener}
                type="button"
                onClick={() => {
                  setDraft(opener);
                  composer.current?.focus();
                }}
                className="rounded-sm border border-input px-[9px] py-[3px] text-[11px] text-muted-foreground transition-colors duration-[120ms] ease-instrument hover:bg-accent hover:text-accent-foreground"
              >
                {opener}
              </button>
            ))}
          </div>

          <div className="flex items-end gap-2">
            <textarea
              ref={composer}
              rows={1}
              value={draft}
              onChange={(event) => setDraft(event.target.value)}
              onKeyDown={handleKeyDown}
              placeholder="Ask about your work…"
              aria-label="Message your chief of staff"
              className="max-h-40 min-h-[34px] flex-1 resize-none rounded-md border border-input bg-background px-3 py-2 text-sm disabled:opacity-60"
            />
            <Button type="submit" size="icon" disabled={isThinking || draft.trim() === ''}>
              <Send aria-hidden />
              <span className="sr-only">Send message</span>
            </Button>
          </div>
        </div>
      </form>
    </div>
  );
}
