#!/usr/bin/env node
/**
 * Sizes the built frontend the way it actually ships: one JS bundle, one CSS
 * file, and the media directory that rides along inside both the desktop
 * `.app`/installer and the Android `.apk` — `assetsInlineLimit: 0` in
 * vite.config.ts means every image is a real file under dist/, not a data URI
 * vite could otherwise hide from a simple `du`.
 *
 *   pnpm exec vite build && node perf/bundle/measure-bundle.mjs
 *   node perf/bundle/measure-bundle.mjs path/to/other-dist
 *
 * Categorised by extension rather than by directory: dist/assets holds JS and
 * CSS side by side, and a future code-split bundle would still need to be
 * counted as JS regardless of which chunk file it lands in.
 */
import { gzipSync } from "node:zlib";
import { readFile, readdir, stat } from "node:fs/promises";
import { extname, join, resolve } from "node:path";
import { fileURLToPath } from "node:url";

const ROOT = resolve(fileURLToPath(import.meta.url), "..", "..", "..");

const CATEGORY_BY_EXTENSION = {
  ".js": "js",
  ".css": "css",
  ".png": "media",
  ".jpg": "media",
  ".jpeg": "media",
  ".webp": "media",
  ".avif": "media",
  ".svg": "media",
  ".mp4": "media",
  ".ico": "media",
};

async function walk(dir) {
  const entries = await readdir(dir, { withFileTypes: true });
  const files = [];
  for (const entry of entries) {
    const full = join(dir, entry.name);
    if (entry.isDirectory()) files.push(...(await walk(full)));
    else files.push(full);
  }
  return files;
}

/**
 * Pure so a test can hand it a small synthetic tree instead of the real,
 * 48 MB `dist/media` — exercising this against the whole build directory on
 * every `pnpm test` run would make the unit suite depend on a production
 * build having happened first.
 */
export async function summarizeDist(distDir) {
  const files = await walk(distDir);
  const totals = { js: 0, css: 0, media: 0, other: 0 };
  const gzip = { js: 0, css: 0 };
  for (const file of files) {
    const { size } = await stat(file);
    const category = CATEGORY_BY_EXTENSION[extname(file).toLowerCase()] ?? "other";
    totals[category] += size;
    if (category === "js" || category === "css") {
      gzip[category] += gzipSync(await readFile(file)).length;
    }
  }
  return { totals, gzip, fileCount: files.length };
}

function formatKb(bytes) {
  return `${(bytes / 1024).toFixed(1)} kB`;
}

async function main() {
  const distDir = resolve(ROOT, process.argv[2] ?? "dist");
  const { totals, gzip, fileCount } = await summarizeDist(distDir);
  const total = totals.js + totals.css + totals.media + totals.other;
  console.log(`Bundle size — ${distDir} (${fileCount} files)`);
  console.log(`  JS      ${formatKb(totals.js).padStart(10)}   gzip ${formatKb(gzip.js)}`);
  console.log(`  CSS     ${formatKb(totals.css).padStart(10)}   gzip ${formatKb(gzip.css)}`);
  console.log(`  media   ${formatKb(totals.media).padStart(10)}`);
  console.log(`  other   ${formatKb(totals.other).padStart(10)}`);
  console.log(`  total   ${formatKb(total).padStart(10)}`);
}

// Only run as a CLI; `summarizeDist` is what the test imports.
if (process.argv[1] === fileURLToPath(import.meta.url)) {
  await main();
}
