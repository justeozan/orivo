import { defineConfig, devices } from "@playwright/test";

/**
 * End-to-end and visual-golden configuration.
 *
 *   pnpm test:e2e                       run every spec at both acceptance sizes
 *   pnpm test:e2e --update-snapshots    (re)generate the six golden screenshots
 *   pnpm test:e2e e2e/store.spec.ts     run one spec
 *   pnpm test:e2e --project=chromium-1040
 *
 * The suite drives `pnpm dev`, so the app runs in a plain browser with no Tauri
 * runtime. Every page detects that and renders its deterministic editorial /
 * fallback data instead of calling `invoke`, which is what the specs assert
 * against.
 */
export default defineConfig({
  testDir: "./e2e",
  fullyParallel: false,
  forbidOnly: !!process.env.CI,
  timeout: 45_000,
  expect: {
    timeout: 7_000,
    toHaveScreenshot: {
      animations: "disabled",
      caret: "hide",
      scale: "css",
      maxDiffPixelRatio: 0.01,
    },
  },
  reporter: process.env.CI ? [["github"], ["list"]] : [["list"]],
  // Failure artefacts land under an already-ignored path so the repo stays clean
  // without a .gitignore change.
  outputDir: "node_modules/.cache/playwright",
  // Golden files land in e2e/__screenshots__/<project>/<name>.png — six files.
  snapshotPathTemplate: "{testDir}/__screenshots__/{projectName}/{arg}{ext}",
  use: {
    baseURL: "http://127.0.0.1:5173",
    colorScheme: "dark",
    screenshot: "only-on-failure",
    trace: "retain-on-failure",
  },
  projects: [
    {
      name: "chromium-1536",
      use: { ...devices["Desktop Chrome"], viewport: { width: 1536, height: 1024 } },
    },
    {
      name: "chromium-1040",
      use: { ...devices["Desktop Chrome"], viewport: { width: 1040, height: 700 } },
    },
  ],
  webServer: {
    command: "pnpm dev --host 127.0.0.1 --port 5173",
    url: "http://127.0.0.1:5173",
    reuseExistingServer: true,
    timeout: 60_000,
  },
});
