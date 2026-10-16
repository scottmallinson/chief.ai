import { invoke } from '@tauri-apps/api/core';

/** One thing Chief knows, with where it came from and when. */
export interface Fact {
  id: number;
  /** `you`, or a person's handle. */
  about: string;
  /** greeting, sign-off, length, emoji, spelling, reviews, name or meets. */
  what: string;
  /** The repository, for the facts that are about one. */
  scope: string;
  value: string;
  count: number;
  total: number;
  source: string;
  seenOn: string;
  /** `counted` by Chief, or `yours` once the user has typed or edited it. */
  origin: string;
}

/** What the user types when they add or correct a fact. */
export interface FactInput {
  about: string;
  what: string;
  scope: string;
  value: string;
}

/** The kinds a user may add, in the words they would use. */
export const KINDS: { what: string; label: string; about: 'you' | 'person'; needs: string[] }[] = [
  { what: 'reviews', label: 'Reviews my pull requests in', about: 'person', needs: ['scope'] },
  { what: 'name', label: 'Is called', about: 'person', needs: ['value'] },
  { what: 'greeting', label: 'I open with', about: 'you', needs: ['value'] },
  { what: 'sign-off', label: 'I sign off with', about: 'you', needs: ['value'] },
  { what: 'length', label: 'My messages run to about (words)', about: 'you', needs: ['value'] },
  { what: 'emoji', label: 'Emoji', about: 'you', needs: ['value'] },
  { what: 'spelling', label: 'I spell', about: 'you', needs: ['value'] },
];

export function listFacts(): Promise<Fact[]> {
  return invoke<Fact[]>('list_facts');
}

export function saveFact(id: number | null, fact: FactInput): Promise<void> {
  return invoke<void>('save_fact', { id, fact });
}

export function deleteFact(id: number): Promise<void> {
  return invoke<void>('delete_fact', { id });
}

/** A fact as a sentence, counts said and never an impression. */
export function describeFact(fact: Fact): string {
  const of = fact.total > 0 ? ` (${fact.count} of ${fact.total})` : '';

  switch (fact.what) {
    case 'greeting':
      return fact.value === '' ? `Opens with nothing${of}` : `Opens with “${fact.value}”${of}`;
    case 'sign-off':
      return fact.value === ''
        ? `Signs off with nothing${of}`
        : `Signs off with “${fact.value}”${of}`;
    case 'length':
      return `Writes about ${fact.value} words`;
    case 'emoji':
      return fact.total > 0
        ? fact.count === 0
          ? `Uses no emoji${of}`
          : `Uses emoji in ${fact.count} of ${fact.total}`
        : fact.value === ''
          ? 'Uses emoji'
          : `Emoji: ${fact.value}`;
    case 'spelling':
      return `Spells the ${fact.value} way`;
    case 'reviews':
      return fact.total > 0
        ? `@${fact.about} reviewed ${fact.count} of your last ${fact.total} pull requests in ${fact.scope}`
        : `@${fact.about} reviews pull requests in ${fact.scope}`;
    case 'name':
      return `@${fact.about} is called ${fact.value}`;
    case 'meets':
      return `${fact.value === '' ? fact.about : fact.value} · ${fact.count} meetings`;
    default:
      return `${fact.about}: ${fact.what} ${fact.value}`.trim();
  }
}
