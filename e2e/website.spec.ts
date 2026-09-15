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
 * The top and bottom of what a mark actually draws, in page coordinates.
 *
 * Not its box: an icon is drawn with padding inside its viewBox, so the box
 * says nothing about where the ink lands. Nor `getBoundingClientRect` on the
 * geometry, which in Chromium is the path without its stroke — a round cap
 * reaches half a stroke further. So this reads the artwork out of the DOM,
 * rather than restating it here where it could drift: the viewBox and the
 * stroke width come from the symbol or the path itself.
 */
async function inkOf(page: Page, selector: string) {
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

    // Half the stroke at each end, because a stroke straddles the path it
    // follows and a round linecap carries it the whole way to the line's end.
    const reach = (stroke / 2) * scale;

    return { top: drawn.top - reach, bottom: drawn.bottom + reach };
  }, selector);
}

/**
 * The cap height of an element's own text, in px.
 *
 * Measured from the font the element actually computes to, at ten times the
 * size so the answer is not rounded to a whole pixel, rather than assumed from
 * a ratio. It is what the marks are aligned against, so it cannot be a
 * constant in the test.
 */
async function capHeightOf(page: Page, selector: string) {
  return page.evaluate(async (target) => {
    await document.fonts.ready;

    const element = document.querySelector(target);
    if (element === null) throw new Error(`nothing matched ${target}`);

    const style = getComputedStyle(element);
    const context = document.createElement('canvas').getContext('2d');
    if (context === null) throw new Error('no 2d context');

    context.font = `${style.fontWeight} ${parseFloat(style.fontSize) * 10}px ${style.fontFamily}`;

    return context.measureText('H').actualBoundingBoxAscent / 10;
  }, selector);
}

/* The logo is a letter: its arc is drawn to the cap height of the type beside
   it, so it sits on the baseline exactly as the `C` of "Chief" does. Measured,
   because a `vertical-align` offset is a number nobody can check by reading
   it. */
for (const path of PAGES) {
  test(`the logo stands on the header baseline on ${path}`, async ({ page }) => {
    await page.goto(path);

    const baseline = await baselineOf(page, '.site-header .wordmark');
    const { top, bottom } = await inkOf(page, '.site-header .wordmark svg');
    const cap = await capHeightOf(page, '.site-header .wordmark p');

    expect(Math.abs(bottom - baseline)).toBeLessThan(0.5);
    // And it is a letter's height, not an icon's, which is what makes sitting
    // on the baseline the right answer for this one and not for the others.
    expect(Math.abs(bottom - top - cap)).toBeLessThan(0.5);
  });
}

/* The two icons are not letters — both are drawn taller than the cap height of
   the word they label — so they are centred on that cap height instead, and
   the overshoot splits evenly above the cap and below the baseline. Standing
   one on the baseline throws all of it upward, which is what this guards
   against. */
const ICONS = [
  ['the GitHub mark', '.site-header .nav-external svg', '.site-header .nav-external'],
  ['the download arrow', '.site-header .btn svg', '.site-header .btn'],
] as const;

async function offCentre(page: Page, mark: string, text: string) {
  const baseline = await baselineOf(page, text);
  const cap = await capHeightOf(page, text);
  const { top, bottom } = await inkOf(page, mark);

  // Where the ink's middle sits, against the middle of the cap height.
  return (top + bottom) / 2 - (baseline - cap / 2);
}

for (const path of PAGES) {
  for (const [what, mark, text] of ICONS) {
    test(`${what} is centred on the text beside it on ${path}`, async ({ page }) => {
      await page.goto(path);

      expect(Math.abs(await offCentre(page, mark, text))).toBeLessThan(0.5);
    });
  }
}

test('the marks hold their alignment once the phone rules step the type down', async ({ page }) => {
  // 430px: narrow enough for the 560px rules, wide enough to keep the GitHub
  // link, which leaves the header altogether below 400px.
  await page.setViewportSize({ width: 430, height: 800 });
  await page.goto('/index.html');

  const logo = page.locator('.site-header .wordmark svg');
  expect(await logo.evaluate((svg) => svg.getBoundingClientRect().height)).toBe(22);

  const baseline = await baselineOf(page, '.site-header .wordmark');
  expect(
    Math.abs((await inkOf(page, '.site-header .wordmark svg')).bottom - baseline),
  ).toBeLessThan(0.5);

  for (const [, mark, text] of ICONS) {
    expect(Math.abs(await offCentre(page, mark, text))).toBeLessThan(0.5);
  }
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
