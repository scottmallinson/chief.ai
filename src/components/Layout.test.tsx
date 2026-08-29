import { render, screen } from '@testing-library/react';
import userEvent from '@testing-library/user-event';
import { describe, expect, it, vi } from 'vitest';

import { Layout } from '@/components/Layout';

describe('Layout', () => {
  it('marks the active destination and renders its content', () => {
    render(
      <Layout activeView="work-log" onNavigate={vi.fn()} onOpenChat={vi.fn()}>
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
      <Layout
        activeView="today"
        onNavigate={vi.fn()}
        onOpenChat={vi.fn()}
        model="Llama 3.2 3B Instruct (Q4_K_M)"
      >
        <p>brief</p>
      </Layout>,
    );

    expect(screen.getByText('on-device · Llama 3.2 3B Instruct (Q4_K_M)')).toBeInTheDocument();
  });

  it('still says on-device before the model is known', () => {
    render(
      <Layout activeView="today" onNavigate={vi.fn()} onOpenChat={vi.fn()}>
        <p>brief</p>
      </Layout>,
    );

    expect(screen.getByText('on-device')).toBeInTheDocument();
  });

  it('names each rail item, since the rail itself carries no labels', () => {
    render(
      <Layout activeView="today" onNavigate={vi.fn()} onOpenChat={vi.fn()}>
        <p>brief</p>
      </Layout>,
    );

    for (const label of ['Today', 'Work Log', 'Settings']) {
      expect(screen.getByRole('button', { name: label })).toBeInTheDocument();
    }
  });

  it('reports the destination the user picked', async () => {
    const onNavigate = vi.fn();
    render(
      <Layout activeView="today" onNavigate={onNavigate} onOpenChat={vi.fn()}>
        <p>brief</p>
      </Layout>,
    );

    await userEvent.click(screen.getByRole('button', { name: 'Settings' }));

    expect(onNavigate).toHaveBeenCalledWith('settings');
  });

  it('opens chat from the rail without leaving the destination', async () => {
    const onOpenChat = vi.fn();
    const onNavigate = vi.fn();
    render(
      <Layout activeView="today" onNavigate={onNavigate} onOpenChat={onOpenChat}>
        <p>brief</p>
      </Layout>,
    );

    await userEvent.click(screen.getByRole('button', { name: 'Ask Chief' }));

    expect(onOpenChat).toHaveBeenCalled();
    expect(onNavigate).not.toHaveBeenCalled();
  });

  it('renders a list pane when the destination brings one', () => {
    render(
      <Layout activeView="today" onNavigate={vi.fn()} onOpenChat={vi.fn()} list={<p>the days</p>}>
        <p>brief</p>
      </Layout>,
    );

    expect(screen.getByText('the days')).toBeInTheDocument();
  });

  it('leaves the pane out entirely when the destination has no list', () => {
    render(
      <Layout activeView="settings" onNavigate={vi.fn()} onOpenChat={vi.fn()}>
        <p>settings</p>
      </Layout>,
    );

    expect(screen.queryByTestId('list-pane')).not.toBeInTheDocument();
  });
});
