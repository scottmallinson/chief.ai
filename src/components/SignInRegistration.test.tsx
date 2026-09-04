import { render, screen } from '@testing-library/react';
import userEvent from '@testing-library/user-event';
import { beforeEach, describe, expect, it, vi } from 'vitest';

import { SignInRegistration } from '@/components/SignInRegistration';
import type { Registration } from '@/lib/registration';

const invoke = vi.hoisted(() => vi.fn());

vi.mock('@tauri-apps/api/core', () => ({ invoke }));

const builtIn: Registration = {
  service: 'github',
  clientId: 'Ov23liBuiltIn',
  source: 'builtIn',
  hasBuiltIn: true,
  overriddenByEnvironment: false,
};

const missing: Registration = {
  service: 'github',
  clientId: null,
  source: 'missing',
  hasBuiltIn: false,
  overriddenByEnvironment: false,
};

/**
 * Every command answers a `Registration[]`, because all three of them do.
 *
 * `set_sign_in_registration` and `clear_sign_in_registration` return the list
 * as it now stands rather than an acknowledgement, so the screen cannot show a
 * saved id it has not read back.
 */
function answering(...replies: (Registration[] | Error)[]) {
  invoke.mockReset();

  for (const reply of replies) {
    invoke.mockImplementationOnce(() =>
      reply instanceof Error ? Promise.reject(reply) : Promise.resolve(reply),
    );
  }

  invoke.mockImplementation(() => Promise.resolve(replies.at(-1) ?? []));
}

function show(registration: Registration[] | Error = [builtIn]) {
  answering(registration);

  render(<SignInRegistration service="github" name="GitHub" where="Make one on GitHub." />);
}

describe('SignInRegistration', () => {
  beforeEach(() => invoke.mockReset());

  it('says where the id in use came from, without showing the field', async () => {
    show();

    expect(await screen.findByText('Built into this release')).toBeInTheDocument();
    expect(screen.queryByLabelText('GitHub client id')).not.toBeInTheDocument();
  });

  it('shows the id itself, which is public by design, once asked', async () => {
    show();

    await userEvent.click(await screen.findByRole('button', { name: 'Change' }));

    expect(screen.getByText('Ov23liBuiltIn')).toBeInTheDocument();
    expect(screen.getByLabelText('GitHub client id')).toBeInTheDocument();
  });

  /**
   * The defect REC-60 is about.
   *
   * A release compiled without a client id told the user to set an
   * environment variable — advice nobody who double-clicked an installer can
   * take, from a screen that offered no other way forward. A build with none
   * has to say so and put the field in front of the user, because there is
   * nothing else on the card that can work until it is filled in.
   *
   * Proved by folding the missing case away like any other:
   *
   * ```text
   * Unable to find a label with the text of: GitHub client id
   * ```
   */
  it('offers the field unasked when the build has no id at all', async () => {
    show([missing]);

    expect(
      await screen.findByText(/Chief has no GitHub client id, so signing in is not possible yet/),
    ).toBeInTheDocument();
    expect(screen.getByLabelText('GitHub client id')).toBeInTheDocument();
    // Nothing to fold away, so nothing offers to.
    expect(screen.queryByRole('button', { name: 'Change' })).not.toBeInTheDocument();
  });

  it('stores a pasted id and reports back what is now in use', async () => {
    const stored: Registration = {
      service: 'github',
      clientId: 'Ov23liTheirs',
      source: 'stored',
      hasBuiltIn: true,
      overriddenByEnvironment: false,
    };

    answering([builtIn], [stored]);
    render(<SignInRegistration service="github" name="GitHub" where="Make one on GitHub." />);

    await userEvent.click(await screen.findByRole('button', { name: 'Change' }));
    await userEvent.type(screen.getByLabelText('GitHub client id'), 'Ov23liTheirs');
    await userEvent.click(screen.getByRole('button', { name: 'Save' }));

    expect(await screen.findByText('Saved. Sign in again to use it.')).toBeInTheDocument();
    expect(screen.getByText('Ov23liTheirs')).toBeInTheDocument();
    expect(invoke).toHaveBeenCalledWith('set_sign_in_registration', {
      service: 'github',
      clientId: 'Ov23liTheirs',
    });
  });

  it('surfaces an id the backend refused, in the words it refused it with', async () => {
    answering([missing], new Error('a client id is letters, digits and one of -._~'));
    render(<SignInRegistration service="github" name="GitHub" where="Make one on GitHub." />);

    await userEvent.type(await screen.findByLabelText('GitHub client id'), 'https://example.com');
    await userEvent.click(screen.getByRole('button', { name: 'Save' }));

    expect(await screen.findByRole('alert')).toHaveTextContent(
      'a client id is letters, digits and one of -._~',
    );
  });

  /** Nothing to go back to is not a button that puts you back to nothing. */
  it('offers the built-in one back only when the build carries one', async () => {
    const orphan: Registration = {
      service: 'microsoft',
      clientId: 'theirs',
      source: 'stored',
      hasBuiltIn: false,
      overriddenByEnvironment: false,
    };

    answering([orphan]);
    render(<SignInRegistration service="microsoft" name="Outlook" where="Register one." />);

    await userEvent.click(await screen.findByRole('button', { name: 'Change' }));

    expect(screen.queryByRole('button', { name: 'Use the built-in one' })).not.toBeInTheDocument();
  });

  it('goes back to the built-in id on request', async () => {
    const stored: Registration = { ...builtIn, clientId: 'theirs', source: 'stored' };

    answering([stored], [builtIn]);
    render(<SignInRegistration service="github" name="GitHub" where="Make one on GitHub." />);

    await userEvent.click(await screen.findByRole('button', { name: 'Change' }));
    await userEvent.click(screen.getByRole('button', { name: 'Use the built-in one' }));

    expect(await screen.findByText('Ov23liBuiltIn')).toBeInTheDocument();
    expect(invoke).toHaveBeenCalledWith('clear_sign_in_registration', { service: 'github' });
  });

  /**
   * A field that silently does nothing is worse than one that is not there:
   * the environment wins, so the screen says so rather than letting somebody
   * store an id and wonder why sign-in still uses another.
   */
  it('says when an environment variable is overriding what is stored', async () => {
    show([{ ...builtIn, source: 'environment', overriddenByEnvironment: true }]);

    await userEvent.click(await screen.findByRole('button', { name: 'Change' }));

    expect(screen.getByText(/An environment variable is setting this/)).toBeInTheDocument();
  });

  /** A backend that cannot answer leaves the rest of the card alone. */
  it('renders nothing at all when the registration cannot be read', async () => {
    show(new Error('the local database is not available'));

    await expect(screen.findByText('Sign-in registration', {}, { timeout: 200 })).rejects.toThrow();
  });
});
