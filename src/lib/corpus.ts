import { invoke } from '@tauri-apps/api/core';

/** One markdown file in the corpus. */
export interface CorpusEntry {
  /** Relative to the root, slash-separated. */
  path: string;
  /** Bytes on disk. */
  size: number;
  /** When it last changed, ISO-8601. */
  modifiedAt: string;
  /** Roughly what it would cost to put in a prompt. */
  estimatedTokens: number;
}

/** Where the corpus is, and what is in it. */
export interface CorpusLocation {
  /** The folder, as a person would type it. */
  root: string;
  /** Whether it is there yet. */
  exists: boolean;
  /** How many markdown files are in it. */
  files: number;
  /**
   * Roughly what the whole corpus would cost in a prompt — far more than any
   * one prompt may spend, which is the point: retrieval picks from this.
   */
  estimatedTokens: number;
}

/** Where the corpus is, and what is in it. */
export function corpusLocation(): Promise<CorpusLocation> {
  return invoke<CorpusLocation>('corpus_location');
}

/** Every markdown file in the corpus. */
export function listCorpus(): Promise<CorpusEntry[]> {
  return invoke<CorpusEntry[]>('list_corpus');
}

/** One file's contents. */
export function readCorpusFile(path: string): Promise<string> {
  return invoke<string>('read_corpus_file', { path });
}

/** Write one file, and get the refreshed listing back. */
export function writeCorpusFile(path: string, contents: string): Promise<CorpusEntry[]> {
  return invoke<CorpusEntry[]>('write_corpus_file', { path, contents });
}

/** Point the corpus somewhere else, or pass null to go back to the default. */
export function setCorpusRoot(path: string | null): Promise<CorpusLocation> {
  return invoke<CorpusLocation>('set_corpus_root', { path });
}
