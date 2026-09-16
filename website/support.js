/*
 * Chief website — visitor-dependent bits.
 *
 * Three jobs, all of them done in the visitor's browser with no request to
 * anybody: work out which build to offer, work out which currency to print a
 * zero in, and fill in the download links.
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
const VERSION = '0.6.0';

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
  if (!platform.startsWith('macos')) return platform;
  if (!navigator.userAgentData?.getHighEntropyValues) return platform;

  try {
    const { architecture } = await navigator.userAgentData.getHighEntropyValues(['architecture']);
    if (architecture === 'x86') return 'macos-x64';
    if (architecture === 'arm') return 'macos-arm64';
  } catch {
    /* The browser declined. The Apple silicon default stands, and the Intel
       link is on the page either way. */
  }

  return platform;
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

function applyDownloads(platform) {
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
    }
  }

  /* The note under it, and the platform tiles. */
  const note = document.querySelector('[data-download-note]');
  if (note) {
    note.textContent = supported
      ? `Version ${VERSION} · no account, no telemetry`
      : `Chief is a desktop app for macOS, Windows and Linux. Version ${VERSION}.`;
  }

  document.querySelectorAll('[data-build]').forEach((tile) => {
    const build = tile.getAttribute('data-build');
    if (!BUILDS[build]) return;

    tile.href = downloadUrl(build);
    tile.classList.toggle('is-primary', build === primary && supported);

    const meta = tile.querySelector('[data-build-meta]');
    if (meta) meta.textContent = BUILDS[build].meta;
  });
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

  const initial = detectPlatform();
  applyDownloads(initial);

  const refined = await refinePlatform(initial);
  if (refined !== initial) applyDownloads(refined);
}

if (document.readyState === 'loading') {
  document.addEventListener('DOMContentLoaded', start);
} else {
  start();
}
