import { invoke } from '@tauri-apps/api/core';
import { listen } from '@tauri-apps/api/event';

/** Whether the local inference engine can answer a question right now. */
export type EngineStatus = 'ready' | 'loading' | 'down';

/** Whether this machine can answer questions yet. */
export interface Readiness {
  /** The model Chief runs, named for a person to read. */
  model: string;
  /** Whether the weights have been downloaded to this machine. */
  modelInstalled: boolean;
  engine: EngineStatus;
  /** Why the engine is not answering, when it is not. */
  problem: string | null;
}

/** How a model download is progressing. */
export interface DownloadProgress {
  status: string;
  completed: number;
  total: number;
}

/** Ask whether the model is here and the engine is answering. */
export function checkReadiness(): Promise<Readiness> {
  return invoke<Readiness>('check_readiness');
}

/** Download the model. Progress arrives through {@link onDownloadProgress}. */
export function downloadModel(): Promise<void> {
  return invoke<void>('download_model');
}

/** Start the local engine, resolving once it can answer. */
export function startEngine(): Promise<void> {
  return invoke<void>('start_engine');
}

/** Subscribe to download progress. Returns a function that stops listening. */
export function onDownloadProgress(handler: (progress: DownloadProgress) => void) {
  return listen<DownloadProgress>('model-download-progress', (event) => handler(event.payload));
}

/** Everything Chief needs before it can be useful. */
export function isReady(readiness: Readiness | null): boolean {
  return readiness !== null && readiness.modelInstalled && readiness.engine === 'ready';
}
