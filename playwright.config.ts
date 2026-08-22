import { defineConfig, devices } from '@playwright/test';

import type { ShellOptions } from './e2e/fixtures';

/** Where the built app is served for these tests. */
const PORT = 4173;
const BASE_URL = `http://localhost:${PORT}`;

const isCi = process.env.CI !== undefined;

/**
 * Layout tests, run against the built app in a real browser.
 *
 * These cover what Vitest structurally cannot: jsdom has no layout engine, so a
 * scrollbar, a clipped composer or a window that scrolls when it should not are
 * all invisible to it. They are kept out of `pnpm check` because they need a
 * build and a browser — run them with `pnpm test:e2e`.
 *
 * The app is served from `dist` rather than the dev server so that the CSS under
 * test is the CSS that ships.
 */
export default defineConfig<ShellOptions>({
  testDir: './e2e',
  fullyParallel: true,
  forbidOnly: isCi,
  retries: isCi ? 1 : 0,
  reporter: isCi ? [['github'], ['html', { open: 'never' }]] : [['list']],

  use: {
    baseURL: BASE_URL,
    trace: 'on-first-retry',
  },

  projects: [
    {
      // A comfortable window.
      name: 'desktop',
      use: { ...devices['Desktop Chrome'], viewport: { width: 1280, height: 800 } },
    },
    {
      // Small enough that everything competes for height at once.
      name: 'small window',
      use: { ...devices['Desktop Chrome'], viewport: { width: 720, height: 480 } },
    },
    {
      // Windows draws scrollbars that take space out of the layout rather than
      // floating over it, which is where the original break was reported.
      // Playwright hides scrollbars in headless Chromium by default, which no
      // real user ever sees, so this project puts them back. That is enough on
      // Linux, where CI runs; on macOS Chromium follows the OS and overlays them
      // whatever the flags say, so `classicScrollbars` also has the fixture
      // style the scrollbar, which forces a non-overlay one on any host.
      name: 'classic scrollbars',
      use: {
        ...devices['Desktop Chrome'],
        viewport: { width: 1085, height: 660 },
        classicScrollbars: true,
        launchOptions: { ignoreDefaultArgs: ['--hide-scrollbars'] },
      },
    },
  ],

  webServer: {
    command: `pnpm build && pnpm exec vite preview --port ${PORT} --strictPort`,
    url: BASE_URL,
    reuseExistingServer: !isCi,
    timeout: 120_000,
  },
});
