import { act, renderHook, waitFor } from '@testing-library/react';
import { beforeEach, describe, expect, it, vi } from 'vitest';

import { useBrief } from '@/hooks/use-brief';

const invoke = vi.hoisted(() => vi.fn());

vi.mock('@tauri-apps/api/core', () => ({ invoke }));

const today = {
  date: '2026-08-29',
  path: 'briefs/2026-08-29.md',
  markdown: '- Standup at 09:30',
  sources: ['calendar'],
};

function corpus(...paths: string[]) {
  return paths.map((path) => ({
    path,
    size: 100,
    modifiedAt: '2026-08-29T08:00:00Z',
    estimatedTokens: 20,
  }));
}

describe('useBrief', () => {
  beforeEach(() => {
    invoke.mockReset();
  });

  it('opens on today, and offers the days that came before it', async () => {
    invoke.mockImplementation((command: string) => {
      switch (command) {
        case 'todays_brief':
          return Promise.resolve(today);
        case 'list_corpus':
          return Promise.resolve(
            corpus('briefs/2026-08-27.md', 'briefs/2026-08-29.md', 'briefs/2026-08-28.md'),
          );
        default:
          return Promise.resolve(null);
      }
    });

    const { result } = renderHook(() => useBrief());

    await waitFor(() => expect(result.current.status).toBe('ready'));

    expect(result.current.brief?.date).toBe('2026-08-29');
    expect(result.current.days.map((day) => day.date)).toEqual([
      '2026-08-29',
      '2026-08-28',
      '2026-08-27',
    ]);
  });

  it('reads an earlier day from the corpus when one is picked', async () => {
    invoke.mockImplementation((command: string, args?: Record<string, unknown>) => {
      switch (command) {
        case 'todays_brief':
          return Promise.resolve(today);
        case 'list_corpus':
          return Promise.resolve(corpus('briefs/2026-08-29.md', 'briefs/2026-08-28.md'));
        case 'read_corpus_file':
          expect(args?.path).toBe('briefs/2026-08-28.md');
          return Promise.resolve('- Shipped the release workflow');
        default:
          return Promise.resolve(null);
      }
    });

    const { result } = renderHook(() => useBrief());
    await waitFor(() => expect(result.current.status).toBe('ready'));

    act(() => result.current.select('2026-08-28'));

    await waitFor(() => expect(result.current.brief?.date).toBe('2026-08-28'));
    expect(result.current.brief?.markdown).toBe('- Shipped the release workflow');
  });

  it('does not lose today when an earlier day fails to read', async () => {
    invoke.mockImplementation((command: string) => {
      switch (command) {
        case 'todays_brief':
          return Promise.resolve(today);
        case 'list_corpus':
          return Promise.resolve(corpus('briefs/2026-08-29.md', 'briefs/2026-08-28.md'));
        case 'read_corpus_file':
          return Promise.reject(new Error('briefs/2026-08-28.md is not in the corpus.'));
        default:
          return Promise.resolve(null);
      }
    });

    const { result } = renderHook(() => useBrief());
    await waitFor(() => expect(result.current.status).toBe('ready'));

    act(() => result.current.select('2026-08-28'));

    await waitFor(() => expect(result.current.status).toBe('error'));
    expect(result.current.error).toContain('is not in the corpus.');
    expect(result.current.brief?.date).toBe('2026-08-29');
  });

  it('still lists the days when today has no brief', async () => {
    invoke.mockImplementation((command: string) => {
      switch (command) {
        case 'todays_brief':
          return Promise.resolve(null);
        case 'list_corpus':
          return Promise.resolve(corpus('briefs/2026-08-28.md'));
        default:
          return Promise.resolve(null);
      }
    });

    const { result } = renderHook(() => useBrief());

    await waitFor(() => expect(result.current.status).toBe('ready'));

    expect(result.current.brief).toBeNull();
    expect(result.current.days.map((day) => day.date)).toEqual(['2026-08-28']);
  });

  it('brings the new day into the list when one is written', async () => {
    let written = false;
    invoke.mockImplementation((command: string) => {
      switch (command) {
        case 'todays_brief':
          return Promise.resolve(null);
        case 'list_corpus':
          return Promise.resolve(written ? corpus('briefs/2026-08-29.md') : []);
        case 'generate_brief':
          written = true;
          return Promise.resolve(today);
        default:
          return Promise.resolve(null);
      }
    });

    const { result } = renderHook(() => useBrief());
    await waitFor(() => expect(result.current.status).toBe('ready'));
    expect(result.current.days).toEqual([]);

    act(() => result.current.write());

    await waitFor(() => expect(result.current.days.map((day) => day.date)).toEqual(['2026-08-29']));
  });

  it('survives a corpus that cannot be listed, since the brief is the point', async () => {
    invoke.mockImplementation((command: string) => {
      switch (command) {
        case 'todays_brief':
          return Promise.resolve(today);
        case 'list_corpus':
          return Promise.reject(new Error('the corpus folder is missing.'));
        default:
          return Promise.resolve(null);
      }
    });

    const { result } = renderHook(() => useBrief());

    await waitFor(() => expect(result.current.status).toBe('ready'));

    expect(result.current.brief?.date).toBe('2026-08-29');
    expect(result.current.days).toEqual([]);
  });
});
