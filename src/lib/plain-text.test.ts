import { describe, expect, it } from 'vitest';

import { plainText } from '@/lib/plain-text';

describe('plainText', () => {
  it('removes bold, headings and code marks but keeps every word', () => {
    expect(plainText('## Done\n**Completed in the last 7 days:**\nRun `pnpm check`')).toBe(
      'Done\nCompleted in the last 7 days:\nRun pnpm check',
    );
  });

  it('turns star bullets into dashes, as the prompt asks the model to write them', () => {
    expect(plainText('*   `chief.ai` #123: merged\n* second')).toBe(
      '- chief.ai #123: merged\n- second',
    );
  });

  it('removes single-star emphasis but not a multiplication sign', () => {
    expect(plainText('this is *important* today')).toBe('this is important today');
    expect(plainText('5 * 3 = 15')).toBe('5 * 3 = 15');
  });

  it('removes a bold mark that has not closed yet, so a streaming answer never flashes it', () => {
    expect(plainText('Waiting on **Review')).toBe('Waiting on Review');
  });

  it('leaves plain text exactly as it was', () => {
    const plain = '- Review chief.ai#121 first\n- Then #126 (draft)';
    expect(plainText(plain)).toBe(plain);
  });
});
