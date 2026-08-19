import { invoke } from '@tauri-apps/api/core';

/** Who authored a message in the conversation. */
export type ChatRole = 'user' | 'assistant';

/** A message as the UI holds it. */
export interface ChatMessage {
  id: string;
  role: ChatRole;
  content: string;
}

/** A turn as the backend expects it. */
export interface ChatTurn {
  role: ChatRole;
  content: string;
}

/**
 * Ask the local model to answer the conversation so far.
 *
 * The system prompt is set in Rust, not here — the renderer cannot change how
 * the agent is instructed.
 */
export function askAgent(messages: ChatTurn[]): Promise<string> {
  return invoke<string>('ask_agent', { messages });
}
