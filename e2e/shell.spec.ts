/**
 * The shell is the size the design system says it is.
 *
 * These are numbers jsdom cannot see — it reports every height as zero — and
 * they are the ones the whole layout is hung off: a 56px icon rail, a 48px
 * header, and a detail measure that stops at 680 however wide the window gets.
 * The bundled typefaces are here for the same reason: whether a font actually
 * loaded is a question only a real browser can answer, and the CSP allows no
 * second chance from a CDN.
 */

import { expect, longAnswer, test, type Account, type SyncState } from './fixtures';

const octocat: Account = {
  id: 1,
  service: 'github',
  accountKey: 'octocat',
  label: null,
  identity: 'octocat',
  connectedAt: '2026-08-19T14:00:00.000Z',
};

test.describe('shell metrics', () => {
  test('runs a 56px icon rail beside a 48px header', async ({ chief, page }) => {
    await chief.open();

    const rail = await page.locator('aside').boundingBox();
    const header = await page.locator('header').boundingBox();

    expect(rail, 'the rail should be on screen').not.toBeNull();
    expect(header, 'the header should be on screen').not.toBeNull();
    expect(Math.round(rail?.width ?? 0)).toBe(56);
    expect(Math.round(header?.height ?? 0)).toBe(48);
  });

  test('still runs a 48px header once the sync time is in it', async ({ chief, page }) => {
    // The header gained a third clause. It is one line of 11px mono in a
    // 48px band, and the band is what the whole shell is hung off.
    await chief.open({
      accounts: [octocat],
      syncStates: [
        {
          accountId: 1,
          source: 'github',
          status: 'ok',
          lastSyncedAt: new Date(Date.now() - 30 * 60 * 1000).toISOString(),
          errorMessage: null,
        },
      ],
    });

    await expect(page.locator('header p')).toContainText('synced 30m ago');

    const header = await page.locator('header').boundingBox();

    expect(Math.round(header?.height ?? 0)).toBe(48);
  });

  test('keeps the rail full height and the header beside it', async ({ chief, page }) => {
    // Settings has no list pane, so this is the rail against the detail with
    // nothing in between. Where the header starts once a pane *is* there is
    // measured in `feed.spec.ts`.
    await chief.open();
    await chief.goTo('Settings');

    const rail = await page.locator('aside').boundingBox();
    const header = await page.locator('header').boundingBox();

    expect(Math.round(rail?.height ?? 0)).toBe(page.viewportSize()?.height);
    expect(Math.round(header?.x ?? 0)).toBe(56);
  });

  test('stops the transcript at the 680px measure', async ({ chief, page }) => {
    await chief.open({ answer: longAnswer(4) });
    await chief.ask('What did I ship?');

    const measure = await page.locator('[role="dialog"] ul').boundingBox();

    expect(measure?.width ?? 0).toBeLessThanOrEqual(680);
  });
});

test.describe('reduced motion', () => {
  test('swaps the sweeping hairline for a still dot', async ({ chief, page }) => {
    await page.emulateMedia({ reducedMotion: 'reduce' });
    await chief.open({ holdAnswers: true });
    await chief.ask('What is waiting on me?');
    await chief.stream({ kind: 'tool', name: 'fetch_github_prs' });

    const status = page.getByRole('status');

    await expect(status).toContainText('Reading your pull requests on GitHub');
    await expect(status.locator('.motion-loop')).toBeHidden();
    await expect(status.locator('.motion-still')).toBeVisible();
  });

  test('sweeps while the loop is allowed', async ({ chief, page }) => {
    await page.emulateMedia({ reducedMotion: 'no-preference' });
    await chief.open({ holdAnswers: true });
    await chief.ask('What is waiting on me?');
    await chief.stream({ kind: 'tool', name: 'fetch_github_prs' });

    const status = page.getByRole('status');

    await expect(status.locator('.motion-loop')).toBeVisible();
    await expect(status.locator('.motion-still')).toBeHidden();
  });
});

test.describe('the focus ring', () => {
  test('is one ring, and every field wears it', async ({ chief, page }) => {
    // A Tailwind utility outranks the `:focus-visible` rule in globals.css, so
    // a control that brings its own ring quietly leaves the system — and the
    // difference is a shadow, which jsdom cannot see at all. Measured against
    // the composer rather than against a literal: the rule is that they match.
    await chief.open({ accounts: [octocat] });

    await chief.openChat();
    await chief.composer.focus();
    const composer = await chief.composer.evaluate((node) => getComputedStyle(node).boxShadow);

    await chief.closeChat();
    await chief.goTo('Settings');
    const name = page.getByLabel('Name for octocat');
    await name.focus();

    expect(await name.evaluate((node) => getComputedStyle(node).boxShadow)).toBe(composer);
    expect(composer, 'the ring should be a 3px spread').toContain('3px');
  });
});

test.describe('the bundled typefaces', () => {
  test('load from the app itself, not a font CDN', async ({ chief, page }) => {
    await chief.open();
    await page.waitForFunction(() => document.fonts.status === 'loaded');

    const faces = await page.evaluate(() => ({
      sans: document.fonts.check('600 15px "Instrument Sans Variable"'),
      mono: document.fonts.check('400 11px "IBM Plex Mono"'),
      // Nothing may be fetched from anywhere but this origin.
      offOrigin: performance
        .getEntriesByType('resource')
        .map((entry) => new URL(entry.name).origin)
        .filter((origin) => origin !== location.origin),
    }));

    expect(faces.sans, 'Instrument Sans should be available').toBe(true);
    expect(faces.mono, 'IBM Plex Mono should be available').toBe(true);
    expect(faces.offOrigin, 'the app should fetch nothing off its own origin').toEqual([]);
  });
});

test.describe('a connected account', () => {
  /**
   * The row is a field with a button beside it, and the two have to read as
   * one line.
   *
   * They did not. The row centred the button against the *left column* — the
   * field with its "Connected … · Synced …" caption underneath — rather than
   * against the field, so the button sat half a caption lower than the thing
   * it belongs to. jsdom cannot see this: it reports every height as zero, so
   * a browser is the only place the question can be asked.
   */
  test('lines the disconnect button up with the field it belongs to', async ({ chief, page }) => {
    await chief.open({ accounts: [octocat] });
    await chief.goTo('Settings');

    const field = await page.getByLabel('Name for octocat').boundingBox();
    const button = await page.getByRole('button', { name: 'Disconnect octocat' }).boundingBox();

    if (field === null || button === null) throw new Error('the account row should be on screen');

    const middleOf = (box: { y: number; height: number }) => box.y + box.height / 2;

    // A pixel of slack for rounding, and nothing like the half-caption the
    // two were out by.
    expect(Math.abs(middleOf(field) - middleOf(button))).toBeLessThanOrEqual(1);
  });
});

test.describe('the one colour that means you are needed', () => {
  const revoked: SyncState = {
    accountId: 1,
    source: 'github',
    status: 'authRequired',
    lastSyncedAt: '2026-09-01T09:00:00.000Z',
    errorMessage: 'the credential was refused',
  };

  /**
   * The design system reserves amber for *you are needed*, and only a revoked
   * credential is that. A class name is not the assertion — what the browser
   * actually paints is, because the tone is a token and a token can be
   * redefined out from under the class that names it.
   */
  test('paints a revoked credential amber', async ({ chief, page }) => {
    await chief.open({ accounts: [octocat], syncStates: [revoked] });
    await chief.goTo('Settings');

    const asking = page.getByTestId('freshness');
    await expect(asking).toHaveText('Sign in again');

    const filled = await asking.evaluate((node) => getComputedStyle(node).backgroundColor);

    expect(filled, 'an amber chip is a tinted fill, not bare text').not.toBe('rgba(0, 0, 0, 0)');
    await expect(page.getByRole('button', { name: 'Reconnect' })).toBeVisible();
  });

  /** A machine that is working is not somebody being asked for something. */
  test('leaves a healthy account uncoloured', async ({ chief, page }) => {
    await chief.open({
      accounts: [octocat],
      syncStates: [{ ...revoked, status: 'ok', lastSyncedAt: new Date().toISOString() }],
    });
    await chief.goTo('Settings');

    const fine = page.getByTestId('freshness');
    await expect(fine).toHaveText('Synced just now');

    const painted = await fine.evaluate((node) => getComputedStyle(node).backgroundColor);

    expect(painted, 'a working account is not a signal').toBe('rgba(0, 0, 0, 0)');
    await expect(page.getByRole('button', { name: 'Reconnect' })).toHaveCount(0);
  });
});
