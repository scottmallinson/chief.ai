import { useState } from 'react';
import { render, screen } from '@testing-library/react';
import userEvent from '@testing-library/user-event';
import { describe, expect, it, vi } from 'vitest';

import { Drawer } from '@/components/Drawer';

/** The drawer as it is actually used: opened from a button, closing back to it. */
function Harness({ title = 'Ask Chief' }: { title?: string }) {
  const [open, setOpen] = useState(false);

  return (
    <>
      <button type="button" onClick={() => setOpen(true)}>
        Open
      </button>
      <button type="button">Behind</button>

      <Drawer open={open} title={title} onClose={() => setOpen(false)}>
        <button type="button">First</button>
        <button type="button">Last</button>
      </Drawer>
    </>
  );
}

describe('Drawer', () => {
  it('is not in the document until it is opened', () => {
    render(<Harness />);

    expect(screen.queryByRole('dialog')).not.toBeInTheDocument();
  });

  it('is a modal dialog named by its title', async () => {
    render(<Harness />);
    await userEvent.click(screen.getByRole('button', { name: 'Open' }));

    const dialog = screen.getByRole('dialog', { name: 'Ask Chief' });

    expect(dialog).toHaveAttribute('aria-modal', 'true');
  });

  it('moves focus into itself when it opens', async () => {
    render(<Harness />);
    await userEvent.click(screen.getByRole('button', { name: 'Open' }));

    expect(screen.getByRole('dialog')).toContainElement(document.activeElement as HTMLElement);
  });

  it('closes on Escape', async () => {
    render(<Harness />);
    await userEvent.click(screen.getByRole('button', { name: 'Open' }));
    await userEvent.keyboard('{Escape}');

    expect(screen.queryByRole('dialog')).not.toBeInTheDocument();
  });

  it('returns focus to whatever opened it', async () => {
    render(<Harness />);
    const opener = screen.getByRole('button', { name: 'Open' });

    await userEvent.click(opener);
    await userEvent.keyboard('{Escape}');

    expect(opener).toHaveFocus();
  });

  it('wraps Tab from the last control back to the first', async () => {
    render(<Harness />);
    await userEvent.click(screen.getByRole('button', { name: 'Open' }));

    await userEvent.click(screen.getByRole('button', { name: 'Last' }));
    await userEvent.tab();

    expect(screen.getByRole('button', { name: 'Close' })).toHaveFocus();
  });

  it('wraps Shift+Tab from the first control round to the last', async () => {
    render(<Harness />);
    await userEvent.click(screen.getByRole('button', { name: 'Open' }));

    // Close is the first stop, and opening already put focus on it.
    expect(screen.getByRole('button', { name: 'Close' })).toHaveFocus();
    await userEvent.tab({ shift: true });

    expect(screen.getByRole('button', { name: 'Last' })).toHaveFocus();
  });

  it('never lets Tab reach the page behind it', async () => {
    render(<Harness />);
    await userEvent.click(screen.getByRole('button', { name: 'Open' }));

    const behind = screen.getByRole('button', { name: 'Behind' });

    for (let press = 0; press < 6; press += 1) {
      await userEvent.tab();
      expect(behind).not.toHaveFocus();
    }
  });

  it('closes when the scrim behind it is clicked', async () => {
    render(<Harness />);
    await userEvent.click(screen.getByRole('button', { name: 'Open' }));

    await userEvent.click(screen.getByTestId('drawer-scrim'));

    expect(screen.queryByRole('dialog')).not.toBeInTheDocument();
  });

  it('closes from its own close button', async () => {
    render(<Harness />);
    await userEvent.click(screen.getByRole('button', { name: 'Open' }));

    await userEvent.click(screen.getByRole('button', { name: 'Close' }));

    expect(screen.queryByRole('dialog')).not.toBeInTheDocument();
  });

  it('leaves the opener alone when it was never opened', () => {
    const onClose = vi.fn();
    render(
      <Drawer open={false} title="Ask Chief" onClose={onClose}>
        <button type="button">First</button>
      </Drawer>,
    );

    expect(onClose).not.toHaveBeenCalled();
  });
});
