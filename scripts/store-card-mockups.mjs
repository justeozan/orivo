#!/usr/bin/env node

/**
 * What the Store card's second half — the block of facts under the capsule —
 * looked like before and after it was rebuilt.
 *
 *   node scripts/store-card-mockups.mjs
 *
 * Output → assets/store-card-mockups/
 *   00-comparaison.png       both states, stacked and labelled
 *   0N-<state>.png           the first three cards of the shelf
 *   0N-<state>-jointure.png  a close-up of the cut between picture and facts
 *   0N-<state>-page.png      the whole page
 *
 * "Avant" is drawn by putting the old rules back over the live page: an opaque
 * near-black block, a hard edge under the artwork, the dimmer type that block
 * could carry. Nothing here ships — it exists so the two can be compared, and
 * so a later change to the card can be judged against the same three shots.
 *
 * Requires the Vite dev server to be reachable at http://127.0.0.1:5173.
 */

import { chromium } from "@playwright/test";
import { mkdirSync, readFileSync } from "node:fs";
import { resolve, dirname } from "node:path";
import { fileURLToPath } from "node:url";

const __dirname = dirname(fileURLToPath(import.meta.url));
const ROOT = resolve(__dirname, "..");
const OUT = resolve(ROOT, "assets", "store-card-mockups");
const BASE = process.env.ORIVO_BASE_URL ?? "http://127.0.0.1:5173";

// The window the design was reviewed in, rather than a wider one where
// everything fits anyway.
const VIEWPORT = { width: 1051, height: 805 };

const STATES = [
  {
    name: "avant",
    label: "Avant",
    note: "Bloc noir opaque, arête franche sous la jaquette, photo arrêtée au bord de la carte.",
    css: `
      .store-card { background: #0a0c0e; border-color: rgba(255, 255, 255, 0.1); }
      .store-card__body {
        background: none;
        -webkit-backdrop-filter: none;
        backdrop-filter: none;
      }
      .store-card__body::before { display: none; }
      .store-card__genres { color: #c6cbc9; }
      .store-card__stat-label { color: #adb3b2; }
      .store-card__tagline { color: #8d9391; }
    `,
  },
  {
    name: "livre",
    label: "Livré",
    note: "Le panneau du hero à la teinte de la jaquette, et la jaquette qui continue floue dans le bloc.",
    css: "",
  },
];

async function openShelf(browser, scale) {
  const page = await browser.newPage({
    viewport: VIEWPORT,
    deviceScaleFactor: scale,
    colorScheme: "dark",
  });
  await page.goto(`${BASE}/#/store`, { waitUntil: "domcontentloaded" });
  await page.waitForSelector("#app-page-store:not([hidden]) .store-card", { timeout: 20_000 });
  await page
    .waitForFunction(
      () => {
        const art = [...document.querySelectorAll(".store-card__art")];
        return art.length > 0 && art.every((img) => img.complete);
      },
      null,
      { timeout: 20_000 },
    )
    .catch(() => {});
  await page.waitForLoadState("networkidle").catch(() => {});
  await page.waitForTimeout(700);
  return page;
}

/** The first three cards, with room around them. */
const shelfClip = (page) =>
  page.evaluate(() => {
    const boxes = [...document.querySelectorAll(".store-card")]
      .slice(0, 3)
      .map((card) => card.getBoundingClientRect());
    const left = Math.min(...boxes.map((b) => b.left));
    const top = Math.min(...boxes.map((b) => b.top));
    const right = Math.max(...boxes.map((b) => b.right));
    const bottom = Math.max(...boxes.map((b) => b.bottom));
    const pad = 14;
    return {
      x: Math.max(0, Math.round(left - pad)),
      y: Math.max(0, Math.round(top - pad)),
      width: Math.min(window.innerWidth, Math.round(right - left + pad * 2)),
      height: Math.round(bottom - top + pad * 2),
    };
  });

/** The band either side of the cut, on the second card. */
const seamClip = (page) =>
  page.evaluate(() => {
    const card = document.querySelectorAll(".store-card")[1];
    const media = card.querySelector(".store-card__media").getBoundingClientRect();
    const box = card.getBoundingClientRect();
    return {
      x: Math.round(box.left),
      y: Math.round(media.bottom - 56),
      width: Math.round(box.width),
      height: 132,
    };
  });

async function main() {
  mkdirSync(OUT, { recursive: true });
  const browser = await chromium.launch({ headless: true });

  const shots = [];
  for (const [index, state] of STATES.entries()) {
    const stem = `0${index + 1}-${state.name}`;

    const detail = await openShelf(browser, 2);
    if (state.css) await detail.addStyleTag({ content: state.css });
    await detail.waitForTimeout(300);
    const shelf = resolve(OUT, `${stem}.png`);
    const seam = resolve(OUT, `${stem}-jointure.png`);
    await detail.screenshot({ path: shelf, clip: await shelfClip(detail) });
    await detail.screenshot({ path: seam, clip: await seamClip(detail) });
    await detail.close();

    const whole = await openShelf(browser, 1);
    if (state.css) await whole.addStyleTag({ content: state.css });
    await whole.waitForTimeout(300);
    await whole.screenshot({ path: resolve(OUT, `${stem}-page.png`) });
    await whole.close();

    shots.push({ ...state, shelf, seam });
    console.log(`  ${stem}.png`);
  }

  // One board, so the two are compared rather than remembered.
  const board = await browser.newPage({
    viewport: { width: 1180, height: 1400 },
    deviceScaleFactor: 1.5,
    colorScheme: "dark",
  });
  const rows = shots
    .map(
      (shot) => `<figure>
        <figcaption><b>${shot.label}</b><span>${shot.note}</span></figcaption>
        <img src="data:image/png;base64,${readFileSync(shot.shelf).toString("base64")}" alt="">
        <img class="seam" src="data:image/png;base64,${readFileSync(shot.seam).toString("base64")}" alt="">
      </figure>`,
    )
    .join("");
  await board.setContent(`<!doctype html><meta charset="utf-8"><style>
    :root { color-scheme: dark; }
    body {
      margin: 0; padding: 30px 30px 34px; display: grid; gap: 26px;
      background: #0b0b0d; color: #f4f2f7;
      font: 400 15px/1.35 -apple-system, "SF Pro Text", "Segoe UI", sans-serif;
    }
    h1 { margin: 0 0 2px; font-size: 19px; font-weight: 600; letter-spacing: -0.01em; }
    p.sub { margin: 0 0 4px; color: #9b97a3; font-size: 13.5px; }
    figure { margin: 0; display: grid; gap: 9px; }
    figcaption { display: flex; align-items: baseline; gap: 10px; }
    figcaption b { font-weight: 600; font-size: 15px; }
    figcaption span { color: #9b97a3; font-size: 13px; }
    img { width: 100%; display: block; border-radius: 12px; }
    img.seam { width: 420px; border-radius: 8px; }
  </style>
  <h1>Store — le bloc de faits sous la jaquette</h1>
  <p class="sub">La deuxième vignette de chaque ligne est un zoom sur la coupe entre l'image et le bloc.</p>
  ${rows}`);
  await board.waitForTimeout(400);
  await board.screenshot({ path: resolve(OUT, "00-comparaison.png"), fullPage: true });
  console.log("  00-comparaison.png");

  await browser.close();
}

main().catch((error) => {
  console.error(error);
  process.exit(1);
});
