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

  it('says where the data is on every screen', () => {
    render(
      <Layout activeView="chat" onNavigate={vi.fn()} model="Llama 3.2 3B Instruct (Q4_K_M)">
        <p>chat</p>
      </Layout>,
    );

    expect(screen.getByText('on-device · Llama 3.2 3B Instruct (Q4_K_M)')).toBeInTheDocument();
  });

  it('still says on-device before the model is known', () => {
    render(
      <Layout activeView="chat" onNavigate={vi.fn()}>
        <p>chat</p>
      </Layout>,
    );

    expect(screen.getByText('on-device')).toBeInTheDocument();
  });

  it('names each rail item, since the rail itself carries no labels', () => {
    render(
      <Layout activeView="chat" onNavigate={vi.fn()}>
        <p>chat</p>
      </Layout>,
    );

    for (const label of ['Chat', 'Work Log', 'Settings']) {
      expect(screen.getByRole('button', { name: label })).toBeInTheDocument();
    }
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
