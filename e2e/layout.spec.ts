/**
 * The window holds still.
 *
 * Chief is a desktop application, so the frame is fixed and the content moves
 * inside it. These are the guarantees that were broken once already: a long
 * answer scrolled the whole application, taking the header and the navigation
 * rail out of the window, and put a second scrollbar beside the first.
 */

import { expect, longAnswer, longWorkLog, sentenceSummary, test, type Account } from './fixtures';

/** An account named right up to the limit the field allows. */
const longNamed: Account = {
  id: 1,
  service: 'github',
  accountKey: 'octocat',
  label: 'a'.repeat(40),
  identity: 'octocat',
  connectedAt: '2026-08-19T14:00:00.000Z',
};

test.describe('a conversation taller than the window', () => {
  test('moves inside the transcript, never the window', async ({ chief }) => {
    await chief.open({ answer: longAnswer() });
    await chief.ask('What did I ship?');

    const shell = await chief.measure();

    expect(shell.windowScrolls, 'the document should have nowhere to scroll').toBe(false);
    expect(shell.windowScrollTop).toBe(0);
  });

  test('puts one scrollbar on the transcript, not two', async ({ chief }) => {
    await chief.open({ answer: longAnswer() });
    await chief.ask('What did I ship?');

    const shell = await chief.measure();

    expect(shell.scrollingRegions, 'exactly one region should be scrolling').toBe(1);
    expect(
      shell.nestedScrollRegions,
      'the transcript should be the only thing the wheel can move',
    ).toBe(1);
  });

  test('leaves the header and the navigation where they are', async ({ chief, page }) => {
    await chief.open({ answer: longAnswer() });
    await chief.ask('What did I ship?');

    const shell = await chief.measure();

    expect(shell.headerTop, 'the header should stay at the top of the window').toBe(0);
    await expect(page.getByRole('button', { name: 'Ask Chief' })).toBeInViewport();
    await expect(page.getByText(/^on-device/)).toBeInViewport();
  });

  test('keeps the composer inside the window', async ({ chief }) => {
    await chief.open({ answer: longAnswer() });
    await chief.ask('What did I ship?');

    const shell = await chief.measure();

    expect(shell.composerWithinWindow, 'the composer should not be pushed out of frame').toBe(true);
    await expect(chief.composer).toBeInViewport();
  });
});

test.describe('a work log taller than the window', () => {
  test('scrolls within its own region', async ({ chief }) => {
    await chief.open({ workLog: longWorkLog() });
    await chief.goTo('Work Log');

    const shell = await chief.measure();

    expect(shell.windowScrolls).toBe(false);
    expect(shell.scrollingRegions).toBe(1);
    expect(shell.nestedScrollRegions).toBe(1);
    expect(shell.headerTop).toBe(0);
  });

  /**
   * The defect REC-63 is about.
   *
   * Every row deterministic ingestion writes has a short summary — "merged",
   * "open", a meeting's time — and the row rendered it as a chip in a flex
   * line that could not wrap. The daemon that came before it wrote sentences,
   * migration 8 gave those rows a title as well, and one of them was enough:
   * the chip refused to shrink below its own text, pushed the timestamp out
   * through the side of the card, and put a horizontal scrollbar on the whole
   * window.
   *
   * jsdom reports every width as zero, so this can only be measured here.
   *
   * Proved by putting the summary back in the chip:
   *
   * ```text
   * the row must stay inside the card it is in
   * Expected: <= 1
   * Received: 476.5
   * ```
   */
  test('keeps a row from before deterministic ingestion inside its card', async ({
    chief,
    page,
  }) => {
    await chief.open({ workLog: [sentenceSummary] });
    await chief.goTo('Work Log');

    const card = await page.getByRole('listitem').first().boundingBox();
    const time = await page.locator('time').first().boundingBox();

    expect(card, 'the row should render').not.toBeNull();
    expect(time, 'the timestamp should render').not.toBeNull();

    const cardRight = (card?.x ?? 0) + (card?.width ?? 0);
    const timeRight = (time?.x ?? 0) + (time?.width ?? 0);

    // A pixel of slack for sub-pixel layout, and no more.
    expect(timeRight - cardRight, 'the row must stay inside the card it is in').toBeLessThanOrEqual(
      1,
    );

    // `measure()` reports vertical scrolling only, and this defect was
    // horizontal, so the document is asked directly.
    const scrollsSideways = await page.evaluate(
      () => document.documentElement.scrollWidth > document.documentElement.clientWidth + 1,
    );

    expect(scrollsSideways, 'and must not scroll the window sideways').toBe(false);
  });
});

test.describe('following an answer as it is written', () => {
  test('stays with the newest words', async ({ chief }) => {
    await chief.open({ holdAnswers: true });
    await chief.ask('What did I ship?');

    for (const line of longAnswer().split('\n')) {
      await chief.stream({ kind: 'delta', text: `${line}\n` });
    }

    const position = await chief.position();

    expect(position.furthest, 'the answer should have outgrown the window').toBeGreaterThan(0);
    expect(position.atBottom, 'the transcript should be showing the newest words').toBe(true);

    // ...and still without moving the window.
    const shell = await chief.measure();
    expect(shell.windowScrolls).toBe(false);
    expect(shell.scrollingRegions).toBe(1);
  });

  test('leaves a reader who has scrolled up where they are', async ({ chief }) => {
    await chief.open({ holdAnswers: true });
    await chief.ask('What did I ship?');
    await chief.stream({ kind: 'delta', text: longAnswer() });

    await chief.scrollTo('top');
    expect((await chief.position()).scrollTop).toBe(0);

    await chief.stream({ kind: 'delta', text: `\n${longAnswer()}` });

    expect(
      (await chief.position()).scrollTop,
      'a reader re-reading something should not be dragged to the bottom',
    ).toBe(0);
  });

  test('picks the answer back up when the reader returns to the bottom', async ({ chief }) => {
    await chief.open({ holdAnswers: true });
    await chief.ask('What did I ship?');
    await chief.stream({ kind: 'delta', text: longAnswer() });

    await chief.scrollTo('top');
    await chief.scrollTo('bottom');
    await chief.stream({ kind: 'delta', text: `\n${longAnswer()}` });

    expect((await chief.position()).atBottom).toBe(true);
  });
});

test.describe('an account with a long name', () => {
  test('does not squeeze the field that names it', async ({ chief, page }) => {
    await chief.open({ accounts: [longNamed] });
    await chief.goTo('Settings');

    const field = await page.getByLabel('Name for octocat').boundingBox();
    const button = await page.getByRole('button', { name: /^Disconnect/ }).boundingBox();

    // The button echoes the whole name and never wraps, so without a bound on
    // it the field it names is what gives way.
    expect(button?.width ?? 0, 'the button should stop growing').toBeLessThanOrEqual(220);
    expect(field?.width ?? 0, 'the field should keep most of the row').toBeGreaterThan(300);
  });
});

test.describe('the browser Chief is standing in for', () => {
  test('renders the scrollbars this project asked for', async ({ chief, classicScrollbars }) => {
    await chief.open({ answer: longAnswer() });
    await chief.ask('What did I ship?');

    const { scrollbarWidth } = await chief.measure();

    // Without this the "classic scrollbars" project would quietly be a second
    // copy of the default one, and the layout it is meant to cover untested.
    if (classicScrollbars) {
      expect(scrollbarWidth, 'a classic scrollbar should take layout width').toBeGreaterThan(0);
    } else {
      expect(scrollbarWidth, 'an overlay scrollbar should take no layout width').toBe(0);
    }
  });
});
