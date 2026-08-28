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

import { expect, longAnswer, test, type Account } from './fixtures';

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

  test('keeps the rail full height and the header beside it', async ({ chief, page }) => {
    await chief.open();

    const rail = await page.locator('aside').boundingBox();
    const header = await page.locator('header').boundingBox();

    expect(Math.round(rail?.height ?? 0)).toBe(page.viewportSize()?.height);
    expect(Math.round(header?.x ?? 0)).toBe(56);
  });

  test('stops the transcript at the 680px measure', async ({ chief, page }) => {
    await chief.open({ answer: longAnswer(4) });
    await chief.ask('What did I ship?');

    const measure = await page.locator('main ul').boundingBox();

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

    await chief.composer.focus();
    const composer = await chief.composer.evaluate((node) => getComputedStyle(node).boxShadow);

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
