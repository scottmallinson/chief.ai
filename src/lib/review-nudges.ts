import { invoke } from '@tauri-apps/api/core';

/** Whether Chief drafts review requests for pull requests that have been sitting. */
export function reviewNudges(): Promise<boolean> {
  return invoke<boolean>('review_nudges');
}

export function setReviewNudges(enabled: boolean): Promise<void> {
  return invoke<void>('set_review_nudges', { enabled });
}
