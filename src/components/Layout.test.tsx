import { render, screen } from '@testing-library/react';
import userEvent from '@testing-library/user-event';
import { describe, expect, it, vi } from 'vitest';

import { Layout } from '@/components/Layout';

describe('Layout', () => {
  it('marks the active destination and renders its content', () => {
    render(
      <Layout activeView="work-log" onNavigate={vi.fn()}>
        <p>log entries</p>
      </Layout>,
    );

    expect(screen.getByRole('button', { name: 'Work Log' })).toHaveAttribute(
      'aria-current',
      'page',
    );
    expect(screen.getByText('log entries')).toBeInTheDocument();
  });

  it('reports the destination the user picked', async () => {
    const onNavigate = vi.fn();
    render(
      <Layout activeView="chat" onNavigate={onNavigate}>
        <p>chat</p>
      </Layout>,
    );

    await userEvent.click(screen.getByRole('button', { name: 'Settings' }));

    expect(onNavigate).toHaveBeenCalledWith('settings');
  });
});
