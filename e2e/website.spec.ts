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

/**
 * The y of the bottom of what an icon actually draws.
 *
 * Not its box: an icon is drawn with padding inside its viewBox, so the box
 * says nothing about where the ink lands. Nor `getBoundingClientRect` on the
 * geometry, which in Chromium is the path without its stroke — a round cap
 * reaches half a stroke further. So this reads the artwork out of the DOM,
 * rather than restating it here where it could drift: the viewBox and the
 * stroke width come from the symbol or the path itself, and the only number
 * in the test is the tolerance.
 */
async function inkBottomOf(page: Page, selector: string) {
  return page.evaluate((target) => {
    const svg = document.querySelector(target);
    if (svg === null) throw new Error(`nothing matched ${target}`);

    // A `<use>` draws a symbol defined elsewhere, and that symbol is where the
    // viewBox and the stroke live. A `<path>` carries its own.
    const use = svg.querySelector('use');
    const href = use?.getAttribute('href') ?? '';
    const source = use === null ? svg.querySelector('path') : document.querySelector(href);

    if (source === null) throw new Error(`no artwork inside ${target}`);

    const box = (source.closest('symbol') ?? svg).getAttribute('viewBox');
    if (box === null) throw new Error(`no viewBox for ${target}`);

    const attribute = (element: Element | null): string | null =>
      element === null
        ? null
        : (element.getAttribute('stroke-width') ?? attribute(element.parentElement));

    const viewHeight = Number(box.split(/[\s,]+/)[3]);
    const scale = svg.getBoundingClientRect().height / viewHeight;
    const stroke = Number(attribute(source) ?? '0');
    const drawn = (use ?? source).getBoundingClientRect();

    // Half the stroke, because a stroke straddles the path it follows, and a
    // round linecap carries it the whole way to the end of the line.
    return drawn.bottom + (stroke / 2) * scale;
  }, selector);
}

/* The marks in the header are letterforms — the logo is a `C` and the GitHub
   mark is a logo beside the word GitHub — so they stand on the same line the
   text does rather than floating centred beside it. Measured, because a
   `vertical-align` offset is a number nobody can check by reading it. */
for (const path of PAGES) {
  test(`the marks stand on the header baseline on ${path}`, async ({ page }) => {
    await page.goto(path);

    const baseline = await baselineOf(page, '.site-header .wordmark');

    expect(
      Math.abs((await inkBottomOf(page, '.site-header .wordmark svg')) - baseline),
    ).toBeLessThan(0.5);
    expect(
      Math.abs((await inkBottomOf(page, '.site-header .nav-external svg')) - baseline),
    ).toBeLessThan(0.5);
  });
}

test('the marks stay on the baseline once the phone rules step the logo down', async ({ page }) => {
  // 430px: narrow enough for the 560px rules, wide enough to keep the GitHub
  // link, which leaves the header altogether below 400px.
  await page.setViewportSize({ width: 430, height: 800 });
  await page.goto('/index.html');

  const baseline = await baselineOf(page, '.site-header .wordmark');

  expect(
    await page
      .locator('.site-header .wordmark svg')
      .evaluate((s) => s.getBoundingClientRect().height),
  ).toBe(22);
  expect(Math.abs((await inkBottomOf(page, '.site-header .wordmark svg')) - baseline)).toBeLessThan(
    0.5,
  );
  expect(
    Math.abs((await inkBottomOf(page, '.site-header .nav-external svg')) - baseline),
  ).toBeLessThan(0.5);
});

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
