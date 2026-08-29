import { invoke } from '@tauri-apps/api/core';

/** What seeding the profile would read and write, said before it runs. */
export interface ProfilePlan {
  /** What Chief would read, in the user's words rather than in API names. */
  reads: string[];
  /** Which corpus files it would write. */
  writes: string[];
  /** Files it would leave alone, because they are no longer Chief's starter. */
  keeps: string[];
}

/** What seeding actually did. */
export interface ProfileReport {
  written: string[];
  kept: string[];
}

/** What seeding would read and write. Runs nothing. */
export function profilePlan(): Promise<ProfilePlan> {
  return invoke<ProfilePlan>('profile_plan');
}

/** Seed the profile. Only ever called because the user pressed the button. */
export function bootstrapProfile(): Promise<ProfileReport> {
  return invoke<ProfileReport>('bootstrap_profile');
}
