import { ExternalLink } from 'lucide-react';
import { openUrl } from '@tauri-apps/plugin-opener';

interface OpenSourceProps {
  /** The stored URL, or null when there is nowhere to go. */
  url: string | null;
  /** What it is, so the link says where it goes when read aloud. */
  label: string;
}

/**
 * Open the thing a work log entry is about.
 *
 * **The target is `work_logs.url` verbatim.** Canonicalisation happened once,
 * at ingestion, where the tracking parameters were stripped; re-deriving or
 * rewriting it here would mean two places deciding where a link goes and one
 * of them being wrong. Nothing asks the model, and nothing asks the network —
 * this is the deterministic half of the trade DLE-1 made when it kept links
 * out of the prompt.
 *
 * A row with no URL renders nothing at all. Entries the user typed have none,
 * and so does everything logged before migration 8, so an affordance that led
 * nowhere would be the common case rather than the exception.
 *
 * The link opens through `tauri-plugin-opener` in the user's own browser. The
 * renderer never navigates: a webview that followed a link would replace the
 * app with a web page and offer no way back.
 */
export function OpenSource({ url, label }: OpenSourceProps) {
  if (url === null || url.trim() === '') return null;

  return (
    <button
      type="button"
      aria-label={`Open ${label}`}
      // Best effort. An opener the platform refused leaves the row exactly as
      // it was, which is better than an error about a link the user can see.
      onClick={() => void openUrl(url).catch(() => undefined)}
      className="inline-flex items-center gap-1 rounded-sm micro text-muted-foreground transition-colors duration-[120ms] ease-instrument hover:text-foreground"
    >
      <ExternalLink className="size-[11px]" aria-hidden />
      Open
    </button>
  );
}
