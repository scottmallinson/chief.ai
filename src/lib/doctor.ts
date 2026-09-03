import { invoke } from '@tauri-apps/api/core';

/** What this machine did when it was last asked to answer something. */
export interface Measurement {
  /** Milliseconds from asking to the first character coming back, cold. */
  firstTokenMs: number;
  /**
   * The same asked again, against the prefix the first left in the cache.
   *
   * The gap between the two is what `--cache-reuse` and the stable-parts-first
   * prompt are worth on this machine. Null for a measurement taken before this
   * was recorded — absent, not zero.
   */
  warmFirstTokenMs: number | null;
  /** Milliseconds for the whole answer. */
  totalMs: number;
  /** Characters written. */
  characters: number;
}

/** What Chief has worked out about this machine. */
export interface Report {
  /** `standard` or `light`. */
  tier: string;
  /** The model that tier runs, named for a person to read. */
  model: string;
  /** Roughly what it holds once loaded, in mebibytes. */
  modelSizeMb: number;
  /** The window the engine was started with. */
  contextSize: number;
  /** What the machine says it has, in mebibytes. */
  memoryMb: number;
  /** Cores available. */
  cores: number;
  /** `null` before anything has been measured. */
  measurement: Measurement | null;
  /** Roughly how fast the answer was written, once it started. */
  charactersPerSecond: number | null;
  /** What a full KV cache costs at this context window, in mebibytes. */
  kvCacheMb: number;
  /**
   * Weights plus that cache — what the engine actually holds.
   *
   * Stated rather than capped: llama.cpp has no allocation cap, so resident
   * memory is chosen by the tier, the context window and KV precision. Showing
   * the arithmetic is what makes the choice somebody's rather than a surprise.
   */
  residentMb: number;
  /** The same with a quantised KV cache, a lever Chief does not pull yet. */
  residentMbQ8: number;
}

/**
 * Ask what this machine is and what it can do.
 *
 * `remeasure` spends a generation finding out again — a second or two of the
 * engine's time — so the screen opens without it and the button asks for it.
 */
export async function runDoctor(remeasure = false): Promise<Report> {
  return invoke<Report>('run_doctor', { remeasure });
}
