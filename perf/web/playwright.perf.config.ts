import { defineConfig, devices } from "@playwright/test";

/**
 * O1's desktop performance bench, deliberately outside `playwright.config.ts`.
 *
 * This is not the e2e suite: it prints timing numbers for `docs/performance.md`
 * instead of asserting golden screenshots or filtering pull requests, so it
 * lives in its own config with its own port — never `pnpm test:e2e`, never
 * 5173 (the user's own `pnpm dev`), never the per-worktree e2e port either.
 *
 *   pnpm exec vite build
 *   pnpm exec playwright test -c perf/web/playwright.perf.config.ts
 *
 * `workers: 1` and no retries are deliberate: a perf run is trying to measure
 * real cost, not to survive flakiness by hiding it. One worker also keeps the
 * dev server's own CPU time from competing with the page under measurement.
 */
const PORT = Number(process.env.ORIVO_PERF_PORT ?? 5313);

export default defineConfig({
  testDir: "./specs",
  fullyParallel: false,
  workers: 1,
  retries: 0,
  timeout: 60_000,
  reporter: [["list"]],
  outputDir: "../../node_modules/.cache/playwright-perf",
  use: {
    baseURL: `http://127.0.0.1:${PORT}`,
    colorScheme: "dark",
    trace: "off",
    screenshot: "off",
    video: "off",
  },
  projects: [
    {
      name: "desktop-1536",
      use: { ...devices["Desktop Chrome"], viewport: { width: 1536, height: 1024 } },
    },
  ],
  webServer: {
    command: `pnpm dev --host 127.0.0.1 --port ${PORT} --strictPort`,
    url: `http://127.0.0.1:${PORT}`,
    reuseExistingServer: false,
    cwd: "../..",
    timeout: 60_000,
  },
});
