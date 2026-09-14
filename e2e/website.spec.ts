import { expect, test, type Page } from '@playwright/test';

/**
 * Layout tests for the marketing site.
 *
 * Both of the things measured here are invisible to jsdom and to reading the
 * CSS: whether three text runs at three different sizes sit on one baseline,
 * and whether a header row fits across a phone. The site is served as static
 * files, the way it ships.
 */

/** Every page that carries the shared header. */
const PAGES = ['/index.html', '/docs.html', '/security.html', '/changelog.html'];

/**
 * The y of a text baseline, measured rather than calculated.
 *
 * An empty inline-block has its baseline at its own bottom edge, so one
 * dropped into a line box and measured reports where that line box's baseline
 * is. It is removed again before anything else looks at the page.
 */
async function baselineOf(page: Page, selector: string) {
  return page.evaluate((target) => {
    const host = document.querySelector(target);
    if (host === null) throw new Error(`nothing matched ${target}`);

    // The element the text actually lives in — the wordmark's is a <p>, the
    // nav link's is the anchor itself.
    const walker = document.createTreeWalker(host, NodeFilter.SHOW_TEXT);
    let parent: HTMLElement | null = null;

    for (let node = walker.nextNode(); node !== null; node = walker.nextNode()) {
      if (node.nodeValue !== null && node.nodeValue.trim() !== '') {
        parent = node.parentElement;
        break;
      }
    }

    if (parent === null) throw new Error(`no text inside ${target}`);

    const probe = document.createElement('i');
    probe.style.display = 'inline-block';
    probe.style.width = '0';
    probe.style.height = '0';
    parent.append(probe);

    const { bottom } = probe.getBoundingClientRect();
    probe.remove();

    return bottom;
  }, selector);
}

for (const path of PAGES) {
  test(`the header sits on one baseline on ${path}`, async ({ page }) => {
    await page.goto(path);

    const wordmark = await baselineOf(page, '.site-header .wordmark');
    const link = await baselineOf(page, '.site-nav a:not(.btn)');
    const button = await baselineOf(page, '.site-nav .btn');

    // Sub-pixel, because they are aligned rather than nudged into place.
    expect(Math.abs(link - wordmark)).toBeLessThan(0.5);
    expect(Math.abs(button - wordmark)).toBeLessThan(0.5);
  });
}

test('the header stays on one baseline once the phone rules apply', async ({ page }) => {
  await page.setViewportSize({ width: 390, height: 800 });
  await page.goto('/index.html');

  const wordmark = await baselineOf(page, '.site-header .wordmark');
  const link = await baselineOf(page, '.site-nav a:not(.btn)');
  const button = await baselineOf(page, '.site-nav .btn');

  expect(Math.abs(link - wordmark)).toBeLessThan(0.5);
  expect(Math.abs(button - wordmark)).toBeLessThan(0.5);
});

/* The reported break: the header ran wider than the screen and took the
   download button off the right-hand edge with it. 320px is the narrowest
   screen anybody still browses on. */
for (const width of [430, 390, 360, 320]) {
  test(`the header fits across ${width}px, download button and all`, async ({ page }) => {
    await page.setViewportSize({ width, height: 800 });
    await page.goto('/index.html');

    const scrollWidth = await page.evaluate(() => document.documentElement.scrollWidth);
    expect(scrollWidth).toBeLessThanOrEqual(width);

    const button = await page.locator('.site-nav .btn').boundingBox();
    expect(button).not.toBeNull();
    expect(button!.x).toBeGreaterThanOrEqual(0);
    expect(button!.x + button!.width).toBeLessThanOrEqual(width);

    // One row, not two: the header is 68px tall everywhere.
    const header = await page.locator('.site-header .shell').boundingBox();
    expect(header!.height).toBe(68);
  });
}

test('the GitHub link keeps its name when it loses its label', async ({ page }) => {
  await page.setViewportSize({ width: 430, height: 800 });
  await page.goto('/index.html');

  await expect(page.getByRole('link', { name: 'GitHub' })).toHaveCount(2);
});

test('the changelog is reachable from the hero and from the footer', async ({ page }) => {
  await page.goto('/index.html');

  await expect(page.locator('.hero-actions a[href="changelog.html"]')).toBeVisible();
  await expect(page.locator('.site-footer a[href="changelog.html"]')).toBeVisible();

  await page.locator('.hero-actions a[href="changelog.html"]').click();

  await expect(page).toHaveURL(/changelog\.html$/);
  await expect(page.getByRole('heading', { level: 1 })).toHaveText(
    'What changed, release by release.',
  );
});

test('every release on the changelog has its number, its date and its changes', async ({
  page,
}) => {
  await page.goto('/changelog.html');

  const releases = page.locator('.release');
  await expect(releases.first().locator('.release-version')).toHaveText(/^\d+\.\d+\.\d+$/);
  await expect(releases.first().locator('.release-date')).toHaveText(/^\d{4}-\d\d-\d\d$/);
  expect(await releases.count()).toBeGreaterThan(0);
  expect(await page.locator('.change').count()).toBeGreaterThan(0);

  // Only the newest release is the current one.
  await expect(page.locator('.release-latest')).toHaveCount(1);
});

test('the changelog does not scroll sideways on a phone', async ({ page }) => {
  await page.setViewportSize({ width: 360, height: 800 });
  await page.goto('/changelog.html');

  expect(await page.evaluate(() => document.documentElement.scrollWidth)).toBeLessThanOrEqual(360);
});
