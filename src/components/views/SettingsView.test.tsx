import { act, render, screen, waitFor } from '@testing-library/react';
import userEvent from '@testing-library/user-event';
import { beforeEach, describe, expect, it, vi } from 'vitest';

import { SettingsView } from '@/components/views/SettingsView';

const invoke = vi.hoisted(() => vi.fn());
const openUrl = vi.hoisted(() => vi.fn());

vi.mock('@tauri-apps/api/core', () => ({ invoke }));
vi.mock('@tauri-apps/plugin-opener', () => ({ openUrl }));

// The backend answers every one of these commands with the accounts as they
// now stand, so a fixture is a list rather than a status.
const octocat = {
  id: 1,
  service: 'github',
  accountKey: 'octocat',
  label: null,
  identity: 'octocat',
  connectedAt: '2026-08-19T14:00:00.000Z',
};

const hubot = {
  id: 2,
  service: 'github',
  accountKey: 'hubot',
  label: 'Work',
  identity: 'hubot',
  connectedAt: '2026-08-20T09:00:00.000Z',
};

const deviceLogin = {
  kind: 'device',
  userCode: 'WDJB-MJHT',
  verificationUri: 'https://github.com/login/device',
  expiresIn: 900,
};

describe('SettingsView', () => {
  beforeEach(() => {
    invoke.mockReset();
    openUrl.mockReset();
    openUrl.mockResolvedValue(undefined);
  });

  it('offers to connect GitHub when it is not connected', async () => {
    invoke.mockResolvedValue([]);

    render(<SettingsView />);

    expect(await screen.findByRole('button', { name: 'Connect GitHub' })).toBeInTheDocument();
  });

  const report = {
    tier: 'standard',
    model: 'Llama 3.2 3B Instruct (Q4_K_M)',
    modelSizeMb: 2400,
    contextSize: 8192,
    memoryMb: 16384,
    cores: 8,
    measurement: { firstTokenMs: 900, totalMs: 4900, characters: 400 },
    charactersPerSecond: 100,
  };

  it('carries the connection as state, not as an action', async () => {
    invoke.mockImplementation((command: string) =>
      command === 'run_doctor' ? Promise.resolve(report) : Promise.resolve([]),
    );

    render(<SettingsView />);

    // Two services now, each carrying its own state.
    expect(await screen.findAllByText('Not connected')).toHaveLength(2);
    expect(screen.getByRole('button', { name: 'Connect GitHub' })).toBeInTheDocument();
    expect(screen.getByRole('button', { name: 'Connect Outlook' })).toBeInTheDocument();
    expect(screen.getByText('On this machine')).toBeInTheDocument();

    // The model is whatever this machine was given, not a name written into
    // the component — there is more than one now.
    expect(await screen.findByText('Llama 3.2 3B Instruct (Q4_K_M)')).toBeInTheDocument();

    // And what it worked out about the machine, in the units a person reads.
    expect(screen.getByText('16.0 GB')).toBeInTheDocument();
    expect(screen.getByText('8192 tokens')).toBeInTheDocument();
    expect(screen.getByText('0.9s')).toBeInTheDocument();
  });

  it('says when a connected account was connected', async () => {
    invoke.mockResolvedValue([octocat]);

    render(<SettingsView />);

    expect(await screen.findByText(/^Connected /)).toBeInTheDocument();
  });

  it('shows the device code and opens the browser', async () => {
    invoke.mockImplementation((command: string) => {
      if (command === 'connections') return Promise.resolve([]);
      if (command === 'start_login') return Promise.resolve(deviceLogin);
      // Never settles, so the waiting state stays on screen.
      return new Promise(() => {});
    });

    render(<SettingsView />);
    await userEvent.click(await screen.findByRole('button', { name: 'Connect GitHub' }));

    expect(await screen.findByText('WDJB-MJHT')).toBeInTheDocument();
    expect(openUrl).toHaveBeenCalledWith('https://github.com/login/device');
  });

  it('sends an Outlook sign-in to the browser rather than showing a code', async () => {
    const browserLogin = {
      kind: 'browser',
      url: 'https://login.microsoftonline.com/common/oauth2/v2.0/authorize?client_id=abc',
    };

    invoke.mockImplementation((command: string) => {
      if (command === 'connections') return Promise.resolve([]);
      if (command === 'start_login') return Promise.resolve(browserLogin);
      // Never settles, so the waiting state stays on screen.
      return new Promise(() => {});
    });

    render(<SettingsView />);
    await userEvent.click(await screen.findByRole('button', { name: 'Connect Outlook' }));

    expect(await screen.findByText(/Finish signing in on the page/)).toBeInTheDocument();
    expect(openUrl).toHaveBeenCalledWith(browserLogin.url);

    // There is no code in this flow, and offering one would be a lie.
    expect(screen.queryByText(/Enter this code/)).not.toBeInTheDocument();

    // The destination is shown as well as opened, so a browser that did not
    // open leaves the user something to act on.
    expect(screen.getByText(browserLogin.url)).toBeInTheDocument();
  });

  it('reports the connection once the user finishes', async () => {
    invoke.mockImplementation((command: string) => {
      if (command === 'connections') return Promise.resolve([]);
      if (command === 'start_login') return Promise.resolve(deviceLogin);
      return Promise.resolve([octocat]);
    });

    render(<SettingsView />);
    await userEvent.click(await screen.findByRole('button', { name: 'Connect GitHub' }));

    expect(await screen.findByRole('button', { name: 'Disconnect octocat' })).toBeInTheDocument();
    expect(screen.queryByText('WDJB-MJHT')).not.toBeInTheDocument();
  });

  it('surfaces a missing client id rather than failing silently', async () => {
    invoke.mockImplementation((command: string) => {
      if (command === 'connections') return Promise.resolve([]);
      return Promise.reject(
        new Error('no GitHub client id is configured. Register an OAuth app...'),
      );
    });

    render(<SettingsView />);
    await userEvent.click(await screen.findByRole('button', { name: 'Connect GitHub' }));

    expect(await screen.findByRole('alert')).toHaveTextContent('no GitHub client id is configured');
  });

  it('lets a connected account be disconnected', async () => {
    invoke.mockImplementation((command: string) =>
      Promise.resolve(command === 'disconnect' ? [] : [octocat]),
    );

    render(<SettingsView />);
    await userEvent.click(await screen.findByRole('button', { name: 'Disconnect octocat' }));

    expect(await screen.findByRole('button', { name: 'Connect GitHub' })).toBeInTheDocument();
    // Keyed on the account, not the service: forgetting one of two GitHub
    // accounts has to leave the other connected.
    expect(invoke).toHaveBeenCalledWith('disconnect', { accountId: 1 });
  });

  it('names the service it is signing in to', async () => {
    invoke.mockImplementation((command: string) => {
      if (command === 'connections') return Promise.resolve([]);
      if (command === 'start_login') return Promise.resolve(deviceLogin);
      return new Promise(() => {});
    });

    render(<SettingsView />);
    await userEvent.click(await screen.findByRole('button', { name: 'Connect GitHub' }));

    await screen.findByText('WDJB-MJHT');
    expect(invoke).toHaveBeenCalledWith('start_login', { service: 'github' });
  });

  it('lists every connected account for a service', async () => {
    invoke.mockResolvedValue([octocat, hubot]);

    render(<SettingsView />);

    expect(await screen.findByLabelText('Name for octocat')).toBeInTheDocument();
    expect(screen.getByLabelText('Name for hubot')).toBeInTheDocument();
  });

  it('prefers the name the user gave an account', async () => {
    invoke.mockResolvedValue([hubot]);

    render(<SettingsView />);

    // The chosen name is the field's value; the raw identity is only its
    // placeholder, so the disconnect button is where the name has to surface.
    expect(await screen.findByRole('button', { name: 'Disconnect Work' })).toBeInTheDocument();
    expect(screen.getByLabelText('Name for hubot')).toHaveValue('Work');
  });

  it('lets an account be named', async () => {
    invoke.mockResolvedValue([octocat, hubot]);

    render(<SettingsView />);

    expect(await screen.findByLabelText('Name for hubot')).toHaveValue('Work');

    await userEvent.type(screen.getByLabelText('Name for octocat'), 'Personal');
    await userEvent.tab();

    expect(invoke).toHaveBeenCalledWith('label_account', { accountId: 1, label: 'Personal' });
  });

  it('clears a name when the field is emptied', async () => {
    invoke.mockResolvedValue([hubot]);

    render(<SettingsView />);

    await userEvent.clear(await screen.findByLabelText('Name for hubot'));
    await userEvent.tab();

    expect(invoke).toHaveBeenCalledWith('label_account', { accountId: 2, label: null });
  });

  it('offers to add another account when one is already connected', async () => {
    invoke.mockResolvedValue([octocat]);

    render(<SettingsView />);

    expect(
      await screen.findByRole('button', { name: 'Add another GitHub account' }),
    ).toBeInTheDocument();
  });

  it('disconnects the account whose button was pressed', async () => {
    invoke.mockImplementation((command: string) => {
      if (command === 'connections') return Promise.resolve([octocat, hubot]);
      return Promise.resolve([octocat]);
    });

    render(<SettingsView />);

    const buttons = await screen.findAllByRole('button', { name: /^Disconnect/ });
    await userEvent.click(buttons[1]);

    expect(invoke).toHaveBeenCalledWith('disconnect', { accountId: 2 });
  });

  it('disconnects an account named in the same gesture', async () => {
    invoke.mockImplementation((command: string) => {
      if (command === 'connections') return Promise.resolve([octocat, hubot]);
      if (command === 'label_account') {
        return Promise.resolve([{ ...octocat, label: 'Personal' }, hubot]);
      }
      return Promise.resolve([hubot]);
    });

    render(<SettingsView />);

    // Naming a field commits on blur, and the blur here is the mousedown of
    // the click that follows. That write must not swallow the click.
    await userEvent.type(await screen.findByLabelText('Name for octocat'), 'Personal');
    await userEvent.click(screen.getByRole('button', { name: 'Disconnect octocat' }));

    expect(invoke).toHaveBeenCalledWith('disconnect', { accountId: 1 });
  });
  it('disconnects one account while another is being renamed', async () => {
    invoke.mockImplementation((command: string) => {
      if (command === 'connections') return Promise.resolve([octocat, hubot]);
      // Never settles: the write is still in flight when the click lands,
      // which is the whole of the race on a machine doing real work.
      if (command === 'label_account') return new Promise(() => {});
      return Promise.resolve([{ ...octocat, label: 'Personal' }]);
    });

    render(<SettingsView />);

    await userEvent.type(await screen.findByLabelText('Name for octocat'), 'Personal');
    await userEvent.click(screen.getByRole('button', { name: 'Disconnect Work' }));

    expect(invoke).toHaveBeenCalledWith('disconnect', { accountId: 2 });
  });

  it('keeps the other account nameable while one rename is in flight', async () => {
    invoke.mockImplementation((command: string) => {
      if (command === 'connections') return Promise.resolve([octocat, hubot]);
      if (command === 'label_account') return new Promise(() => {});
      return Promise.resolve([octocat, hubot]);
    });

    render(<SettingsView />);

    await userEvent.type(await screen.findByLabelText('Name for octocat'), 'Personal');
    await userEvent.tab();

    // Renaming two accounts in a row is one gesture per field, not one at a
    // time: the second field cannot freeze because the first is saving.
    await userEvent.type(screen.getByLabelText('Name for hubot'), '!');
    expect(screen.getByLabelText('Name for hubot')).toHaveValue('Work!');
  });
  it('lets an abandoned sign-in be given up on', async () => {
    invoke.mockImplementation((command: string) => {
      if (command === 'connections') return Promise.resolve([octocat]);
      if (command === 'start_login') return Promise.resolve(deviceLogin);
      // Rust polls until GitHub expires the code, which is fifteen minutes.
      return new Promise(() => {});
    });

    render(<SettingsView />);
    await userEvent.click(
      await screen.findByRole('button', { name: 'Add another GitHub account' }),
    );
    await screen.findByText('WDJB-MJHT');

    await userEvent.click(screen.getByRole('button', { name: 'Cancel' }));

    expect(screen.queryByText('WDJB-MJHT')).not.toBeInTheDocument();
    expect(screen.getByRole('button', { name: 'Add another GitHub account' })).toBeEnabled();
    expect(screen.getByRole('button', { name: 'Disconnect octocat' })).toBeEnabled();
  });

  it('ignores a sign-in that finishes after it was given up on', async () => {
    let finish: (accounts: unknown[]) => void = () => {};

    invoke.mockImplementation((command: string) => {
      if (command === 'connections') return Promise.resolve([]);
      if (command === 'start_login') return Promise.resolve(deviceLogin);
      return new Promise((resolve) => {
        finish = resolve;
      });
    });

    render(<SettingsView />);
    await userEvent.click(await screen.findByRole('button', { name: 'Connect GitHub' }));
    await screen.findByText('WDJB-MJHT');
    await userEvent.click(screen.getByRole('button', { name: 'Cancel' }));

    // The poll in Rust cannot be called off, so it may still answer. An answer
    // nobody is waiting for must not connect an account behind the user.
    await act(async () => {
      finish([octocat]);
      await Promise.resolve();
    });

    expect(screen.queryByRole('button', { name: 'Disconnect octocat' })).not.toBeInTheDocument();
    expect(screen.getByRole('button', { name: 'Connect GitHub' })).toBeEnabled();
  });

  it('says how long the code has left', async () => {
    invoke.mockImplementation((command: string) => {
      if (command === 'connections') return Promise.resolve([]);
      if (command === 'start_login') return Promise.resolve(deviceLogin);
      return new Promise(() => {});
    });

    render(<SettingsView />);
    await userEvent.click(await screen.findByRole('button', { name: 'Connect GitHub' }));

    // The wait is bounded and the user is the one waiting, so say by how much.
    expect(await screen.findByText(/15:00 left/)).toBeInTheDocument();
  });
  it('puts the stored name back when a rename is refused', async () => {
    invoke.mockImplementation((command: string) => {
      if (command === 'connections') return Promise.resolve([hubot]);
      if (command === 'label_account') return Promise.reject(new Error('the database is locked'));
      return Promise.resolve([hubot]);
    });

    render(<SettingsView />);

    const field = await screen.findByLabelText('Name for hubot');
    await userEvent.clear(field);
    await userEvent.type(field, 'Personal');
    await userEvent.tab();

    expect(await screen.findByRole('alert')).toHaveTextContent('the database is locked');
    await waitFor(() => expect(field).toHaveValue('Work'));

    // And the field now agrees with the database again, so passing through it
    // does not fire the same refused write a second time.
    invoke.mockClear();
    await userEvent.click(field);
    await userEvent.tab();

    expect(invoke).not.toHaveBeenCalled();
  });
  it('bounds how long a name can be', async () => {
    invoke.mockResolvedValue([octocat]);

    render(<SettingsView />);

    const field = await screen.findByLabelText('Name for octocat');
    await userEvent.type(field, 'a'.repeat(60));

    // The row is a field beside a button in a 680px measure, so a name has to
    // end somewhere. What it does to the layout is measured in the browser.
    expect(field).toHaveValue('a'.repeat(40));
  });
});
