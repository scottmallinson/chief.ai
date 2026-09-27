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

/* ---- What a browser can measure about accessibility ---- */

/**
 * The contrast of every rendered run of text against what is actually behind
 * it.
 *
 * Reading the tokens cannot do this: the failures on this site were a muted
 * grey that passes on one dark surface and fails on the raised one two levels
 * up, and a light-page grey used inside a graphite panel. Only the composed
 * page says which pairs exist.
 */
const CONTRAST = `(${(() => {
  const luminance = (color: string) => {
    const parts = color.match(/[\d.]+/g);
    if (parts === null) return null;
    const [r, g, b] = parts
      .slice(0, 3)
      .map(Number)
      .map((value) => {
        const channel = value / 255;
        return channel <= 0.03928 ? channel / 12.92 : ((channel + 0.055) / 1.055) ** 2.4;
      });
    return 0.2126 * r + 0.7152 * g + 0.0722 * b;
  };

  const behind = (element: Element) => {
    let node: Element | null = element;
    while (node !== null && node !== document.documentElement) {
      const color = getComputedStyle(node).backgroundColor;
      if (!/rgba\(0, 0, 0, 0\)|transparent/.test(color)) return color;
      node = node.parentElement;
    }
    return getComputedStyle(document.body).backgroundColor;
  };

  const failures: { text: string; size: number; ratio: number; need: number }[] = [];
  let measured = 0;

  const walker = document.createTreeWalker(document.body, NodeFilter.SHOW_TEXT);
  for (let node = walker.nextNode(); node !== null; node = walker.nextNode()) {
    const text = (node.nodeValue ?? '').trim();
    const element = node.parentElement;
    if (text === '' || element === null) continue;

    const style = getComputedStyle(element);
    if (style.display === 'none' || style.visibility === 'hidden') continue;

    const front = luminance(style.color);
    const back = luminance(behind(element));
    if (front === null || back === null) continue;

    const size = parseFloat(style.fontSize);
    const weight = parseInt(style.fontWeight, 10) || 400;
    const large = size >= 24 || (size >= 18.66 && weight >= 700);
    const need = large ? 3 : 4.5;
    const ratio = (Math.max(front, back) + 0.05) / (Math.min(front, back) + 0.05);

    measured += 1;
    if (ratio < need) {
      failures.push({ text: text.slice(0, 40), size, ratio: Math.round(ratio * 100) / 100, need });
    }
  }

  return { measured, failures };
}).toString()})()`;

for (const path of PAGES) {
  test(`every run of text on ${path} meets WCAG AA contrast`, async ({ page }) => {
    await page.goto(path);

    const { measured, failures } = await page.evaluate<{
      measured: number;
      failures: { text: string; size: number; ratio: number; need: number }[];
    }>(CONTRAST);

    // A sweep over nothing passes without checking anything.
    expect(measured).toBeGreaterThan(40);
    expect(
      failures,
      failures.map((f) => `${f.ratio}:1 at ${f.size}px — "${f.text}"`).join('\n'),
    ).toEqual([]);
  });

  /* Hover is a state with its own contrast, and it is the one nothing looks at.
     `a:hover` is a (0,1,1) selector with a colour in it, so it outranks every
     component that sets its own colour with a single class — which is how the
     skip link and the panel's light button both ended up painting themselves
     into their own background when pointed at. */
  test(`${path} keeps every link readable while it is hovered`, async ({ page }) => {
    await page.goto(path);

    const links = page.locator('a');
    const count = await links.count();
    expect(count).toBeGreaterThan(4);

    const failures: string[] = [];

    for (let i = 0; i < count; i += 1) {
      const link = links.nth(i);
      if (!(await link.isVisible())) continue;

      await link.hover({ force: true });

      const check = await link.evaluate((element) => {
        const luminance = (color: string) => {
          const parts = color.match(/[\d.]+/g);
          if (parts === null) return null;
          const [r, g, b] = parts
            .slice(0, 3)
            .map(Number)
            .map((value) => {
              const channel = value / 255;
              return channel <= 0.03928 ? channel / 12.92 : ((channel + 0.055) / 1.055) ** 2.4;
            });
          return 0.2126 * r + 0.7152 * g + 0.0722 * b;
        };

        let node: Element | null = element;
        let back = '';
        while (node !== null && node !== document.documentElement) {
          const color = getComputedStyle(node).backgroundColor;
          if (!/rgba\(0, 0, 0, 0\)|transparent/.test(color)) {
            back = color;
            break;
          }
          node = node.parentElement;
        }

        const style = getComputedStyle(element);
        const front = luminance(style.color);
        const behind = luminance(back || getComputedStyle(document.body).backgroundColor);
        if (front === null || behind === null) return null;

        const size = parseFloat(style.fontSize);
        const weight = parseInt(style.fontWeight, 10) || 400;
        return {
          text: (element.textContent ?? '').replace(/\s+/g, ' ').trim().slice(0, 30),
          ratio:
            Math.round(
              ((Math.max(front, behind) + 0.05) / (Math.min(front, behind) + 0.05)) * 100,
            ) / 100,
          need: size >= 24 || (size >= 18.66 && weight >= 700) ? 3 : 4.5,
        };
      });

      if (check !== null && check.ratio < check.need) {
        failures.push(`${check.ratio}:1 hovering "${check.text}"`);
      }
    }

    expect(failures, failures.join('\n')).toEqual([]);
  });

  test(`${path} has a heading outline a screen reader can follow`, async ({ page }) => {
    await page.goto(path);

    const levels = await page.evaluate(() =>
      [...document.querySelectorAll('h1,h2,h3,h4,h5,h6')].map((h) => ({
        level: Number(h.tagName[1]),
        text: (h.textContent ?? '').replace(/\s+/g, ' ').trim().slice(0, 40),
      })),
    );

    expect(levels.filter((h) => h.level === 1)).toHaveLength(1);

    const skips = levels
      .filter((h, i) => i > 0 && h.level > levels[i - 1].level + 1)
      .map((h) => `jumped to h${h.level} at "${h.text}"`);

    expect(skips, skips.join('\n')).toEqual([]);
  });

  test(`${path} opens with a skip link that reaches the content`, async ({ page }) => {
    await page.goto(path);
    await page.keyboard.press('Tab');

    const first = page.locator(':focus');
    await expect(first).toHaveClass(/skip-link/);
    expect(await first.getAttribute('href')).toBe('#main');

    // Hidden until it is the thing being used, then actually on screen.
    await expect(first).toBeInViewport();
    await expect(page.locator('#main')).toHaveCount(1);
  });

  test(`${path} does not scroll sideways at 320px`, async ({ page }) => {
    await page.setViewportSize({ width: 320, height: 812 });
    await page.goto(path);

    const over = await page.evaluate(() => {
      const root = document.documentElement;
      const widest = [...document.querySelectorAll('*')]
        .map((el) => el.getBoundingClientRect())
        .filter((box) => box.width > 0 && box.right > root.clientWidth + 0.5)
        .sort((a, b) => b.right - a.right)[0];

      return {
        overflow: root.scrollWidth - root.clientWidth,
        widest: widest === undefined ? null : Math.round(widest.right),
      };
    });

    expect(over.overflow, `something reaches ${over.widest}px`).toBeLessThanOrEqual(0);
  });
}

/* ---- The downloads ---- */

/**
 * Load the page as a visitor on `platform`.
 *
 * `support.js` reads `navigator.userAgentData` first and `navigator.userAgent`
 * second, and both of those describe the machine these tests run on — so the
 * Linux case would pass on CI for the wrong reason and fail on a maintainer's
 * Mac. Stubbing both before the page loads makes the answer the same
 * everywhere, which is the only way this measures the detection rather than
 * the host.
 */
type Visitor = { userAgent: string; uaPlatform: string; architecture?: string };

async function visitAs(page: Page, { userAgent, uaPlatform, architecture }: Visitor) {
  await page.addInitScript(
    ([ua, plat, arch]) => {
      Object.defineProperty(Navigator.prototype, 'userAgent', { get: () => ua });
      Object.defineProperty(Navigator.prototype, 'userAgentData', {
        // No `getHighEntropyValues` unless the visitor is given an
        // architecture: that absence is Safari, and it is the case the page
        // has to be honest about rather than the exception.
        get: () =>
          arch === undefined
            ? { platform: plat }
            : {
                platform: plat,
                getHighEntropyValues: () => Promise.resolve({ architecture: arch }),
              },
      });
    },
    [userAgent, uaPlatform, architecture],
  );

  await page.goto('/index.html');
}

/* Every build the page carries has to be one `support.js` knows how to name, or
   it keeps the releases-page link it ships with and the visitor has to find the
   file themselves. Markup added without its entry in `BUILDS` is exactly that
   mistake, and it looks completely fine on screen. */
test('every build links to a file the release publishes', async ({ page }) => {
  await page.goto('/index.html');

  const builds = page.locator('[data-build]');
  const count = await builds.count();

  // A loop over nothing passes without checking anything.
  expect(count).toBeGreaterThanOrEqual(6);

  const version = await page.locator('[data-version]').first().textContent();
  expect(version).toMatch(/^\d+\.\d+\.\d+$/);

  for (let i = 0; i < count; i += 1) {
    const build = builds.nth(i);
    const href = await build.getAttribute('href');

    expect(href, `${await build.getAttribute('data-build')} has no download`).toContain(
      `/releases/download/v${version}/`,
    );
  }
});

/* Six builds behind one list, and two of them are macOS and three are Linux. If
   any two rows read the same, the visitor is choosing at random — which is what
   this section used to ask of them, with "macOS" printed twice and "Linux"
   three times as the only thing set at full weight. */
test('no two builds in the list read the same', async ({ page }) => {
  await page.goto('/index.html');

  const rows = await page.locator('[data-build]').evaluateAll((builds) =>
    builds.map((build) => ({
      name: (build.textContent ?? '').replace(/\s+/g, ' ').trim(),
      label: build.getAttribute('aria-label') ?? '',
    })),
  );

  expect(rows).toHaveLength(6);
  expect(new Set(rows.map((row) => row.name)).size).toBe(6);

  // And each one names its platform out of context, because a screen reader
  // reading the list of links does not get the group heading with it.
  expect(new Set(rows.map((row) => row.label)).size).toBe(6);
  for (const row of rows) {
    expect(row.label, `${row.name} has no platform in its accessible name`).toMatch(
      /macOS|Windows|Linux/,
    );
  }
});

/* The section shipped with all six links as `href="#"`, rewritten by
   `support.js` on load. A visitor with JavaScript off — which is a thing this
   audience actually does — could not reach a single build from a site whose
   selling point is that it needs no server. Nothing here may be the only path
   to a file again. */
test.describe('with JavaScript off', () => {
  test.use({ javaScriptEnabled: false });

  test('every download still goes somewhere a build can be found', async ({ page }) => {
    await page.goto('/index.html');

    const targets = await page
      .locator('.get a[href], .hero-actions .btn-primary')
      .evaluateAll((links) => links.map((link) => link.getAttribute('href') ?? ''));

    expect(targets.length).toBeGreaterThanOrEqual(7);
    for (const href of targets) {
      expect(href, 'a download link that goes nowhere without JavaScript').not.toBe('#');
    }

    // The six in the list reach the releases page, where every asset is listed.
    const builds = await page
      .locator('[data-build]')
      .evaluateAll((links) => links.map((link) => link.getAttribute('href') ?? ''));

    expect(builds).toHaveLength(6);
    for (const href of builds) {
      expect(href).toMatch(/github\.com\/[^/]+\/[^/]+\/releases/);
    }
  });
});

/* The three Linux packagings are all on the page, because nothing in a browser
   says which package manager the visitor uses. */
test('Linux is offered as an AppImage, a .deb and an .rpm', async ({ page }) => {
  await page.goto('/index.html');

  const hrefs = await page
    .locator('[data-build^="linux"]')
    .evaluateAll((tiles) => tiles.map((tile) => tile.getAttribute('href') ?? ''));

  expect(hrefs).toHaveLength(3);
  expect(hrefs.some((href) => href.endsWith('.AppImage'))).toBe(true);
  expect(hrefs.some((href) => href.endsWith('.deb'))).toBe(true);
  expect(hrefs.some((href) => href.endsWith('.rpm'))).toBe(true);
});

/* The CTA is the only download most visitors will see, so it has to lead with
   their own platform. The AppImage is the Linux lead: it is the one of the
   three that asks nothing of the machine it lands on. */
const VISITORS: [string, Visitor, string][] = [
  ['Linux', { userAgent: 'Mozilla/5.0 (X11; Linux x86_64)', uaPlatform: 'Linux' }, '.AppImage'],
  [
    'Windows',
    { userAgent: 'Mozilla/5.0 (Windows NT 10.0; Win64; x64)', uaPlatform: 'Windows' },
    '.exe',
  ],
  [
    'macOS',
    { userAgent: 'Mozilla/5.0 (Macintosh; Intel Mac OS X 10_15_7)', uaPlatform: 'macOS' },
    '.dmg',
  ],
];

for (const [label, navigatorStub, extension] of VISITORS) {
  test(`the hero button leads a ${label} visitor to a ${label} build`, async ({ page }) => {
    await visitAs(page, navigatorStub);

    const hero = page.locator('.hero-actions .btn-primary');
    await expect(hero.locator('[data-download-label]')).toHaveText(`Download for ${label}`);
    expect(await hero.getAttribute('href')).toMatch(
      new RegExp(`/releases/download/v[\\d.]+/.*\\${extension}$`),
    );

    // And the download section leads with that same file, so the two halves of
    // the page agree.
    const lead = page.locator('[data-lead]');
    await expect(lead.locator('[data-lead-name]')).toHaveText(`Download for ${label}`);
    expect(await lead.getAttribute('href')).toBe(await hero.getAttribute('href'));
  });
}

/* Safari does not report the architecture, and Safari is most of the Mac
   traffic a page like this gets — so on a Mac the lead is a default rather than
   a detection, and it used to be presented with exactly the same confidence as
   one. A visitor who cannot answer "which chip is in your Mac?" was given no
   help at the one decision the page asks them to make. */
const MAC = { userAgent: 'Mozilla/5.0 (Macintosh; Intel Mac OS X 10_15_7)', uaPlatform: 'macOS' };

test('a Mac whose browser will not name the chip is told the lead is a guess', async ({ page }) => {
  await visitAs(page, MAC);

  await expect(page.locator('[data-lead-meta]')).toHaveText('Apple silicon · .dmg');
  await expect(page.locator('[data-lead-guess]')).toBeVisible();
  await expect(page.locator('[data-lead-guess]')).toContainText('Apple silicon assumed');
});

test('a Mac whose browser does name the chip is told nothing extra', async ({ page }) => {
  await visitAs(page, { ...MAC, architecture: 'x86' });

  await expect(page.locator('[data-lead-meta]')).toHaveText('Intel · .dmg');
  expect(await page.locator('[data-lead]').getAttribute('href')).toContain('_x64.dmg');
  await expect(page.locator('[data-lead-guess]')).toBeHidden();
});

/* The one predictable failure between pressing the button and running the app
   is the operating system refusing to open it. That was documented on
   /security and in /docs and nowhere near the download, so every visitor who
   took the shortest path met a Gatekeeper dialog with no warning from the
   site — and concluded the site had hidden it. */
test('the download section warns that the builds are not signed', async ({ page }) => {
  await page.goto('/index.html');

  const warning = page.locator('.get .get-warn');
  await expect(warning).toBeVisible();
  await expect(warning).toContainText(/not signed/i);
  await expect(warning).toContainText(/macOS/);
  await expect(warning).toContainText(/SmartScreen/);

  // And it says where to go, rather than leaving the reader to find out.
  expect(await warning.locator('a').getAttribute('href')).toBe('docs.html#install');
});

/* The hero mock is the thing that makes the claim believable, and `.frame` hides
   its overflow — so anything taller than the body is silently cut off with no
   scroll affordance to say so. The mock's height is content-driven on a phone
   for that reason, and this is what would catch a fixed one being put back.

   Note the measurement: `scrollHeight` against `clientHeight` reports zero
   whatever the height is, because a stretched flex child is always exactly as
   tall as its parent. Only the rendered position of the ink says what is lost. */
for (const width of [375, 320]) {
  test(`the hero mock is not cut off at ${width}px`, async ({ page }) => {
    await page.setViewportSize({ width, height: 812 });
    await page.goto('/index.html');

    const { overflow, culprit } = await page.locator('.frame').evaluate((frame) => {
      const edge = frame.getBoundingClientRect().bottom;
      let overflow = 0;
      let culprit: string | null = null;

      for (const element of frame.querySelectorAll('*')) {
        const style = getComputedStyle(element);
        if (style.display === 'none' || style.visibility === 'hidden') continue;

        const box = element.getBoundingClientRect();
        if (box.height === 0) continue;
        if (box.bottom - edge > overflow) {
          overflow = box.bottom - edge;
          culprit = (element.textContent ?? '').replace(/\s+/g, ' ').trim().slice(0, 50);
        }
      }

      return { overflow: Math.round(overflow), culprit };
    });

    expect(overflow, `cut off past the frame: "${culprit}"`).toBeLessThanOrEqual(0);
  });
}

/* A phone gets no build at all, and the button says so rather than handing
   over a desktop installer. */
test('a phone is told there is nothing to install, not given a file', async ({ page }) => {
  await visitAs(page, {
    userAgent: 'Mozilla/5.0 (iPhone; CPU iPhone OS 17_0 like Mac OS X)',
    uaPlatform: 'iOS',
  });

  const hero = page.locator('.hero-actions .btn-primary');
  await expect(hero.locator('[data-download-label]')).toHaveText('See the builds');
  expect(await hero.getAttribute('href')).toBe('#get');

  // The mark has to agree with the label. A download arrow over a button that
  // scrolls the page is the affordance contradicting the action.
  expect(await hero.locator('use').getAttribute('href')).toBe('#i-arrow-down');

  // And the section leads with nothing rather than promoting a build that will
  // not run, so the list of every build is open instead.
  const lead = page.locator('[data-lead]');
  expect(await lead.getAttribute('href')).not.toContain('/releases/download/');
  await expect(page.locator('[data-more]')).toHaveAttribute('open', '');

  // The one thing a phone can actually use: this page's address, to open on a
  // machine that can run Chief.
  await expect(page.locator('[data-copy]')).toBeVisible();
});

/* And that control never appears where it would do nothing. */
test('a machine with a build of its own is not offered a link to copy', async ({ page }) => {
  await visitAs(page, {
    userAgent: 'Mozilla/5.0 (Windows NT 10.0; Win64; x64)',
    uaPlatform: 'Windows',
  });

  await expect(page.locator('[data-lead-meta]')).toHaveText('x64 · .exe');
  await expect(page.locator('[data-copy]')).toBeHidden();
});

/* Every interactive element shipped with the browser's own focus ring, which
   belongs to no design system — orange in Chromium, and never designed against
   the graphite controls or the dark panel. */
test('keyboard focus is drawn by the design system, not the browser', async ({ page }) => {
  await page.goto('/index.html');
  await page.keyboard.press('Tab');

  const ring = await page.evaluate(() => {
    const style = getComputedStyle(document.activeElement as Element);
    return { color: style.outlineColor, width: style.outlineWidth, offset: style.outlineOffset };
  });

  // --slate, the one colour the system uses for a live control.
  expect(ring.color).toBe('rgb(44, 92, 122)');
  expect(ring.width).toBe('2px');
  expect(ring.offset).toBe('2px');
});

/* The docs sidebar and the breadcrumb both shipped hard-coded to Install and
   stayed there through seven sections. Install is right at the top of the page,
   so the failure was invisible until somebody scrolled. */
test('the docs sidebar follows the section being read', async ({ page }) => {
  await page.goto('/docs.html');

  await expect(page.locator('.docs-nav-links a.is-current')).toHaveText('Install');
  await expect(page.locator('[data-crumb]')).toHaveText('docs / start / install');

  await page.locator('#trouble').scrollIntoViewIfNeeded();
  await expect(page.locator('.docs-nav-links a.is-current')).toHaveText('Troubleshooting');
  await expect(page.locator('[data-crumb]')).toHaveText(
    'docs / if something is wrong / troubleshooting',
  );

  // One at a time, and the current one is announced as the current one.
  await expect(page.locator('.docs-nav-links a[aria-current="location"]')).toHaveCount(1);
});

/* This site counts its own page views, and it used to tell every visitor on
   every page that it collected nothing. The claim is the app's; the site's own
   behaviour is disclosed rather than folded into it. */
for (const path of PAGES) {
  test(`${path} does not claim the website collects nothing`, async ({ page }) => {
    await page.goto(path);

    const footer = page.locator('.footer-base .mono');
    await expect(footer).toHaveText(/no telemetry from the app$/);
    await expect(page.locator('.footer-col a[href="security.html#site"]')).toHaveCount(1);
  });
}

test('the security page accounts for what the website itself counts', async ({ page }) => {
  await page.goto('/security.html');

  const disclosure = page.locator('#site');
  await expect(disclosure).toBeVisible();
  await expect(disclosure).toContainText('_vercel/insights');
  await expect(disclosure).toContainText('This website is not the app');
});
