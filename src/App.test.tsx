import { render, screen } from '@testing-library/react';
import userEvent from '@testing-library/user-event';
import { describe, expect, it } from 'vitest';

import App from '@/App';

describe('App', () => {
  it('opens on the chat view', () => {
    render(<App />);

    expect(screen.getByRole('heading', { name: 'Ask about your work' })).toBeInTheDocument();
  });

  it('switches views from the sidebar', async () => {
    render(<App />);

    await userEvent.click(screen.getByRole('button', { name: 'Work Log' }));

    expect(screen.getByRole('heading', { name: 'No entries yet' })).toBeInTheDocument();
  });
});
