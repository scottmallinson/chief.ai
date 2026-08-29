import { invoke } from '@tauri-apps/api/core';

/** A draft Chief prepared, with its body as it is on disk right now. */
export interface Proposal {
  id: number;
  /** Where the thing it is about came from, e.g. `github`. */
  source: string;
  /** What it is about, for the card. */
  title: string;
  /** The item as Chief read it, so the card can say why this was proposed. */
  context: string;
  /** Where the body is in the corpus. */
  path: string;
  status: string;
  createdAt: string;
  /** The draft itself. */
  body: string;
}

/** The drafts Chief has prepared. Nothing here has been sent. */
export function listProposals(): Promise<Proposal[]> {
  return invoke<Proposal[]>('list_proposals');
}

/** Put an edited draft back on disk. The file is the draft. */
export function saveProposal(path: string, body: string): Promise<void> {
  return invoke<void>('save_proposal', { path, body });
}

/** Say no. The row stays, so the same item is never drafted again. */
export function dismissProposal(id: number): Promise<void> {
  return invoke<void>('dismiss_proposal', { id });
}

/** Rewrite a draft to an instruction. One model call, nothing else. */
export function refineDraft(body: string, instruction: string): Promise<string> {
  return invoke<string>('refine_draft', { body, instruction });
}
