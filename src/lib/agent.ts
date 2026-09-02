import { invoke } from '@tauri-apps/api/core';
import { listen } from '@tauri-apps/api/event';

/** Who authored a message in the conversation. */
export type ChatRole = 'user' | 'assistant';

/** A message as the UI holds it. */
export interface ChatMessage {
  id: string;
  role: ChatRole;
  content: string;
  /** When the turn joined the transcript, for its `you · 14:02` label. */
  at: string;
  /**
   * Where the answer came from, as a line to put under it.
   *
   * **Composed in Rust from the rows the read path returned, never written by
   * the model.** It is the only thing left telling the reader how fresh an
   * answer is, now that a read is a query rather than a call somebody else's
   * API times — so it is the one piece of text here that has to be true, and a
   * line a model composed could be wrong in the way nothing else would catch.
   *
   * Null when nothing local was read: a question the tool loop answered, or a
   * brief, which is a file the user can open rather than rows Chief assembled.
   * An empty footer claiming otherwise would be worse than none.
   */
  provenance: string | null;
}

/** What `ask_agent` resolves with: the answer, and where it came from. */
export interface Answer {
  content: string;
  provenance: string | null;
}

/** A turn as the backend expects it. */
export interface ChatTurn {
  role: ChatRole;
  content: string;
}

/** The event carrying an answer as it is written. */
const STREAM_EVENT = 'agent-stream';

/** A step in an answer, while the model is still working on it. */
export type AgentUpdate =
  /** More of the answer, to append to what is already showing. */
  | { kind: 'delta'; text: string }
  /** The model was thinking out loud and then reached for a tool. Discard what
   *  has been shown so far; the real answer follows. */
  | { kind: 'restart' }
  /** A tool is running, so the wait has a reason to show. */
  | { kind: 'tool'; name: string }
  /** The engine was stopped to give its memory back and is starting again. */
  | { kind: 'waking' };

/** An update, tagged with the question it belongs to. */
type StreamEvent = AgentUpdate & { requestId: string };

/** How a running tool is described while the user waits on it. */
const TOOL_ACTIVITY: Record<string, string> = {
  fetch_github_prs: 'Reading your pull requests on GitHub',
};

/** What to say while `name` is running. */
export function describeTool(name: string): string {
  return TOOL_ACTIVITY[name] ?? 'Looking that up';
}

/**
 * Ask the local model to answer the conversation so far.
 *
 * The whole answer is returned when the model has finished, and `onUpdate` is
 * called as it is written — a model this size takes tens of seconds to finish a
 * paragraph, so waiting for all of it before showing any of it makes a working
 * app feel broken. `requestId` ties the updates to this question, since the
 * events go to the whole window.
 *
 * The system prompt is set in Rust, not here — the renderer cannot change how
 * the agent is instructed.
 */
export async function askAgent(
  messages: ChatTurn[],
  requestId: string,
  onUpdate: (update: AgentUpdate) => void,
): Promise<Answer> {
  // Subscribed before the question is asked, so the opening words are not lost.
  const stopListening = await listen<StreamEvent>(STREAM_EVENT, ({ payload }) => {
    if (payload.requestId === requestId) onUpdate(payload);
  });

  try {
    return await invoke<Answer>('ask_agent', { messages, requestId });
  } finally {
    stopListening();
  }
}
