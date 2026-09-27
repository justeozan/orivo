import { test } from "@playwright/test";
import { report, timeNavigation } from "../helpers";

/**
 * "Le coût d'ouverture de Réglages → Plugins" (docs/plugin-system-plan.md,
 * étape 2.4).
 *
 * Running without Tauri (`isTauriRuntime()` is false — see
 * playwright.config.ts's own comment), `plugin-manager.ts` always resolves to
 * `emptyPluginCatalog()`: browser mode cannot show installed plugins, so this
 * measures the same "0 plugins" case every e2e run already does. What N
 * installed plugins cost the *backend* commands this panel calls
 * (`get_plugin_catalog`) is measured on the Rust side instead — see
 * `src-tauri/src/perf_bench.rs` and the table in docs/performance.md — because
 * that cost lives entirely behind `invoke()`, decoupled from this render.
 */
const ITERATIONS = 6;

test("opening Settings › Plugins", async ({ page }) => {
  await page.goto("/#/library", { waitUntil: "domcontentloaded" });
  await page.waitForSelector("#game-cards .game-card", { state: "visible" });

  const samples: number[] = [];
  for (let i = 0; i < ITERATIONS; i += 1) {
    samples.push(
      await timeNavigation(
        page,
        "#/settings/plugins",
        "#app-page-settings:not([hidden]) #plugins-catalog-panel",
      ),
    );
    // Back to Library so the next iteration is a real navigation, not a no-op.
    await page.goto("/#/library", { waitUntil: "domcontentloaded" });
    await page.waitForSelector("#game-cards .game-card", { state: "visible" });
  }
  report("navigate.settings_plugins", samples.slice(1));
});
