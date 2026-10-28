// Draws the images the installers show, in the Chief design system, from the
// bundled fonts and the mark. Run `node scripts/installer-art.mjs` after a
// change to the design and commit the output: the release only reads files.
//
// NSIS and WiX want 24-bit BMP, which a browser cannot write, so each image is
// drawn at its exact size, screenshotted, and encoded here. The DMG takes PNG.
/* global document, Image */
import { mkdirSync, readFileSync, writeFileSync } from 'node:fs';
import { dirname, join } from 'node:path';
import { fileURLToPath } from 'node:url';
import { chromium } from '@playwright/test';

const root = join(dirname(fileURLToPath(import.meta.url)), '..');
const out = join(root, 'src-tauri', 'installer');
const font = (f) => readFileSync(join(root, 'website/fonts', f)).toString('base64');

const GRAPHITE = '#14171a';
const PAPER = '#f4f5f3';
const SLATE = '#2c5c7a';
const MUTED = '#8b929a';

const mark = (size, tile, arc) => `<svg width="${size}" height="${size}" viewBox="0 0 32 32">
  <rect width="32" height="32" rx="8" fill="${tile}"/>
  <path d="M21.5 11.3a7 7 0 1 0 0 9.4" fill="none" stroke="${arc}" stroke-width="3.2" stroke-linecap="round"/></svg>`;

const css = `
@font-face{font-family:'Instrument Sans';font-weight:400 700;src:url(data:font/woff2;base64,${font('instrument-sans-latin-wght-normal.woff2')})}
@font-face{font-family:'IBM Plex Mono';font-weight:400;src:url(data:font/woff2;base64,${font('ibm-plex-mono-latin-400-normal.woff2')})}
*{margin:0;box-sizing:border-box}
body{font-family:'Instrument Sans',sans-serif;overflow:hidden}
.micro{font:400 10px 'IBM Plex Mono',monospace;letter-spacing:.1em;text-transform:uppercase}
`;

// Light fields for the dialogs (the installers draw their own paper behind
// them), graphite for the sidebar and the disk image.
const images = [
  {
    file: 'nsis-sidebar.bmp',
    w: 164,
    h: 314,
    html: `<div style="width:164px;height:314px;background:${GRAPHITE};padding:20px 18px;position:relative">
      ${mark(40, PAPER, GRAPHITE)}
      <div style="color:${PAPER};font-size:20px;font-weight:600;margin-top:14px">Chief</div>
      <div class="micro" style="color:${MUTED};margin-top:6px;line-height:1.6">Your chief<br>of staff</div>
      <div style="position:absolute;left:18px;right:18px;bottom:20px;border-top:1px solid #2c3238;padding-top:10px">
        <div class="micro" style="color:${MUTED};line-height:1.7">On this machine<br>Nothing leaves it</div></div></div>`,
  },
  {
    file: 'nsis-header.bmp',
    w: 150,
    h: 57,
    html: `<div style="width:150px;height:57px;background:${PAPER};display:flex;align-items:center;justify-content:flex-end;padding-right:12px;gap:8px;border-bottom:1px solid #d6d9d4">
      <span style="color:${GRAPHITE};font-size:16px;font-weight:600">Chief</span>${mark(28, GRAPHITE, PAPER)}</div>`,
  },
  {
    file: 'wix-banner.bmp',
    w: 493,
    h: 58,
    html: `<div style="width:493px;height:58px;background:${PAPER};display:flex;align-items:center;justify-content:flex-end;padding-right:16px;gap:10px;border-bottom:1px solid #d6d9d4">
      <span class="micro" style="color:${SLATE}">On-device</span><span style="color:${GRAPHITE};font-size:16px;font-weight:600">Chief</span>${mark(32, GRAPHITE, PAPER)}</div>`,
  },
  {
    file: 'wix-dialog.bmp',
    w: 493,
    h: 312,
    html: `<div style="width:493px;height:312px;background:${GRAPHITE};position:relative">
      <div style="position:absolute;left:0;top:0;bottom:0;width:164px;padding:20px 18px;border-right:1px solid #2c3238">
        ${mark(40, PAPER, GRAPHITE)}
        <div style="color:${PAPER};font-size:20px;font-weight:600;margin-top:14px">Chief</div>
        <div class="micro" style="color:${MUTED};margin-top:6px;line-height:1.6">Your chief<br>of staff</div></div></div>`,
  },
  {
    file: 'dmg-background.png',
    w: 660,
    h: 400,
    png: true,
    html: `<div style="width:660px;height:400px;background:${GRAPHITE};position:relative">
      <div style="position:absolute;left:36px;top:30px;display:flex;align-items:center;gap:12px">
        ${mark(32, PAPER, GRAPHITE)}<span style="color:${PAPER};font-size:20px;font-weight:600">Chief</span></div>
      <div class="micro" style="position:absolute;left:36px;right:36px;top:296px;text-align:center;color:${PAPER}">Drag Chief to Applications</div>
      <div class="micro" style="position:absolute;left:36px;bottom:24px;color:${MUTED}">Runs on this machine. Nothing leaves it.</div></div>`,
  },
];

// 24-bit, bottom-up, rows padded to four bytes: what NSIS and WiX read.
function bmp({ data, width, height }) {
  const row = Math.ceil((width * 3) / 4) * 4;
  const buf = Buffer.alloc(54 + row * height);
  buf.write('BM', 0);
  buf.writeUInt32LE(buf.length, 2);
  buf.writeUInt32LE(54, 10);
  buf.writeUInt32LE(40, 14);
  buf.writeInt32LE(width, 18);
  buf.writeInt32LE(height, 22);
  buf.writeUInt16LE(1, 26);
  buf.writeUInt16LE(24, 28);
  buf.writeUInt32LE(row * height, 34);
  for (let y = 0; y < height; y++) {
    for (let x = 0; x < width; x++) {
      const s = ((height - 1 - y) * width + x) * 4;
      const d = 54 + y * row + x * 3;
      buf[d] = data[s + 2];
      buf[d + 1] = data[s + 1];
      buf[d + 2] = data[s];
    }
  }
  return buf;
}

mkdirSync(out, { recursive: true });
const browser = await chromium.launch(
  process.env.CHROMIUM_PATH ? { executablePath: process.env.CHROMIUM_PATH } : {},
);
for (const { file, w, h, html, png } of images) {
  const page = await browser.newPage({ viewport: { width: w, height: h }, deviceScaleFactor: 1 });
  await page.setContent(`<style>${css}</style>${html}`);
  await page.evaluate(() => document.fonts.ready);
  const shot = await page.screenshot({ clip: { x: 0, y: 0, width: w, height: h } });
  if (png) {
    writeFileSync(join(out, file), shot);
  } else {
    // Read the pixels back through a canvas rather than add a PNG decoder.
    const rgba = await page.evaluate(async (b64) => {
      const img = new Image();
      img.src = `data:image/png;base64,${b64}`;
      await img.decode();
      const c = document.createElement('canvas');
      c.width = img.width;
      c.height = img.height;
      const g = c.getContext('2d');
      g.drawImage(img, 0, 0);
      return Array.from(g.getImageData(0, 0, c.width, c.height).data);
    }, shot.toString('base64'));
    writeFileSync(join(out, file), bmp({ data: rgba, width: w, height: h }));
  }
  await page.close();
}
await browser.close();
