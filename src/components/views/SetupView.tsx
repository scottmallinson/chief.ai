import { Check, Download, ExternalLink, Loader2, RefreshCw } from 'lucide-react';
import { openUrl } from '@tauri-apps/plugin-opener';

import { Button } from '@/components/ui/button';
import { useSetup } from '@/hooks/use-setup';
import { cn } from '@/lib/utils';
import { isReady, OLLAMA_DOWNLOAD_URL, type PullProgress } from '@/lib/setup';

interface StepProps {
  index: number;
  title: string;
  description: string;
  done: boolean;
  children?: React.ReactNode;
}

function Step({ index, title, description, done, children }: StepProps) {
  return (
    <li className="rounded-lg border border-border p-4">
      <div className="flex items-start gap-3">
        <span
          className={cn(
            'flex size-6 shrink-0 items-center justify-center rounded-full text-xs font-medium',
            done ? 'bg-primary text-primary-foreground' : 'border border-border',
          )}
        >
          {done ? <Check className="size-3.5" aria-hidden /> : index}
        </span>
        <div className="min-w-0 flex-1">
          <h2 className="text-sm font-medium">
            {title}
            {done && <span className="sr-only"> — done</span>}
          </h2>
          <p className="mt-1 text-sm text-muted-foreground">{description}</p>
          {children}
        </div>
      </div>
    </li>
  );
}

function megabytes(bytes: number): string {
  return `${Math.round(bytes / 1_000_000)} MB`;
}

function ProgressBar({ progress }: { progress: PullProgress }) {
  const fraction = progress.total > 0 ? progress.completed / progress.total : null;

  return (
    <div className="mt-3">
      <div
        className="h-2 w-full overflow-hidden rounded-full bg-muted"
        role="progressbar"
        aria-valuemin={0}
        aria-valuemax={100}
        {...(fraction !== null ? { 'aria-valuenow': Math.round(fraction * 100) } : {})}
        aria-label="Model download progress"
      >
        <div
          className="h-full bg-primary transition-[width] duration-300"
          style={{ width: fraction !== null ? `${fraction * 100}%` : '100%' }}
        />
      </div>
      <p className="mt-2 text-xs text-muted-foreground">
        {progress.status}
        {progress.total > 0 &&
          ` — ${megabytes(progress.completed)} of ${megabytes(progress.total)}`}
      </p>
    </div>
  );
}

interface SetupViewProps {
  /** Let the user in anyway; Chat will not work until Ollama is running. */
  onSkip: () => void;
}

/**
 * First-run screen. Chief needs a local model before it can answer anything,
 * so this checks for one and offers to fetch it — no terminal required.
 */
export function SetupView({ onSkip }: SetupViewProps) {
  const { readiness, status, progress, error, recheck, download } = useSetup();

  const ollamaRunning = readiness?.ollamaRunning === true;
  const modelInstalled = readiness?.modelInstalled === true;
  const isDownloading = status === 'downloading';

  // Anything that went wrong. Ollama being unreachable is only worth repeating
  // while it still is — once it is running, that message is stale.
  const problem = error ?? (ollamaRunning ? null : (readiness?.problem ?? null));

  return (
    <div className="h-full overflow-y-auto">
      <div className="mx-auto flex min-h-full max-w-xl flex-col justify-center p-6">
        <header className="mb-6">
          <h1 className="text-lg font-semibold">Set up Chief</h1>
          <p className="mt-1 text-sm text-muted-foreground">
            Chief answers questions using a model that runs on this machine. Two things to sort out
            first — both stay entirely local.
          </p>
        </header>

        <ol className="space-y-3">
          <Step
            index={1}
            title="Install Ollama"
            description={
              ollamaRunning
                ? `Running${readiness?.ollamaVersion !== null ? ` (version ${readiness?.ollamaVersion})` : ''}.`
                : 'Ollama runs the model locally. Install it, then check again.'
            }
            done={ollamaRunning}
          >
            {!ollamaRunning && (
              <div className="mt-3 flex flex-wrap items-center gap-2">
                <Button
                  size="sm"
                  onClick={() => {
                    void openUrl(OLLAMA_DOWNLOAD_URL);
                  }}
                >
                  <ExternalLink aria-hidden />
                  Get Ollama
                </Button>
                <Button
                  variant="outline"
                  size="sm"
                  onClick={recheck}
                  disabled={status === 'checking'}
                >
                  <RefreshCw aria-hidden />
                  Check again
                </Button>
              </div>
            )}
          </Step>

          <Step
            index={2}
            title="Download the model"
            description={
              modelInstalled
                ? `${readiness?.model} is installed.`
                : `Chief uses ${readiness?.model ?? 'a small local model'}, about 2 GB. It is downloaded once.`
            }
            done={modelInstalled}
          >
            {ollamaRunning && !modelInstalled && (
              <>
                <Button size="sm" className="mt-3" onClick={download} disabled={isDownloading}>
                  {isDownloading ? (
                    <Loader2 className="animate-spin" aria-hidden />
                  ) : (
                    <Download aria-hidden />
                  )}
                  {isDownloading ? 'Downloading…' : 'Download model'}
                </Button>
                {progress !== null && <ProgressBar progress={progress} />}
              </>
            )}
          </Step>
        </ol>

        {problem !== null && (
          <p
            className="mt-4 rounded-md border border-destructive/50 p-3 text-sm text-muted-foreground"
            role="alert"
          >
            {problem}
          </p>
        )}

        <div className="mt-6 flex items-center justify-between gap-4">
          <button
            type="button"
            onClick={onSkip}
            className="text-xs text-muted-foreground underline underline-offset-4 hover:text-foreground"
          >
            Skip for now
          </button>
          {isReady(readiness) && (
            <Button size="sm" onClick={onSkip}>
              Start using Chief
            </Button>
          )}
        </div>
      </div>
    </div>
  );
}
