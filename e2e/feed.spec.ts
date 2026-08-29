/**
 * The Executive Feed, and the drawer chat moved into.
 *
 * These are the numbers jsdom cannot see. The one this file exists for is the
 * width of the detail column with the drawer open and closed: decision D6 put
 * chat in an overlay rather than a third column so that asking a question does
 * not reflow the thing you asked it about, and "does not reflow" is a measured
 * pixel, not a claim. jsdom reports every width as zero and would agree with
 * either answer.
 */

import { expect, test, type Brief, type Proposal } from './fixtures';

const today: Brief = {
  date: '2026-08-29',
  path: 'briefs/2026-08-29.md',
  markdown: '- Standup at 09:30 with Ana\n- Review chief.ai #44 before the retro',
  sources: ['calendar', 'github'],
};

const week = ['briefs/2026-08-29.md', 'briefs/2026-08-28.md', 'briefs/2026-08-27.md'];

test.describe('the list pane', () => {
  test('is the 240px column the design canvas specifies', async ({ chief, page }) => {
    await chief.open({ brief: today, corpus: week });

    const pane = await page.getByTestId('list-pane').boundingBox();

    expect(Math.round(pane?.width ?? 0)).toBe(240);
  });

  test('sits between the rail and the detail, at full height', async ({ chief, page }) => {
    await chief.open({ brief: today, corpus: week });

    const pane = await page.getByTestId('list-pane').boundingBox();
    const header = await page.locator('header').boundingBox();

    expect(Math.round(pane?.x ?? 0), 'the pane should start where the rail ends').toBe(56);
    expect(Math.round(pane?.height ?? 0)).toBe(page.viewportSize()?.height);
    expect(Math.round(header?.x ?? 0), 'the detail should start where the pane ends').toBe(296);
  });

  test('is not there on a destination with no list to show', async ({ chief, page }) => {
    await chief.open({ brief: today, corpus: week });
    await chief.goTo('Settings');

    await expect(page.getByTestId('list-pane')).toBeHidden();
    expect(Math.round((await page.locator('header').boundingBox())?.x ?? 0)).toBe(56);
  });
});

test.describe('the brief', () => {
  test('is readable in the app, without opening a file manager', async ({ chief, page }) => {
    await chief.open({ brief: today, corpus: week });

    await expect(page.getByText('Standup at 09:30 with Ana')).toBeVisible();
    await expect(page.getByText('briefs/2026-08-29.md')).toBeVisible();
  });

  test('scrolls in its own region, never the window', async ({ chief }) => {
    await chief.open({
      brief: {
        ...today,
        markdown: Array.from({ length: 60 }, (_, at) => `- item ${at}`).join('\n'),
      },
      corpus: week,
    });

    const shell = await chief.measure();

    expect(shell.windowScrolls, 'the document should have nowhere to scroll').toBe(false);
    expect(shell.nestedScrollRegions, 'one region should own the wheel').toBe(1);
  });
});

test.describe('the chat drawer', () => {
  test('leaves the detail column exactly as wide as it found it', async ({ chief }) => {
    await chief.open({ brief: today, corpus: week });

    const closed = await chief.detailWidth();
    await chief.openChat();
    const open = await chief.detailWidth();
    await chief.closeChat();

    expect(open, 'opening the drawer must not reflow the view behind it').toBe(closed);
    expect(await chief.detailWidth(), 'and closing it must put nothing back').toBe(closed);
  });

  test('overlays the view rather than sitting beside it', async ({ chief, page }) => {
    await chief.open({ brief: today, corpus: week });
    await chief.openChat();

    const dialog = await page.getByRole('dialog').boundingBox();
    const main = await page.locator('main').boundingBox();

    // The two occupy the same pixels; a third column would not.
    expect(dialog?.x ?? 0).toBeLessThan((main?.x ?? 0) + (main?.width ?? 0));
    expect(Math.round((dialog?.x ?? 0) + (dialog?.width ?? 0))).toBe(page.viewportSize()?.width);
  });

  test('closes on Escape and gives focus back to the rail', async ({ chief, page }) => {
    await chief.open({ brief: today, corpus: week });
    await chief.openChat();
    await chief.closeChat();

    await expect(page.getByRole('dialog')).toBeHidden();
    await expect(page.getByRole('button', { name: 'Ask Chief' })).toBeFocused();
  });

  test('keeps the question and its answer when it is closed and opened again', async ({
    chief,
    page,
  }) => {
    await chief.open({ brief: today, corpus: week, answer: 'Two are waiting on review.' });
    await chief.ask('What is waiting on me?');
    await chief.closeChat();
    await chief.openChat();

    await expect(page.getByText('Two are waiting on review.')).toBeVisible();
  });
});

test.describe('reduced motion', () => {
  test('opens the drawer without animating it', async ({ chief, page }) => {
    await page.emulateMedia({ reducedMotion: 'reduce' });
    await chief.open({ brief: today, corpus: week });
    await chief.openChat();

    const name = await page
      .getByRole('dialog')
      .evaluate((node) => getComputedStyle(node).animationName);

    expect(name, 'motion-safe should have withheld the animation entirely').toBe('none');
  });

  test('animates it once when motion is allowed', async ({ chief, page }) => {
    await page.emulateMedia({ reducedMotion: 'no-preference' });
    await chief.open({ brief: today, corpus: week });
    await chief.openChat();

    const name = await page
      .getByRole('dialog')
      .evaluate((node) => getComputedStyle(node).animationName);

    expect(name).toBe('chief-drawer');
  });
});

const drafted: Proposal = {
  id: 1,
  source: 'github',
  title: 'Ask for a review on scottmallinson/chief.ai #44',
  context: 'scottmallinson/chief.ai #44',
  path: 'proposed/2026-08-29-scottmallinson-chief-ai-44.md',
  status: 'drafted',
  createdAt: '2026-08-29T09:00:00.000Z',
  body: '# Ask for a review\n\nCould you take a look at #44 when you get a moment?',
};

test.describe('a drafted action', () => {
  test('appears in the feed saying nothing was sent', async ({ chief, page }) => {
    await chief.open({ brief: today, corpus: week, proposals: [drafted] });

    await expect(page.getByText('Drafted for you · nothing sent')).toBeVisible();
    await expect(
      page.getByText('Could you take a look at #44 when you get a moment?'),
    ).toBeVisible();
    await expect(page.getByText('Not sent')).toBeVisible();
  });

  test('opens in the drawer without reflowing the feed behind it', async ({ chief, page }) => {
    await chief.open({ brief: today, corpus: week, proposals: [drafted] });

    const closed = await chief.detailWidth();
    await page.getByRole('button', { name: 'Edit' }).click();
    await page.getByRole('dialog', { name: 'Edit draft' }).waitFor();

    // The same guarantee the chat drawer gives, on the path that actually
    // replaces the drawer's contents rather than just opening it.
    expect(await chief.detailWidth()).toBe(closed);
    await expect(page.getByRole('textbox', { name: 'Draft' })).toHaveValue(drafted.body);
  });

  test('goes back to chat once the draft is closed', async ({ chief, page }) => {
    await chief.open({ brief: today, corpus: week, proposals: [drafted] });

    await page.getByRole('button', { name: 'Edit' }).click();
    await page.getByRole('dialog', { name: 'Edit draft' }).waitFor();
    await page.keyboard.press('Escape');
    await page.getByRole('dialog').waitFor({ state: 'detached' });

    await chief.openChat();
    await expect(page.getByRole('dialog', { name: 'Ask Chief' })).toBeVisible();
  });
});
