import { render, screen } from '@testing-library/react';
import userEvent from '@testing-library/user-event';
import { beforeEach, describe, expect, it, vi } from 'vitest';

import App from '@/App';

const invoke = vi.hoisted(() => vi.fn());

vi.mock('@tauri-apps/api/core', () => ({ invoke }));

describe('App', () => {
  beforeEach(() => {
    invoke.mockReset();
    invoke.mockResolvedValue([]);
  });

  it('opens on the chat view', () => {
    render(<App />);

    expect(screen.getByRole('heading', { name: 'Ask about your work' })).toBeInTheDocument();
  });

  it('switches views from the sidebar', async () => {
    render(<App />);

    await userEvent.click(screen.getByRole('button', { name: 'Work Log' }));

    expect(screen.getByRole('heading', { level: 1, name: 'Work Log' })).toBeInTheDocument();
    expect(await screen.findByRole('heading', { name: 'No entries yet' })).toBeInTheDocument();
  });
});
