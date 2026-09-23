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
  verificationUriComplete: 'https://github.com/login/device?user_code=WDJB-MJHT',
  expiresIn: 900,
};

/**
 * The profile card asks for a plan on every render of this view. An empty one
 * keeps it out of the way of tests about something else, while still being the
 * shape the command actually returns — a stub that answered `[]` to everything
 * took the whole screen down with it.
 */
const NO_PLAN = { reads: [], writes: [], keeps: [] };

/**
 * Both services signing in against the id this build was compiled with, which
 * is the ordinary case and the one that says nothing much.
 *
 * A `Registration[]` and not an account list: the two share a `service` field
 * and nothing else, so a catch-all answering this command with accounts would
 * hand the card an object with no `source` and it would render the word
 * `undefined` — the shortcut CLAUDE.md records taking the screen down three
 * times, in miniature.
 */
const BUILT_IN = [
  {
    service: 'github',
    clientId: 'Ov23liBuiltIn',
    source: 'builtIn',
    hasBuiltIn: true,
    overriddenByEnvironment: false,
  },
  {
    service: 'microsoft',
    clientId: null,
    source: 'missing',
    hasBuiltIn: false,
    overriddenByEnvironment: false,
  },
];

/**
 * Answer the commands whose replies are not account lists in their own shapes,
 * and everything else with `value`.
 *
 * Each is dispatched on the command name rather than being swept up by the
 * catch-all, because none of them returns a list of accounts: `sync_status`
 * returns `SyncState[]` keyed on `accountId`, and answering it with the
 * account list would key the map on `undefined` and quietly render nothing.
 */
/**
 * What `finish_login` actually answers.
 *
 * Not the account list: the list alone cannot say whether a sign-in added an
 * account or landed back on one already there, which is the whole of what
 * "add another account" can get wrong. A stub answering an array here is the
 * shortcut CLAUDE.md records taking the screen down three times — the hook
 * reads `.accounts` off it and gets `undefined`.
 */
function connected(accounts: unknown[], reconnected = false) {
  return { accounts, account: accounts.at(-1), reconnected };
}

/** What `window_behaviour` answers on a machine with a tray and a choice made. */
const KEEPS_RUNNING = { tray: true, keepRunning: true, trayName: 'system tray' };

function answering(value: unknown, syncStates: unknown[] = [], registrations = BUILT_IN) {
  invoke.mockImplementation((command: string) => {
    if (command === 'profile_plan') return Promise.resolve(NO_PLAN);
    if (command === 'sync_status') return Promise.resolve(syncStates);
    if (command === 'account_data') return Promise.resolve({ entries: 0, proposals: 0 });
    if (command === 'sign_in_registrations') return Promise.resolve(registrations);
    if (command === 'window_behaviour') return Promise.resolve(KEEPS_RUNNING);
    if (command === 'launch_at_login') return Promise.resolve(false);
    if (command === 'finish_login') {
      return Promise.resolve(connected(Array.isArray(value) ? value : []));
    }

    return Promise.resolve(value);
  });
}

describe('SettingsView', () => {
  beforeEach(() => {
    invoke.mockReset();
    openUrl.mockReset();
    openUrl.mockResolvedValue(undefined);
  });

  it('offers to connect GitHub when it is not connected', async () => {
    answering([]);

    render(<SettingsView />);

    expect(await screen.findByRole('button', { name: 'Connect GitHub' })).toBeInTheDocument();
  });

  /**
   * **One read of the connections to render one screen.**
   *
   * Every card used to call `useIntegrations` itself — three `Integration`
   * cards plus the calendar and Linear — so the table was read five times and
   * each card held its own copy of the answer. Cheap, since it is local
   * SQLite, and wrong in a way that shows: after an action the cards disagree
   * until whichever one owns the change reloads, and they settle at five
   * different moments.
   *
   * Counted against the stub rather than assumed, because the number is the
   * whole claim.
   */
  it('reads the connections once, not once per card', async () => {
    answering([]);

    render(<SettingsView />);

    // Waited on the last card to settle, so this counts a finished render
    // rather than however far through one the assertion happened to land.
    expect(await screen.findByRole('button', { name: 'Connect Atlassian' })).toBeInTheDocument();

    const reads = invoke.mock.calls.filter(([command]) => command === 'connections');

    expect(reads).toHaveLength(1);
  });

  const corpus = {
    root: '/Users/someone/Chief',
    exists: true,
    files: 7,
    estimatedTokens: 1840,
  };

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
    invoke.mockImplementation((command: string) => {
      if (command === 'profile_plan') return Promise.resolve(NO_PLAN);
      if (command === 'run_doctor') return Promise.resolve(report);
      if (command === 'corpus_location') return Promise.resolve(corpus);
      return Promise.resolve([]);
    });

    render(<SettingsView />);

    // GitHub, Outlook, Linear and Atlassian each say so; the calendar card
    // says "None", because a subscription is not a connection. A plain
    // assertion again since REC-42: one hook serves every card, so they settle
    // in one render and the first match means all of them. It was a `waitFor`
    // on the count for as long as each card held its own hook and they landed
    // at four different moments — the test working around the design.
    expect(await screen.findAllByText('Not connected')).toHaveLength(4);
    expect(screen.getByRole('button', { name: 'Connect GitHub' })).toBeInTheDocument();
    expect(screen.getByRole('button', { name: 'Connect Outlook' })).toBeInTheDocument();
    expect(screen.getByRole('button', { name: 'Connect Atlassian' })).toBeInTheDocument();
    expect(screen.getByText('On this machine')).toBeInTheDocument();

    // The model is whatever this machine was given, not a name written into
    // the component — there is more than one now.
    expect(await screen.findByText('Llama 3.2 3B Instruct (Q4_K_M)')).toBeInTheDocument();

    // And what it worked out about the machine, in the units a person reads.
    expect(screen.getByText('16.0 GB')).toBeInTheDocument();
    expect(screen.getByText('8192 tokens')).toBeInTheDocument();
    expect(screen.getByText('0.9s')).toBeInTheDocument();

    // And where the corpus is, in words a person can act on: a folder they
    // can open, not a path buried in the app's own data.
    expect(screen.getByText('/Users/someone/Chief')).toBeInTheDocument();
    expect(screen.getByText('7 files')).toBeInTheDocument();
  });

  it('says when a connected account was connected', async () => {
    answering([octocat]);

    render(<SettingsView />);

    expect(await screen.findByText(/^Connected /)).toBeInTheDocument();
  });

  it('shows the device code and opens the browser', async () => {
    invoke.mockImplementation((command: string) => {
      if (command === 'profile_plan') return Promise.resolve(NO_PLAN);
      if (command === 'connections') return Promise.resolve([]);
      if (command === 'start_login') return Promise.resolve(deviceLogin);
      // Never settles, so the waiting state stays on screen.
      return new Promise(() => {});
    });

    render(<SettingsView />);
    await userEvent.click(await screen.findByRole('button', { name: 'Connect GitHub' }));

    expect(await screen.findByText('WDJB-MJHT')).toBeInTheDocument();
    // The page with the code already in it. The code stays on screen anyway,
    // because the prefill is undocumented and may stop working.
    expect(openUrl).toHaveBeenCalledWith('https://github.com/login/device?user_code=WDJB-MJHT');
  });

  it('says so when the corpus folder is not there yet', async () => {
    invoke.mockImplementation((command: string) => {
      if (command === 'profile_plan') return Promise.resolve(NO_PLAN);
      if (command === 'corpus_location') {
        return Promise.resolve({ ...corpus, exists: false, files: 0 });
      }
      return Promise.resolve([]);
    });

    render(<SettingsView />);

    expect(await screen.findByText(/This folder is not there/)).toBeInTheDocument();

    // Nothing to show yet, so the control that would show it is not offered.
    expect(screen.getByRole('button', { name: /Show folder/ })).toBeDisabled();
  });

  it('sends an Outlook sign-in to the browser rather than showing a code', async () => {
    const browserLogin = {
      kind: 'browser',
      url: 'https://login.microsoftonline.com/common/oauth2/v2.0/authorize?client_id=abc',
    };

    invoke.mockImplementation((command: string) => {
      if (command === 'profile_plan') return Promise.resolve(NO_PLAN);
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
    expect(screen.queryByText(/Your browser is open at/)).not.toBeInTheDocument();

    // The destination is shown as well as opened, so a browser that did not
    // open leaves the user something to act on.
    expect(screen.getByText(browserLogin.url)).toBeInTheDocument();
  });

  it('reports the connection once the user finishes', async () => {
    invoke.mockImplementation((command: string) => {
      if (command === 'profile_plan') return Promise.resolve(NO_PLAN);
      if (command === 'connections') return Promise.resolve([]);
      if (command === 'start_login') return Promise.resolve(deviceLogin);
      if (command === 'finish_login') return Promise.resolve(connected([octocat]));
      return Promise.resolve([octocat]);
    });

    render(<SettingsView />);
    await userEvent.click(await screen.findByRole('button', { name: 'Connect GitHub' }));

    expect(await screen.findByRole('button', { name: 'Disconnect octocat' })).toBeInTheDocument();
    expect(screen.queryByText('WDJB-MJHT')).not.toBeInTheDocument();
  });

  it('surfaces a missing client id rather than failing silently', async () => {
    invoke.mockImplementation((command: string) => {
      if (command === 'profile_plan') return Promise.resolve(NO_PLAN);
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
    invoke.mockImplementation((command: string) => {
      if (command === 'profile_plan') return Promise.resolve(NO_PLAN);
      return Promise.resolve(command === 'disconnect' ? [] : [octocat]);
    });

    render(<SettingsView />);
    await userEvent.click(await screen.findByRole('button', { name: 'Disconnect octocat' }));
    await userEvent.click(
      await screen.findByRole('button', { name: 'Delete octocat and everything it stored' }),
    );

    expect(await screen.findByRole('button', { name: 'Connect GitHub' })).toBeInTheDocument();
    // Keyed on the account, not the service: forgetting one of two GitHub
    // accounts has to leave the other connected.
    expect(invoke).toHaveBeenCalledWith('disconnect', { accountId: 1 });
  });

  it('names the service it is signing in to', async () => {
    invoke.mockImplementation((command: string) => {
      if (command === 'profile_plan') return Promise.resolve(NO_PLAN);
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
    answering([octocat, hubot]);

    render(<SettingsView />);

    expect(await screen.findByLabelText('Name for octocat')).toBeInTheDocument();
    expect(screen.getByLabelText('Name for hubot')).toBeInTheDocument();
  });

  it('prefers the name the user gave an account', async () => {
    answering([hubot]);

    render(<SettingsView />);

    // The chosen name is the field's value; the raw identity is only its
    // placeholder, so the disconnect button is where the name has to surface.
    expect(await screen.findByRole('button', { name: 'Disconnect Work' })).toBeInTheDocument();
    expect(screen.getByLabelText('Name for hubot')).toHaveValue('Work');
  });

  it('lets an account be named', async () => {
    answering([octocat, hubot]);

    render(<SettingsView />);

    expect(await screen.findByLabelText('Name for hubot')).toHaveValue('Work');

    await userEvent.type(screen.getByLabelText('Name for octocat'), 'Personal');
    await userEvent.tab();

    expect(invoke).toHaveBeenCalledWith('label_account', { accountId: 1, label: 'Personal' });
  });

  it('clears a name when the field is emptied', async () => {
    answering([hubot]);

    render(<SettingsView />);

    await userEvent.clear(await screen.findByLabelText('Name for hubot'));
    await userEvent.tab();

    expect(invoke).toHaveBeenCalledWith('label_account', { accountId: 2, label: null });
  });

  it('offers to add another account when one is already connected', async () => {
    answering([octocat]);

    render(<SettingsView />);

    expect(
      await screen.findByRole('button', { name: 'Add another GitHub account' }),
    ).toBeInTheDocument();
  });

  /**
   * **One answer, shared — so nothing has to be told twice.**
   *
   * When each card held its own `useIntegrations`, a disconnect updated the
   * list the acting card was holding and left every other card's copy as it
   * was. Nothing visibly broke, because the cards partition by service, but
   * the screen held five lists that were free to disagree and the only cure
   * was another read.
   *
   * Asserted as the absence of that read: the change lands, and the table is
   * not consulted again to make it land.
   */
  it('carries a disconnect to every card without reading the table again', async () => {
    answering([octocat]);
    invoke.mockImplementation((command: string) => {
      if (command === 'profile_plan') return Promise.resolve(NO_PLAN);
      if (command === 'sync_status') return Promise.resolve([]);
      if (command === 'account_data') return Promise.resolve({ entries: 0, proposals: 0 });
      if (command === 'sign_in_registrations') return Promise.resolve(BUILT_IN);
      if (command === 'disconnect') return Promise.resolve([]);

      return Promise.resolve([octocat]);
    });

    render(<SettingsView />);

    await userEvent.click(await screen.findByRole('button', { name: 'Disconnect octocat' }));
    await userEvent.click(
      await screen.findByRole('button', { name: 'Delete octocat and everything it stored' }),
    );

    // The account is gone from the card that owned it.
    await waitFor(() =>
      expect(screen.queryByRole('button', { name: 'Disconnect octocat' })).not.toBeInTheDocument(),
    );

    // And the whole screen agrees, having read the connections exactly once —
    // at mount, before any of this.
    expect(invoke.mock.calls.filter(([command]) => command === 'connections')).toHaveLength(1);
    await waitFor(() => expect(screen.getAllByText('Not connected')).toHaveLength(4));
  });

  it('disconnects the account whose button was pressed', async () => {
    invoke.mockImplementation((command: string) => {
      if (command === 'profile_plan') return Promise.resolve(NO_PLAN);
      if (command === 'connections') return Promise.resolve([octocat, hubot]);
      if (command === 'account_data') return Promise.resolve({ entries: 3, proposals: 1 });
      return Promise.resolve([octocat]);
    });

    render(<SettingsView />);

    const buttons = await screen.findAllByRole('button', { name: /^Disconnect/ });
    await userEvent.click(buttons[1]);

    // The confirmation says what goes, and it is the second account's counts
    // that are being read — the first row is untouched.
    expect(await screen.findByText(/3 work log entries and 1 draft/)).toBeInTheDocument();
    expect(invoke).toHaveBeenCalledWith('account_data', { accountId: 2 });
    expect(invoke).not.toHaveBeenCalledWith('disconnect', { accountId: 2 });

    await userEvent.click(
      screen.getByRole('button', { name: 'Delete Work and everything it stored' }),
    );

    expect(invoke).toHaveBeenCalledWith('disconnect', { accountId: 2 });
  });

  it('disconnects an account named in the same gesture', async () => {
    invoke.mockImplementation((command: string) => {
      if (command === 'profile_plan') return Promise.resolve(NO_PLAN);
      if (command === 'connections') return Promise.resolve([octocat, hubot]);
      if (command === 'label_account') {
        return Promise.resolve([{ ...octocat, label: 'Personal' }, hubot]);
      }
      if (command === 'account_data') return Promise.resolve({ entries: 0, proposals: 0 });
      return Promise.resolve([hubot]);
    });

    render(<SettingsView />);

    // Naming a field commits on blur, and the blur here is the mousedown of
    // the click that follows. That write must not swallow the click.
    await userEvent.type(await screen.findByLabelText('Name for octocat'), 'Personal');
    await userEvent.click(screen.getByRole('button', { name: 'Disconnect octocat' }));
    // The rename has landed by now, so the confirmation calls the account what
    // the user just called it.
    await userEvent.click(
      await screen.findByRole('button', { name: 'Delete Personal and everything it stored' }),
    );

    expect(invoke).toHaveBeenCalledWith('disconnect', { accountId: 1 });
  });
  it('disconnects one account while another is being renamed', async () => {
    invoke.mockImplementation((command: string) => {
      if (command === 'profile_plan') return Promise.resolve(NO_PLAN);
      if (command === 'connections') return Promise.resolve([octocat, hubot]);
      // Never settles: the write is still in flight when the click lands,
      // which is the whole of the race on a machine doing real work.
      if (command === 'label_account') return new Promise(() => {});
      if (command === 'account_data') return Promise.resolve({ entries: 0, proposals: 0 });
      return Promise.resolve([{ ...octocat, label: 'Personal' }]);
    });

    render(<SettingsView />);

    await userEvent.type(await screen.findByLabelText('Name for octocat'), 'Personal');
    await userEvent.click(screen.getByRole('button', { name: 'Disconnect Work' }));
    await userEvent.click(
      await screen.findByRole('button', { name: 'Delete Work and everything it stored' }),
    );

    expect(invoke).toHaveBeenCalledWith('disconnect', { accountId: 2 });
  });

  it('keeps the other account nameable while one rename is in flight', async () => {
    invoke.mockImplementation((command: string) => {
      if (command === 'profile_plan') return Promise.resolve(NO_PLAN);
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
      if (command === 'profile_plan') return Promise.resolve(NO_PLAN);
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
    let finish: (result: unknown) => void = () => {};

    invoke.mockImplementation((command: string) => {
      if (command === 'profile_plan') return Promise.resolve(NO_PLAN);
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
      finish(connected([octocat]));
      await Promise.resolve();
    });

    expect(screen.queryByRole('button', { name: 'Disconnect octocat' })).not.toBeInTheDocument();
    expect(screen.getByRole('button', { name: 'Connect GitHub' })).toBeEnabled();
  });

  /**
   * The defect this reports on: adding a second GitHub account looks exactly
   * like adding nothing.
   *
   * The device flow authorises whoever the browser is signed in to, which is
   * the account already connected unless the user changed it there. GitHub
   * hands back that same login, `save` upserts onto its row, and the settings
   * screen re-renders one account — the same one — and says nothing at all.
   * The button reads as broken when what actually happened is that the sign-in
   * worked and went to the wrong person.
   *
   * Proved by not reporting it, which is what this replaced:
   *
   * ```text
   * Unable to find an element with the text: /signed in as octocat, which was
   *   already connected/
   * ```
   */
  it('says when adding an account signed in as the one already connected', async () => {
    invoke.mockImplementation((command: string) => {
      if (command === 'profile_plan') return Promise.resolve(NO_PLAN);
      if (command === 'sign_in_registrations') return Promise.resolve(BUILT_IN);
      if (command === 'connections') return Promise.resolve([octocat]);
      if (command === 'start_login') return Promise.resolve(deviceLogin);
      if (command === 'finish_login') return Promise.resolve(connected([octocat], true));
      return Promise.resolve([]);
    });

    render(<SettingsView />);
    await userEvent.click(
      await screen.findByRole('button', { name: 'Add another GitHub account' }),
    );

    // Naming the account is the message: it is what tells the user the browser
    // is the thing to change.
    expect(
      await screen.findByText(/signed in as octocat, which was already connected/),
    ).toBeVisible();
    expect(screen.getByText(/private window/)).toBeVisible();

    // Nothing failed, so this is not an alert.
    expect(screen.queryByRole('alert')).not.toBeInTheDocument();
  });

  /**
   * The same outcome, wanted. Pressing Reconnect on an account whose
   * credential was refused is *asking* to come back as that account, so
   * telling the user they did is noise — and noise on the amber that is
   * supposed to mean they are needed.
   */
  it('says nothing when reconnecting the account that asked to be reconnected', async () => {
    invoke.mockImplementation((command: string) => {
      if (command === 'profile_plan') return Promise.resolve(NO_PLAN);
      if (command === 'sign_in_registrations') return Promise.resolve(BUILT_IN);
      if (command === 'connections') return Promise.resolve([octocat]);
      if (command === 'start_login') return Promise.resolve(deviceLogin);
      if (command === 'finish_login') return Promise.resolve(connected([octocat], true));
      if (command === 'sync_status') {
        return Promise.resolve([
          {
            accountId: 1,
            source: 'github',
            status: 'authRequired',
            lastSyncedAt: '2026-09-01T09:00:00.000Z',
            errorMessage: 'the credential was refused',
          },
        ]);
      }
      return Promise.resolve([]);
    });

    render(<SettingsView />);
    await userEvent.click(await screen.findByRole('button', { name: 'Reconnect' }));

    await waitFor(() => expect(screen.queryByText('WDJB-MJHT')).not.toBeInTheDocument());
    expect(screen.queryByText(/was already connected/)).not.toBeInTheDocument();
  });

  it('says how long the code has left', async () => {
    invoke.mockImplementation((command: string) => {
      if (command === 'profile_plan') return Promise.resolve(NO_PLAN);
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
      if (command === 'profile_plan') return Promise.resolve(NO_PLAN);
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
    answering([octocat]);

    render(<SettingsView />);

    const field = await screen.findByLabelText('Name for octocat');
    await userEvent.type(field, 'a'.repeat(60));

    // The row is a field beside a button in a 680px measure, so a name has to
    // end somewhere. What it does to the layout is measured in the browser.
    expect(field).toHaveValue('a'.repeat(40));
  });
  describe('confirming a disconnect', () => {
    function withData(entries: number, proposals: number) {
      invoke.mockImplementation((command: string) => {
        if (command === 'profile_plan') return Promise.resolve(NO_PLAN);
        if (command === 'sync_status') return Promise.resolve([]);
        if (command === 'account_data') return Promise.resolve({ entries, proposals });

        return Promise.resolve([octocat]);
      });
    }

    /**
     * Disconnecting now deletes the account's work log entries, its drafts and
     * their markdown along with the credential — because that is what the word
     * means to somebody reading it. It is irreversible and removes something
     * they may not know is there, so it is confirmed with counts read from the
     * database rather than guessed at.
     */
    it('states what will go, in counts, before anything does', async () => {
      withData(42, 3);

      render(<SettingsView />);

      await userEvent.click(await screen.findByRole('button', { name: 'Disconnect octocat' }));

      expect(
        await screen.findByText(
          '42 work log entries and 3 drafts will be deleted from this machine.',
        ),
      ).toBeInTheDocument();
    });

    it('counts in the singular when there is one of something', async () => {
      withData(1, 1);

      render(<SettingsView />);

      await userEvent.click(await screen.findByRole('button', { name: 'Disconnect octocat' }));

      expect(
        await screen.findByText('1 work log entry and 1 draft will be deleted from this machine.'),
      ).toBeInTheDocument();
    });

    it('says so plainly when the account has left nothing behind', async () => {
      withData(0, 0);

      render(<SettingsView />);

      await userEvent.click(await screen.findByRole('button', { name: 'Disconnect octocat' }));

      expect(
        await screen.findByText('This account has left nothing on this machine.'),
      ).toBeInTheDocument();
    });

    /**
     * Asserted rather than assumed: this is the irreversible one.
     *
     * Proved by making Cancel confirm:
     *
     *   expected "spy" to not be called with arguments: [ 'disconnect', Anything ]
     */
    it('deletes nothing when the confirmation is cancelled', async () => {
      withData(42, 3);

      render(<SettingsView />);

      await userEvent.click(await screen.findByRole('button', { name: 'Disconnect octocat' }));
      await userEvent.click(await screen.findByRole('button', { name: 'Cancel' }));

      expect(invoke).not.toHaveBeenCalledWith('disconnect', expect.anything());
      // And the row is back to offering it, rather than stuck mid-question.
      expect(await screen.findByRole('button', { name: 'Disconnect octocat' })).toBeInTheDocument();
    });

    /**
     * A count Chief could not read is not a reason to refuse to disconnect,
     * and not a reason to claim there is nothing there either.
     */
    it('still offers the disconnect when the counts cannot be read', async () => {
      invoke.mockImplementation((command: string) => {
        if (command === 'profile_plan') return Promise.resolve(NO_PLAN);
        if (command === 'sync_status') return Promise.resolve([]);
        if (command === 'account_data') return Promise.reject(new Error('the database is locked'));

        return Promise.resolve([octocat]);
      });

      render(<SettingsView />);

      await userEvent.click(await screen.findByRole('button', { name: 'Disconnect octocat' }));

      expect(
        await screen.findByRole('button', { name: 'Delete octocat and everything it stored' }),
      ).toBeInTheDocument();
    });
  });

  describe('freshness', () => {
    /**
     * The crash class CLAUDE.md calls the most expensive shortcut in this
     * repository: a view receiving a shape it did not expect. A fresh install
     * has no `sync_state` rows at all, which is a legitimate state.
     */
    it('says never synced when nothing has been read yet, and throws nothing', async () => {
      answering([octocat], []);

      render(<SettingsView />);

      expect(await screen.findByText('Never synced')).toBeInTheDocument();
    });

    it('says how long ago the last successful read was', async () => {
      vi.useFakeTimers();
      vi.setSystemTime(new Date('2026-09-02T09:30:00.000Z'));

      try {
        answering(
          [octocat],
          [
            {
              accountId: 1,
              source: 'github',
              status: 'ok',
              lastSyncedAt: '2026-09-02T09:00:00.000Z',
              errorMessage: null,
            },
          ],
        );

        render(<SettingsView />);

        await vi.waitFor(() => {
          expect(screen.getByText('Synced 30m ago')).toBeInTheDocument();
        });
      } finally {
        vi.useRealTimers();
      }
    });

    /**
     * Amber is reserved for *you are needed*, and a revoked credential is the
     * only one of the four states that is. A host that could not be reached is
     * Chief's problem and will be tried again.
     */
    it('offers a way back in when the credential is gone', async () => {
      answering(
        [octocat],
        [
          {
            accountId: 1,
            source: 'github',
            status: 'authRequired',
            lastSyncedAt: '2026-09-01T09:00:00.000Z',
            errorMessage: 'the credential was refused',
          },
        ],
      );

      render(<SettingsView />);

      expect(await screen.findByText('Sign in again')).toBeInTheDocument();
      expect(screen.getByRole('button', { name: 'Reconnect' })).toBeInTheDocument();
    });

    it('does not offer a way back in, or use amber, for an account that is fine', async () => {
      answering(
        [octocat],
        [
          {
            accountId: 1,
            source: 'github',
            status: 'ok',
            lastSyncedAt: '2026-09-02T09:00:00.000Z',
            errorMessage: null,
          },
        ],
      );

      render(<SettingsView />);

      await screen.findByLabelText('Name for octocat');

      expect(screen.queryByRole('button', { name: 'Reconnect' })).not.toBeInTheDocument();
      expect(screen.queryByText('Sign in again')).not.toBeInTheDocument();
    });

    /**
     * The crash this hook was taught about the expensive way. Freshness is
     * rendered per account across three cards, so one answer of the wrong
     * shape took all three down at once — found by a test rather than by a
     * user, which is the system working, but only because something rendered
     * the real component.
     *
     * Proved by dropping the `Array.isArray` guard:
     *
     *   Uncaught [TypeError: all.map is not a function]
     */
    it('survives an answer that is not a list of states', async () => {
      invoke.mockImplementation((command: string) => {
        if (command === 'profile_plan') return Promise.resolve(NO_PLAN);
        if (command === 'sync_status') return Promise.resolve({ nothing: 'like a state' });

        return Promise.resolve([octocat]);
      });

      render(<SettingsView />);

      expect(await screen.findByLabelText('Name for octocat')).toBeInTheDocument();
      expect(screen.getByText('Never synced')).toBeInTheDocument();
    });

    /**
     * A failure reading freshness must not take the screen that carries it.
     * Freshness is an annotation on a screen whose actual job is connecting
     * accounts, and every row degrading to "Never synced" is the same thing a
     * fresh install shows.
     */
    it('still lets an account be managed when freshness cannot be read', async () => {
      invoke.mockImplementation((command: string) => {
        if (command === 'profile_plan') return Promise.resolve(NO_PLAN);
        if (command === 'sync_status') return Promise.reject(new Error('the database is locked'));

        return Promise.resolve([octocat]);
      });

      render(<SettingsView />);

      expect(await screen.findByLabelText('Name for octocat')).toBeInTheDocument();
      expect(screen.getByText('Never synced')).toBeInTheDocument();
    });
  });

  describe('what a connection permits', () => {
    /**
     * REC-22's acceptance criterion, and the reason the `grants` prop exists.
     *
     * Atlassian's write scopes sit on the same resource as its read scopes, so
     * asking for none of them is what makes read-only enforceable at the
     * authorization server rather than a promise about Chief's code. That is
     * worth something to the reader only if the screen says it, so the screen
     * is what this asserts — the scope list itself is guarded in Rust by
     * `atlassian::tests::asks_for_no_write_scope`.
     */
    it('says on screen that Atlassian is connected read-only', async () => {
      invoke.mockImplementation((command: string) => {
        if (command === 'profile_plan') return Promise.resolve(NO_PLAN);
        return Promise.resolve([]);
      });

      render(<SettingsView />);

      const stated = await screen.findByText(/read-only/, { selector: 'strong' });

      expect(stated).toBeInTheDocument();
      expect(stated.parentElement?.textContent).toMatch(
        /cannot create, edit or transition anything/,
      );
    });

    /**
     * The same rule applied to the provider it is least comfortable for.
     * GitHub's `repo` scope is read *and* write across private repositories,
     * which is broader than Chief needs, and leaving that implied would be the
     * omission this whole prop exists to prevent.
     */
    it('says on screen that GitHub grants more than Chief uses', async () => {
      invoke.mockImplementation((command: string) => {
        if (command === 'profile_plan') return Promise.resolve(NO_PLAN);
        return Promise.resolve([]);
      });

      render(<SettingsView />);

      const scope = await screen.findByText('repo');

      expect(scope.parentElement?.textContent).toMatch(/read .*and write/);
    });
  });
  describe('in the background', () => {
    function behaving(behaviour: unknown, atLogin: unknown = false) {
      invoke.mockImplementation((command: string) => {
        if (command === 'launch_at_login') {
          return atLogin instanceof Error ? Promise.reject(atLogin) : Promise.resolve(atLogin);
        }
        if (command === 'set_launch_at_login') return Promise.resolve(null);
        if (command === 'profile_plan') return Promise.resolve(NO_PLAN);
        if (command === 'sync_status') return Promise.resolve([]);
        if (command === 'sign_in_registrations') return Promise.resolve(BUILT_IN);
        if (command === 'window_behaviour') return Promise.resolve(behaviour);
        return Promise.resolve([]);
      });
    }

    it('stores the choice when the box is unticked', async () => {
      behaving(KEEPS_RUNNING);

      render(<SettingsView />);

      const box = await screen.findByRole('checkbox', {
        name: 'Keep running in the system tray when the window is closed',
      });
      expect(box).toBeChecked();

      await userEvent.click(box);

      expect(invoke).toHaveBeenCalledWith('set_keep_running', { keepRunning: false });
      await waitFor(() => expect(box).not.toBeChecked());
    });

    it('says it will ask when nothing has been chosen', async () => {
      behaving({ ...KEEPS_RUNNING, keepRunning: null });

      render(<SettingsView />);

      expect(
        await screen.findByText(/will ask the first time you close the window/),
      ).toBeInTheDocument();
    });

    /**
     * A switch that did nothing would be worse than none: hiding the window
     * with no icon to bring it back leaves a process nobody can reach, which
     * is why the backend quits on such a machine whatever was chosen.
     */
    it('offers no switch on a desktop with no tray', async () => {
      behaving({ tray: false, keepRunning: true, trayName: 'system tray' });

      render(<SettingsView />);

      expect(await screen.findByText(/closing the window\s+quits Chief/)).toBeInTheDocument();
      expect(
        screen.queryByRole('checkbox', { name: /Keep running in the/ }),
      ).not.toBeInTheDocument();
    });
    it('leaves launching at login off until it is turned on', async () => {
      behaving(KEEPS_RUNNING);

      render(<SettingsView />);

      const box = await screen.findByRole('checkbox', { name: 'Open Chief when you log in' });
      await waitFor(() => expect(box).toBeEnabled());
      expect(box).not.toBeChecked();
      expect(invoke).not.toHaveBeenCalledWith('set_launch_at_login', expect.anything());

      await userEvent.click(box);

      expect(invoke).toHaveBeenCalledWith('set_launch_at_login', { enabled: true });
      await waitFor(() => expect(box).toBeChecked());
    });

    it('says a launch at login starts in the tray', async () => {
      behaving(KEEPS_RUNNING, true);

      render(<SettingsView />);

      expect(
        await screen.findByRole('checkbox', { name: 'Open Chief when you log in' }),
      ).toBeInTheDocument();
      await waitFor(() =>
        expect(screen.getByRole('checkbox', { name: 'Open Chief when you log in' })).toBeChecked(),
      );
      expect(screen.getByText(/starts in the system tray without opening a window/)).toBeVisible();
    });

    it('shows why the login entry could not be read', async () => {
      behaving(KEEPS_RUNNING, new Error('no autostart directory'));

      render(<SettingsView />);

      expect(await screen.findByText('no autostart directory')).toBeInTheDocument();
    });
  });
});
