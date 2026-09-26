#!/usr/bin/env node

/**
 * Capture Orivo screenshots at every Apple App Store preview size.
 *
 *   pnpm screenshots:app-store
 *
 * Output → docs/screenshots/app-store/<page>/<WxH>.png
 *
 * Requires the Vite dev server to be reachable at http://127.0.0.1:5173.
 */

import { chromium } from "@playwright/test";
import { mkdirSync, existsSync } from "node:fs";
import { resolve, dirname } from "node:path";
import { fileURLToPath } from "node:url";

const __dirname = dirname(fileURLToPath(import.meta.url));
const ROOT = resolve(__dirname, "..");
const OUT = resolve(ROOT, "docs", "screenshots", "app-store");

// Apple App Store macOS preview sizes (CSS pixels × deviceScaleFactor)
const SIZES = [
  { w: 1280, h: 800, scale: 1 },
  { w: 1440, h: 900, scale: 1 },
  { w: 1280, h: 800, scale: 2 }, // renders at 2560 × 1600
  { w: 1440, h: 900, scale: 2 }, // renders at 2880 × 1800
];

// Pages to capture — the core Orivo experience
const PAGES = [
  {
    name: "library",
    hash: "#/library",
    waitSelector: "#app-page-library:not([hidden]) #hero-title",
  },
  {
    name: "store",
    hash: "#/store",
    waitSelector: "#app-page-store:not([hidden]) .store-hero__title",
  },
  {
    name: "game-detail",
    hash: "#/games/steam%3A1245620?from=store",
    waitSelector: "#app-page-game:not([hidden]) .gd-hero__title",
    masks: [".gd-stats"],
  },
  {
    name: "settings",
    hash: "#/settings/general",
    waitSelector: "#app-page-settings:not([hidden]) .settings-layout",
  },
];

async function waitForImages(page, timeoutMs = 15_000) {
  try {
    await page.waitForFunction(
      () => {
        const vw = document.documentElement.clientWidth;
        const vh = document.documentElement.clientHeight;
        const imgs = [...document.querySelectorAll("img")].filter((img) => {
          const r = img.getBoundingClientRect();
          return r.width > 0 && r.height > 0 && r.top < vh && r.bottom > 0 && r.left < vw && r.right > 0;
        });
        // If no visible images yet, that's fine — don't block forever
        return imgs.length === 0 || imgs.every((i) => i.complete);
      },
      { timeout: timeoutMs },
    );
  } catch {
    // Proceed anyway — some pages may not have visible images
  }
}

async function main() {
  mkdirSync(OUT, { recursive: true });

  const browser = await chromium.launch({ headless: true });

  for (const page_def of PAGES) {
    const pageDir = resolve(OUT, page_def.name);
    mkdirSync(pageDir, { recursive: true });

    for (const size of SIZES) {
      const outW = size.w * size.scale;
      const outH = size.h * size.scale;
      const filename = `${outW}x${outH}.png`;
      const filepath = resolve(pageDir, filename);

      if (existsSync(filepath)) {
        console.log(`  skip  ${page_def.name}/${filename} (exists)`);
        continue;
      }

      const context = await browser.newContext({
        viewport: { width: size.w, height: size.h },
        deviceScaleFactor: size.scale,
        colorScheme: "dark",
      });
      const page = await context.newPage();

      try {
        await page.goto(`http://127.0.0.1:5173/${page_def.hash}`, {
          waitUntil: "domcontentloaded",
        });
        await page.waitForSelector(page_def.waitSelector, { state: "visible", timeout: 20_000 });

        await waitForImages(page);
        // Let compositor paint
        await page.evaluate(
          () => new Promise((r) => requestAnimationFrame(() => requestAnimationFrame(() => r()))),
        );

        const maskLocators = (page_def.masks || []).map((sel) => {
          return page.locator(`.app-page:not([hidden]) ${sel}`);
        });

        await page.screenshot({
          path: filepath,
          mask: maskLocators,
          maskColor: "#101014",
        });

        console.log(`  ✓    ${page_def.name}/${filename}`);
      } catch (err) {
        console.error(`  ✗    ${page_def.name}/${filename}: ${err.message.split("\n")[0]}`);
      } finally {
        await context.close();
      }
    }
  }

  await browser.close();
  console.log(`\nDone — screenshots in ${OUT}`);
}

main().catch((err) => {
  console.error(err);
  process.exit(1);
});
