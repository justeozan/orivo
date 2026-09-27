import { test } from "@playwright/test";
import { jsHeapUsedBytes, report, timeNavigation } from "../helpers";

/**
 * "Temps jusqu'au premier shell utilisable et jusqu'à la bibliothèque
 * hydratée" (docs/plugin-system-plan.md, étape 2.4). The dev-mode fallback
 * library (`fallbackLibrary`, 10 games — see e2e/helpers.ts
 * `FALLBACK_LIBRARY_IDS`) is synchronous and local, so "hydrated" here means
 * the same thing it means for a real, small local library with no Steam
 * account connected: there is no network round trip to wait out.
 *
 * The first iteration is reported separately from the rest: it is the only
 * one Chromium has not already parsed the module graph and warmed its JS
 * caches for, and that is the number closest to what a user's first-ever
 * launch (or first launch after an update) actually feels like.
 */
const ITERATIONS = 6;

test("first shell + hydrated library, cold vs warm", async ({ page }) => {
  const shellSamples: number[] = [];
  const hydratedSamples: number[] = [];

  for (let i = 0; i < ITERATIONS; i += 1) {
    const start = performance.now();
    await page.goto("/#/library", { waitUntil: "domcontentloaded" });
    await page.waitForSelector("header.topbar", { state: "visible" });
    const shellMs = performance.now() - start;
    await page.waitForSelector("#game-cards .game-card", { state: "visible" });
    const hydratedMs = performance.now() - start;

    if (i === 0) {
      report("startup.shell (cold)", [shellMs]);
      report("startup.hydrated_library (cold)", [hydratedMs]);
    } else {
      shellSamples.push(shellMs);
      hydratedSamples.push(hydratedMs);
    }
  }

  report("startup.shell (warm)", shellSamples);
  report("startup.hydrated_library (warm)", hydratedSamples);

  const heapBytes = await jsHeapUsedBytes(page);
  console.log(`PERF startup.js_heap_after_library         ${(heapBytes / 1_048_576).toFixed(2)} MB`);
});

test("opening the Store", async ({ page }) => {
  await page.goto("/#/library", { waitUntil: "domcontentloaded" });
  await page.waitForSelector("#game-cards .game-card", { state: "visible" });

  const samples: number[] = [];
  for (let i = 0; i < ITERATIONS; i += 1) {
    samples.push(await timeNavigation(page, "#/store", "#app-page-store:not([hidden]) .store-hero__title"));
  }
  report("navigate.store", samples.slice(1));
});

test("opening a game's detail page", async ({ page }) => {
  await page.goto("/#/library", { waitUntil: "domcontentloaded" });
  await page.waitForSelector("#game-cards .game-card", { state: "visible" });

  const samples: number[] = [];
  for (let i = 0; i < ITERATIONS; i += 1) {
    samples.push(
      await timeNavigation(
        page,
        "#/games/steam%3A1245620",
        "#app-page-game:not([hidden]) .gd-hero__title",
      ),
    );
  }
  report("navigate.game_detail", samples.slice(1));
});
