import { Send } from 'lucide-react';

import { Button } from '@/components/ui/button';

/**
 * Chat surface for the agent.
 *
 * The composer is intentionally inert until Step 3 wires it to the `ask_agent`
 * Tauri command backed by a local Ollama instance.
 */
export function ChatView() {
  return (
    <div className="flex h-full flex-col">
      <div className="flex flex-1 items-center justify-center p-6">
        <div className="max-w-md text-center">
          <h2 className="text-lg font-semibold">Ask about your work</h2>
          <p className="mt-2 text-sm text-muted-foreground">
            &ldquo;What did I ship this week?&rdquo; &middot; &ldquo;Which pull requests are still
            waiting on me?&rdquo;
          </p>
          <p className="mt-6 text-xs text-muted-foreground">
            The local model is not connected yet — that arrives with the Ollama engine.
          </p>
        </div>
      </div>

      <div className="border-t border-border p-4">
        <div className="mx-auto flex max-w-3xl items-end gap-2">
          <textarea
            rows={1}
            disabled
            placeholder="Message your chief of staff…"
            aria-label="Message your chief of staff"
            className="max-h-40 min-h-9 flex-1 resize-none rounded-md border border-input bg-background px-3 py-2 text-sm focus-visible:ring-2 focus-visible:ring-ring focus-visible:outline-none disabled:opacity-60"
          />
          <Button size="icon" disabled aria-label="Send message">
            <Send aria-hidden />
          </Button>
        </div>
      </div>
    </div>
  );
}
