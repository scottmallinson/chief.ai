import { invoke } from '@tauri-apps/api/core';
import { listen } from '@tauri-apps/api/event';

/** Where Ollama is downloaded from. */
export const OLLAMA_DOWNLOAD_URL = 'https://ollama.com/download';

/** Whether this machine can answer questions yet. */
export interface Readiness {
  ollamaRunning: boolean;
  ollamaVersion: string | null;
  model: string;
  modelInstalled: boolean;
  /** Why Ollama could not be reached, when it could not. */
  problem: string | null;
}

/** How a model download is progressing. */
export interface PullProgress {
  status: string;
  completed: number;
  total: number;
}

/** Ask whether Ollama is running and the model is installed. */
export function checkReadiness(): Promise<Readiness> {
  return invoke<Readiness>('check_readiness');
}

/** Download the model. Progress arrives through {@link onPullProgress}. */
export function pullModel(): Promise<void> {
  return invoke<void>('pull_model');
}

/** Subscribe to download progress. Returns a function that stops listening. */
export function onPullProgress(handler: (progress: PullProgress) => void) {
  return listen<PullProgress>('model-pull-progress', (event) => handler(event.payload));
}

/** Everything Chief needs before it can be useful. */
export function isReady(readiness: Readiness | null): boolean {
  return readiness !== null && readiness.ollamaRunning && readiness.modelInstalled;
}
