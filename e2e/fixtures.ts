/**
 * Driving the real frontend in a real browser.
 *
 * jsdom has no layout engine: it reports every height as zero, so it cannot see
 * a scrollbar, a clipped composer or a window that scrolls when it should not.
 * These tests run the built app in Chromium instead and measure the result.
 *
 * The Rust side is replaced rather than the components: `@tauri-apps/api` talks
 * to the shell through `window.__TAURI_INTERNALS__`, so standing in for that one
 * object is enough to exercise the real views, the real CSS and the real event
 * plumbing against answers a test chooses.
 *
 * What this cannot do is test the webview Chief actually ships in. Chromium is
 * close to WebView2 and WKWebView but is not either of them, so a rendering
 * difference peculiar to one of those will still get through. Catching those
 * would mean driving the packaged binary with `tauri-driver`.
 */

import { test as base, type Locator, type Page } from '@playwright/test';

/** An entry as `list_work_logs` returns it. */
export interface WorkLogEntry {
  id: number;
  timestamp: string;
  source: string;
  content: string;
  summary: string | null;
  /** Where the thing is, or null when there is nowhere to go. */
  url: string | null;
  externalId: string | null;
  accountId: number;
}

/** A brief, as `todays_brief` and `generate_brief` return it. */
export interface Brief {
  date: string;
  path: string;
  markdown: string;
  sources: string[];
}

/** A draft Chief prepared, as `list_proposals` returns it. */
export interface Proposal {
  id: number;
  source: string;
  title: string;
  context: string;
  path: string;
  status: string;
  createdAt: string;
  body: string;
}

/** One account's freshness, as `sync_status` returns it. */
export interface SyncState {
  accountId: number;
  source: string;
  status: 'ok' | 'syncing' | 'authRequired' | 'error';
  lastSyncedAt: string | null;
  errorMessage: string | null;
}

/** One connected account, as `connections` returns it. */
export interface Account {
  id: number;
  service: string;
  accountKey: string;
  label: string | null;
  identity: string | null;
  connectedAt: string;
}

/** What `ask_agent` resolves with: the answer, and where it came from. */
export interface Answer {
  content: string;
  /** Null when nothing local was read — a tool answer, or a brief. */
  provenance: string | null;
}

/** A step in an answer, as `agent::Update` serialises it. */
export type AgentUpdate =
  { kind: 'delta'; text: string } | { kind: 'restart' } | { kind: 'tool'; name: string };

/** What the Rust side should answer for one test. */
export interface Backend {
  /** What `ask_agent` answers with. A string is the content and no footer. */
  answer?: string;
  /** The provenance line under the answer, when the read path found rows. */
  provenance?: string;
  /** What the work log is filled with. */
  workLog?: WorkLogEntry[];
  /** Which accounts the settings screen finds connected. */
  accounts?: Account[];
  /**
   * How fresh each account is. An account with no entry here has never been
   * read, which is a legitimate state and reads as "Never synced" — so the
   * default is an empty list rather than one fabricated row per account.
   */
  syncStates?: SyncState[];
  /** Today's brief, or null when none has been written. */
  brief?: Brief | null;
  /** What `list_corpus` finds, which is where the day list comes from. */
  corpus?: string[];
  /** The drafts Chief has prepared. Nothing here has been sent. */
  proposals?: Proposal[];
  /**
   * Leave questions unanswered until {@link Chief.finish} is called, so the
   * streaming states can be held still and measured.
   */
  holdAnswers?: boolean;
}

/** Where the window and its scroll regions sit right now. */
export interface Measurements {
  /** Whether the document itself can scroll. It never should: this is a window. */
  windowScrolls: boolean;
  windowScrollTop: number;
  /** How many elements are actually scrolling — one scrollbar each. */
  scrollingRegions: number;
  /**
   * How many scroll regions are stacked inside one another around the content.
   *
   * More than one means the wheel has no single owner: whether the inner or the
   * outer region moves depends on where the pointer is and which reached its
   * end first. Only one of them shows a scrollbar until the content is long
   * enough, which is why {@link scrollingRegions} alone would not catch it.
   */
  nestedScrollRegions: number;
  /** Distance from the top of the window to the header, or null if there is none. */
  headerTop: number | null;
  /** Whether the composer is still inside the window rather than pushed below it. */
  composerWithinWindow: boolean;
  /** Width the view's scrollbar takes out of the layout — 0 when it overlays. */
  scrollbarWidth: number;
}

/** Where a reader is within the scrolling part of the current view. */
export interface Position {
  scrollTop: number;
  furthest: number;
  atBottom: boolean;
}

/** Options a project sets to describe the browser Chief is standing in for. */
export interface ShellOptions {
  /**
   * Whether this project renders scrollbars that take space out of the layout,
   * the way Windows does, rather than ones that take none, the way macOS does.
   *
   * Turning it on takes two things, and this flag drives both. The project's
   * `launchOptions` pass `ignoreDefaultArgs: ['--hide-scrollbars']`, because
   * Playwright hides scrollbars in headless Chromium by default, which no real
   * user ever sees. That alone is enough on Linux, where CI runs — but not on
   * macOS, where Chromium takes its scrollbar style from the OS and draws
   * overlay scrollbars whatever the flags say. So {@link CLASSIC_SCROLLBARS} is
   * installed on top: a styled scrollbar is a custom one, and a custom one is
   * never an overlay on any host. One test checks the pair agree, so this
   * project cannot quietly become a second copy of the default one.
   */
  classicScrollbars: boolean;
}

/** The handle a test drives the app through. */
export interface Chief {
  open(backend?: Backend): Promise<void>;
  ask(question: string): Promise<void>;
  /** Push one update on `agent-stream`, as `agent::respond` would. */
  stream(update: AgentUpdate): Promise<void>;
  /** Answer the question in flight, ending the stream. */
  finish(answer: string): Promise<void>;
  goTo(view: 'Today' | 'Work Log' | 'Settings'): Promise<void>;
  /** Open the chat drawer from the rail, the way a user does. */
  openChat(): Promise<void>;
  /** Close it with Escape, the way the design system says it closes. */
  closeChat(): Promise<void>;
  /** How wide the detail column is right now, to the pixel. */
  detailWidth(): Promise<number>;
  /** Move the scrolling part of the view, the way a reader would. */
  scrollTo(position: 'top' | 'bottom'): Promise<void>;
  measure(): Promise<Measurements>;
  /** Where the reader is within the scrolling part of the view. */
  position(): Promise<Position>;
  composer: Locator;
}

interface Setup {
  answer: string;
  provenance: string | null;
  workLog: WorkLogEntry[];
  accounts: Account[];
  syncStates: SyncState[];
  brief: Brief | null;
  corpus: string[];
  proposals: Proposal[];
  holdAnswers: boolean;
}

/**
 * The bridge a test reaches the page through, once installed.
 *
 * Everything a test needs to run *inside* the page hangs off here, because a
 * function handed to `page.evaluate` is serialised without its surroundings and
 * so cannot call anything defined in this module.
 */
interface Bridge {
  stream: (update: AgentUpdate) => void;
  finish: (answer: string) => void;
  /**
   * The part of the current view that scrolls, found by looking rather than by
   * selector — so it holds for whichever view is showing, and a test notices if
   * the region turns up somewhere unexpected.
   *
   * The drawer wins while it is open: it is over the view, it is what the wheel
   * moves, and it is deliberately *outside* `main` so that opening it cannot
   * change the width of the column behind.
   */
  scroller: () => HTMLElement | null;
  /**
   * Wait until the view has stopped reacting.
   *
   * Pushing an update returns as soon as the listener has been called, which is
   * before React has rendered it and long before the effect that follows the
   * answer has run. Without waiting, the next step races a scroll that has not
   * happened yet — and a test that measures a position mid-flight reads whatever
   * the timing gave it.
   */
  settle: () => Promise<void>;
}

/**
 * Stand in for the Rust side. Serialised into the page before anything else
 * runs, so it must not reach outside its own arguments.
 */
function installBackend(setup: Setup) {
  const callbacks = new Map<number, (event: unknown) => void>();
  const listeners = new Map<string, Array<(event: unknown) => void>>();
  let nextCallbackId = 1;
  let requestId: string | null = null;
  let answerInFlight: ((answer: Answer) => void) | null = null;

  const emit = (event: string, payload: unknown) => {
    for (const listener of listeners.get(event) ?? []) {
      listener({ event, id: 0, payload });
    }
  };

  const bridge: Bridge = {
    stream: (update) => emit('agent-stream', { requestId, ...update }),
    finish: (answer) => {
      answerInFlight?.({ content: answer, provenance: null });
      answerInFlight = null;
    },
    scroller: () => {
      const within = document.querySelector('[role="dialog"]') ?? document.querySelector('main');

      return (
        [...(within?.querySelectorAll<HTMLElement>('*') ?? [])].find((element) => {
          const overflow = getComputedStyle(element).overflowY;

          return overflow === 'auto' || overflow === 'scroll';
        }) ?? null
      );
    },

    settle: async () => {
      const frame = () => new Promise<void>((painted) => requestAnimationFrame(() => painted()));

      // Quiet means two consecutive frames where neither the amount of content
      // nor the reader's position within it moved.
      let previous = '';

      for (let attempt = 0; attempt < 30; attempt += 1) {
        await frame();

        const region = bridge.scroller();
        const now = region === null ? 'none' : `${region.scrollHeight}:${region.scrollTop}`;

        if (now === previous) return;
        previous = now;
      }
    },
  };

  const internals = {
    transformCallback(callback: (event: unknown) => void) {
      const id = nextCallbackId++;
      callbacks.set(id, callback);
      return id;
    },

    invoke(command: string, args?: Record<string, unknown>): Promise<unknown> {
      switch (command) {
        case 'check_readiness':
          return Promise.resolve({
            model: 'Llama 3.2 3B Instruct (Q4_K_M)',
            tier: 'standard',
            modelSizeMb: 2400,
            modelInstalled: true,
            engine: 'ready',
            problem: null,
          });

        case 'ask_agent': {
          requestId = (args?.requestId as string | undefined) ?? null;
          if (!setup.holdAnswers) {
            return Promise.resolve({
              content: setup.answer,
              provenance: setup.provenance,
            });
          }

          return new Promise<Answer>((resolve) => {
            answerInFlight = resolve;
          });
        }

        case 'list_work_logs':
          return Promise.resolve(setup.workLog);

        case 'connections':
          return Promise.resolve(setup.accounts);

        // Dispatched on the command name, and answering with a list of
        // `SyncState` rather than with whatever `answers[command] ?? []`
        // would have produced. CLAUDE.md calls the stub that answers `[]` to
        // everything the most expensive shortcut in this repository.
        case 'sync_status':
          return Promise.resolve(setup.syncStates);

        case 'todays_brief':
          return Promise.resolve(setup.brief);

        case 'list_proposals':
          return Promise.resolve(setup.proposals);

        case 'profile_plan':
          return Promise.resolve({ reads: [], writes: [], keeps: [] });

        case 'generate_brief':
          return Promise.resolve(setup.brief);

        case 'list_corpus':
          return Promise.resolve(
            setup.corpus.map((path) => ({
              path,
              size: 200,
              modifiedAt: '2026-08-29T08:00:00.000Z',
              estimatedTokens: 50,
            })),
          );

        case 'plugin:event|listen': {
          const event = args?.event as string;
          const handler = args?.handler as number;
          const callback = callbacks.get(handler);

          if (callback !== undefined) {
            listeners.set(event, [...(listeners.get(event) ?? []), callback]);
          }

          return Promise.resolve(handler);
        }

        default:
          return Promise.resolve(null);
      }
    },
  };

  const target = window as unknown as { __TAURI_INTERNALS__: unknown; __chief: Bridge };
  target.__TAURI_INTERNALS__ = internals;
  target.__chief = bridge;
}

/** Read the window and its scroll regions from inside the page. */
function readMeasurements(): Measurements {
  const doc = document.scrollingElement ?? document.documentElement;
  const header = document.querySelector('header');
  const composer = document.querySelector('form');
  const region = (window as unknown as { __chief: Bridge }).__chief.scroller();

  const scrollingRegions = [...document.querySelectorAll('*')].filter((element) => {
    const overflow = getComputedStyle(element).overflowY;
    const scrollable = overflow === 'auto' || overflow === 'scroll';

    return scrollable && element.scrollHeight > element.clientHeight + 1;
  }).length;

  let nestedScrollRegions = 0;
  for (let element = region; element !== null; element = element.parentElement) {
    const overflow = getComputedStyle(element).overflowY;

    if (overflow === 'auto' || overflow === 'scroll') nestedScrollRegions += 1;
  }

  return {
    windowScrolls: doc.scrollHeight > doc.clientHeight + 1,
    windowScrollTop: doc.scrollTop,
    scrollingRegions,
    nestedScrollRegions,
    headerTop: header === null ? null : Math.round(header.getBoundingClientRect().top),
    composerWithinWindow:
      composer === null ||
      Math.round(composer.getBoundingClientRect().bottom) <= window.innerHeight,
    scrollbarWidth: region === null ? 0 : region.offsetWidth - region.clientWidth,
  };
}

/** Read the reader's position within the scrolling part of the view. */
function readPosition(): Position {
  const region = (window as unknown as { __chief: Bridge }).__chief.scroller();

  if (region === null) return { scrollTop: 0, furthest: 0, atBottom: true };

  const furthest = region.scrollHeight - region.clientHeight;

  return {
    scrollTop: Math.round(region.scrollTop),
    furthest: Math.round(furthest),
    atBottom: furthest - region.scrollTop <= 2,
  };
}

/** Wait for the page to stop reacting to whatever just happened. */
function settle(page: Page): Promise<void> {
  return page.evaluate(() => (window as unknown as { __chief: Bridge }).__chief.settle());
}

/**
 * A scrollbar that takes space out of the layout, on any host.
 *
 * 15px is what Chromium gives a Windows scrollbar at 100% scaling, which is the
 * layout this stands in for. It is applied to the test page rather than shipped:
 * Chief's own CSS leaves scrollbars to the platform, and the point here is to
 * measure the app under a platform that draws them wide.
 */
const CLASSIC_SCROLLBARS = `
  ::-webkit-scrollbar { width: 15px; height: 15px; }
  ::-webkit-scrollbar-thumb { background: #8883; }
`;

function handleFor(page: Page, classicScrollbars: boolean): Chief {
  const composer = page.getByRole('textbox', { name: 'Message your chief of staff' });

  return {
    composer,

    async open(backend: Backend = {}) {
      await page.addInitScript(installBackend, {
        answer: backend.answer ?? 'Two pull requests are waiting on review.',
        provenance: backend.provenance ?? null,
        workLog: backend.workLog ?? [],
        accounts: backend.accounts ?? [],
        syncStates: backend.syncStates ?? [],
        brief: backend.brief ?? null,
        corpus: backend.corpus ?? [],
        proposals: backend.proposals ?? [],
        holdAnswers: backend.holdAnswers ?? false,
      });

      await page.goto('/');
      if (classicScrollbars) await page.addStyleTag({ content: CLASSIC_SCROLLBARS });
      // The shell, not the composer: chat is a drawer now and is not on screen
      // until somebody opens it.
      await page.getByRole('navigation', { name: 'Main' }).waitFor();
    },

    async openChat() {
      const drawer = page.getByRole('dialog', { name: 'Ask Chief' });

      await page.getByRole('button', { name: 'Ask Chief' }).click();
      await drawer.waitFor();
      // The drawer arrives 8px to the right of where it lands. Measuring before
      // that has finished reads the offset, not the layout.
      await drawer.evaluate((node) =>
        Promise.all(node.getAnimations().map((animation) => animation.finished)),
      );
      await settle(page);
    },

    async closeChat() {
      await page.keyboard.press('Escape');
      await page.getByRole('dialog', { name: 'Ask Chief' }).waitFor({ state: 'detached' });
      await settle(page);
    },

    async detailWidth() {
      const box = await page.locator('main').boundingBox();

      return Math.round(box?.width ?? 0);
    },

    async ask(question: string) {
      // Chat is a drawer now, so asking opens it first. Idempotent: a test that
      // opened it itself, to measure the shell around it, is left alone.
      if (!(await composer.isVisible())) await this.openChat();

      await composer.fill(question);
      await composer.press('Enter');
      await page.waitForFunction(
        () => (document.querySelector('[role="dialog"]')?.querySelectorAll('li').length ?? 0) > 0,
      );
      await settle(page);
    },

    async stream(update: AgentUpdate) {
      await page.evaluate(async (sent) => {
        const bridge = (window as unknown as { __chief: Bridge }).__chief;

        bridge.stream(sent);
        await bridge.settle();
      }, update);
    },

    async finish(answer: string) {
      await page.evaluate(async (sent) => {
        const bridge = (window as unknown as { __chief: Bridge }).__chief;

        bridge.finish(sent);
        await bridge.settle();
      }, answer);
    },

    async goTo(view) {
      await page.getByRole('button', { name: view, exact: true }).click();
      await page.getByRole('heading', { level: 1, name: view }).waitFor();
      await settle(page);
    },

    async scrollTo(position) {
      await page.evaluate(async (to) => {
        const region = (window as unknown as { __chief: Bridge }).__chief.scroller();
        if (region === null) return;

        // A real reader's scroll reaches the view long before the next token
        // does. Setting `scrollTop` fires the event asynchronously, so wait for
        // the view to have seen it rather than racing the next assertion.
        await new Promise<void>((settled) => {
          region.addEventListener('scroll', () => settled(), { once: true });
          region.scrollTop = to === 'top' ? 0 : region.scrollHeight;

          // Nothing is dispatched when it was already in that position.
          requestAnimationFrame(() => requestAnimationFrame(() => settled()));
        });

        // The view may react to a reader moving; let it finish before measuring.
        await (window as unknown as { __chief: Bridge }).__chief.settle();
      }, position);
    },

    measure() {
      return page.evaluate(readMeasurements);
    },

    position() {
      return page.evaluate(readPosition);
    },
  };
}

export const test = base.extend<ShellOptions & { chief: Chief }>({
  classicScrollbars: [false, { option: true }],

  chief: async ({ page, classicScrollbars }, use) => {
    await use(handleFor(page, classicScrollbars));
  },
});

export { expect } from '@playwright/test';

/** An answer far taller than any window these tests use. */
export function longAnswer(lines = 60): string {
  return Array.from(
    { length: lines },
    (_, index) => `* fix(stats): something that shipped (#${500 + index})`,
  ).join('\n');
}

/** A work log long enough to need scrolling. */
export function longWorkLog(entries = 40): WorkLogEntry[] {
  return Array.from({ length: entries }, (_, index) => ({
    id: index + 1,
    timestamp: '2026-08-19T14:00:00Z',
    source: 'github',
    content: `Merged pull request #${index} in scottmallinson/chief.ai`,
    summary: `Shipped something, number ${index}.`,
    // Every other one has somewhere to go, so a list mixes rows that carry an
    // affordance with rows that carry none — which is what the feed actually
    // holds, since a hand-written entry has no URL.
    url: index % 2 === 0 ? `https://github.com/scottmallinson/chief.ai/pull/${index}` : null,
    externalId: `scottmallinson/chief.ai#${index}`,
    accountId: 1,
  }));
}
