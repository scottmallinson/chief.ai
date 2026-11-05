import { invoke } from '@tauri-apps/api/core';

/** Where the model runs, as `gpu_status` answers it. */
export interface GpuStatus {
  /** Whether the model is set to run on a graphics card right now. */
  useGpu: boolean;
  /** The person's own choice, or null while Chief decides. */
  chosen: boolean | null;
  /** True when the environment overrides both, for a developer. */
  overridden: boolean;
  /** What Chief found, when it has looked. Null where the engine's default is used. */
  summary: string | null;
}

export function gpuStatus(): Promise<GpuStatus> {
  return invoke<GpuStatus>('gpu_status');
}

/** Choose whether the model uses a graphics card, or null to go back to the platform default. */
export function setUseGpu(enabled: boolean | null): Promise<void> {
  return invoke<void>('set_use_gpu', { enabled });
}
