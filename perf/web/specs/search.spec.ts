import { test } from "@playwright/test";
import { report } from "../helpers";

/**
 * "Latence de la recherche locale" (docs/plugin-system-plan.md, étape 2.4).
 *
 * `app.ts`'s library search handler is synchronous — `state.query = value`
 * then a direct `renderSelection()`, no debounce timer — unlike the Store's
 * search, which does debounce. Measuring it round-trip through Playwright
 * would mostly time IPC between the Node process and the browser, so this
 * times the input-to-render span from inside the page instead, with
 * `performance.now()` on both sides of the same synchronous call stack.
 *
 * Ten games (the dev-mode fallback library) is a small fixture: this number
 * is "cost of one JS-side filter pass", useful to compare against future
 * changes, not a proof that search stays this fast at library sizes the
 * browser-mode fallback data cannot represent.
 */
const ITERATIONS = 8;

test("typing into the library search re-renders synchronously", async ({ page }) => {
  await page.goto("/#/library", { waitUntil: "domcontentloaded" });
  await page.waitForSelector("#game-cards .game-card", { state: "visible" });

  const queries = ["e", "el", "eld", "elde", "elden", "elden ", "elden r", ""];
  const samples: number[] = [];
  for (let i = 0; i < ITERATIONS; i += 1) {
    const query = queries[i % queries.length];
    // eslint-disable-next-line no-await-in-loop -- each iteration depends on the DOM state the previous one left.
    const ms = await page.evaluate((value) => {
      const input = document.querySelector<HTMLInputElement>("#topbar-search");
      if (!input) throw new Error("topbar search input not found");
      const start = performance.now();
      input.value = value;
      input.dispatchEvent(new Event("input", { bubbles: true }));
      return performance.now() - start;
    }, query);
    samples.push(ms);
  }

  report("search.library_filter (first keystroke)", [samples[0]]);
  report("search.library_filter (warm)", samples.slice(1));
});
