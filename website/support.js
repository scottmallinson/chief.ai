/*
 * Chief website — visitor-dependent bits.
 *
 * Five jobs, all of them done in the visitor's browser with no request to
 * anybody: work out which build to offer and how sure it is of that, work out
 * which currency to print a zero in, fill in the download links, hand a phone
 * a way to get this page onto a desktop, and keep the docs sidebar pointing at
 * the section being read.
 *
 * Nothing here calls a network. The GitHub release API would resolve the
 * current version for us, but it would also hand every visitor's IP address to
 * GitHub on page load, which is a poor opening for a page whose headline is
 * that nothing about you is sent anywhere. The version is a constant instead,
 * stamped at release time by scripts/release.mjs alongside the four other
 * files that carry it.
 */

/* The released version. Rewritten by scripts/release.mjs — keep the shape of
   this line intact, it is matched by a regular expression. */
const VERSION = '0.8.0';

const REPO = 'scottmallinson/chief.ai';

/* One entry per build the release workflow actually publishes: two macOS
   architectures, Windows, and three Linux packagings of the same x64 build.
   The keys `detectPlatform` can return are the leads — one per platform — and
   the rest are alternatives that only ever appear as tiles.

   Every filename here is Tauri's own bundler naming, and the release workflow
   checks the files it produced against these names before the release is
   published. A tile that 404s is a worse answer than no tile. */
const BUILDS = {
  'macos-arm64': {
    label: 'macOS',
    meta: 'Apple silicon · .dmg',
    asset: (v) => `Chief_${v}_aarch64.dmg`,
  },
  'macos-x64': {
    label: 'macOS',
    meta: 'Intel · .dmg',
    asset: (v) => `Chief_${v}_x64.dmg`,
  },
  windows: {
    label: 'Windows',
    meta: 'x64 · .exe',
    asset: (v) => `Chief_${v}_x64-setup.exe`,
  },
  /* The lead for Linux is the AppImage, because it is the only one of the
     three that asks nothing of the machine it lands on. Detection can tell a
     visitor is on Linux; nothing in a browser tells us which package manager
     they use, so .deb and .rpm are offered beside it rather than guessed at. */
  linux: {
    label: 'Linux',
    meta: 'x64 · .AppImage',
    asset: (v) => `Chief_${v}_amd64.AppImage`,
  },
  'linux-deb': {
    label: 'Linux',
    meta: 'x64 · .deb',
    asset: (v) => `Chief_${v}_amd64.deb`,
  },
  'linux-rpm': {
    label: 'Linux',
    meta: 'x64 · .rpm',
    asset: (v) => `Chief-${v}-1.x86_64.rpm`,
  },
};

function downloadUrl(build) {
  return `https://github.com/${REPO}/releases/download/v${VERSION}/${BUILDS[build].asset(VERSION)}`;
}

/*
 * Which build to lead with.
 *
 * The honest limit is CPU architecture on macOS. Chromium exposes it through
 * getHighEntropyValues; Safari — which is most of the Mac traffic a page like
 * this gets — exposes nothing at all. So a Mac gets Apple silicon as the lead
 * and Intel stays visible beside it rather than being guessed at, and the
 * async refinement below corrects the lead where the browser will say.
 */
function detectPlatform() {
  const ua = navigator.userAgent || '';
  const uaPlatform = navigator.userAgentData?.platform || '';
  const platform = navigator.platform || '';

  const isTouchMac = platform === 'MacIntel' && navigator.maxTouchPoints > 1;
  const mobile = /Android|iPhone|iPad|iPod/i.test(ua) || isTouchMac;

  if (mobile) return 'mobile';
  if (/Win/i.test(uaPlatform || platform) || /Windows/i.test(ua)) return 'windows';
  if (/mac/i.test(uaPlatform || platform) || /Mac OS X/i.test(ua)) return 'macos-arm64';
  if (/Linux|X11|CrOS/i.test(uaPlatform || platform) || /Linux/i.test(ua)) return 'linux';

  return 'unknown';
}

async function refinePlatform(platform) {
  /* Everywhere but macOS the user agent settles it, so the answer is certain.
     On a Mac it is certain only when the browser will name the architecture,
     and the page says so when it will not — a default presented with the
     confidence of a detection is the thing to avoid here. */
  if (!platform.startsWith('macos')) return { platform, certain: true };
  if (!navigator.userAgentData?.getHighEntropyValues) return { platform, certain: false };

  try {
    const { architecture } = await navigator.userAgentData.getHighEntropyValues(['architecture']);
    if (architecture === 'x86') return { platform: 'macos-x64', certain: true };
    if (architecture === 'arm') return { platform: 'macos-arm64', certain: true };
  } catch {
    /* The browser declined. The Apple silicon default stands, and the Intel
       link is on the page either way. */
  }

  return { platform, certain: false };
}

/*
 * The currency a zero is printed in.
 *
 * Intl resolves a locale to a lot of things but not to a currency, so the
 * region has to be mapped. This covers the regions the site is likely to see
 * and falls back to USD, which is what the copy said before any of this.
 */
const CURRENCY_BY_REGION = {
  GB: 'GBP',
  US: 'USD',
  CA: 'CAD',
  AU: 'AUD',
  NZ: 'NZD',
  JP: 'JPY',
  CH: 'CHF',
  SE: 'SEK',
  NO: 'NOK',
  DK: 'DKK',
  PL: 'PLN',
  CZ: 'CZK',
  IN: 'INR',
  SG: 'SGD',
  HK: 'HKD',
  BR: 'BRL',
  MX: 'MXN',
  ZA: 'ZAR',
  KR: 'KRW',
  CN: 'CNY',
  TR: 'TRY',
  IL: 'ILS',
  AE: 'AED',
};

/* The euro is a currency without a single region, so the members are listed
   rather than guessed at from the language. */
const EURO_REGIONS = [
  'AT',
  'BE',
  'CY',
  'DE',
  'EE',
  'ES',
  'FI',
  'FR',
  'GR',
  'HR',
  'IE',
  'IT',
  'LT',
  'LU',
  'LV',
  'MT',
  'NL',
  'PT',
  'SI',
  'SK',
];

function detectCurrency() {
  const locale = navigator.languages?.[0] || navigator.language || 'en-US';

  let region;
  try {
    region = new Intl.Locale(locale).maximize().region;
  } catch {
    region = locale.split('-')[1];
  }

  if (!region) return { locale, currency: 'USD' };
  if (EURO_REGIONS.includes(region)) return { locale, currency: 'EUR' };

  return { locale, currency: CURRENCY_BY_REGION[region] || 'USD' };
}

function formatZero() {
  const { locale, currency } = detectCurrency();

  try {
    return new Intl.NumberFormat(locale, {
      style: 'currency',
      currency,
      minimumFractionDigits: 0,
      maximumFractionDigits: 0,
    }).format(0);
  } catch {
    return '$0';
  }
}

/* ---- Applying it to the page ---- */

function applyDownloads(platform, certain = true) {
  const primary = BUILDS[platform] ? platform : 'macos-arm64';
  const supported = Boolean(BUILDS[platform]);

  /* The hero button. */
  const hero = document.querySelector('[data-download-primary]');
  if (hero) {
    if (supported) {
      hero.href = downloadUrl(primary);
      const label = hero.querySelector('[data-download-label]');
      if (label) label.textContent = `Download for ${BUILDS[primary].label}`;
    } else {
      /* A phone, or a platform nothing here recognises. There is no build to
         hand over, so the button says what is true rather than offering a file
         that will not run. */
      hero.href = '#get';
      hero.removeAttribute('data-download-primary');
      const label = hero.querySelector('[data-download-label]');
      if (label) label.textContent = 'See the builds';

      /* And the mark changes with it. A download arrow over a button that
         scrolls the page is the affordance disagreeing with the action. */
      const mark = hero.querySelector('use');
      if (mark) mark.setAttribute('href', '#i-arrow-down');
    }
  }

  /* The note under it. "no telemetry" on its own was a claim about the whole
     product made on a page that counts its own views, so it says which half
     it is about. security.html#site is where the other half is accounted for. */
  const note = document.querySelector('[data-download-note]');
  if (note) {
    note.textContent = supported
      ? `Version ${VERSION} · no account · no telemetry from the app`
      : `Chief is a desktop app for macOS, Windows and Linux. Version ${VERSION}.`;
  }

  /* The lead in the download section. Its markup names the releases page, so a
     visitor with no JavaScript reaches every build in one extra click; here it
     becomes the one file this machine can actually run. */
  const lead = document.querySelector('[data-lead]');
  const leadName = document.querySelector('[data-lead-name]');
  const leadMeta = document.querySelector('[data-lead-meta]');
  if (lead && supported) {
    lead.href = downloadUrl(primary);
    if (leadName) leadName.textContent = `Download for ${BUILDS[primary].label}`;
    if (leadMeta) leadMeta.textContent = BUILDS[primary].meta;
  } else if (leadMeta) {
    /* A phone, or a platform nothing here recognises. There is no file to
       lead with, so the list is opened rather than a wrong build promoted. */
    leadMeta.textContent = 'a desktop app — macOS, Windows or Linux';
    const more = document.querySelector('[data-more]');
    if (more) more.open = true;
  }

  /* Said out loud when the lead is a default rather than a detection. */
  const guess = document.querySelector('[data-lead-guess]');
  if (guess) guess.hidden = certain || !supported;

  /* A phone has no build to be given, so it gets the one thing it can use: the
     address of this page, to open on the machine that can run Chief. */
  const copy = document.querySelector('[data-copy]');
  if (copy) copy.hidden = supported || !navigator.clipboard?.writeText;

  /* Every other build. Only the href changes: what distinguishes these from
     each other is written in the markup, because the reader needs to be able
     to tell them apart whether or not this file ran. */
  document.querySelectorAll('[data-build]').forEach((tile) => {
    const build = tile.getAttribute('data-build');
    if (BUILDS[build]) tile.href = downloadUrl(build);
  });
}

/*
 * Getting this page onto a desktop.
 *
 * A QR code would be the wrong way round — the reader is already holding the
 * phone. What they need is the address somewhere they can paste it, so the
 * button is a clipboard write and nothing else. It is hidden unless there is
 * no build for this machine and the browser actually has the API, so it never
 * appears as a control that does nothing.
 */
function startCopyLink() {
  const button = document.querySelector('[data-copy]');
  const label = button?.querySelector('[data-copy-label]');
  if (!button || !label) return;

  const fallback = document.querySelector('[data-copy-fallback]');
  const idle = label.textContent;
  let revert;

  function flash(message) {
    label.textContent = message;
    clearTimeout(revert);
    revert = setTimeout(() => {
      label.textContent = idle;
    }, 2400);
  }

  button.addEventListener('click', async () => {
    const address = `${location.origin}${location.pathname}`;

    try {
      await navigator.clipboard.writeText(address);
      flash('Link copied');
    } catch {
      /* Permission refused, or a browser that has the API and will not use it.
         Saying only "couldn't copy" leaves the reader where they started, so
         the address is put on the page as selectable text and left there —
         it is the thing they came for, and a two-second toast would take it
         away again. */
      flash('Could not copy');
      if (fallback) {
        fallback.textContent = location.host;
        fallback.hidden = false;
      }
    }
  });
}

/*
 * The docs sidebar, and the breadcrumb above the article.
 *
 * Both shipped hard-coded to "Install" and stayed there through seven
 * sections, so the navigation was wrong from the first scroll. Install is the
 * right answer at the top of the page, which is where a visitor arrives and
 * where a browser that never runs this leaves it — so this only ever refines
 * what the markup already says.
 *
 * The groups and the link text are the single source of truth for both the
 * highlight and the crumb; nothing here restates them.
 */
function startDocsNav() {
  const nav = document.querySelector('.docs-nav');
  if (!nav) return;

  const crumb = document.querySelector('[data-crumb]');
  const sections = [...nav.querySelectorAll('.docs-nav-links a[href^="#"]')]
    .map((link) => {
      const heading = document.getElementById(link.hash.slice(1));
      if (!heading) return null;
      const group = link.closest('.docs-nav-group')?.querySelector('.micro');
      return {
        link,
        heading,
        group: group ? group.textContent.trim() : 'docs',
        name: link.textContent.trim().toLowerCase(),
      };
    })
    .filter(Boolean);

  if (sections.length === 0) return;

  let current = null;

  function update() {
    /* The section being read is the last one whose heading has gone past the
       sticky header. Above the first heading that is the first section, which
       is what the markup says already. */
    const line = 120;
    let found = sections[0];
    for (const section of sections) {
      if (section.heading.getBoundingClientRect().top <= line) found = section;
    }

    if (found === current) return;
    current = found;

    for (const section of sections) {
      const isCurrent = section === found;
      section.link.classList.toggle('is-current', isCurrent);
      if (isCurrent) section.link.setAttribute('aria-current', 'location');
      else section.link.removeAttribute('aria-current');
    }

    if (crumb) crumb.textContent = `docs / ${found.group} / ${found.name}`;
  }

  update();
  addEventListener('scroll', update, { passive: true });
  addEventListener('resize', update, { passive: true });
}

function applyCurrency() {
  const zero = formatZero();
  document.querySelectorAll('[data-zero]').forEach((el) => {
    el.textContent = zero;
  });
}

function applyVersion() {
  document.querySelectorAll('[data-version]').forEach((el) => {
    el.textContent = VERSION;
  });
}

async function start() {
  applyCurrency();
  applyVersion();
  startDocsNav();
  startCopyLink();

  /* Applied twice on a Mac: once from the user agent, then again once the
     architecture is known or known to be unavailable. The first call says
     nothing about certainty, so the caveat never flashes either way. */
  const initial = detectPlatform();
  applyDownloads(initial);

  const { platform, certain } = await refinePlatform(initial);
  applyDownloads(platform, certain);
}

if (document.readyState === 'loading') {
  document.addEventListener('DOMContentLoaded', start);
} else {
  start();
}
