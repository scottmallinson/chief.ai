import { defineConfig, devices } from '@playwright/test';

import type { ShellOptions } from './e2e/fixtures';

/** Where the built app is served for these tests. */
const PORT = 4173;
const BASE_URL = `http://localhost:${PORT}`;

/** And where the marketing site is, which is static and needs no build. */
const SITE_PORT = 4174;
const SITE_URL = `http://localhost:${SITE_PORT}`;

/** The site's tests, which run against a different server from the app's. */
const SITE_TESTS = /website\.spec\.ts/;

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
      testIgnore: SITE_TESTS,
      use: { ...devices['Desktop Chrome'], viewport: { width: 1280, height: 800 } },
    },
    {
      // Small enough that everything competes for height at once.
      name: 'small window',
      testIgnore: SITE_TESTS,
      use: { ...devices['Desktop Chrome'], viewport: { width: 720, height: 480 } },
    },
    {
      // Chief draws its own 10px scrollbar, which takes space out of the layout
      // on every platform. Playwright hides scrollbars in headless Chromium by
      // default, which no real user ever sees, so this project puts them back
      // and measures the one that ships. Chromium is the engine of WebView2, so
      // this is the Windows build's scrollbar.
      name: 'classic scrollbars',
      testIgnore: SITE_TESTS,
      use: {
        ...devices['Desktop Chrome'],
        viewport: { width: 1085, height: 660 },
        classicScrollbars: true,
        launchOptions: { ignoreDefaultArgs: ['--hide-scrollbars'] },
      },
    },
    {
      // WebKit is the engine of both other webviews Chief ships in — WKWebView
      // on macOS and WebKitGTK on Linux — and it styles scrollbars through the
      // same pseudo-elements by a separate implementation. Every app test runs
      // here too, so a rule one engine honours and the other ignores fails.
      name: 'webkit',
      testIgnore: SITE_TESTS,
      use: {
        ...devices['Desktop Safari'],
        viewport: { width: 1085, height: 660 },
        classicScrollbars: true,
      },
    },
    {
      // The website, which is hand-written HTML with no build step. Its header
      // is one row of three different type sizes on a shared baseline, and at
      // phone widths it has to stay one row without pushing the download
      // button off the edge — neither of which jsdom can see.
      name: 'website',
      testMatch: SITE_TESTS,
      use: {
        ...devices['Desktop Chrome'],
        baseURL: SITE_URL,
        viewport: { width: 1280, height: 900 },
      },
    },
  ],

  webServer: [
    {
      command: `pnpm build && pnpm exec vite preview --port ${PORT} --strictPort`,
      url: BASE_URL,
      reuseExistingServer: !isCi,
      timeout: 120_000,
    },
    {
      // Served as it ships: static files, no bundler in front of them.
      command: `pnpm exec vite preview --outDir website --port ${SITE_PORT} --strictPort`,
      url: `${SITE_URL}/index.html`,
      reuseExistingServer: !isCi,
      timeout: 60_000,
    },
  ],
});
