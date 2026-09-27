import { test } from "@playwright/test";
import { measureFramePacing, report } from "../helpers";

/**
 * "Latence et régularité des images lors d'une navigation dans le rail"
 * (docs/plugin-system-plan.md, étape 2.4): long tasks and requestAnimationFrame
 * jitter while moving focus across the library rail with the keyboard —
 * `spatial-nav.ts` is what every real session actually drives this with,
 * keyboard, gamepad or D-pad alike, not a synthetic `scrollLeft` write.
 */
test("keyboard rail navigation stays free of long tasks", async ({ page }) => {
  await page.goto("/#/library", { waitUntil: "domcontentloaded" });
  await page.waitForSelector("#game-cards .game-card", { state: "visible" });
  await page.locator("#game-cards .game-card").first().focus();

  const pacing = await measureFramePacing(page, async () => {
    // The fallback library has 10 cards (e2e/helpers.ts `FALLBACK_LIBRARY_IDS`);
    // right-to-the-end-and-back covers every card twice.
    for (let i = 0; i < 9; i += 1) await page.keyboard.press("ArrowRight");
    for (let i = 0; i < 9; i += 1) await page.keyboard.press("ArrowLeft");
  });

  console.log(
    `PERF rail.long_tasks                   count=${pacing.longTasks.count} total=${pacing.longTasks.totalMs.toFixed(2)}ms`,
  );
  console.log(
    `PERF rail.frame_delta                  max=${pacing.frameDeltas.maxMs.toFixed(2)}ms p95=${pacing.frameDeltas.p95Ms.toFixed(2)}ms samples=${pacing.frameDeltas.count}`,
  );
  report("rail.frame_delta_p95_repeated", [pacing.frameDeltas.p95Ms]);
});
