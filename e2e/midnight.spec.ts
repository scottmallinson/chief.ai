/**
 * A window left open overnight.
 *
 * Chief lives in the tray, so the window is mounted when the date changes. These
 * run the built app in a real browser with the clock under the test's control,
 * which is what the unit tests for the hook cannot show: that the heading and
 * the brief on screen are the new day's.
 */

import { expect, test, type Brief } from './fixtures';

const evening: Brief = {
  date: '2026-08-29',
  path: 'briefs/2026-08-29.md',
  markdown: '- Parents evening at 17:30',
  sources: ['calendar'],
};

const morning: Brief = {
  date: '2026-08-30',
  path: 'briefs/2026-08-30.md',
  markdown: '- Dentist for Leo at 08:15',
  sources: ['calendar'],
};

test.describe('a window left open across midnight', () => {
  test('shows the new day, and the brief written overnight, when it is shown again', async ({
    chief,
    page,
  }) => {
    await page.clock.install({ time: new Date(2026, 7, 29, 23, 30) });
    await chief.open({
      briefByDate: { '2026-08-29': evening, '2026-08-30': morning },
      corpus: ['briefs/2026-08-29.md'],
    });

    await expect(page.getByText('Parents evening at 17:30')).toBeVisible();

    // The laptop sleeps through midnight and the lid opens at 07:45.
    await page.clock.setSystemTime(new Date(2026, 7, 30, 7, 45));
    await page.evaluate(() => window.dispatchEvent(new Event('focus')));

    await expect(page.getByText('Dentist for Leo at 08:15')).toBeVisible();
    await expect(page.getByText('Parents evening at 17:30')).toBeHidden();
    await expect(page.getByText(/August 30, 2026|30 August 2026/).first()).toBeVisible();
    await page.screenshot({ path: 'test-results/midnight-after.png' });
  });

  test('moves on by itself when the window is left open and visible', async ({ chief, page }) => {
    await page.clock.install({ time: new Date(2026, 7, 29, 23, 59, 30) });
    await chief.open({ briefByDate: { '2026-08-29': evening, '2026-08-30': morning } });

    await expect(page.getByText('Parents evening at 17:30')).toBeVisible();

    await page.clock.fastForward(2 * 60 * 1000);

    await expect(page.getByText('Dentist for Leo at 08:15')).toBeVisible();
  });
});
