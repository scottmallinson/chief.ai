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

import { expect, longAnswer, test } from './fixtures';

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
