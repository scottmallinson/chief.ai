import { Check, Cpu, Download, Loader2, RefreshCw } from 'lucide-react';

import { Button } from '@/components/ui/button';
import { useSetup } from '@/hooks/use-setup';
import { cn } from '@/lib/utils';
import { isReady, type DownloadProgress } from '@/lib/setup';

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

function ProgressBar({ progress }: { progress: DownloadProgress }) {
  const fraction = progress.total > 0 ? Math.min(progress.completed / progress.total, 1) : null;

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
  /** Let the user in anyway; Chat will not work until the engine is running. */
  onSkip: () => void;
}

/**
 * First-run screen.
 *
 * The engine ships with Chief, so there is nothing to install: the only thing
 * this machine is missing is the model itself, and starting the engine on it is
 * a button rather than a terminal.
 */
export function SetupView({ onSkip }: SetupViewProps) {
  const { readiness, status, progress, error, recheck, download, start } = useSetup();

  const modelInstalled = readiness?.modelInstalled === true;
  const engine = readiness?.engine ?? 'down';
  const isDownloading = status === 'downloading';
  const isStarting = status === 'starting' || engine === 'loading';

  // Anything that went wrong. A stopped engine is only worth repeating while it
  // still is — once it is answering, that message is stale.
  const problem = error ?? (engine === 'down' ? (readiness?.problem ?? null) : null);

  return (
    <div className="h-full overflow-y-auto">
      <div className="mx-auto flex min-h-full max-w-xl flex-col justify-center p-6">
        <header className="mb-6">
          <h1 className="text-lg font-semibold">Set up Chief</h1>
          <p className="mt-1 text-sm text-muted-foreground">
            Chief answers questions using a model that runs on this machine. It came with the engine
            that runs it, so there is one thing left to fetch — and it stays entirely local.
          </p>
        </header>

        <ol className="space-y-3">
          <Step
            index={1}
            title="Download the model"
            description={
              modelInstalled
                ? `${readiness?.model} is on this machine.`
                : `Chief uses ${readiness?.model ?? 'a small local model'}, about 2 GB. It is downloaded once, and an interrupted download resumes where it left off.`
            }
            done={modelInstalled}
          >
            {!modelInstalled && (
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

          <Step
            index={2}
            title="Start the engine"
            description={
              engine === 'ready'
                ? 'Running on this machine, on a loopback address only Chief can reach.'
                : engine === 'loading'
                  ? 'Reading the model into memory. This takes a few seconds.'
                  : 'The engine runs the model here, in a process Chief starts and stops with it.'
            }
            done={engine === 'ready'}
          >
            {modelInstalled && engine !== 'ready' && (
              <div className="mt-3 flex flex-wrap items-center gap-2">
                <Button size="sm" onClick={start} disabled={isStarting}>
                  {isStarting ? (
                    <Loader2 className="animate-spin" aria-hidden />
                  ) : (
                    <Cpu aria-hidden />
                  )}
                  {isStarting ? 'Starting…' : 'Start engine'}
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
