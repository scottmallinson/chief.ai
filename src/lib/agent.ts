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
   * How many tools the answer drew on, so the transcript can say `local · 3
   * sources` rather than asking the reader to take the answer on trust. Zero
   * for a question, and for an answer the model wrote from what it already had.
   */
  sources: number;
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
): Promise<string> {
  // Subscribed before the question is asked, so the opening words are not lost.
  const stopListening = await listen<StreamEvent>(STREAM_EVENT, ({ payload }) => {
    if (payload.requestId === requestId) onUpdate(payload);
  });

  try {
    return await invoke<string>('ask_agent', { messages, requestId });
  } finally {
    stopListening();
  }
}
